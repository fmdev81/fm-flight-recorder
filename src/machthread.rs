//! Read the run state of one thread (FileMaker's main thread) from another
//! thread (the watchdog).
//!
//! The run loop says "about to sleep" in its BeforeWaiting observer pass.
//! Code can still run after that, in the same pass (FileMaker redraws and
//! evaluates button calcs there, from an NSPostWhenIdle notification). The
//! watchdog uses [`ThreadWatch::is_busy`] to see if the thread really sleeps
//! (blocked in `mach_msg`) or still works.

/// A thread to watch. The port comes from `pthread_mach_thread_np`, which
/// adds no reference: it stays valid while the thread lives. FileMaker's
/// main thread lives as long as the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadWatch(libc::mach_port_t);

impl ThreadWatch {
    /// The calling thread.
    pub fn current() -> Self {
        // SAFETY: pthread_self() is always a valid pthread_t for the caller.
        Self(unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) })
    }

    pub fn from_port(port: u32) -> Self {
        Self(port)
    }

    pub fn port(self) -> u32 {
        self.0
    }

    /// Some(true): the thread runs, waits for a CPU, or waits for disk I/O.
    /// Some(false): the thread is blocked (for example asleep in the run
    /// loop's `mach_msg`, or in `nanosleep`). None: the state is not readable
    /// (the thread is gone).
    pub fn is_busy(self) -> Option<bool> {
        let mut info = std::mem::MaybeUninit::<libc::thread_basic_info>::zeroed();
        let mut count = libc::THREAD_BASIC_INFO_COUNT;
        // SAFETY: info has room for THREAD_BASIC_INFO_COUNT integers; the
        // kernel writes at most `count` of them.
        let kr = unsafe {
            libc::thread_info(
                self.0,
                libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if kr != libc::KERN_SUCCESS {
            return None;
        }
        let info = unsafe { info.assume_init() };
        Some(matches!(
            info.run_state,
            libc::TH_STATE_RUNNING | libc::TH_STATE_UNINTERRUPTIBLE
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn the_calling_thread_is_busy() {
        assert_eq!(ThreadWatch::current().is_busy(), Some(true));
    }

    #[test]
    fn a_sleeping_thread_is_not_busy_and_a_spinning_one_is() {
        let port = Arc::new(AtomicU32::new(0));
        let spin = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, s2, st2) = (port.clone(), spin.clone(), stop.clone());
        let h = std::thread::spawn(move || {
            p2.store(ThreadWatch::current().port(), Ordering::SeqCst);
            while !st2.load(Ordering::SeqCst) {
                if s2.load(Ordering::SeqCst) {
                    std::hint::spin_loop();
                } else {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        while port.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        let w = ThreadWatch::from_port(port.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(50));
        // Asleep most of the time: most reads say "not busy".
        let idle = (0..20)
            .filter(|_| {
                std::thread::sleep(Duration::from_millis(3));
                w.is_busy() == Some(false)
            })
            .count();
        assert!(idle >= 15, "sleeping thread read as not busy {idle}/20");
        spin.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        let busy = (0..20)
            .filter(|_| {
                std::thread::sleep(Duration::from_millis(3));
                w.is_busy() == Some(true)
            })
            .count();
        assert!(busy >= 15, "spinning thread read as busy {busy}/20");
        stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
    }
}
