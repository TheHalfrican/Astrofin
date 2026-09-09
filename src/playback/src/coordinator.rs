//! Coordinator: owns the single mutable state machine, worker thread, and
//! sink fanout. Producers post inputs from any thread; the worker drains
//! them in batches, runs transitions, publishes the canonical snapshot,
//! and hands events to registered sinks via the FFI vtable.

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::ffi::{ActionSink, EventSink};
use crate::state_machine::PlaybackStateMachine;
use crate::types::*;

#[derive(Debug)]
pub enum Input {
    FileLoaded,
    LoadStarting(String),
    PauseChanged(bool),
    EndFile {
        reason: EndReason,
        error_message: String,
    },
    SeekingChanged(bool),
    PausedForCache(bool),
    CoreIdle(bool),
    Position(i64),
    MediaType(MediaType),
    VideoFrameAvailable(bool),
    Speed(f64),
    Duration(i64),
    Fullscreen {
        fullscreen: bool,
        was_maximized: bool,
    },
    BufferedRanges(Vec<PlaybackBufferedRange>),
    DisplayHz(f64),
    Metadata(MediaMetadata),
    Artwork(String),
    QueueCaps {
        can_go_next: bool,
        can_go_prev: bool,
    },
    Seeked(i64),
}

struct Shared {
    /// Producer end, cleared by `stop` so the worker's `recv` disconnects
    /// even while handles remain alive.
    tx: Mutex<Option<Sender<Input>>>,
    snapshot: Mutex<PlaybackSnapshot>,
    event_sinks: Mutex<Vec<EventSink>>,
    action_sinks: Mutex<Vec<ActionSink>>,
    builtin_event_sinks: Mutex<Vec<EventSink>>,
    builtin_action_sinks: Mutex<Vec<ActionSink>>,
}

pub struct PlaybackCoordinator {
    shared: Arc<Shared>,
    /// Handed to the worker by `start`; `None` afterwards, which makes a
    /// second `start` a no-op.
    rx: Option<Receiver<Input>>,
    join: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct CoordinatorHandle(Arc<Shared>);

impl CoordinatorHandle {
    pub fn enqueue(&self, in_: Input) {
        if let Some(tx) = self.0.tx.lock().as_ref() {
            let _ = tx.send(in_);
        }
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.0.snapshot.lock().clone()
    }
}

impl PlaybackCoordinator {
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            shared: Arc::new(Shared {
                tx: Mutex::new(Some(tx)),
                snapshot: Mutex::new(PlaybackSnapshot::fresh()),
                event_sinks: Mutex::new(Vec::new()),
                action_sinks: Mutex::new(Vec::new()),
                builtin_event_sinks: Mutex::new(Vec::new()),
                builtin_action_sinks: Mutex::new(Vec::new()),
            }),
            rx: Some(rx),
            join: None,
        }
    }

    pub fn start(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        self.join = Some(thread::spawn(move || worker(rx, shared)));
    }

    pub fn stop(&mut self) {
        drop(self.shared.tx.lock().take());
        if let Some(h) = self.join.take()
            && let Err(e) = h.join()
        {
            eprintln!("[playback] coordinator worker panicked: {e:?}");
        }
    }

    pub fn handle(&self) -> CoordinatorHandle {
        CoordinatorHandle(Arc::clone(&self.shared))
    }

    pub fn enqueue(&self, in_: Input) {
        self.handle().enqueue(in_);
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.handle().snapshot()
    }

    pub fn add_event_sink(&self, sink: EventSink) {
        self.shared.event_sinks.lock().push(sink);
    }

    pub fn add_action_sink(&self, sink: ActionSink) {
        self.shared.action_sinks.lock().push(sink);
    }

    pub fn add_builtin_event_sink(&self, sink: EventSink) {
        self.shared.builtin_event_sinks.lock().push(sink);
    }

    pub fn add_builtin_action_sink(&self, sink: ActionSink) {
        self.shared.builtin_action_sinks.lock().push(sink);
    }
}

