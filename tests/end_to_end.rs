//! End-to-end tests without FileMaker: real TCP sockets in this process
//! stand in for the FileMaker Server connection, a thread that calls
//! `beat()` stands in for FileMaker's idle messages.

#![cfg(target_os = "macos")]

use fm_flight_recorder::context::Context;
use fm_flight_recorder::eventlog::{parse_line, Event};
use fm_flight_recorder::net::{NetHistory, NetProbe, TcpPortProbe};
use fm_flight_recorder::recorder::{mono_ns, Config, Level, Recorder};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

/// A fake "FileMaker Server": answers every request with `reply` bytes.
fn fake_server(reply: usize) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for s in l.incoming() {
            let mut s = s.unwrap();
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let out = vec![7u8; reply];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if s.write_all(&out).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

fn round_trip(c: &mut TcpStream, reply: usize) {
    c.write_all(b"query").unwrap();
    let mut got = 0;
    let mut buf = [0u8; 8192];
    while got < reply {
        got += c.read(&mut buf).unwrap();
    }
}

#[test]
fn probe_counts_round_trips_on_a_real_socket() {
    let port = fake_server(600);
    let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
    client.set_nodelay(true).unwrap();
    let mut probe = TcpPortProbe::new(port);
    let mut hist = NetHistory::new(60);

    // Sample on a thread at 1 ms while the "client" does 20 round trips
    // with 6 ms think time between them.
    let (tx, rx) = std::sync::mpsc::channel();
    let sampler = thread::spawn(move || {
        let t0 = mono_ns();
        while rx.try_recv().is_err() {
            if let Some(c) = probe.sample() {
                hist.push(mono_ns(), c);
            }
            thread::sleep(Duration::from_millis(1));
        }
        hist.window(t0, mono_ns())
    });
    thread::sleep(Duration::from_millis(20));
    for _ in 0..20 {
        round_trip(&mut client, 600);
        thread::sleep(Duration::from_millis(6));
    }
    thread::sleep(Duration::from_millis(20));
    tx.send(()).unwrap();
    let w = sampler.join().unwrap();
    assert!(w.connected);
    assert_eq!(w.tx_bytes, 20 * 5, "{w:?}");
    assert_eq!(w.rx_bytes, 20 * 600, "{w:?}");
    // Lower bound by design; with 6 ms gaps and 1 ms sampling it is exact or close.
    assert!((17..=20).contains(&w.round_trips), "{w:?}");
}

#[test]
fn probe_ignores_other_ports() {
    let port = fake_server(10);
    let _client = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut probe = TcpPortProbe::new(1); // tcpmux: nothing here connects to it
    let c = probe.sample().unwrap();
    assert_eq!(c.connections, 0);
}

fn read_events(dir: &std::path::Path) -> Vec<Event> {
    let mut out = Vec::new();
    for f in std::fs::read_dir(dir).unwrap() {
        let p = f.unwrap().path();
        if p.extension().is_none_or(|e| e != "jsonl") {
            continue; // the state/ folder with the install id
        }
        let text = std::fs::read_to_string(p).unwrap();
        out.extend(text.lines().filter_map(parse_line));
    }
    out
}

#[test]
fn recorder_logs_a_freeze_with_context_and_round_trips() {
    let port = fake_server(2000);
    let dir = std::env::temp_dir().join(format!("fr-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = Config {
        threshold_ms: 150,
        poll_ms: 5,
        net_port: Some(port),
        net_sample_ms: 1,
        log_dir: dir.clone(),
        state_dir: dir.join("state"),
        app: "test".into(),
        ..Default::default()
    };
    let rec = Recorder::start(cfg, fm_flight_recorder::default_probe(Some(port)));
    let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
    client.set_nodelay(true).unwrap();
    thread::sleep(Duration::from_millis(30)); // let the probe find the socket

    // "Main thread": healthy idle beats for 200 ms, with context.
    for _ in 0..20 {
        rec.beat(Level::UserNotIdle);
        if rec.wants_context(Level::UserNotIdle) {
            rec.offer_context(Context {
                file: "Invoices".into(),
                layout: "Customers".into(),
                ..Default::default()
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
    rec.mark("open invoice", Context::default());
    // Freeze: 600 ms with 12 round trips and no beats.
    for _ in 0..12 {
        round_trip(&mut client, 2000);
        thread::sleep(Duration::from_millis(45));
    }
    thread::sleep(Duration::from_millis(60));
    // Back: beats resume, the glue offers the "after" context.
    for _ in 0..10 {
        rec.beat(Level::UserNotIdle);
        if rec.wants_context(Level::UserNotIdle) {
            rec.offer_context(Context {
                file: "Invoices".into(),
                layout: "Invoice Detail".into(),
                ..Default::default()
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
    let st = rec.stop();
    assert_eq!(st.freezes, 1, "{st:?}");

    let events = read_events(&dir);
    assert!(
        matches!(events.first(), Some(Event::SessionStart { .. })),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(Event::SessionStop { freezes: 1, .. })),
        "{events:?}"
    );
    let freeze = events.iter().find_map(|e| match e {
        Event::Freeze {
            duration_ms,
            before,
            after,
            net,
            marks,
            ..
        } => Some((*duration_ms, before, after, net, marks)),
        _ => None,
    });
    let (dur, before, after, net, marks) = freeze.expect("a freeze event");
    assert!((550..=900).contains(&dur), "duration {dur}");
    assert_eq!(before.layout, "Customers");
    assert_eq!(after.as_ref().unwrap().layout, "Invoice Detail");
    assert!(net.connected);
    assert!((10..=12).contains(&net.round_trips), "{net:?}");
    // The fake server answers at once: waits are tiny next to the 45 ms gaps.
    assert!(net.wait_ms < 120, "{net:?}");
    assert_eq!(marks, &vec!["open invoice".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn idle_gaps_while_run_loop_sleeps_are_not_freezes() {
    let dir = std::env::temp_dir().join(format!("fr-e2e-sleep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = Config {
        threshold_ms: 50,
        poll_ms: 5,
        net_port: None,
        log_dir: dir.clone(),
        state_dir: dir.join("state"),
        ..Default::default()
    };
    let rec = Recorder::start(cfg, fm_flight_recorder::default_probe(None));
    rec.beat(Level::UserIdle);
    rec.set_waiting(true);
    thread::sleep(Duration::from_millis(300));
    rec.set_waiting(false);
    rec.beat(Level::UserIdle);
    let st = rec.stop();
    assert_eq!(st.freezes, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn long_script_is_logged_as_script_busy() {
    let dir = std::env::temp_dir().join(format!("fr-e2e-busy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = Config {
        threshold_ms: 100,
        poll_ms: 5,
        net_port: None,
        log_dir: dir.clone(),
        state_dir: dir.join("state"),
        ..Default::default()
    };
    let rec = Recorder::start(cfg, fm_flight_recorder::default_probe(None));
    rec.offer_context(Context {
        script: "Nightly Import".into(),
        ..Default::default()
    });
    // Script running, but FileMaker still sends idle beats: busy, not frozen.
    for _ in 0..30 {
        rec.beat(Level::ScriptRunning);
        thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..5 {
        rec.beat(Level::UserNotIdle);
        thread::sleep(Duration::from_millis(10));
    }
    let st = rec.stop();
    assert_eq!(st.freezes, 0);
    let events = read_events(&dir);
    let busy = events.iter().find_map(|e| match e {
        Event::ScriptBusy {
            duration_ms,
            before,
            ..
        } => Some((*duration_ms, before.script.clone())),
        _ => None,
    });
    let (d, script) = busy.expect("script_busy event");
    assert!(d >= 250, "{d}");
    assert_eq!(script, "Nightly Import");
    let _ = std::fs::remove_dir_all(&dir);
}
