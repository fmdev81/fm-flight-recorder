//! The recorder: connects heartbeats, the watchdog, the round-trip sampler
//! and the log writer.
//!
//! Threads:
//! * main thread (FileMaker's): calls [`Recorder::beat`], [`Recorder::set_waiting`],
//!   [`Recorder::offer_context`], [`Recorder::mark`]. These only touch
//!   atomics or a mutex that nothing holds for long; no I/O.
//! * watchdog: polls the heartbeat every `poll_ms`, runs the [`StallDetector`].
//! * net sampler: reads TCP counters every `net_sample_ms`.
//! * writer: appends JSONL lines.

use crate::context::{CaptureGate, Context};
use crate::detector::{BusySpan, Sample, StallDetector, Transition};
use crate::eventlog::{iso_from_unix_ms, Event, LogWriter};
use crate::net::{NetHistory, NetProbe};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::*};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// FileMaker idle levels (FMXExtern.h, FMX_IdleLevel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Level {
    UserIdle = 0,
    UserNotIdle = 1,
    ScriptPaused = 2,
    ScriptRunning = 3,
    Unsafe = 4,
    /// Heartbeat from the run-loop observer, not from FileMaker.
    RunLoop = 9,
}

impl Level {
    pub fn from_u8(v: u8) -> Level {
        match v {
            0 => Level::UserIdle,
            1 => Level::UserNotIdle,
            2 => Level::ScriptPaused,
            3 => Level::ScriptRunning,
            4 => Level::Unsafe,
            _ => Level::RunLoop,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Level::UserIdle => "user_idle",
            Level::UserNotIdle => "user_not_idle",
            Level::ScriptPaused => "script_paused",
            Level::ScriptRunning => "script_running",
            Level::Unsafe => "unsafe",
            Level::RunLoop => "run_loop",
        }
    }
    /// Levels at which the plug-in may call the calc engine during idle.
    pub fn safe_for_calls(self, allow_script_running: bool) -> bool {
        match self {
            Level::UserIdle | Level::UserNotIdle | Level::ScriptPaused => true,
            Level::ScriptRunning => allow_script_running,
            Level::Unsafe | Level::RunLoop => false,
        }
    }
}

/// Monotonic ns since the first call in this process.
pub fn mono_ns() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    // +1 so that 0 can mean "never".
    T0.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

#[derive(Debug, Clone)]
pub struct Config {
    pub threshold_ms: u64,
    pub poll_ms: u64,
    /// TCP peer port to watch. 5003 = FileMaker Server.
    pub net_port: Option<u16>,
    pub net_sample_ms: u64,
    pub context_interval_ms: u64,
    /// Also read Get() values while a script runs (level 3). Off by default.
    pub context_during_scripts: bool,
    pub log_dir: PathBuf,
    /// Free text that goes in session_start, e.g. "idle+runloop".
    pub heartbeat: String,
    pub app: String,
    /// How long a freeze waits for its "after" context before it is written.
    pub after_wait_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            threshold_ms: 250,
            poll_ms: 10,
            net_port: Some(5003),
            net_sample_ms: 2,
            context_interval_ms: 250,
            context_during_scripts: false,
            log_dir: crate::eventlog::default_dir(),
            heartbeat: "idle".into(),
            app: String::new(),
            after_wait_ms: 1500,
        }
    }
}

impl Config {
    /// Parse "threshold=250;port=5003;scripts=1;dir=/path" (FlightRecorder_Start option text).
    /// Unknown keys are ignored. `port=0` disables the round-trip counter.
    pub fn apply_options(&mut self, text: &str) {
        for part in text.split([';', '\n', '\r', ',']) {
            let Some((k, v)) = part.split_once('=') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "threshold" | "threshold_ms" => {
                    if let Ok(n) = v.parse::<u64>() {
                        self.threshold_ms = n.clamp(20, 600_000);
                    }
                }
                "port" => {
                    if let Ok(n) = v.parse::<u16>() {
                        self.net_port = (n != 0).then_some(n);
                    }
                }
                "sample_ms" => {
                    if let Ok(n) = v.parse::<u64>() {
                        self.net_sample_ms = n.clamp(1, 1000);
                    }
                }
                "context_ms" => {
                    if let Ok(n) = v.parse::<u64>() {
                        self.context_interval_ms = n.clamp(0, 60_000);
                    }
                }
                "scripts" => self.context_during_scripts = matches!(v, "1" | "true" | "yes"),
                "dir" if !v.is_empty() => self.log_dir = PathBuf::from(v),
                _ => {}
            }
        }
    }
}