impl Default for PlaybackCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PlaybackCoordinator {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker(rx: Receiver<Input>, shared: Arc<Shared>) {
    let mut sm = PlaybackStateMachine::new();
    while let Ok(first) = rx.recv() {
        jfn_mpv::memprobe::note_coordinator_qlen(rx.len());
        let mut events: Vec<PlaybackEvent> = Vec::new();
        let mut actions: Vec<PlaybackAction> = Vec::new();
        for input in std::iter::once(first).chain(rx.try_iter()) {
            apply(&mut sm, input, &mut events);
            actions.extend(sm.consume_actions());
        }

        let snap = sm.snapshot();
        for e in &mut events {
            e.snapshot = snap.clone();
        }
        {
            let mut s = shared.snapshot.lock();
            *s = snap;
        }

        // Sinks: dispatched in registration order. Each closure must
        // not block; sinks own their own queue + consumer thread.
        let event_sinks = shared.event_sinks.lock();
        for sink in event_sinks.iter() {
            for e in &events {
                sink(e);
            }
        }
        let action_sinks = shared.action_sinks.lock();
        for sink in action_sinks.iter() {
            for a in &actions {
                sink(a);
            }
        }
        let builtin_event_sinks = shared.builtin_event_sinks.lock();
        for sink in builtin_event_sinks.iter() {
            for e in &events {
                sink(e);
            }
        }
        let builtin_action_sinks = shared.builtin_action_sinks.lock();
        for sink in builtin_action_sinks.iter() {
            for a in &actions {
                sink(a);
            }
        }
    }
}

fn apply(sm: &mut PlaybackStateMachine, input: Input, out: &mut Vec<PlaybackEvent>) {
    let mut emitted = match input {
        Input::FileLoaded => sm.on_file_loaded(),
        Input::LoadStarting(id) => sm.on_load_starting(id),
        Input::PauseChanged(paused) => sm.on_pause_changed(paused),
        Input::EndFile {
            reason,
            error_message,
        } => sm.on_end_file(reason, error_message),
        Input::SeekingChanged(seeking) => sm.on_seeking_changed(seeking),
        Input::PausedForCache(pfc) => sm.on_paused_for_cache(pfc),
        Input::CoreIdle(ci) => sm.on_core_idle(ci),
        Input::Position(p) => sm.on_position(p),
        Input::MediaType(t) => sm.on_media_type(t),
        Input::VideoFrameAvailable(a) => sm.on_video_frame_available(a),
        Input::Speed(r) => sm.on_speed(r),
        Input::Duration(d) => sm.on_duration(d),
        Input::Fullscreen {
            fullscreen,
            was_maximized,
        } => sm.on_fullscreen(fullscreen, was_maximized),
        Input::BufferedRanges(r) => sm.on_buffered_ranges(r),
        Input::DisplayHz(h) => sm.on_display_hz(h),
        Input::Metadata(m) => {
            // Route media_type through the SM so snapshot.media_type
            // tracks metadata changes (idle inhibit reads it).
            let mut events = sm.on_media_type(m.media_type);
            let mut ev = PlaybackEvent::new(PlaybackEventKind::MetadataChanged);
            ev.metadata = m;
            events.push(ev);
            events
        }
        Input::Artwork(uri) => {
            let mut ev = PlaybackEvent::new(PlaybackEventKind::ArtworkChanged);
            ev.artwork_uri = uri;
            vec![ev]
        }
        Input::QueueCaps {
            can_go_next,
            can_go_prev,
        } => {
            let mut ev = PlaybackEvent::new(PlaybackEventKind::QueueCapsChanged);
            ev.can_go_next = can_go_next;
            ev.can_go_prev = can_go_prev;
            vec![ev]
        }
        Input::Seeked(p) => {
            let mut events = sm.on_position(p);
            events.push(PlaybackEvent::new(PlaybackEventKind::Seeked));
            events
        }
    };
    out.append(&mut emitted);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support::WORKER_TIMEOUT;
    use std::sync::mpsc;

    /// A started coordinator plus a channel that fires once per event the
    /// worker dispatches, so a test can wait on the worker instead of
    /// sleeping.
    fn started() -> (PlaybackCoordinator, mpsc::Receiver<PlaybackEventKind>) {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        let (tx, rx) = mpsc::channel();
        coord.add_event_sink(Box::new(move |ev| {
            let _ = tx.send(ev.kind);
        }));
        (coord, rx)
    }

    fn drain(rx: &mpsc::Receiver<PlaybackEventKind>, n: usize) -> Vec<PlaybackEventKind> {
        (0..n)
            .map(|_| rx.recv_timeout(WORKER_TIMEOUT).expect("worker went quiet"))
            .collect()
    }

    #[test]
    fn snapshot_starts_fresh() {
        let coord = PlaybackCoordinator::new();
        let s = coord.snapshot();
        assert_eq!(s.presence, PlayerPresence::None);
        assert_eq!(s.phase, PlaybackPhase::Stopped);
        assert_eq!(s.rate, 1.0);
    }

    #[test]
    fn worker_updates_snapshot_after_input() {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        // Register a sink BEFORE enqueuing so the first dispatched batch
        // signals the channel. Sinks fire on the worker thread after the
        // snapshot is published, so receiving = snapshot is up-to-date.
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        coord.add_event_sink(Box::new(move |_ev| {
            let _ = tx.try_send(());
        }));
        coord.enqueue(Input::FileLoaded);
        rx.recv().expect("worker never published an event");
        let s = coord.snapshot();
        assert_eq!(s.presence, PlayerPresence::Present);
        assert_eq!(s.phase, PlaybackPhase::Starting);
        coord.stop();
    }

    #[test]
    fn default_matches_a_new_coordinator() {
        let coord = PlaybackCoordinator::default();
        assert_eq!(coord.snapshot(), PlaybackCoordinator::new().snapshot());
    }

    #[test]
    fn a_handle_shares_the_coordinators_queue_and_snapshot() {
        let (mut coord, rx) = started();
        let handle = coord.handle();
        handle.enqueue(Input::Duration(42_000_000));
        drain(&rx, 1);
        assert_eq!(handle.snapshot().duration_us, 42_000_000);
        assert_eq!(coord.snapshot(), handle.snapshot());
        coord.stop();
    }

    #[test]
    fn a_second_start_does_not_spawn_a_second_worker() {
        let (mut coord, rx) = started();
        coord.start();
        coord.enqueue(Input::FileLoaded);
        assert_eq!(drain(&rx, 1), vec![PlaybackEventKind::TrackLoaded]);
        // A second worker draining the same queue would double every event.
        coord.enqueue(Input::Duration(1));
        assert_eq!(drain(&rx, 1), vec![PlaybackEventKind::DurationChanged]);
        coord.stop();
    }

    #[test]
    fn stop_is_idempotent_and_further_inputs_are_dropped() {
        let (mut coord, rx) = started();
        coord.enqueue(Input::FileLoaded);
        drain(&rx, 1);
        coord.stop();
        coord.stop();
        let before = coord.snapshot();
        coord.enqueue(Input::Duration(999));
        assert_eq!(coord.snapshot(), before);
    }

    #[test]
    fn dropping_a_running_coordinator_joins_the_worker_and_releases_its_sinks() {
        let (tx, rx) = mpsc::channel::<PlaybackEventKind>();
        {
            let mut coord = PlaybackCoordinator::new();
            coord.start();
            coord.add_event_sink(Box::new(move |ev| {
                let _ = tx.send(ev.kind);
            }));
            coord.enqueue(Input::FileLoaded);
            drain(&rx, 1);
        }
        // The worker is joined and the sink closures are gone, so the
        // channel is disconnected rather than merely silent.
        assert!(matches!(
            rx.recv_timeout(WORKER_TIMEOUT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn builtin_sinks_run_after_the_registered_ones() {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        let (tx, rx) = mpsc::channel::<&'static str>();
        let builtin_tx = tx.clone();
        coord.add_event_sink(Box::new(move |_ev| {
            let _ = tx.send("registered");
        }));
        coord.add_builtin_event_sink(Box::new(move |_ev| {
            let _ = builtin_tx.send("builtin");
        }));
        coord.enqueue(Input::FileLoaded);
        assert_eq!(
            rx.recv_timeout(WORKER_TIMEOUT).unwrap(),
            "registered",
            "registered sinks are dispatched before the builtin ones"
        );
        assert_eq!(rx.recv_timeout(WORKER_TIMEOUT).unwrap(), "builtin");
        coord.stop();
    }

    #[test]
    fn both_action_sink_kinds_see_the_track_selection_action() {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        let (tx, rx) = mpsc::channel::<&'static str>();
        let builtin_tx = tx.clone();
        coord.add_action_sink(Box::new(move |a| {
            assert_eq!(
                a.kind,
                PlaybackActionKind::ApplyPendingTrackSelectionAndPlay
            );
            let _ = tx.send("registered");
        }));
        coord.add_builtin_action_sink(Box::new(move |_a| {
            let _ = builtin_tx.send("builtin");
        }));
        coord.enqueue(Input::FileLoaded);
        assert_eq!(rx.recv_timeout(WORKER_TIMEOUT).unwrap(), "registered");
        assert_eq!(rx.recv_timeout(WORKER_TIMEOUT).unwrap(), "builtin");
        coord.stop();
    }

    #[test]
    fn metadata_routes_the_media_type_through_the_state_machine() {
        let (mut coord, rx) = started();
        coord.enqueue(Input::Metadata(MediaMetadata {
            id: "1".into(),
            title: "Track".into(),
            media_type: MediaType::Audio,
            ..Default::default()
        }));
        let kinds = drain(&rx, 2);
        assert_eq!(
            kinds,
            vec![
                PlaybackEventKind::MediaTypeChanged,
                PlaybackEventKind::MetadataChanged
            ]
        );
        // The idle-inhibit sink reads media_type off the snapshot, so it has
        // to move with the metadata.
        assert_eq!(coord.snapshot().media_type, MediaType::Audio);
        coord.stop();
    }

    #[test]
    fn an_artwork_input_emits_only_an_artwork_event() {
        let (mut coord, rx) = started();
        let (atx, arx) = mpsc::channel::<String>();
        coord.add_event_sink(Box::new(move |ev| {
            if ev.kind == PlaybackEventKind::ArtworkChanged {
                let _ = atx.send(ev.artwork_uri.clone());
            }
        }));
        coord.enqueue(Input::Artwork("data:image/png;base64,AAAA".into()));
        assert_eq!(drain(&rx, 1), vec![PlaybackEventKind::ArtworkChanged]);
        assert_eq!(
            arx.recv_timeout(WORKER_TIMEOUT).unwrap(),
            "data:image/png;base64,AAAA"
        );
        coord.stop();
    }

    #[test]
    fn queue_caps_carry_both_flags_through_to_the_sink() {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        let (tx, rx) = mpsc::channel::<(bool, bool)>();
        coord.add_event_sink(Box::new(move |ev| {
            if ev.kind == PlaybackEventKind::QueueCapsChanged {
                let _ = tx.send((ev.can_go_next, ev.can_go_prev));
            }
        }));
        coord.enqueue(Input::QueueCaps {
            can_go_next: true,
            can_go_prev: false,
        });
        assert_eq!(rx.recv_timeout(WORKER_TIMEOUT).unwrap(), (true, false));
        coord.stop();
    }

    #[test]
    fn a_seek_reports_the_new_position_and_then_the_seek() {
        let (mut coord, rx) = started();
        coord.enqueue(Input::Seeked(12_000_000));
        assert_eq!(
            drain(&rx, 2),
            vec![
                PlaybackEventKind::PositionChanged,
                PlaybackEventKind::Seeked
            ]
        );
        assert_eq!(coord.snapshot().position_us, 12_000_000);
        coord.stop();
    }

    #[test]
    fn every_event_of_a_batch_carries_the_post_transition_snapshot() {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        let (tx, rx) = mpsc::channel::<(PlaybackEventKind, i64)>();
        coord.add_event_sink(Box::new(move |ev| {
            let _ = tx.send((ev.kind, ev.snapshot.duration_us));
        }));
        coord.enqueue(Input::FileLoaded);
        coord.enqueue(Input::Duration(7_000_000));
        // Both events are stamped with the state *after* the whole batch.
        for _ in 0..2 {
            let (_, duration) = rx.recv_timeout(WORKER_TIMEOUT).unwrap();
            assert_eq!(duration, 7_000_000);
        }
        coord.stop();
    }

    #[test]
    fn an_input_that_moves_nothing_emits_nothing() {
        let (mut coord, rx) = started();
        coord.enqueue(Input::Duration(5));
        drain(&rx, 1);
        coord.enqueue(Input::Duration(5));
        coord.enqueue(Input::Speed(2.0));
        // The repeated duration is swallowed; the speed change is not.
        assert_eq!(drain(&rx, 1), vec![PlaybackEventKind::RateChanged]);
        coord.stop();
    }
}
