//! A CFRunLoopObserver on the main run loop: a second heartbeat source.
//!
//! FileMaker idle messages alone cannot tell "asleep, nothing to do" from
//! "frozen": both are a gap. The run loop can. A thread that is about to
//! sleep (kCFRunLoopBeforeWaiting) is not frozen. A thread that woke up
//! (kCFRunLoopAfterWaiting) and has not come back to the run loop for
//! `threshold` is frozen. Hang detectors on macOS and iOS use this method.
//!
//! The observers are added in kCFRunLoopCommonModes, so they also run
//! during modal dialogs and menu tracking.
//!
//! Two observers, because of the order inside one observer pass:
//! * early (order `isize::MIN`, all activities except BeforeWaiting): the
//!   beats, and AfterWaiting (the thread woke up) as early as possible.
//! * late (order `isize::MAX`, BeforeWaiting only): "about to sleep". Other
//!   observers of the BeforeWaiting pass can run for a long time. FileMaker
//!   redraws there (NSPostWhenIdle -> DispatchNullEvent) and evaluates
//!   button calcs. If we said "waiting" first, the detector would ignore
//!   such a stall. As the last observer, we say it just before the sleep.

use std::ffi::c_void;

pub type CFRunLoopRef = *mut c_void;
type CFRunLoopObserverRef = *mut c_void;
type CFRunLoopTimerRef = *mut c_void;
type CFStringRef = *const c_void;

pub const ENTRY: u64 = 1;
pub const BEFORE_TIMERS: u64 = 2;
pub const BEFORE_SOURCES: u64 = 4;
pub const BEFORE_WAITING: u64 = 32;
pub const AFTER_WAITING: u64 = 64;
pub const EXIT: u64 = 128;
const ALL: u64 = 0x0FFF_FFFF;

#[repr(C)]
struct ObserverContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    static kCFRunLoopDefaultMode: CFStringRef;
    pub fn CFRunLoopGetMain() -> CFRunLoopRef;
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopObserverCreate(
        allocator: *const c_void,
        activities: u64,
        repeats: u8,
        order: isize,
        callout: extern "C" fn(CFRunLoopObserverRef, u64, *mut c_void),
        context: *mut ObserverContext,
    ) -> CFRunLoopObserverRef;
    fn CFRunLoopAddObserver(rl: CFRunLoopRef, observer: CFRunLoopObserverRef, mode: CFStringRef);
    fn CFRunLoopObserverInvalidate(observer: CFRunLoopObserverRef);
    fn CFRelease(cf: *const c_void);
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source_handled: u8) -> i32;
    fn CFRunLoopStop(rl: CFRunLoopRef);
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopTimerCreate(
        allocator: *const c_void,
        fire_date: f64,
        interval: f64,
        flags: u64,
        order: isize,
        callout: extern "C" fn(CFRunLoopTimerRef, *mut c_void),
        context: *mut ObserverContext,
    ) -> CFRunLoopTimerRef;
    fn CFRunLoopAddTimer(rl: CFRunLoopRef, timer: CFRunLoopTimerRef, mode: CFStringRef);
    fn CFRunLoopTimerInvalidate(timer: CFRunLoopTimerRef);
}

type Handler = Box<dyn Fn(u64) + Send + Sync>;

extern "C" fn observer_cb(_o: CFRunLoopObserverRef, activity: u64, info: *mut c_void) {
    // SAFETY: info is the Box<Handler> owned by the RunLoopObserver below,
    // which invalidates the observer before it frees the box.
    let h = unsafe { &*(info as *const Handler) };
    h(activity);
}

/// Owns the two observers (early and late, see the module docs). Dropping
/// it removes them. Drop it on the thread that owns the run loop
/// (FileMaker: the main thread, at shutdown).
pub struct RunLoopObserver {
    early: CFRunLoopObserverRef,
    late: CFRunLoopObserverRef,
    handler: *mut Handler,
}

