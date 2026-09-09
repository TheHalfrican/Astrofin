//! Headless app control-plane thread.
//!
//! A long-lived worker that routes app-level control work off the platform
//! main loop and off CEF's UI thread. Mirrors the playback coordinator's
//! queue + drain idiom (`jfn_playback::coordinator`), but lives in
//! the binary crate because it drives `jfn_cef` + `platform_abi` — layers
//! *above* `playback`, so it can't fold into the coordinator without a
//! dependency cycle.
//!
//! Owns the process-wide lifecycle FSM. Subsystems (X11/Wayland/macOS/Windows
//! platform layers) translate native window/power events into `ManagerMsg`
//! and post them via `jfn_manager_send`; the manager loop folds each message
//! into a single `LifecycleState` and drives the side effects (CEF visibility
//! fan-out, shutdown drain).
//!
//! The `SHUTTING_DOWN` flag (set async-signal-safely by `jfn_shutdown_initiate`)
//! carries shutdown *state* — read synchronously by the TID_UI recreate guards;
//! the queue carries the orchestration *command*. The SIGINT handler can't lock
//! the queue (interrupt context), so it wakes the manager via the signal-safe
//! bridge, and the manager — a normal-context thread — translates that wake into
//! a queued `Shutdown`.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

use jfn_playback::shutdown::jfn_shutting_down;
use jfn_wake_event::WakeEvent;

/// Work routed to the manager thread. Producers post via `jfn_manager_send`
/// (platform threads) or the signal-handler bridge `jfn_manager_notify_shutdown`
/// (signal context).
pub enum ManagerMsg {
    /// Window/app became visible (true) or hidden (false). Posted by platform
    /// layers on OS-level visibility changes — Wayland xdg_toplevel
    /// suspended, X11 Map/Unmap/WM_STATE, macOS hide/unhide,
    /// Windows WM_SHOWWINDOW / SC_MINIMIZE.
    SetVisible(bool),
    /// System-level suspend / resume — power transitions (laptop lid close,
    /// macOS sleep, Windows WM_POWERBROADCAST). Treated as a stronger Hidden.
    Suspend,
    Resume,
    /// Shutdown drain. Synthesized by the manager loop when it observes the
    /// `SHUTTING_DOWN` flag — the SIGINT path can't enqueue from its handler.
    Shutdown,
}

/// Process-wide lifecycle phase. Owned by the manager loop; subsystems
/// observe transitions via the side effects the manager invokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleState {
    /// Foreground + visible. Default after boot.
    Running,
    /// User-visible hiding (minimize, occlusion, app hide). CEF browsers
    /// receive `WasHidden(true)`; mpv is left alone (jellyfin-web is the
    /// playback authority — see project notes).
    Hidden,
    /// System-level suspend. Same CEF posture as Hidden plus a marker so a
    /// later `Resume` always transitions back to Running regardless of any
    /// intervening visibility flap.
    Suspended,
    /// Shutdown drain in progress. Terminal.
    ShuttingDown,
}

struct Manager {
    queue: Mutex<VecDeque<ManagerMsg>>,
    wake: WakeEvent,
}

#[allow(clippy::expect_used)] // boot invariant: wake eventfd alloc is fatal if it fails
fn manager() -> &'static Manager {
    static M: OnceLock<&'static Manager> = OnceLock::new();
    M.get_or_init(|| {
        Box::leak(Box::new(Manager {
            queue: Mutex::new(VecDeque::new()),
            wake: WakeEvent::new().expect("manager WakeEvent allocation failed"),
        }))
    })
}

/// Spawn the manager thread. Long-lived; returns the join handle so the
/// teardown tail can join it once shutdown drains. Called once from
/// `run_with_cef`. Also installs the lifecycle dispatchers so platform
/// layers can post visibility / suspend / resume events without a direct
/// dep on this crate.
#[allow(clippy::expect_used)] // boot invariant: control-plane thread spawn is fatal if it fails
pub fn jfn_manager_start() -> JoinHandle<()> {
    // Materialize the singleton so its wake event exists before any producer
    // (shutdown handler / sender) signals it.
    let _ = manager();
    jfn_playback::lifecycle::jfn_lifecycle_set_handlers(
        |v| jfn_manager_send(ManagerMsg::SetVisible(v)),
        || jfn_manager_send(ManagerMsg::Suspend),
        || jfn_manager_send(ManagerMsg::Resume),
    );
    thread::Builder::new()
        .name("jfn-manager".into())
        .spawn(manager_loop)
        .expect("spawn jfn-manager thread")
}

