//! Public Rust API for the playback module.
//!
//! Sinks register typed closures (`Box<dyn Fn(&PlaybackEvent) + Send +
//! Sync>`); the coordinator worker fans events out by invoking them
//! directly. Producers call [`post`] with a typed [`Input`].

use parking_lot::Mutex;
use std::sync::OnceLock;

pub use crate::coordinator::Input;
use crate::coordinator::{CoordinatorHandle, PlaybackCoordinator};
use crate::types::*;

// =====================================================================
// Sink closure types
// =====================================================================

pub type EventSink = Box<dyn Fn(&PlaybackEvent) + Send + Sync>;
pub type ActionSink = Box<dyn Fn(&PlaybackAction) + Send + Sync>;

// =====================================================================
// Singleton coordinator
// =====================================================================

static COORD: OnceLock<Mutex<Option<PlaybackCoordinator>>> = OnceLock::new();

pub(crate) fn coord_slot() -> &'static Mutex<Option<PlaybackCoordinator>> {
    COORD.get_or_init(|| Mutex::new(None))
}

fn with_coord<F: FnOnce(&PlaybackCoordinator)>(f: F) {
    let guard = coord_slot().lock();
    if let Some(c) = guard.as_ref() {
        f(c);
    }
}

fn coord_handle() -> Option<CoordinatorHandle> {
    coord_slot()
        .lock()
        .as_ref()
        .map(PlaybackCoordinator::handle)
}

pub fn jfn_playback_init() {
    {
        let mut guard = coord_slot().lock();
        if guard.is_some() {
            return;
        }
        let mut c = PlaybackCoordinator::new();
        register_builtin_sinks(&c);
        c.start();
        *guard = Some(c);
    }
    // The immediate reconcile is load-bearing: mode posts made before the
    // coordinator existed were dropped by `post`, and no wakeup replays them.
    jfn_platform_abi::subscribe_window_changed(
        crate::ingest_driver::jfn_playback_reconcile_window_mode,
    );
    crate::ingest_driver::jfn_playback_reconcile_window_mode();
}

fn register_builtin_sinks(c: &PlaybackCoordinator) {
    c.add_builtin_action_sink(Box::new(|a: &PlaybackAction| match a.kind {
        PlaybackActionKind::ApplyPendingTrackSelectionAndPlay => {
            jfn_mpv::api::jfn_mpv_apply_pending_track_selection_and_play();
        }
    }));

    c.add_builtin_event_sink(Box::new(|ev: &PlaybackEvent| {
        crate::idle_inhibit_sink::deliver(ev);
    }));

    c.add_builtin_event_sink(Box::new(|ev: &PlaybackEvent| {
        crate::browser_sink::deliver(ev);
    }));

    c.add_builtin_event_sink(Box::new(|ev: &PlaybackEvent| {
        crate::theme_color_sink::deliver(ev);
    }));
}

pub fn jfn_playback_shutdown() {
    // stop() joins the worker, whose sinks call post() → coord_slot;
    // holding the guard across stop() deadlocks.
    let coord = coord_slot().lock().take();
    if let Some(mut c) = coord {
        c.stop();
    }
}

pub fn register_event_sink(sink: EventSink) {
    with_coord(|c| c.add_event_sink(sink));
}

pub fn register_action_sink(sink: ActionSink) {
    with_coord(|c| c.add_action_sink(sink));
}

pub fn jfn_playback_snapshot() -> PlaybackSnapshot {
    coord_handle().map_or_else(PlaybackSnapshot::fresh, |h| h.snapshot())
}

// =====================================================================
// Producer entry point
// =====================================================================