// The raw pointers are only touched at install and drop.
unsafe impl Send for RunLoopObserver {}
unsafe impl Sync for RunLoopObserver {}

impl RunLoopObserver {
    /// Install `handler(activity)` on `rl` (use `CFRunLoopGetMain()`).
    /// The handler gets BeforeWaiting from the late observer and all other
    /// activities from the early one.
    ///
    /// # Safety
    /// `rl` must be a valid CFRunLoopRef.
    pub unsafe fn install(
        rl: CFRunLoopRef,
        handler: impl Fn(u64) + Send + Sync + 'static,
    ) -> Option<Self> {
        let handler: *mut Handler = Box::into_raw(Box::new(Box::new(handler)));
        let mut ctx = ObserverContext {
            version: 0,
            info: handler.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        unsafe {
            // The context is copied at create, so one ctx serves both.
            let early = CFRunLoopObserverCreate(
                std::ptr::null(),
                ALL & !BEFORE_WAITING,
                1,
                isize::MIN,
                observer_cb,
                &mut ctx,
            );
            let late = CFRunLoopObserverCreate(
                std::ptr::null(),
                BEFORE_WAITING,
                1,
                isize::MAX,
                observer_cb,
                &mut ctx,
            );
            if early.is_null() || late.is_null() {
                for o in [early, late] {
                    if !o.is_null() {
                        CFRelease(o);
                    }
                }
                drop(Box::from_raw(handler));
                return None;
            }
            CFRunLoopAddObserver(rl, early, kCFRunLoopCommonModes);
            CFRunLoopAddObserver(rl, late, kCFRunLoopCommonModes);
            Some(Self {
                early,
                late,
                handler,
            })
        }
    }
}

impl Drop for RunLoopObserver {
    fn drop(&mut self) {
        unsafe {
            for o in [self.early, self.late] {
                CFRunLoopObserverInvalidate(o);
                CFRelease(o);
            }
            drop(Box::from_raw(self.handler));
        }
    }
}

/// Map a run-loop activity onto the recorder.
pub fn feed(rec: &crate::recorder::Recorder, activity: u64) {
    match activity {
        BEFORE_WAITING => {
            // The late observer: the pass is done, the thread sleeps next.
            // The beat marks the end of any work in this pass.
            rec.beat(crate::recorder::Level::RunLoop);
            rec.set_waiting(true);
        }
        AFTER_WAITING => rec.set_waiting(false),
        _ => rec.beat(crate::recorder::Level::RunLoop),
    }
}

// ---- small helpers used by the simulator example and the tests ----

type TimerFn = Box<dyn FnMut()>;

extern "C" fn timer_cb(_t: CFRunLoopTimerRef, info: *mut c_void) {
    let f = unsafe { &mut *(info as *mut TimerFn) };
    f();
}

/// A repeating timer on the current thread's run loop.
pub struct Timer {
    t: CFRunLoopTimerRef,
    f: *mut TimerFn,
}

impl Timer {
    pub fn every(seconds: f64, f: impl FnMut() + 'static) -> Self {
        let f: *mut TimerFn = Box::into_raw(Box::new(Box::new(f)));
        let mut ctx = ObserverContext {
            version: 0,
            info: f.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        unsafe {
            let t = CFRunLoopTimerCreate(
                std::ptr::null(),
                CFAbsoluteTimeGetCurrent() + seconds,
                seconds,
                0,
                0,
                timer_cb,
                &mut ctx,
            );
            CFRunLoopAddTimer(CFRunLoopGetCurrent(), t, kCFRunLoopCommonModes);
            Timer { t, f }
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopTimerInvalidate(self.t);
            CFRelease(self.t);
            drop(Box::from_raw(self.f));
        }
    }
}

/// Run the current thread's run loop for `seconds`.
pub fn run_for(seconds: f64) {
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 0);
    }
}

