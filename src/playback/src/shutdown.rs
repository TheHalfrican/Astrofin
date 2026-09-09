//! Process-wide shutdown signal.
//!
//! A single atomic flag (`SHUTTING_DOWN`) gates teardown across the whole
//! process. Shared between SIGINT/SIGTERM, UI close, hotkeys, and CEF
//! window-close paths. `jfn_shutdown_initiate` is idempotent and
//! async-signal-safe up to whatever the registered handler does — the call
//! itself just CAS's the flag and runs the registered handler.
//!
//! A handler registered via `jfn_shutdown_set_handler` runs once on the first
//! call — it runs *inline on the calling thread*, so it MUST only signal or
//! wake (e.g. signal the shutdown manager); it must never block, close a
//! browser, or reenter CEF. The actual teardown is orchestrated off-thread by
//! the manager, which then calls `jfn_shutdown_fanout` to wake every
//! subsystem thread that registered via `jfn_shutdown_register_waker`.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use jfn_wake_event::WakeEvent;

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static HANDLER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static WAKERS: Mutex<Vec<&'static WakeEvent>> = Mutex::new(Vec::new());

/// Returns true if [`jfn_shutdown_initiate`] has been called at least once.
pub fn jfn_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

/// Install (or clear, with `None`) the callback invoked exactly once on the
/// first [`jfn_shutdown_initiate`] call. The callback runs inline on the
/// calling thread (possibly a signal handler or a CEF dispatch), so it MUST
/// only signal/wake — never block, close a browser, or reenter CEF.
pub fn jfn_shutdown_set_handler(handler: Option<fn()>) {
    let ptr = handler
        .map(|f| f as *mut ())
        .unwrap_or(std::ptr::null_mut());
    HANDLER.store(ptr, Ordering::Release);
}

/// Register a wake event that will be signaled when the shutdown manager
/// fans out (`jfn_shutdown_fanout`). One uniform observation pattern across
/// long-lived threads — each subsystem owns its own `WakeEvent`, polls its
/// own fd/handle alongside its native event source, and exits on signal.
///
/// `ev` must remain live for the rest of the process.
pub fn jfn_shutdown_register_waker(ev: &'static WakeEvent) {
    WAKERS.lock().push(ev);
}

/// Signal every registered waker. Called from the manager once it observes
/// shutdown — never from a signal handler (this locks a mutex).
pub fn jfn_shutdown_fanout() {
    let wakers = WAKERS.lock();
    for ev in wakers.iter() {
        ev.signal();
    }
}

/// Idempotent. First call: sets the flag and runs the registered handler.
/// Subsequent calls are no-ops. Async-signal-safe up to whatever the
/// handler does.
pub fn jfn_shutdown_initiate() {
    if SHUTTING_DOWN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let h = HANDLER.load(Ordering::Acquire);
    if !h.is_null() {
        let f: fn() = unsafe { std::mem::transmute(h) };
        f();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Duration;

    static RUNS: AtomicUsize = AtomicUsize::new(0);

    fn count_run() {
        RUNS.fetch_add(1, Ordering::Relaxed);
    }

    fn never_run() {
        panic!("a cleared shutdown handler must never run");
    }

    /// A leaked event, as every real registration is: the list holds
    /// `&'static WakeEvent` for the life of the process.
    fn leaked_event() -> &'static WakeEvent {
        Box::leak(Box::new(WakeEvent::new().expect("wake event")))
    }

    /// True if `ev` is signaled within `timeout`. `WakeEvent::wait` blocks,
    /// so the wait runs on its own thread and the test never hangs.
    fn signaled_within(ev: &'static WakeEvent, timeout: Duration) -> bool {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            ev.wait();
            let _ = tx.send(());
        });
        rx.recv_timeout(timeout).is_ok()
    }

    #[test]
    fn the_flag_is_a_one_way_latch_and_the_handler_runs_once() {
        // The only test that calls `jfn_shutdown_initiate`, so the latch is
        // observed from its documented starting state whatever order the
        // harness runs in.
        let _g = test_support::lock();
        RUNS.store(0, Ordering::Relaxed);
        assert!(!jfn_shutting_down());
        jfn_shutdown_set_handler(Some(count_run));

        jfn_shutdown_initiate();
        assert!(jfn_shutting_down());
        assert_eq!(RUNS.load(Ordering::Relaxed), 1);

        // Latched: a second call neither clears the flag nor re-runs.
        jfn_shutdown_initiate();
        assert!(jfn_shutting_down());
        assert_eq!(RUNS.load(Ordering::Relaxed), 1);

        // A handler installed after the latch has missed its only chance.
        jfn_shutdown_set_handler(Some(never_run));
        jfn_shutdown_initiate();
        jfn_shutdown_set_handler(None);
    }

    #[test]
    fn installing_a_handler_replaces_the_previous_one_and_none_clears_it() {
        let _g = test_support::lock();
        jfn_shutdown_set_handler(Some(count_run));
        let first = HANDLER.load(Ordering::Acquire);
        assert!(!first.is_null());
        jfn_shutdown_set_handler(Some(never_run));
        let second = HANDLER.load(Ordering::Acquire);
        assert!(!second.is_null());
        assert_ne!(first, second);
        jfn_shutdown_set_handler(None);
        assert!(HANDLER.load(Ordering::Acquire).is_null());
    }

    #[test]
    fn a_registered_waker_is_signaled_by_the_fanout() {
        let _g = test_support::lock();
        let ev = leaked_event();
        jfn_shutdown_register_waker(ev);
        jfn_shutdown_fanout();
        assert!(signaled_within(ev, test_support::WORKER_TIMEOUT));
    }

    #[test]
    fn the_fanout_signals_every_registered_waker() {
        let _g = test_support::lock();
        let a = leaked_event();
        let b = leaked_event();
        jfn_shutdown_register_waker(a);
        jfn_shutdown_register_waker(b);
        jfn_shutdown_fanout();
        assert!(signaled_within(a, test_support::WORKER_TIMEOUT));
        assert!(signaled_within(b, test_support::WORKER_TIMEOUT));
    }

    #[test]
    fn a_waker_registered_after_a_fanout_is_signaled_by_the_next_one() {
        let _g = test_support::lock();
        jfn_shutdown_fanout();
        let late = leaked_event();
        jfn_shutdown_register_waker(late);
        jfn_shutdown_fanout();
        assert!(signaled_within(late, test_support::WORKER_TIMEOUT));
    }
}