pub fn post(input: Input) {
    if let Some(h) = coord_handle() {
        h.enqueue(input);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;
    use std::sync::mpsc;

    /// A channel that fires once per delivered event, registered through the
    /// public sink API so the test also exercises the registration path.
    fn event_pings() -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel::<()>();
        register_event_sink(Box::new(move |_ev| {
            let _ = tx.send(());
        }));
        rx
    }

    #[test]
    fn init_installs_a_coordinator_and_shutdown_removes_it() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        assert!(coord_slot().lock().is_none());
        jfn_playback_init();
        assert!(coord_slot().lock().is_some());
        jfn_playback_shutdown();
        assert!(coord_slot().lock().is_none());
        // Idempotent: a second teardown is a no-op.
        jfn_playback_shutdown();
        assert!(coord_slot().lock().is_none());
    }

    #[test]
    fn a_second_init_keeps_the_running_coordinator() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        jfn_playback_init();
        let rx = event_pings();
        post(Input::FileLoaded);
        test_support::wait_for(&rx, || {
            jfn_playback_snapshot().presence == PlayerPresence::Present
        });
        // A fresh coordinator would report a fresh snapshot again.
        jfn_playback_init();
        assert_eq!(jfn_playback_snapshot().presence, PlayerPresence::Present);
        jfn_playback_shutdown();
    }

    #[test]
    fn the_snapshot_is_fresh_while_no_coordinator_is_installed() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        let s = jfn_playback_snapshot();
        assert_eq!(s.presence, PlayerPresence::None);
        assert_eq!(s.phase, PlaybackPhase::Stopped);
        assert_eq!(s.rate, 1.0);
    }

    #[test]
    fn a_posted_input_reaches_the_state_machine() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        jfn_playback_init();
        let rx = event_pings();
        post(Input::FileLoaded);
        post(Input::Duration(90_000_000));
        test_support::wait_for(&rx, || jfn_playback_snapshot().duration_us == 90_000_000);
        let s = jfn_playback_snapshot();
        assert_eq!(s.presence, PlayerPresence::Present);
        assert_eq!(s.phase, PlaybackPhase::Starting);
        jfn_playback_shutdown();
    }

    #[test]
    fn posting_before_the_coordinator_exists_is_dropped() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        post(Input::FileLoaded);
        post(Input::Position(5_000_000));
        // The drop is silent, and the input is not replayed by a later init.
        jfn_playback_init();
        assert_eq!(jfn_playback_snapshot().position_us, 0);
        jfn_playback_shutdown();
    }

    #[test]
    fn a_registered_event_sink_sees_the_events_of_every_batch() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        jfn_playback_init();
        let seen: std::sync::Arc<parking_lot::Mutex<Vec<PlaybackEventKind>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&seen);
        register_event_sink(Box::new(move |ev| sink.lock().push(ev.kind)));
        let rx = event_pings();
        post(Input::FileLoaded);
        test_support::wait_for(&rx, || {
            seen.lock().contains(&PlaybackEventKind::TrackLoaded)
        });
        jfn_playback_shutdown();
    }

    #[test]
    fn a_registered_action_sink_sees_the_track_selection_action() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        jfn_playback_init();
        let (tx, rx) = mpsc::channel::<PlaybackActionKind>();
        register_action_sink(Box::new(move |a| {
            let _ = tx.send(a.kind);
        }));
        post(Input::FileLoaded);
        assert_eq!(
            rx.recv_timeout(test_support::WORKER_TIMEOUT).unwrap(),
            PlaybackActionKind::ApplyPendingTrackSelectionAndPlay
        );
        jfn_playback_shutdown();
    }

    #[test]
    fn registering_a_sink_without_a_coordinator_is_dropped() {
        let _g = test_support::lock();
        jfn_playback_shutdown();
        let (tx, rx) = mpsc::channel::<()>();
        let atx = tx.clone();
        register_event_sink(Box::new(move |_ev| {
            let _ = tx.send(());
        }));
        register_action_sink(Box::new(move |_a| {
            let _ = atx.send(());
        }));
        // The registrations went nowhere, so a later coordinator never sees
        // them.
        jfn_playback_init();
        post(Input::FileLoaded);
        let pings = event_pings();
        test_support::wait_for(&pings, || {
            jfn_playback_snapshot().presence == PlayerPresence::Present
        });
        assert!(rx.try_recv().is_err());
        jfn_playback_shutdown();
    }
}