pub fn stop_current() {
    unsafe { CFRunLoopStop(CFRunLoopGetCurrent()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn observer_sees_sleep_and_wake_and_timer_block_is_visible() {
        // Runs on the test thread's own run loop.
        let seen = Arc::new([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)]);
        let s2 = seen.clone();
        let obs = unsafe {
            RunLoopObserver::install(CFRunLoopGetCurrent(), move |a| {
                let i = match a {
                    BEFORE_WAITING => 0,
                    AFTER_WAITING => 1,
                    _ => 2,
                };
                s2[i].fetch_add(1, Ordering::Relaxed);
            })
        }
        .expect("observer");
        let mut n = 0;
        let _t = Timer::every(0.02, move || {
            n += 1;
            if n == 3 {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        run_for(0.3);
        drop(obs);
        assert!(seen[0].load(Ordering::Relaxed) >= 3, "before_waiting");
        assert!(seen[1].load(Ordering::Relaxed) >= 3, "after_waiting");
        assert!(seen[2].load(Ordering::Relaxed) >= 1, "other activities");
    }
    /// An observer of the BeforeWaiting pass (order 0, like Foundation's
    /// notification queue) that uses the CPU for `ms` one time, when armed.
    struct Hog {
        obs: CFRunLoopObserverRef,
        armed: *mut AtomicU64,
    }
    extern "C" fn hog_cb(_o: CFRunLoopObserverRef, _a: u64, info: *mut c_void) {
        let armed = unsafe { &*(info as *const AtomicU64) };
        let ms = armed.swap(0, Ordering::SeqCst);
        if ms > 0 {
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_millis(ms) {
                std::hint::spin_loop();
            }
        }
    }
    impl Hog {
        fn install() -> Hog {
            let armed = Box::into_raw(Box::new(AtomicU64::new(0)));
            let mut ctx = ObserverContext {
                version: 0,
                info: armed.cast(),
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
            };
            unsafe {
                let obs = CFRunLoopObserverCreate(
                    std::ptr::null(),
                    BEFORE_WAITING,
                    1,
                    0,
                    hog_cb,
                    &mut ctx,
                );
                assert!(!obs.is_null());
                CFRunLoopAddObserver(CFRunLoopGetCurrent(), obs, kCFRunLoopCommonModes);
                Hog { obs, armed }
            }
        }
        fn arm(&self, ms: u64) {
            unsafe { &*self.armed }.store(ms, Ordering::SeqCst);
        }
    }
    impl Drop for Hog {
        fn drop(&mut self) {
            unsafe {
                CFRunLoopObserverInvalidate(self.obs);
                CFRelease(self.obs);
                drop(Box::from_raw(self.armed));
            }
        }
    }

    fn recorder(name: &str) -> (crate::recorder::Recorder, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "fr-runloop-{name}-{}-{}",
            std::process::id(),
            crate::identity::random_id()
        ));
        let cfg = crate::recorder::Config {
            log_dir: dir.clone(),
            state_dir: dir.join("state"),
            net_port: None,
            ..Default::default()
        };
        (
            crate::recorder::Recorder::start(cfg, Box::new(crate::net::NoProbe)),
            dir,
        )
    }

    /// Stall in the BeforeWaiting pass, after another observer (the live
    /// hang of 2026-11-11: NSPostWhenIdle -> DispatchNullEvent -> button
    /// calc). Only the observer order is under test: no thread watch.
    #[test]
    fn stall_in_the_before_waiting_pass_is_a_freeze() {
        let (rec, dir) = recorder("bw-pass");
        let rec = std::sync::Arc::new(rec);
        let r2 = rec.clone();
        let obs = unsafe { RunLoopObserver::install(CFRunLoopGetCurrent(), move |a| feed(&r2, a)) }
            .expect("observer");
        let hog = std::rc::Rc::new(Hog::install());
        let h2 = hog.clone();
        let mut n = 0;
        let _t = Timer::every(0.05, move || {
            n += 1;
            if n == 4 {
                h2.arm(800);
            }
        });
        run_for(1.5);
        drop(obs);
        drop(_t);
        drop(hog);
        let st = rec.stop();
        assert_eq!(st.freezes, 1, "{st:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