struct Shared {
    last_beat_ns: AtomicU64,
    waiting: AtomicBool,
    level: AtomicU8,
    beats: AtomicU64,
    freezes: AtomicU64,
    running: AtomicBool,
    /// Set by the watchdog after a freeze: capture context at the next safe idle.
    want_context: AtomicBool,
    /// ns of the start of the stall in progress, 0 if none.
    stall_since_ns: AtomicU64,
}

struct Pending {
    event: Event,
    deadline_ns: u64,
}

struct Inner {
    last_context: Context,
    gate: CaptureGate,
    marks: Vec<(u64, String)>,
    pending: Vec<Pending>,
}

/// Mono ns -> wall clock.
#[derive(Clone, Copy)]
struct Clock {
    mono0: u64,
    unix_ms0: i64,
}
impl Clock {
    fn now() -> Self {
        Self { mono0: mono_ns(), unix_ms0: crate::eventlog::unix_ms_now() }
    }
    fn iso(&self, mono: u64) -> String {
        let delta_ms = (mono as i128 - self.mono0 as i128) / 1_000_000;
        iso_from_unix_ms(self.unix_ms0 + delta_ms as i64)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub heartbeats: u64,
    pub freezes: u64,
    pub threshold_ms: u64,
    pub in_freeze_ms: u64,
    pub log: String,
    pub last_context: Context,
}

pub struct Recorder {
    cfg: Config,
    shared: Arc<Shared>,
    inner: Arc<Mutex<Inner>>,
    tx: Mutex<Option<Sender<Event>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    clock: Clock,
    writer_dir: PathBuf,
}

impl Recorder {
    pub fn start(cfg: Config, probe: Box<dyn NetProbe>) -> Recorder {
        let clock = Clock::now();
        let shared = Arc::new(Shared {
            last_beat_ns: AtomicU64::new(0),
            waiting: AtomicBool::new(false),
            level: AtomicU8::new(Level::UserNotIdle as u8),
            beats: AtomicU64::new(0),
            freezes: AtomicU64::new(0),
            running: AtomicBool::new(true),
            want_context: AtomicBool::new(true),
            stall_since_ns: AtomicU64::new(0),
        });
        let inner = Arc::new(Mutex::new(Inner {
            last_context: Context::default(),
            gate: CaptureGate::new(cfg.context_interval_ms),
            marks: Vec::new(),
            pending: Vec::new(),
        }));
        let net = Arc::new(Mutex::new(NetHistory::new(120)));
        let (tx, rx) = channel::<Event>();
        let mut threads = Vec::new();

        // Writer.
        let writer = LogWriter::new(cfg.log_dir.clone());
        threads.push(
            thread::Builder::new()
                .name("fr-writer".into())
                .spawn(move || {
                    for e in rx {
                        let _ = writer.write(&e);
                    }
                })
                .expect("spawn writer"),
        );

        // Net sampler.
        if cfg.net_port.is_some() {
            let (s, net, every) = (shared.clone(), net.clone(), Duration::from_millis(cfg.net_sample_ms));
            let mut probe = probe;
            threads.push(
                thread::Builder::new()
                    .name("fr-net".into())
                    .spawn(move || {
                        while s.running.load(Relaxed) {
                            if let Some(c) = probe.sample() {
                                net.lock().unwrap().push(mono_ns(), c);
                            }
                            thread::sleep(every);
                        }
                    })
                    .expect("spawn net"),
            );
        }

        // Watchdog.
        {
            let (s, inner, net, tx, cfg2) = (shared.clone(), inner.clone(), net.clone(), tx.clone(), cfg.clone());
            threads.push(
                thread::Builder::new()
                    .name("fr-watchdog".into())
                    .spawn(move || watchdog(cfg2, s, inner, net, tx, clock))
                    .expect("spawn watchdog"),
            );
        }

        let _ = tx.send(Event::SessionStart {
            ts: clock.iso(mono_ns()),
            pid: std::process::id(),
            app: cfg.app.clone(),
            plugin: crate::VERSION.into(),
            threshold_ms: cfg.threshold_ms,
            heartbeat: cfg.heartbeat.clone(),
            port: cfg.net_port.unwrap_or(0),
        });

        let writer_dir = cfg.log_dir.clone();
        Recorder { cfg, shared, inner, tx: Mutex::new(Some(tx)), threads: Mutex::new(threads), clock, writer_dir }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn log_path(&self) -> PathBuf {
        LogWriter::new(self.writer_dir.clone()).path_for_now()
    }

    // ---- main-thread API (cheap) ----

    /// A heartbeat: FileMaker idle message or run-loop activity.
    #[inline]
    pub fn beat(&self, level: Level) {
        self.shared.last_beat_ns.store(mono_ns(), Release);
        if level != Level::RunLoop {
            self.shared.level.store(level as u8, Relaxed);
        }
        self.shared.beats.fetch_add(1, Relaxed);
    }

    /// The main run loop is about to sleep (true) or has woken up (false).
    #[inline]
    pub fn set_waiting(&self, waiting: bool) {
        if !waiting {
            self.shared.last_beat_ns.store(mono_ns(), Release);
        }
        self.shared.waiting.store(waiting, Release);
    }

    /// Should the glue read Get() values now? Call at a safe idle.
    pub fn wants_context(&self, level: Level) -> bool {
        if !level.safe_for_calls(self.cfg.context_during_scripts) {
            return false;
        }
        if self.shared.want_context.swap(false, AcqRel) {
            return true;
        }
        let now_ms = mono_ns() / 1_000_000;
        self.inner.lock().map(|mut i| i.gate.should_capture(now_ms)).unwrap_or(false)
    }

    /// Hand over a freshly captured context (main thread).
    pub fn offer_context(&self, ctx: Context) {
        let mut ready = Vec::new();
        if let Ok(mut i) = self.inner.lock() {
            for p in i.pending.drain(..) {
                ready.push(p.event);
            }
            i.last_context = ctx.clone();
        }
        for mut e in ready {
            if let Event::Freeze { after, .. } = &mut e {
                *after = Some(ctx.clone());
            }
            self.send(e);
        }
    }

    /// FlightRecorder_Mark(label). Does NOT count as a heartbeat: a script
    /// that calls Mark may be the very thing that keeps the UI frozen.
    pub fn mark(&self, label: &str, ctx: Context) {
        let now = mono_ns();
        if let Ok(mut i) = self.inner.lock() {
            i.marks.push((now, label.to_string()));
            let excess = i.marks.len().saturating_sub(64);
            i.marks.drain(..excess);
            if !ctx.is_empty() {
                i.last_context = ctx.clone();
            }
        }
        self.send(Event::Mark { ts: self.clock.iso(now), label: label.to_string(), context: ctx });
    }

    pub fn status(&self) -> Status {
        let since = self.shared.stall_since_ns.load(Acquire);
        Status {
            running: self.shared.running.load(Relaxed),
            heartbeats: self.shared.beats.load(Relaxed),
            freezes: self.shared.freezes.load(Relaxed),
            threshold_ms: self.cfg.threshold_ms,
            in_freeze_ms: if since == 0 { 0 } else { (mono_ns() - since) / 1_000_000 },
            log: self.log_path().display().to_string(),
            last_context: self.inner.lock().map(|i| i.last_context.clone()).unwrap_or_default(),
        }
    }

    fn send(&self, e: Event) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(e);
        }
    }

