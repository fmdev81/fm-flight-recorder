//! Ping (#13): the sender tells the server that the client is alive, with
//! the Status counters, even when there are no events to send.

mod common;

use common::{Receiver, TOKEN};
use fm_flight_recorder::recorder::{Config, Level, Recorder};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fr-ping-{name}-{}", std::process::id()));
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
        send_ping_ms: 200,
        send_retry_ms: 100,
        send_retry_max_ms: 200,
        ..Default::default()
    };
    let r = c.apply_options(&format!("endpoint=http://127.0.0.1:{port}; token={TOKEN}"));
    assert!(r.errors.is_empty(), "{r:?}");
    c
}

#[test]
fn pings_carry_the_status_counters() {
    let srv = Receiver::start(0);
    let dir = tmp("counters");
    let rec = Recorder::start(
        cfg(&dir, srv.port),
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    for _ in 0..10 {
        rec.beat(Level::UserIdle);
    }
    rec.beat(Level::RunLoop);
    assert!(srv
        .state
        .wait_for(Duration::from_secs(3), |s| s.pings.lock().unwrap().len()
            >= 2));
    let st = rec.status();
    {
        let pings = srv.state.pings.lock().unwrap();
        let p = pings.last().unwrap();
        assert_eq!(p.session_id, rec.session_id());
        assert!(p.running);
        assert_eq!((p.heartbeats, p.idle_messages, p.freezes), (11, 10, 0));
        assert_eq!(p.heartbeats, st.heartbeats);
        assert_eq!(p.last_seq, 1, "session_start");
        assert_eq!(p.client.install_id.len(), 32);
        assert!(p.ts.ends_with('Z'));
    }
    let s = rec.sender().unwrap();
    assert!(s.stats.pings.load(Relaxed) >= 2);
    assert!(s.stats.last_ping_unix_ms.load(Relaxed) > 0);
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_ping_without_an_endpoint() {
    let dir = tmp("none");
    let rec = Recorder::start(
        Config {
            log_dir: dir.clone(),
            state_dir: dir.join("state"),
            net_port: None,
            send_ping_ms: 100,
            ..Default::default()
        },
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    assert!(rec.sender().is_none(), "no sender thread, so no ping");
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_ping_is_not_queued() {
    let (port, state) = Receiver::start(0).stop();
    let dir = tmp("failed");
    let rec = Recorder::start(cfg(&dir, port), Box::new(fm_flight_recorder::net::NoProbe));
    // About 5 ping times with the server down.
    thread::sleep(Duration::from_millis(1100));
    let s = rec.sender().unwrap();
    assert_eq!(s.stats.pings.load(Relaxed), 0);
    let srv = Receiver::start_with(port, state);
    assert!(srv.state.wait_for(Duration::from_secs(2), |s| !s
        .pings
        .lock()
        .unwrap()
        .is_empty()));
    // No burst of old pings: one now, then one each interval.
    thread::sleep(Duration::from_millis(100));
    let n = srv.state.pings.lock().unwrap().len();
    assert_eq!(n, 1, "{n} pings right after the server came back");
    rec.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