/// Wake the manager to observe the shutdown flag. Async-signal-safe (a single
/// write to the wake event), so it's valid from the `jfn_shutdown_initiate`
/// handler in any calling context (signal handler, CEF dispatch, …).
pub fn jfn_manager_notify_shutdown() {
    manager().wake.signal();
}

/// Route work to the manager thread. Non-blocking, thread-agnostic. (No
/// callers yet — the hub seam for future control-plane work.)
pub fn jfn_manager_send(msg: ManagerMsg) {
    manager().queue.lock().push_back(msg);
    manager().wake.signal();
}

fn manager_loop() {
    let m = manager();
    let mut state = LifecycleState::Running;
    loop {
        m.wake.wait();
        m.wake.drain();

        // The signal-safe bridge wakes us with SHUTTING_DOWN set (from any
        // trigger, including the SIGINT handler that can't lock the queue).
        // Translate it into a queued message so every manager action is a
        // ManagerMsg handled in one place. Loop returns as soon as `handle`
        // observes the ShuttingDown terminal state.
        let work: VecDeque<ManagerMsg> = {
            let mut q = m.queue.lock();
            if jfn_shutting_down() && state != LifecycleState::ShuttingDown {
                q.push_back(ManagerMsg::Shutdown);
            }
            std::mem::take(&mut *q)
        };
        for msg in work {
            state = transition(state, msg);
            if state == LifecycleState::ShuttingDown {
                return;
            }
        }
    }
}

/// What a lifecycle transition asks of the rest of the app. Split out of
/// [`transition`] so the FSM itself is a pure function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    /// Fan a visibility change out to every live CEF browser.
    SetHiddenAll(bool),
    /// Run the shutdown drain.
    Shutdown,
}

/// Apply one message to the lifecycle FSM. Returns the new state; the
/// caller observes terminal `ShuttingDown` to exit the loop. Side effects
/// happen inline (CEF visibility fan-out, shutdown drain).
fn transition(state: LifecycleState, msg: ManagerMsg) -> LifecycleState {
    let (next, effect) = next_state(state, msg);
    match effect {
        Some(Effect::SetHiddenAll(hidden)) => {
            jfn_cef::browsers::jfn_browsers_set_hidden_all(hidden);
        }
        Some(Effect::Shutdown) => run_shutdown(),
        None => {}
    }
    next
}

/// The pure half of [`transition`]: one message folded into the state, plus
/// the effect the caller must perform. No globals, so it is unit-testable.
fn next_state(state: LifecycleState, msg: ManagerMsg) -> (LifecycleState, Option<Effect>) {
    use LifecycleState::*;
    match (state, msg) {
        // Shutdown is terminal and idempotent — once seen, ignore everything
        // else and don't re-enter the drain.
        (ShuttingDown, _) => (ShuttingDown, None),
        (_, ManagerMsg::Shutdown) => (ShuttingDown, Some(Effect::Shutdown)),
        // Visibility flips while running. Suspended is *not* downgraded by a
        // visibility event — the system must explicitly Resume first.
        (Running, ManagerMsg::SetVisible(false)) => (Hidden, Some(Effect::SetHiddenAll(true))),
        (Hidden, ManagerMsg::SetVisible(true)) => (Running, Some(Effect::SetHiddenAll(false))),
        // Hidden is already hidden to CEF; only Running needs the fan-out.
        (Running, ManagerMsg::Suspend) => (Suspended, Some(Effect::SetHiddenAll(true))),
        (Hidden, ManagerMsg::Suspend) => (Suspended, None),
        (Suspended, ManagerMsg::Resume) => (Running, Some(Effect::SetHiddenAll(false))),
        // No-op: already in the requested posture, or a stray event.
        _ => (state, None),
    }
}