    /// Stop threads, flush pending freezes, write session_stop.
    pub fn stop(&self) -> Status {
        self.shutdown();
        self.status()
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Acquire)
    }

    fn shutdown(&self) {
        if !self.shared.running.swap(false, AcqRel) {
            return;
        }
        let pending: Vec<Event> =
            self.inner.lock().map(|mut i| i.pending.drain(..).map(|p| p.event).collect()).unwrap_or_default();
        for e in pending {
            self.send(e);
        }
        self.send(Event::SessionStop {
            ts: self.clock.iso(mono_ns()),
            freezes: self.shared.freezes.load(Relaxed),
            heartbeats: self.shared.beats.load(Relaxed),
        });
        // The watchdog holds a Sender clone; it exits on `running == false`.
        *self.tx.lock().unwrap() = None;
        let threads: Vec<_> = self.threads.lock().unwrap().drain(..).collect();
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn watchdog(
    cfg: Config,
    s: Arc<Shared>,
    inner: Arc<Mutex<Inner>>,
    net: Arc<Mutex<NetHistory>>,
    tx: Sender<Event>,
    clock: Clock,
) {
    let mut det = StallDetector::new(cfg.threshold_ms);
    let mut busy = BusySpan::default();
    let mut busy_ctx = Context::default();
    let poll = Duration::from_millis(cfg.poll_ms.max(1));
    let threshold_ns = cfg.threshold_ms * 1_000_000;
    let mut level_before_stall = Level::UserNotIdle;
    // Context at the moment the stall was found. No context can be offered
    // between the last beat and this point (offers happen on beats), so it
    // is the "before" context even if the main thread offers a new one
    // before we see the stall end.
    let mut ctx_before_stall = Context::default();

    while s.running.load(Acquire) {
        thread::sleep(poll);
        let now = mono_ns();
        let level = Level::from_u8(s.level.load(Relaxed));
        let sample = Sample { now_ns: now, last_beat_ns: s.last_beat_ns.load(Acquire), waiting: s.waiting.load(Acquire) };

        match det.observe(sample) {
            Some(Transition::Started { start_ns }) => {
                level_before_stall = level;
                ctx_before_stall = inner.lock().map(|i| i.last_context.clone()).unwrap_or_default();
                s.stall_since_ns.store(start_ns, Release);
            }
            Some(Transition::Ended { start_ns, end_ns }) => {
                s.stall_since_ns.store(0, Release);
                s.freezes.fetch_add(1, Relaxed);
                // The sampler can lag the heartbeat by one interval: the last
                // reply may land just after `end_ns`. Wait, then count a little
                // past the end.
                let grace_ns = (cfg.net_sample_ms * 3 + 2) * 1_000_000;
                let wait = (end_ns + grace_ns).saturating_sub(mono_ns());
                if wait > 0 {
                    thread::sleep(Duration::from_nanos(wait));
                }
                let netw = net.lock().map(|n| n.window(start_ns, end_ns + grace_ns)).unwrap_or_default();
                let mut i = inner.lock().unwrap();
                let marks = i
                    .marks
                    .iter()
                    .filter(|(t, _)| *t + 1_000_000_000 >= start_ns && *t <= end_ns)
                    .map(|(_, l)| l.clone())
                    .collect();
                let ev = Event::Freeze {
                    ts: clock.iso(start_ns),
                    end_ts: clock.iso(end_ns),
                    duration_ms: (end_ns - start_ns) / 1_000_000,
                    level: level_before_stall.name().into(),
                    before: std::mem::take(&mut ctx_before_stall),
                    after: None,
                    net: netw,
                    marks,
                };
                i.pending.push(Pending { event: ev, deadline_ns: now + cfg.after_wait_ms * 1_000_000 });
                i.gate.reset();
                s.want_context.store(true, Release);
            }
            None => {}
        }

        // Pending freezes that never got an "after" context.
        {
            let mut i = inner.lock().unwrap();
            if i.pending.iter().any(|p| p.deadline_ns <= now) {
                let (late, keep): (Vec<_>, Vec<_>) = i.pending.drain(..).partition(|p| p.deadline_ns <= now);
                i.pending = keep;
                for p in late {
                    let _ = tx.send(p.event);
                }
            }
            if busy_ctx.is_empty() || level != Level::ScriptRunning {
                busy_ctx = i.last_context.clone();
            }
        }

        // Long script runs.
        if let Some((a, b)) = busy.on_level(now, level == Level::ScriptRunning) {
            if b - a >= threshold_ns {
                let netw = net.lock().map(|n| n.window(a, b)).unwrap_or_default();
                let _ = tx.send(Event::ScriptBusy {
                    ts: clock.iso(a),
                    end_ts: clock.iso(b),
                    duration_ms: (b - a) / 1_000_000,
                    before: busy_ctx.clone(),
                    net: netw,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        let mut c = Config::default();
        c.apply_options("threshold=400; port=0; scripts=1; dir=/tmp/x; bogus=1");
        assert_eq!(c.threshold_ms, 400);
        assert_eq!(c.net_port, None);
        assert!(c.context_during_scripts);
        assert_eq!(c.log_dir, PathBuf::from("/tmp/x"));
        c.apply_options("threshold=1");
        assert_eq!(c.threshold_ms, 20);
        c.apply_options("port=15003");
        assert_eq!(c.net_port, Some(15003));
    }

    #[test]
    fn level_safety() {
        assert!(Level::UserIdle.safe_for_calls(false));
        assert!(!Level::ScriptRunning.safe_for_calls(false));
        assert!(Level::ScriptRunning.safe_for_calls(true));
        assert!(!Level::Unsafe.safe_for_calls(true));
        assert_eq!(Level::from_u8(3), Level::ScriptRunning);
    }
}
