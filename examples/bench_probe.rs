//! Cost of the main-thread heartbeat and of one round-trip probe sample.
//!   cargo run --release --example bench_probe
use fm_flight_recorder::net::{NetProbe, TcpPortProbe};
use fm_flight_recorder::recorder::{Config, Level, Recorder};
use std::net::{TcpListener, TcpStream};
use std::time::Instant;

fn main() {
    // 4 connections to the watched port, 40 other sockets as noise.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let _c: Vec<_> = (0..4)
        .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
        .collect();
    let _n: Vec<_> = (0..40)
        .map(|_| TcpStream::connect(other.local_addr().unwrap()).unwrap())
        .collect();

    let mut p = TcpPortProbe::new(port);
    p.sample();
    let n = 20_000;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(p.sample());
    }
    let per = t.elapsed().as_nanos() as f64 / n as f64;
    println!(
        "probe.sample(), 4 watched sockets: {:.2} µs  -> at 2 ms interval: {:.3}% of one core",
        per / 1000.0,
        per / 2_000_000.0 * 100.0
    );

    let t = Instant::now();
    let m = 200;
    for _ in 0..m {
        let mut q = TcpPortProbe::new(port);
        std::hint::black_box(q.sample()); // forces a full fd scan
    }
    println!(
        "full fd scan (~90 fds) + sample: {:.1} µs (every 2 s)",
        t.elapsed().as_nanos() as f64 / m as f64 / 1000.0
    );

    let dir = std::env::temp_dir().join("fr-bench");
    let rec = Recorder::start(
        Config {
            net_port: None,
            log_dir: dir,
            ..Default::default()
        },
        Box::new(fm_flight_recorder::net::NoProbe),
    );
    let k = 1_000_000;
    let t = Instant::now();
    for _ in 0..k {
        rec.beat(Level::UserNotIdle);
    }
    println!(
        "Recorder::beat() (main thread): {:.1} ns",
        t.elapsed().as_nanos() as f64 / k as f64
    );
    rec.stop();
}
