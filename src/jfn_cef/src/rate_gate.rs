//! A minimum-interval gate for page-triggered commands.
//!
//! Some `jmpNative` calls are not idempotent in a way the user would forgive:
//! `openConfigDir` spawns a file-manager process and `appExit` tears the app
//! down. A page (or a stuck key, or a remote's autorepeat) can send either as
//! fast as V8 can loop, so each of them is put behind one of these.
//!
//! No clock lives in here: the caller reads its own monotonic clock and hands
//! the reading in. That keeps the rule a pure function of (window, last
//! accepted, now) and testable without sleeping.

use std::time::Instant;

/// The interval, in milliseconds, within which a repeat of the same command
/// is ignored.
pub(crate) const COMMAND_MIN_INTERVAL_MS: u64 = 250;

/// Accepts a command at most once per `window_ms`.
///
/// The first call is always accepted. After that, a call is accepted only
/// when at least `window_ms` have passed since the last *accepted* one — a
/// rejected call does not extend the window, so a page hammering the command
/// still gets one through every `window_ms`.
#[derive(Debug)]
pub(crate) struct RateGate {
    window_ms: u64,
    last_accepted_ms: Option<u64>,
}

impl RateGate {
    pub(crate) const fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            last_accepted_ms: None,
        }
    }

    /// Whether a command arriving at `now_ms` is accepted, recording it when
    /// it is. `now_ms` must come from a monotonic clock: a reading older than
    /// the last accepted one is treated as inside the window and rejected.
    pub(crate) fn accept(&mut self, now_ms: u64) -> bool {
        let allowed = match self.last_accepted_ms {
            None => true,
            Some(last) => now_ms >= last.saturating_add(self.window_ms),
        };
        if allowed {
            self.last_accepted_ms = Some(now_ms);
        }
        allowed
    }
}

/// Milliseconds since an arbitrary fixed point, from the monotonic clock.
///
/// `Instant` is the only clock in this file's world; it never goes backwards
/// and is unaffected by the wall clock being set.
pub(crate) fn monotonic_now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn a_rate_gate_accepts_the_first_command() {
        let mut gate = RateGate::new(COMMAND_MIN_INTERVAL_MS);
        assert!(gate.accept(0));
        assert!(RateGate::new(COMMAND_MIN_INTERVAL_MS).accept(1_000_000));
    }

    #[test]
    fn a_rate_gate_ignores_a_repeat_inside_the_window() {
        let mut gate = RateGate::new(250);
        assert!(gate.accept(1_000));
        for t in [1_000, 1_001, 1_100, 1_249] {
            assert!(!gate.accept(t), "t={t} is inside the 250 ms window");
        }
        assert!(gate.accept(1_250), "the boundary itself is accepted");
    }

    #[test]
    fn a_rejected_command_does_not_extend_the_window() {
        // A page sending the command every millisecond still gets exactly one
        // through per window, not none.
        let mut gate = RateGate::new(250);
        assert!(gate.accept(0));
        let accepted = (1..=500).filter(|t| gate.accept(*t)).count();
        assert_eq!(accepted, 2, "one at 250 ms and one at 500 ms");
    }

    #[test]
    fn a_rate_gate_rejects_a_clock_reading_that_went_backwards() {
        let mut gate = RateGate::new(250);
        assert!(gate.accept(10_000));
        assert!(!gate.accept(9_000));
        assert!(!gate.accept(0));
        assert!(gate.accept(10_250));
    }

    #[test]
    fn a_zero_window_gate_accepts_everything() {
        let mut gate = RateGate::new(0);
        assert!(gate.accept(5));
        assert!(gate.accept(5));
    }

    #[test]
    fn two_gates_do_not_share_a_window() {
        // `openConfigDir` and `appExit` are throttled per command, so one
        // must never consume the other's allowance.
        let mut a = RateGate::new(250);
        let mut b = RateGate::new(250);
        assert!(a.accept(0));
        assert!(b.accept(0));
        assert!(!a.accept(10));
        assert!(!b.accept(10));
    }

    #[test]
    fn monotonic_now_ms_does_not_go_backwards() {
        let a = monotonic_now_ms();
        let b = monotonic_now_ms();
        assert!(b >= a);
    }
}