/// Orchestrate shutdown off the main thread and off TID_UI: fan out the
/// shutdown signal to every registered subsystem waker (input threads,
/// clipboard, …), then a single TID_UI task closes every browser + ships
/// the wait set back, manager blocks on `OnBeforeClose` for each, then
/// releases the process main thread to run the teardown tail. One
/// snapshot, no race between close set and wait set.
fn run_shutdown() {
    jfn_playback::shutdown::jfn_shutdown_fanout();
    jfn_cef::browsers::jfn_browsers_close_all_blocking();
    jfn_platform_abi::get().wake_main_loop();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::LifecycleState::*;
    use super::*;
    use std::time::Duration;

    /// `manager()` is process-wide, so the two tests that post to its queue
    /// take this first. Nothing here ever calls [`jfn_manager_start`]: the
    /// loop it spawns would fan out to CEF and the platform backend, neither
    /// of which exists in a unit-test process.
    static QUEUE_LOCK: Mutex<()> = Mutex::new(());

    fn take_queue() -> Vec<ManagerMsg> {
        let m = manager();
        m.wake.drain();
        let mut q = m.queue.lock();
        std::mem::take(&mut *q).into_iter().collect()
    }

    /// Runs `post` while a thread is parked in `wake.wait()`, and reports
    /// whether the park was released. Never leaves the thread parked.
    fn wakes_a_parked_manager(post: impl FnOnce()) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            manager().wake.wait();
            let _ = tx.send(());
        });
        post();
        let woken = rx.recv_timeout(Duration::from_secs(5)).is_ok();
        if !woken {
            manager().wake.signal();
        }
        let _ = waiter.join();
        woken
    }

    #[test]
    fn hiding_a_running_app_hides_every_browser() {
        assert_eq!(
            next_state(Running, ManagerMsg::SetVisible(false)),
            (Hidden, Some(Effect::SetHiddenAll(true)))
        );
    }

    #[test]
    fn showing_a_hidden_app_shows_every_browser_again() {
        assert_eq!(
            next_state(Hidden, ManagerMsg::SetVisible(true)),
            (Running, Some(Effect::SetHiddenAll(false)))
        );
    }

    #[test]
    fn a_repeated_visibility_event_changes_nothing() {
        assert_eq!(
            next_state(Running, ManagerMsg::SetVisible(true)),
            (Running, None)
        );
        assert_eq!(
            next_state(Hidden, ManagerMsg::SetVisible(false)),
            (Hidden, None)
        );
    }

    #[test]
    fn suspending_hides_the_browsers_only_when_they_were_still_shown() {
        assert_eq!(
            next_state(Running, ManagerMsg::Suspend),
            (Suspended, Some(Effect::SetHiddenAll(true)))
        );
        // Already hidden: suspending must not re-post the same fan-out.
        assert_eq!(next_state(Hidden, ManagerMsg::Suspend), (Suspended, None));
    }

    #[test]
    fn a_visibility_event_never_downgrades_a_suspended_app() {
        for visible in [true, false] {
            assert_eq!(
                next_state(Suspended, ManagerMsg::SetVisible(visible)),
                (Suspended, None),
                "SetVisible({visible})"
            );
        }
        // Only Resume gets out of Suspended.
        assert_eq!(
            next_state(Suspended, ManagerMsg::Resume),
            (Running, Some(Effect::SetHiddenAll(false)))
        );
    }

    #[test]
    fn a_resume_without_a_suspend_is_ignored() {
        assert_eq!(next_state(Running, ManagerMsg::Resume), (Running, None));
        assert_eq!(next_state(Hidden, ManagerMsg::Resume), (Hidden, None));
    }

    #[test]
    fn shutdown_runs_the_drain_once_from_any_state() {
        for state in [Running, Hidden, Suspended] {
            assert_eq!(
                next_state(state, ManagerMsg::Shutdown),
                (ShuttingDown, Some(Effect::Shutdown)),
                "{state:?}"
            );
        }
    }

    #[test]
    fn shutting_down_is_terminal_and_never_re_enters_the_drain() {
        for msg in [
            ManagerMsg::Shutdown,
            ManagerMsg::Suspend,
            ManagerMsg::Resume,
            ManagerMsg::SetVisible(true),
            ManagerMsg::SetVisible(false),
        ] {
            assert_eq!(next_state(ShuttingDown, msg), (ShuttingDown, None));
        }
    }

    #[test]
    fn send_queues_messages_in_order_and_wakes_the_manager() {
        let _g = QUEUE_LOCK.lock();
        let _ = take_queue();
        let woken = wakes_a_parked_manager(|| {
            jfn_manager_send(ManagerMsg::SetVisible(false));
            jfn_manager_send(ManagerMsg::Resume);
        });
        let queued = take_queue();
        assert!(woken, "send did not wake the manager");
        assert!(
            matches!(
                queued.as_slice(),
                [ManagerMsg::SetVisible(false), ManagerMsg::Resume]
            ),
            "queue did not keep post order"
        );
    }

    #[test]
    fn notify_shutdown_wakes_the_manager_without_queueing_anything() {
        let _g = QUEUE_LOCK.lock();
        let _ = take_queue();
        let woken = wakes_a_parked_manager(jfn_manager_notify_shutdown);
        let queued = take_queue();
        assert!(woken, "notify_shutdown did not wake the manager");
        // A signal handler cannot lock the queue, so the wake carries no
        // message; manager_loop synthesizes Shutdown from the flag instead.
        assert!(queued.is_empty(), "notify_shutdown queued a message");
    }
}
