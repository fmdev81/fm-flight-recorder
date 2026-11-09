//! A tiny test receiver for the ingest API (docs/ingest-api.md): a local
//! HTTP listener that stores batches, drops duplicates by
//! (session_id, seq) like the real server must, and can be told to fail,
//! hang, or go down and come back on the same port.

#![allow(dead_code)]

use fm_flight_recorder::wire::{EventBatch, Ping};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering::*};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const TOKEN: &str = "test-token";

#[derive(Default)]
pub struct RState {
    pub batches: Mutex<Vec<EventBatch>>,
    pub pings: Mutex<Vec<Ping>>,
    seen: Mutex<HashSet<(String, u64)>>,
    pub accepted: AtomicU64,
    pub duplicates: AtomicU64,
    /// Requests of any kind, also refused ones.
    pub requests: AtomicU64,
    /// Reply status for every request; 0 or 200 = normal handling.
    pub status: AtomicU16,
    /// Accept the connection and the request, but never reply.
    pub hang: AtomicBool,
}

pub struct Receiver {
    pub port: u16,
    pub state: Arc<RState>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Receiver {
    /// Listen on 127.0.0.1:`port` (0 = any free port).
    pub fn start(port: u16) -> Receiver {
        Self::start_with(port, Arc::new(RState::default()))
    }

    /// Listen again with the same state (the "server came back").
    pub fn start_with(port: u16, state: Arc<RState>) -> Receiver {
        let l = TcpListener::bind(("127.0.0.1", port)).expect("bind receiver");
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp) = (state.clone(), stop.clone());
        let thread = thread::spawn(move || {
            while !sp.load(Relaxed) {
                match l.accept() {
                    Ok((s, _)) => {
                        let st = st.clone();
                        thread::spawn(move || handle(s, &st));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        Receiver {
            port,
            state,
            stop,
            thread: Some(thread),
        }
    }

    /// Close the listener. New connections are refused.
    pub fn stop(mut self) -> (u16, Arc<RState>) {
        self.halt();
        (self.port, self.state.clone())
    }

    fn halt(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.halt();
    }
}

impl RState {
    /// Unique (session, seq) pairs stored, sorted by seq.
    pub fn seqs(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.seen.lock().unwrap().iter().map(|x| x.1).collect();
        v.sort();
        v
    }

    pub fn batch_count(&self) -> usize {
        self.batches.lock().unwrap().len()
    }

    /// Wait until `f` is true, at most `max`.
    pub fn wait_for(&self, max: Duration, f: impl Fn(&RState) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < max {
            if f(self) {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        f(self)
    }
}

fn reply(s: &mut TcpStream, status: u16, body: &str) {
    let r = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(r.as_bytes());
}

fn handle(mut s: TcpStream, st: &RState) {
    s.set_nonblocking(false).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + 4 + len {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = &buf[head_end + 4..head_end + 4 + len];
    st.requests.fetch_add(1, Relaxed);
    if st.hang.load(Relaxed) {
        thread::sleep(Duration::from_secs(20));
        return;
    }
    let status = st.status.load(Relaxed);
    if status != 0 && status != 200 {
        reply(&mut s, status, "{\"error\":\"test\"}");
        return;
    }
    if !head.contains(&format!("\r\nAuthorization: Bearer {TOKEN}\r\n")) {
        reply(&mut s, 401, "{\"error\":\"bad token\"}");
        return;
    }
    let path = head.split_whitespace().nth(1).unwrap_or("");
    match path {
        "/api/v1/events" => match serde_json::from_slice::<EventBatch>(body) {
            Ok(b) => {
                let (mut a, mut d) = (0, 0);
                {
                    let mut seen = st.seen.lock().unwrap();
                    for e in &b.events {
                        if seen.insert((e.session_id.clone(), e.seq)) {
                            a += 1;
                        } else {
                            d += 1;
                        }
                    }
                }
                st.accepted.fetch_add(a, Relaxed);
                st.duplicates.fetch_add(d, Relaxed);
                st.batches.lock().unwrap().push(b);
                reply(
                    &mut s,
                    200,
                    &format!("{{\"accepted\":{a},\"duplicates\":{d}}}"),
                );
            }
            Err(e) => reply(&mut s, 400, &format!("{{\"error\":{:?}}}", e.to_string())),
        },
        "/api/v1/ping" => match serde_json::from_slice::<Ping>(body) {
            Ok(p) => {
                st.pings.lock().unwrap().push(p);
                reply(&mut s, 200, "{}");
            }
            Err(e) => reply(&mut s, 400, &format!("{{\"error\":{:?}}}", e.to_string())),
        },
        _ => reply(&mut s, 404, "{\"error\":\"not found\"}"),
    }
}

/// A port where nothing listens (connections are refused).
pub fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}
