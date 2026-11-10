//! The sender against a real local HTTP receiver (tests/common): batching,
//! bounded queue, error replies, and a stop that never waits for the
//! network.

mod common;

use common::{closed_port, Receiver, TOKEN};
use fm_flight_recorder::context::Context;
use fm_flight_recorder::recorder::{Config, Recorder};
use fm_flight_recorder::wire::EventLine;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fr-send-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn cfg(dir: &Path, endpoint: &str) -> Config {
    let mut c = Config {
        log_dir: dir.to_path_buf(),
        state_dir: dir.join("state"),
        net_port: None,
        app: "test".into(),
        send_interval_ms: 100,
        send_batch_max: 5,
        send_retry_ms: 100,
        ..Default::default()
    };
    let r = c.apply_options(&format!("endpoint={endpoint}; token={TOKEN}"));
    assert!(r.errors.is_empty() && r.ignored.is_empty(), "{r:?}");
    c
}

fn log_lines(dir: &Path) -> Vec<EventLine> {
    let mut out = Vec::new();
    for f in std::fs::read_dir(dir).unwrap() {
        let p = f.unwrap().path();
        if p.extension().is_some_and(|e| e == "jsonl") {
            let t = std::fs::read_to_string(p).unwrap();
            out.extend(
                t.lines()
                    .map(|l| serde_json::from_str::<EventLine>(l).unwrap()),
            );
        }
    }
    out
}

fn marks(rec: &Recorder, n: usize) -> Duration {
    let mut worst = Duration::ZERO;
    for i in 0..n {
        let t = Instant::now();
        rec.mark(&format!("m{i}"), Context::default());
        worst = worst.max(t.elapsed());
    }
    worst
}

