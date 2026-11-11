//! Stall detector: a pure state machine fed with heartbeat samples.
//!
//! The main thread (FileMaker's UI thread) publishes heartbeats: every
//! `kFMXT_Idle` message and, when the run-loop observer is installed, every
//! run-loop activity. It also says when the run loop goes to sleep
//! ("waiting"). A thread that sleeps in the run loop is not frozen, it has no
//! work to do. A thread that is awake and has not beaten for `threshold` is
//! frozen.
//!
//! The watchdog thread calls [`StallDetector::observe`] every few ms. All
//! times are monotonic nanoseconds, injected, so the logic is testable with
//! no clock and no FileMaker.

/// One reading of the shared heartbeat state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Now, monotonic ns.
    pub now_ns: u64,
    /// Time of the last heartbeat, monotonic ns. 0 = no heartbeat yet.
    pub last_beat_ns: u64,
    /// True when the main run loop is asleep waiting for events.
    pub waiting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The main thread has been silent for `threshold`; the stall began at
    /// `start_ns` (the last heartbeat).
    Started { start_ns: u64 },
    /// The main thread is back. `end_ns` is the first heartbeat after the
    /// stall (or the time it went to sleep).
    Ended { start_ns: u64, end_ns: u64 },
}

impl Transition {
    pub fn duration_ns(&self) -> Option<u64> {
        match *self {
            Transition::Ended { start_ns, end_ns } => Some(end_ns.saturating_sub(start_ns)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StallDetector {
    threshold_ns: u64,
    /// Some(start) while a stall is in progress.
    in_stall: Option<u64>,
    /// Last heartbeat value seen, to detect "a new beat arrived".
    seen_beat_ns: u64,
}

impl StallDetector {
    pub fn new(threshold_ms: u64) -> Self {
        Self {
            threshold_ns: threshold_ms.max(1) * 1_000_000,
            in_stall: None,
            seen_beat_ns: 0,
        }
    }

    pub fn threshold_ms(&self) -> u64 {
        self.threshold_ns / 1_000_000
    }

    pub fn set_threshold_ms(&mut self, ms: u64) {
        self.threshold_ns = ms.max(1) * 1_000_000;
    }

    pub fn in_stall(&self) -> Option<u64> {
        self.in_stall
    }

    /// Feed one sample. Returns a transition when the state changes.
    pub fn observe(&mut self, s: Sample) -> Option<Transition> {
        let new_beat = s.last_beat_ns != 0 && s.last_beat_ns != self.seen_beat_ns;
        self.seen_beat_ns = s.last_beat_ns;

        match self.in_stall {
            Some(start_ns) => {
                // A stall ends on the first new heartbeat, or when the run loop
                // goes back to sleep (it finished the long piece of work).
                if new_beat || s.waiting {
                    self.in_stall = None;
                    let end_ns = if new_beat { s.last_beat_ns } else { s.now_ns };
                    return Some(Transition::Ended {
                        start_ns,
                        end_ns: end_ns.max(start_ns),
                    });
                }
                None
            }
            None => {
                if s.waiting || s.last_beat_ns == 0 {
                    return None;
                }
                if s.now_ns.saturating_sub(s.last_beat_ns) >= self.threshold_ns {
                    self.in_stall = Some(s.last_beat_ns);
                    return Some(Transition::Started {
                        start_ns: s.last_beat_ns,
                    });
                }
                None
            }
        }
    }
}

/// Tracks how long FileMaker reports "script running" (idle level 3)
/// without a break. A long script is not a frozen UI (FileMaker still pumps
/// events), but the user still waits through it, so it is worth a record.
#[derive(Debug, Clone, Default)]
pub struct BusySpan {
    since_ns: Option<u64>,
}

impl BusySpan {
    /// Feed the idle level at a heartbeat. Returns (start, end) of a finished
    /// span when the level leaves "script running".
    pub fn on_level(&mut self, now_ns: u64, script_running: bool) -> Option<(u64, u64)> {
        match (self.since_ns, script_running) {
            (None, true) => {
                self.since_ns = Some(now_ns);
                None
            }
            (Some(start), false) => {
                self.since_ns = None;
                Some((start, now_ns))
            }
            _ => None,
        }
    }
}

/// Second check of "the run loop sleeps". The run loop reports
/// BeforeWaiting, but code can still run after that in the same observer
/// pass. If `waiting` has been true for `threshold` and the main thread is
/// still busy at 2 polls in a row (with no new beat), the thread does not
/// sleep: [`SleepCheck::waiting`] then returns false, so the
/// [`StallDetector`] sees the gap as a freeze.
///
/// Two polls, not one: after a real sleep, the thread is busy for a few
/// microseconds before its AfterWaiting beat. One read in that window must
/// not turn a long idle time into a freeze.
#[derive(Debug, Clone, Default)]
pub struct SleepCheck {
    streak: u32,
    beat_ns: u64,
}

impl SleepCheck {
    /// The `waiting` value that the detector must use for this sample.
    /// `thread_busy` is called only when `s.waiting` has lasted `threshold`.
    pub fn waiting(
        &mut self,
        s: Sample,
        threshold_ns: u64,
        thread_busy: impl FnOnce() -> bool,
    ) -> bool {
        if !s.waiting || s.now_ns.saturating_sub(s.last_beat_ns) < threshold_ns {
            self.streak = 0;
            return s.waiting;
        }
        if s.last_beat_ns != self.beat_ns {
            self.beat_ns = s.last_beat_ns;
            self.streak = 0;
        }
        if thread_busy() {
            self.streak = self.streak.saturating_add(1);
        } else {
            self.streak = 0;
        }
        self.streak < 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MS: u64 = 1_000_000;

    fn s(now_ms: u64, beat_ms: u64, waiting: bool) -> Sample {
        Sample {
            now_ns: now_ms * MS,
            last_beat_ns: beat_ms * MS,
            waiting,
        }
    }

    #[test]
    fn no_beat_yet_is_not_a_stall() {
        let mut d = StallDetector::new(250);
        assert_eq!(
            d.observe(Sample {
                now_ns: 10_000 * MS,
                last_beat_ns: 0,
                waiting: false
            }),
            None
        );
    }

    #[test]
    fn regular_beats_never_stall() {
        let mut d = StallDetector::new(250);
        for t in (0..5_000).step_by(20) {
            assert_eq!(d.observe(s(t + 1, t.max(1), false)), None);
        }
    }

    #[test]
    fn gap_over_threshold_starts_and_next_beat_ends() {
        let mut d = StallDetector::new(250);
        assert_eq!(d.observe(s(1_000, 1_000, false)), None);
        assert_eq!(d.observe(s(1_200, 1_000, false)), None);
        assert_eq!(
            d.observe(s(1_250, 1_000, false)),
            Some(Transition::Started {
                start_ns: 1_000 * MS
            })
        );
        // Still frozen: no second Started.
        assert_eq!(d.observe(s(3_000, 1_000, false)), None);
        let t = d.observe(s(5_210, 5_200, false)).unwrap();
        assert_eq!(
            t,
            Transition::Ended {
                start_ns: 1_000 * MS,
                end_ns: 5_200 * MS
            }
        );
        assert_eq!(t.duration_ns(), Some(4_200 * MS));
        assert!(d.in_stall().is_none());
    }

    #[test]
    fn sleeping_run_loop_is_not_frozen() {
        let mut d = StallDetector::new(250);
        d.observe(s(1_000, 1_000, false));
        // The user walks away: no beats for a minute, but the run loop sleeps.
        assert_eq!(d.observe(s(61_000, 1_000, true)), None);
    }

    #[test]
    fn stall_ends_when_run_loop_goes_to_sleep() {
        let mut d = StallDetector::new(100);
        d.observe(s(1, 1, false));
        assert!(matches!(
            d.observe(s(500, 1, false)),
            Some(Transition::Started { .. })
        ));
        let t = d.observe(s(900, 1, true)).unwrap();
        assert_eq!(
            t,
            Transition::Ended {
                start_ns: MS,
                end_ns: 900 * MS
            }
        );
    }

    #[test]
    fn back_to_back_stalls_are_separate() {
        let mut d = StallDetector::new(100);
        d.observe(s(10, 10, false));
        assert!(d.observe(s(200, 10, false)).is_some());
        assert!(matches!(
            d.observe(s(400, 390, false)),
            Some(Transition::Ended { .. })
        ));
        assert!(
            matches!(d.observe(s(600, 390, false)), Some(Transition::Started { start_ns }) if start_ns == 390 * MS)
        );
    }

    #[test]
    fn threshold_can_change() {
        let mut d = StallDetector::new(1000);
        d.observe(s(1, 1, false));
        assert_eq!(d.observe(s(300, 1, false)), None);
        d.set_threshold_ms(200);
        assert!(d.observe(s(301, 1, false)).is_some());
    }

    #[test]
    fn busy_span() {
        let mut b = BusySpan::default();
        assert_eq!(b.on_level(10, false), None);
        assert_eq!(b.on_level(20, true), None);
        assert_eq!(b.on_level(30, true), None);
        assert_eq!(b.on_level(90, false), Some((20, 90)));
        assert_eq!(b.on_level(95, false), None);
    }
    fn sw(now_ms: u64, beat_ms: u64) -> Sample {
        s(now_ms, beat_ms, true)
    }

    #[test]
    fn sleep_check_trusts_a_real_sleep() {
        let mut c = SleepCheck::default();
        for t in (1_000..5_000).step_by(10) {
            assert!(c.waiting(sw(t, 1_000), 250 * MS, || false));
        }
    }

    #[test]
    fn sleep_check_does_not_ask_before_the_threshold() {
        let mut c = SleepCheck::default();
        assert!(c.waiting(sw(1_100, 1_000), 250 * MS, || panic!("asked")));
        assert!(!c.waiting(s(1_100, 1_000, false), 250 * MS, || panic!("asked")));
    }

    #[test]
    fn sleep_check_needs_two_busy_polls() {
        let mut c = SleepCheck::default();
        // One busy read (a wake before AfterWaiting): still waiting.
        assert!(c.waiting(sw(1_300, 1_000), 250 * MS, || true));
        assert!(c.waiting(sw(1_310, 1_000), 250 * MS, || false));
        // Two in a row: not waiting.
        assert!(c.waiting(sw(1_320, 1_000), 250 * MS, || true));
        assert!(!c.waiting(sw(1_330, 1_000), 250 * MS, || true));
        // A new beat starts a new count.
        assert!(c.waiting(sw(1_600, 1_340), 250 * MS, || true));
    }

    #[test]
    fn busy_in_the_before_waiting_pass_is_a_freeze() {
        // Beat + "waiting" at 1_000, then the thread works for 800 ms in the
        // same observer pass, then really sleeps (not busy) at 1_800.
        let mut d = StallDetector::new(250);
        let mut c = SleepCheck::default();
        let mut got = Vec::new();
        for t in (1_000..2_200).step_by(10) {
            let busy = t < 1_800;
            let raw = sw(t, 1_000);
            let w = c.waiting(raw, 250 * MS, || busy);
            if let Some(tr) = d.observe(Sample { waiting: w, ..raw }) {
                got.push(tr);
            }
        }
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(
            got[0],
            Transition::Started {
                start_ns: 1_000 * MS
            }
        );
        let Transition::Ended { start_ns, end_ns } = got[1] else {
            panic!("{got:?}")
        };
        assert_eq!(start_ns, 1_000 * MS);
        assert_eq!(end_ns, 1_800 * MS);
    }
}
