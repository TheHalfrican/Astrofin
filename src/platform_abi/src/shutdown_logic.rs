//! The decisions inside the Unix shutdown-handler install, split out of the
//! `sigaction` call they sit around.
//!
//! `Platform::install_shutdown_handler`'s default arms a real SIGINT/SIGTERM
//! disposition for the whole process, which is why `process_unix.rs` is exempt
//! from the one-test-per-function rule: a test that armed it would change how
//! the test binary itself dies. What the install *decides* — who owns the
//! callback slot, and what a delivered signal does with it — needs no signal
//! at all once the slot is a parameter, so it lives here with its tests and
//! leaves the syscall next door as the only untested line.

use std::sync::OnceLock;

/// Claim the process-wide callback slot.
///
/// `true` means this call is the one that has to arm the handler. `false`
/// means somebody got there first: the disposition is already ours and
/// pointing at the same handler, and re-arming would only replace one
/// identical handler with another — while the guard that produced, dropped on
/// the spot, would restore the disposition captured *after* the first install
/// rather than the one that predates it.
pub(crate) fn adopt_callback(slot: &OnceLock<fn()>, on_shutdown: fn()) -> bool {
    slot.set(on_shutdown).is_ok()
}

/// What the signal handler does once it has been entered; `false` when there
/// was nothing to run.
///
/// Async-signal-safe, because it runs in signal context: one atomic read and
/// one indirect call, no allocation, no locking, no logging. A signal that
/// arrives before anything claimed the slot is ignored rather than dispatched
/// to whatever the slot happens to hold.
pub(crate) fn dispatch(slot: &OnceLock<fn()>) -> bool {
    match slot.get() {
        Some(on_shutdown) => {
            on_shutdown();
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{adopt_callback, dispatch};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FIRST: AtomicUsize = AtomicUsize::new(0);
    static SECOND: AtomicUsize = AtomicUsize::new(0);

    fn first() {
        FIRST.fetch_add(1, Ordering::SeqCst);
    }

    fn second() {
        SECOND.fetch_add(1, Ordering::SeqCst);
    }

    /// The composition root may install more than once (the CEF layers each
    /// initialize); only the first arms anything.
    #[test]
    fn adopt_callback_is_claimed_by_the_first_caller_alone() {
        let slot: OnceLock<fn()> = OnceLock::new();
        assert!(adopt_callback(&slot, first));
        assert!(!adopt_callback(&slot, second));
        assert_eq!(
            slot.get().map(|cb| *cb as usize),
            Some(first as *const () as usize)
        );
    }

    #[test]
    fn dispatch_runs_the_callback_that_was_adopted() {
        let slot: OnceLock<fn()> = OnceLock::new();
        let before = FIRST.load(Ordering::SeqCst);
        assert!(adopt_callback(&slot, first));
        assert!(dispatch(&slot));
        assert_eq!(FIRST.load(Ordering::SeqCst), before + 1);
        // A second signal is delivered to the same callback, not to a
        // consumed slot.
        assert!(dispatch(&slot));
        assert_eq!(FIRST.load(Ordering::SeqCst), before + 2);
    }

    /// A signal can be delivered between process start and the install; there
    /// is nothing to call, and nothing to call *through*.
    #[test]
    fn dispatch_ignores_a_signal_that_arrives_before_anything_is_adopted() {
        let slot: OnceLock<fn()> = OnceLock::new();
        let before = (FIRST.load(Ordering::SeqCst), SECOND.load(Ordering::SeqCst));
        assert!(!dispatch(&slot));
        assert_eq!(
            (FIRST.load(Ordering::SeqCst), SECOND.load(Ordering::SeqCst)),
            before
        );
    }
}
