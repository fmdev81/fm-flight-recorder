//! The live stream (`GET /api/v1/live`): open pages hear about new events
//! and about clients that connect, within a short time of the ingest.

mod common;

use common::*;
use fr_server::live::{Kind, MAX_STREAMS};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// An open live stream. The reader thread sends each `event:` name with
/// its `data:` line.
struct Stream {
    rx: mpsc::Receiver<(String, String)>,
    _s: TcpStream,
}

impl Stream {
    fn open(base: &str) -> (Stream, String) {
        let authority = base.trim_start_matches("http://");
        let mut s = TcpStream::connect(authority).unwrap();
        write!(
            s,
            "GET /api/v1/live HTTP/1.1\r\nHost: {authority}\r\nAccept: text/event-stream\r\n\r\n"
        )
        .unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut l = String::new();
            if r.read_line(&mut l).unwrap() == 0 || l == "\r\n" {
                break;
            }
            head.push_str(&l);
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut ev = String::new();
            let mut l = String::new();
            while r.read_line(&mut l).unwrap_or(0) > 0 {
                // Chunk sizes of the chunked body are lines too; skip them.
                let t = l.trim_end();
                if let Some(e) = t.strip_prefix("event: ") {
                    ev = e.to_string();
                } else if let Some(d) = t.strip_prefix("data: ") {
                    if tx.send((std::mem::take(&mut ev), d.to_string())).is_err() {
                        return;
                    }
                }
                l.clear();
            }
        });
        (Stream { rx, _s: s }, head)
    }

    fn next(&self, within: Duration) -> Option<(String, String)> {
        self.rx.recv_timeout(within).ok()
    }
}

fn wait_streams(app: &fr_server::app::Shared, n: usize) {
    let t = Instant::now();
    while app.live.streams() != n {
        assert!(t.elapsed() < Duration::from_secs(5), "{} streams", app.live.streams());
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_stored_batch_reaches_open_pages_at_once() {
    let (app, base, _d) = server("live-batch");
    let (st, head) = Stream::open(&base);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.to_ascii_lowercase().contains("content-type: text/event-stream"), "{head}");
    assert_eq!(st.next(Duration::from_secs(2)).unwrap().0, "hello");
    wait_streams(&app, 1);

    let t0 = Instant::now();
    let r = post_json(&base, "/api/v1/events", Some(&app.token()), &doc_examples()[0]);
    assert_eq!(r.status, 200, "{}", r.body);
    let (ev, data) = st.next(Duration::from_secs(2)).expect("no live event");
    assert_eq!((ev.as_str(), data.as_str()), ("events", "2"));
    assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());

    // A resend stores nothing, so the pages hear nothing.
    let r = post_json(&base, "/api/v1/events", Some(&app.token()), &doc_examples()[0]);
    assert_eq!(r.status, 200);
    assert!(st.next(Duration::from_millis(300)).is_none());

    // The status call counts the open stream.
    let s = get(&base, "/api/v1/status");
    let v: serde_json::Value = serde_json::from_str(&s.body).unwrap();
    assert_eq!(v["live_streams"], 1);
}

#[test]
fn a_ping_from_a_new_client_is_a_client_event_once() {
    let (app, base, _d) = server("live-ping");
    let (st, _) = Stream::open(&base);
    assert_eq!(st.next(Duration::from_secs(2)).unwrap().0, "hello");
    let ping = doc_examples()
        .into_iter()
        .find(|b| b.contains("\"heartbeats\""))
        .expect("ping example");
    let r = post_json(&base, "/api/v1/ping", Some(&app.token()), &ping);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(st.next(Duration::from_secs(2)).unwrap(), ("client".into(), "1".into()));
    // The next ping of a connected client changes nothing on the pages.
    let r = post_json(&base, "/api/v1/ping", Some(&app.token()), &ping);
    assert_eq!(r.status, 200);
    assert!(st.next(Duration::from_millis(300)).is_none());
}

#[test]
fn closed_pages_free_their_slot_and_close_ends_the_streams() {
    let (app, base, _d) = server("live-close");
    {
        let (st, _) = Stream::open(&base);
        assert_eq!(st.next(Duration::from_secs(2)).unwrap().0, "hello");
        wait_streams(&app, 1);
        // Dropping the socket closes the page; the server sees it at the
        // next write.
    }
    // The first write to a closed socket can still succeed; keep writing.
    let t = Instant::now();
    while app.live.streams() != 0 {
        assert!(t.elapsed() < Duration::from_secs(5), "slot not freed");
        app.live.notify(Kind::Events, 1);
        thread::sleep(Duration::from_millis(20));
    }

    let (st, _) = Stream::open(&base);
    assert_eq!(st.next(Duration::from_secs(2)).unwrap().0, "hello");
    wait_streams(&app, 1);
    app.live.close();
    wait_streams(&app, 0);
}

#[test]
fn too_many_streams_get_503() {
    let (app, base, _d) = server("live-max");
    let open: Vec<_> = (0..MAX_STREAMS).map(|_| Stream::open(&base)).collect();
    wait_streams(&app, MAX_STREAMS);
    let (_, head) = Stream::open(&base);
    assert!(head.starts_with("HTTP/1.1 503"), "{head}");
    drop(open);
}
