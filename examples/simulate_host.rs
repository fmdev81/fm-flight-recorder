//! Drive the real recorder the way FileMaker would, without FileMaker.
//!
//!   cargo run --example simulate_host -- [LOG_DIR]
//!
//! * The main thread runs a CFRunLoop with the run-loop observer installed,
//!   like FileMaker's UI thread.
//! * A 50 ms timer sends "idle" heartbeats and offers context (layout,
//!   script), like kFMXT_Idle.
//! * A fake "FileMaker Server" on 127.0.0.1 answers requests after a delay
//!   the client asks for (WAN latency, a record lock...).
//! * A scenario freezes the main thread in four typical ways.
//!
//! The JSONL it writes is the same format the plug-in writes.

use fm_flight_recorder::context::Context;
use fm_flight_recorder::recorder::{Config, Level, Recorder};
use fm_flight_recorder::runloop::{self, RunLoopObserver, Timer};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Request = 4-byte LE delay in ms + payload. Reply after the delay with 3 KB.
fn fake_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for s in l.incoming() {
            let mut s = s.unwrap();
            thread::spawn(move || {
                let mut buf = [0u8; 64];
                loop {
                    if s.read_exact(&mut buf[..4]).is_err() {
                        break;
                    }
                    let ms = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                    thread::sleep(Duration::from_millis(ms as u64));
                    if s.write_all(&[1u8; 3000]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

struct Client(TcpStream);
impl Client {
    fn call(&mut self, server_ms: u32) {
        self.0.write_all(&server_ms.to_le_bytes()).unwrap();
        let mut got = 0;
        let mut buf = [0u8; 4096];
        while got < 3000 {
            got += self.0.read(&mut buf).unwrap();
        }
    }
}

fn ctx(layout: &str, table: &str, script: &str) -> Context {
    Context {
        file: "Invoices".into(),
        layout: layout.into(),
        layout_table: table.into(),
        script: script.into(),
        account: "demo".into(),
        user: "Demo User".into(),
        window: "Invoices".into(),
        host: "127.0.0.1".into(),
        mode: "0".into(),
        ..Default::default()
    }
}

fn busy_cpu(ms: u64) {
    let t = Instant::now();
    let mut x = 0u64;
    while t.elapsed() < Duration::from_millis(ms) {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    std::hint::black_box(x);
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "demo-logs".into());
    let port = fake_server();
    let cfg = Config {
        threshold_ms: 250,
        net_port: Some(port),
        net_sample_ms: 1,
        log_dir: dir.clone().into(),
        heartbeat: "idle+runloop (simulated)".into(),
        app: "simulate_host".into(),
        ..Default::default()
    };
    let rec = Arc::new(Recorder::start(
        cfg,
        fm_flight_recorder::default_probe(Some(port)),
    ));
    rec.watch_current_thread();
    let r2 = rec.clone();
    let _obs = unsafe {
        RunLoopObserver::install(runloop::CFRunLoopGetCurrent(), move |a| {
            runloop::feed(&r2, a)
        })
    };

    let client = Rc::new(RefCell::new(Client(
        TcpStream::connect(("127.0.0.1", port)).unwrap(),
    )));
    client.borrow().0.set_nodelay(true).unwrap();
    let here = Rc::new(RefCell::new((
        ctx("Dashboard", "Dashboard", ""),
        Level::UserNotIdle,
    )));

    // kFMXT_Idle every 50 ms.
    let (r3, h3) = (rec.clone(), here.clone());
    let _idle = Timer::every(0.05, move || {
        let (c, level) = h3.borrow().clone();
        r3.beat(level);
        if r3.wants_context(level) {
            r3.offer_context(c);
        }
    });

    // The scenario, one step per second.
    let (r4, h4, c4) = (rec.clone(), here.clone(), client.clone());
    let mut step = 0;
    let _scenario = Timer::every(1.0, move || {
        step += 1;
        let mut cl = c4.borrow_mut();
        match step {
            1 => {
                // Portal on a WAN: 30 sequential queries, 25 ms each.
                println!("step 1: Invoices | Detail portal, 30 round trips at 25 ms");
                r4.mark(
                    "go to Invoices",
                    ctx("Dashboard", "Dashboard", "Go To Invoices"),
                );
                for _ in 0..30 {
                    cl.call(25);
                }
                *h4.borrow_mut() = (ctx("Invoices | Detail", "Invoices", ""), Level::UserNotIdle);
            }
            3 => {
                // Unstored calcs on a list: CPU-bound, few round trips.
                println!("step 3: Customers | List, 700 ms of CPU, 3 round trips");
                for _ in 0..3 {
                    cl.call(2);
                }
                busy_cpu(700);
                *h4.borrow_mut() = (ctx("Customers | List", "Customers", ""), Level::UserNotIdle);
            }
            5 => {
                // OnRecordLoad trigger loop: many quick trips plus CPU.
                println!("step 5: OnRecordLoad trigger, 60 round trips at 8 ms + CPU");
                *h4.borrow_mut() = (
                    ctx("Customers | List", "Customers", "OnRecordLoad: Refresh"),
                    Level::ScriptRunning,
                );
                for i in 0..60 {
                    cl.call(8);
                    if i % 10 == 0 {
                        busy_cpu(15);
                    }
                }
                *h4.borrow_mut() = (ctx("Customers | List", "Customers", ""), Level::UserNotIdle);
            }
            7 => {
                // Commit waits on a record lock held by another user: 1 trip, long.
                println!("step 7: Commit Records waits 2 s on the server, 1 round trip");
                cl.call(2000);
            }
            9 => {
                println!("step 9: done");
                runloop::stop_current();
            }
            _ => {}
        }
    });

    runloop::run_for(15.0);
    drop(_obs);
    let st = rec.stop();
    println!(
        "{} freezes recorded, {} heartbeats -> {}",
        st.freezes, st.heartbeats, st.log
    );
}
