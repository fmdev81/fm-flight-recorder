//! Offline queue and retry (#12): the receiver goes down and comes back on
//! the same port; events must arrive once, the spool must go first, the
//! retry wait must grow, and the spool must stay under its cap.

mod common;

use common::{Receiver, TOKEN};
use fm_flight_recorder::context::Context;
use fm_flight_recorder::recorder::{Config, Recorder};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fr-spool-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn cfg(dir: &Path, port: u16) -> Config {
    let mut c = Config {
        log_dir: dir.to_path_buf(),
        state_dir: dir.join("state"),
        net_port: None,
        app: "test".into(),
        send_interval_ms: 50,
        send_batch_max: 5,
        send_retry_ms: 100,
        send_retry_max_ms: 400,
        ..Default::default()
    };
    let r = c.apply_options(&format!("endpoint=http://127.0.0.1:{port}; token={TOKEN}"));
    assert!(r.errors.is_empty(), "{r:?}");
    c
}

fn marks(rec: &Recorder, n: usize) {
    for i in 0..n {
        rec.mark(&format!("m{i}"), Context::default());
    }
}

fn wait(max: Duration, f: impl Fn() -> bool) -> bool {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < max {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    f()
}

/// A free port with the receiver stopped: (port, state for the restart).
fn down_receiver() -> (u16, std::sync::Arc<common::RState>) {
    Receiver::start(0).stop()
}

#[test]
fn server_down_then_up_all_events_arrive_once() {
    let (port, state) = down_receiver();
    let dir = tmp("downup");
    let rec = Recorder::start(cfg(&dir, port), Box::new(fm_flight_recorder::net::NoProbe));
    marks(&rec, 20);
    let s = rec.sender().unwrap();
    assert!(
        wait(Duration::from_secs(3), || s.queue.is_empty()
            && s.stats.spooled.load(Relaxed) == 21),
        "queue {} spooled {}",
        s.queue.len(),
        s.stats.spooled.load(Relaxed)
    );
    assert_eq!(s.stats.sent.load(Relaxed), 0);
    assert!(s.stats.failures.load(Relaxed) >= 1);
    assert!(dir.join("spool").read_dir().unwrap().count() >= 1);

    // The server comes back on the same port.
    let srv = Receiver::start_with(port, state);
    let want: Vec<u64> = (1..=21).collect();
    assert!(
        srv.state
            .wait_for(Duration::from_secs(5), |st| st.seqs() == want),
        "{:?}",
        srv.state.seqs()
    );
    assert!(wait(Duration::from_secs(2), || s
        .stats
        .spooled
        .load(Relaxed)
        == 0));
    assert_eq!(srv.state.duplicates.load(Relaxed), 0);
    assert_eq!(s.stats.sent.load(Relaxed), 21);

    // Back to normal sending.
    marks(&rec, 5);
    let want: Vec<u64> = (1..=26).collect();
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |st| st.seqs() == want));
    assert_eq!(srv.state.duplicates.load(Relaxed), 0);
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsent_events_are_spooled_at_stop_and_sent_first_at_next_start() {
    let (port, state) = down_receiver();
    let dir = tmp("restart");
    // Session A: the server is down and nothing is sent (long interval).
    let mut ca = cfg(&dir, port);
    ca.send_interval_ms = 60_000;
    let a = Recorder::start(ca, Box::new(fm_flight_recorder::net::NoProbe));
    marks(&a, 5);
    a.stop();
    let sid_a = a.session_id().to_string();
    assert_eq!(
        a.sender().unwrap().stats.spooled.load(Relaxed),
        7,
        "start + 5 marks + stop"
    );

    // "FileMaker restarts" with the server up.
    let srv = Receiver::start_with(port, state);
    let b = Recorder::start(cfg(&dir, port), Box::new(fm_flight_recorder::net::NoProbe));
    let sid_b = b.session_id().to_string();
    assert!(srv.state.wait_for(Duration::from_secs(5), |st| {
        let seen: Vec<(String, u64)> = st
            .batches
            .lock()
            .unwrap()
            .iter()
            .flat_map(|x| x.events.iter().map(|e| (e.session_id.clone(), e.seq)))
            .collect();
        (1..=7).all(|q| seen.contains(&(sid_a.clone(), q))) && seen.contains(&(sid_b.clone(), 1))
    }));
    let batches = srv.state.batches.lock().unwrap();
    assert_eq!(batches[0].session_id, sid_a, "the spool goes first");
    assert!(batches[0].events.iter().all(|e| e.session_id == sid_a));
    drop(batches);
    assert_eq!(srv.state.duplicates.load(Relaxed), 0);
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retry_wait_grows_after_each_failure() {
    let srv = Receiver::start(0);
    srv.state.status.store(503, Relaxed);
    let dir = tmp("backoff");
    let rec = Recorder::start(
        cfg(&dir, srv.port),
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    rec.mark("a", Context::default());
    thread::sleep(Duration::from_millis(1600));
    // Tries near 0.05, 0.15, 0.35, 0.75, 1.15, 1.55 s (100 ms doubling to
    // 400 ms). A fixed 100 ms wait would give about 15.
    let n = srv.state.requests.load(Relaxed);
    assert!((4..=8).contains(&n), "{n} requests");
    // The batch is in the spool while it waits.
    assert_eq!(rec.sender().unwrap().stats.spooled.load(Relaxed), 2);
    srv.state.status.store(200, Relaxed);
    assert!(srv
        .state
        .wait_for(Duration::from_secs(2), |st| st.seqs() == [1, 2]));
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spool_cap_drops_the_oldest_events() {
    let (port, state) = down_receiver();
    let dir = tmp("cap");
    let mut c = cfg(&dir, port);
    c.send_spool_max_bytes = 4000;
    let rec = Recorder::start(c, Box::new(fm_flight_recorder::net::NoProbe));
    for _ in 0..10 {
        marks(&rec, 10);
        thread::sleep(Duration::from_millis(60));
    }
    let s = rec.sender().unwrap();
    assert!(wait(Duration::from_secs(3), || s.queue.is_empty()));
    let spooled = s.stats.spooled.load(Relaxed);
    let dropped = s.stats.dropped.load(Relaxed);
    assert!(dropped > 0, "nothing dropped");
    assert_eq!(
        spooled + dropped,
        101,
        "spooled {spooled} dropped {dropped}"
    );
    let bytes: u64 = dir
        .join("spool")
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(bytes <= 4000, "{bytes} bytes in the spool");

    // The newest events are kept; the oldest are gone.
    let srv = Receiver::start_with(port, state);
    assert!(srv
        .state
        .wait_for(Duration::from_secs(5), |st| st.seqs().len() as u64
            == spooled));
    let seqs = srv.state.seqs();
    assert_eq!(*seqs.last().unwrap(), 101);
    assert!(seqs[0] > 1);
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