#[test]
fn events_arrive_in_batches_with_token_and_identity() {
    let srv = Receiver::start(0);
    let dir = tmp("batches");
    let rec = Recorder::start(
        cfg(&dir, &srv.endpoint()),
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    marks(&rec, 12);
    let want: Vec<u64> = (1..=13).collect();
    assert!(
        srv.state
            .wait_for(Duration::from_secs(5), |s| s.seqs() == want),
        "{:?}",
        srv.state.seqs()
    );
    let sender = rec.sender().unwrap();
    assert_eq!(sender.stats.sent.load(Relaxed), 13);
    assert_eq!(sender.stats.dropped.load(Relaxed), 0);
    {
        let batches = srv.state.batches.lock().unwrap();
        assert!(batches.len() >= 3, "{}", batches.len());
        for b in batches.iter() {
            assert!(b.events.len() <= 5, "{}", b.events.len());
            assert_eq!(b.session_id, rec.session_id());
            assert_eq!(b.client.install_id.len(), 32);
            assert!(b.events.iter().all(|e| e.session_id == b.session_id));
        }
    }
    assert_eq!(srv.state.duplicates.load(Relaxed), 0);
    rec.stop();
    // The JSONL log has everything, also session_stop that was not sent.
    let lines = log_lines(&dir);
    assert_eq!(lines.len(), 14);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_small_batch_waits_for_the_interval() {
    let srv = Receiver::start(0);
    let dir = tmp("interval");
    let mut c = cfg(&dir, &srv.endpoint());
    c.send_interval_ms = 400;
    c.send_batch_max = 100;
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    let t0 = Instant::now();
    rec.mark("one", Context::default());
    thread::sleep(Duration::from_millis(150));
    assert_eq!(srv.state.batch_count(), 0, "sent before the interval");
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.seqs() == [1, 2]));
    assert!(
        t0.elapsed() >= Duration::from_millis(350),
        "{:?}",
        t0.elapsed()
    );
    assert_eq!(srv.state.batch_count(), 1, "one batch for both events");
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn send_ms_option_sets_the_batch_interval() {
    let srv = Receiver::start(0);
    let dir = tmp("send-ms");
    let mut c = Config {
        log_dir: dir.clone(),
        state_dir: dir.join("state"),
        net_port: None,
        app: "test".into(),
        send_batch_max: 100,
        ..Default::default()
    };
    let r = c.apply_options(&format!(
        "endpoint={}; token={TOKEN}; send_ms=250; ping_ms=5000",
        srv.endpoint()
    ));
    assert!(r.errors.is_empty() && r.ignored.is_empty(), "{r:?}");
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    let st = rec.status();
    assert_eq!((st.send_ms, st.ping_ms), (250, 5000));
    // session_start goes out alone, about 250 ms after the start.
    assert!(srv.state.wait_for(Duration::from_secs(3), |s| s.seqs() == [1]));
    // A new event arrives 250 ms (plus the post) after it was written,
    // not 5 s (the default).
    let t0 = Instant::now();
    rec.mark("live", Context::default());
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.seqs() == [1, 2]));
    let took = t0.elapsed();
    assert!(
        took >= Duration::from_millis(200) && took < Duration::from_millis(1500),
        "{took:?}"
    );
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn queue_is_bounded_when_the_server_is_down() {
    let dir = tmp("bounded");
    let mut c = cfg(&dir, &format!("http://127.0.0.1:{}", closed_port()));
    c.send_queue_max = 10;
    c.send_spool_max_bytes = 0; // no spool: unsent events wait in memory
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    marks(&rec, 50);
    thread::sleep(Duration::from_millis(300));
    let s = rec.sender().unwrap();
    assert!(s.queue.len() <= 10, "{}", s.queue.len());
    // 51 events (session_start + 50 marks), at most 10 kept.
    assert!(
        s.stats.dropped.load(Relaxed) >= 41,
        "{}",
        s.stats.dropped.load(Relaxed)
    );
    assert!(s.stats.failures.load(Relaxed) >= 1);
    assert!(s.stats.last_error.lock().unwrap().starts_with("send: "));
    assert_eq!(s.stats.sent.load(Relaxed), 0);
    rec.stop();
    assert_eq!(log_lines(&dir).len(), 52, "the log still has every event");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_and_marks_do_not_wait_for_a_hanging_server() {
    let srv = Receiver::start(0);
    srv.state.hang.store(true, Relaxed);
    let dir = tmp("hang");
    let mut c = cfg(&dir, &srv.endpoint());
    c.send_interval_ms = 10;
    c.send_io_ms = 10_000;
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    rec.mark("a", Context::default());
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.requests.load(Relaxed) >= 1));
    // A post is in flight and blocked. The main-thread API stays fast.
    let worst = marks(&rec, 200);
    assert!(worst < Duration::from_millis(5), "slowest mark {worst:?}");
    let t = Instant::now();
    rec.stop();
    let took = t.elapsed();
    eprintln!("stop took {took:?}");
    assert!(took < Duration::from_millis(150), "stop took {took:?}");
    let s = rec.sender().unwrap();
    assert_eq!(s.stats.sent.load(Relaxed), 0);
    // 1 start + 1 + 200 marks + stop, none sent: all in the spool.
    assert_eq!(s.stats.dropped.load(Relaxed), 0);
    assert_eq!(s.stats.spooled.load(Relaxed), 203);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_does_not_wait_for_a_connect_that_hangs() {
    // A non-routable address: connect waits for its timeout (or fails
    // at once on some networks). Stop must not wait either way.
    let dir = tmp("connect");
    let mut c = cfg(&dir, "http://10.255.255.1:9");
    c.send_interval_ms = 10;
    c.send_connect_ms = 3000;
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    rec.mark("a", Context::default());
    thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    rec.stop();
    let took = t.elapsed();
    eprintln!("stop took {took:?}");
    assert!(took < Duration::from_millis(150), "stop took {took:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_token_stops_sending() {
    let srv = Receiver::start(0);
    let dir = tmp("401");
    let mut c = cfg(&dir, &srv.endpoint());
    c.token = "wrong".into();
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    rec.mark("a", Context::default());
    let s = rec.sender().unwrap();
    assert!(srv.state.wait_for(Duration::from_secs(3), |_| s
        .stats
        .auth_failed
        .load(Relaxed)));
    marks(&rec, 20);
    thread::sleep(Duration::from_millis(400));
    assert_eq!(srv.state.requests.load(Relaxed), 1, "no retry after 401");
    assert!(s.stats.last_error.lock().unwrap().contains("401"));
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refused_batch_is_dropped_not_retried() {
    let srv = Receiver::start(0);
    srv.state.status.store(400, Relaxed);
    let dir = tmp("400");
    let mut c = cfg(&dir, &srv.endpoint());
    c.send_batch_max = 100;
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    marks(&rec, 3);
    let s = rec.sender().unwrap();
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |_| s.stats.dropped.load(Relaxed)
            == 4));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(srv.state.requests.load(Relaxed), 1);
    assert!(s.queue.is_empty());
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn server_error_is_retried() {
    let srv = Receiver::start(0);
    srv.state.status.store(503, Relaxed);
    let dir = tmp("503");
    let rec = Recorder::start(
        cfg(&dir, &srv.endpoint()),
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    marks(&rec, 2);
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.requests.load(Relaxed) >= 2));
    srv.state.status.store(200, Relaxed);
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.seqs() == [1, 2, 3]));
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn status_shows_send_counters_and_errors() {
    // Server up: sent and last_send.
    let srv = Receiver::start(0);
    let dir = tmp("status-ok");
    let rec = Recorder::start(
        cfg(&dir, &srv.endpoint()),
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    marks(&rec, 4);
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.seqs().len() == 5));
    let st = rec.status();
    assert_eq!(
        (st.sent, st.queued, st.dropped, st.send_failures),
        (5, 0, 0, 0),
        "{st:?}"
    );
    assert!(st.last_send.ends_with('Z'), "{}", st.last_send);
    assert_eq!(st.last_send_error, "");
    assert_eq!(st.endpoint, srv.endpoint());
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);

    // Server down: failures, spooled, last_send_error.
    let dir = tmp("status-down");
    let rec = Recorder::start(
        cfg(&dir, &format!("http://127.0.0.1:{}", closed_port())),
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    marks(&rec, 4);
    let s = rec.sender().unwrap();
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |_| s.stats.spooled.load(Relaxed)
            == 5));
    let st = rec.status();
    assert!(st.send_failures >= 1, "{st:?}");
    assert_eq!((st.sent, st.spooled, st.queued), (0, 5, 0));
    assert!(
        st.last_send_error.starts_with("send: "),
        "{}",
        st.last_send_error
    );
    assert_eq!(st.last_send, "");
    assert!(!st.auth_failed);
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
