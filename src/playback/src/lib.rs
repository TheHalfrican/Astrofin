//! Playback state machine + coordinator.
//!
//! Worker thread drains queued inputs into a deterministic state machine,
//! stamps each emitted event with the post-transition snapshot, and fans
//! out to registered sinks via the FFI vtable. Sink delivery is
//! non-blocking: sinks own their own consumer threads.

pub mod ab_loop;
pub mod browser_sink;
mod coordinator;
pub mod exec_js;
pub mod ffi;
pub mod hotkey;
pub mod idle_inhibit_sink;
mod ingest;
pub mod ingest_driver;
pub mod lifecycle;
pub mod shutdown;
pub mod sink_core;
mod state_machine;
pub mod stats;
pub mod theme_color_sink;
mod types;
pub mod window_source;

pub use coordinator::PlaybackCoordinator;
pub use ffi::*;
pub use hotkey::*;
pub use shutdown::*;
pub use state_machine::PlaybackStateMachine;
pub use types::*;

#[cfg(test)]
mod test_support {
    //! Shared fixtures for this crate's unit tests.
    //!
    //! Much of `jfn-playback` is process-global by design: the coordinator
    //! slot, the exec_js handler, the ingest state and each sink's handler
    //! slot all live in statics. Every test that touches one of those takes
    //! [`lock`] first, so no test can observe another's half-installed state
    //! and none of them depend on the order the harness runs them in.
    #![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::ffi::{CStr, c_char};
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use parking_lot::{Mutex, MutexGuard};

    use crate::coordinator::{Input, PlaybackCoordinator};
    use crate::types::{MediaType, PlaybackPhase, PlaybackSnapshot, PlayerPresence};

    static LOCK: Mutex<()> = Mutex::new(());

    /// How long a test waits on the coordinator worker before giving up.
    pub(crate) const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

    /// Serialise this test against every other one that touches a crate
    /// singleton, and guarantee a platform is installed.
    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        let guard = LOCK.lock();
        install_stub_platform();
        guard
    }

    // ---- exec_js recorder ---------------------------------------------

    static JS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn record_js(s: *const c_char) {
        let text = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
        JS.lock().push(text);
    }

    /// Installs a recording `exec_js` handler for the life of the value and
    /// clears it again on drop, so a test that fails cannot leave the
    /// recorder wired into the next one.
    pub(crate) struct JsRecorder;

    impl JsRecorder {
        pub(crate) fn install() -> Self {
            JS.lock().clear();
            crate::exec_js::jfn_playback_set_web_exec_js_handler(Some(record_js));
            Self
        }

        /// Everything recorded so far, oldest first; the log stays.
        pub(crate) fn calls(&self) -> Vec<String> {
            JS.lock().clone()
        }

        /// Everything recorded so far, clearing the log.
        pub(crate) fn take(&self) -> Vec<String> {
            std::mem::take(&mut *JS.lock())
        }

        /// The single call recorded since the last drain.
        pub(crate) fn only(&self) -> String {
            let calls = self.take();
            assert_eq!(
                calls.len(),
                1,
                "expected exactly one exec_js call, got {calls:?}"
            );
            calls.into_iter().next().unwrap()
        }
    }

    impl Drop for JsRecorder {
        fn drop(&mut self) {
            crate::exec_js::jfn_playback_set_web_exec_js_handler(None);
        }
    }

    // ---- stub platform -------------------------------------------------

    static WINDOW_FULLSCREEN: AtomicBool = AtomicBool::new(false);
    static WINDOW_MAXIMIZED: AtomicBool = AtomicBool::new(false);

    /// What the stub [`jfn_platform_abi::WindowSource`] reports next.
    pub(crate) fn set_stub_window_mode(fullscreen: bool, maximized: bool) {
        WINDOW_FULLSCREEN.store(fullscreen, Ordering::Relaxed);
        WINDOW_MAXIMIZED.store(maximized, Ordering::Relaxed);
    }

    struct StubWindowSource;

    impl jfn_platform_abi::WindowSource for StubWindowSource {
        fn snapshot(&self) -> jfn_platform_abi::WindowSnapshot {
            jfn_platform_abi::WindowSnapshot {
                extent: None,
                position: None,
                maximized: WINDOW_MAXIMIZED.load(Ordering::Relaxed),
                fullscreen: WINDOW_FULLSCREEN.load(Ordering::Relaxed),
            }
        }
    }

    struct StubMediaSink;

    impl jfn_platform_abi::MediaSink for StubMediaSink {
        fn start(&self, _instance: &jfn_platform_abi::Instance) {}
        fn stop(&self) {}
    }

    static STUB_WINDOW_SOURCE: StubWindowSource = StubWindowSource;
    static STUB_MEDIA_SINK: StubMediaSink = StubMediaSink;

    struct StubPlatform;

    impl jfn_platform_abi::Platform for StubPlatform {
        fn display(&self) -> jfn_platform_abi::DisplayBackend {
            jfn_platform_abi::DisplayBackend::Windows
        }
        fn default_window_decorations(&self) -> jfn_platform_abi::WindowDecorations {
            jfn_platform_abi::WindowDecorations::Server
        }
        fn menu_delivery(
            &self,
            _kind: jfn_platform_abi::MenuKind,
        ) -> jfn_platform_abi::MenuDelivery {
            jfn_platform_abi::MenuDelivery::Page
        }
        fn media_session(&self) -> &dyn jfn_platform_abi::MediaSink {
            &STUB_MEDIA_SINK
        }
        fn cef_paths(&self) -> jfn_platform_abi::CefPaths {
            jfn_platform_abi::CefPaths::default()
        }
        fn window_source(&self) -> &dyn jfn_platform_abi::WindowSource {
            &STUB_WINDOW_SOURCE
        }
    }

    /// `jfn_platform_abi::install` panics on a second call, so the whole test
    /// binary shares one stub.
    fn install_stub_platform() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| jfn_platform_abi::install(Box::new(StubPlatform)));
    }

    // ---- coordinator fixtures -------------------------------------------

    /// A started coordinator whose snapshot already says "a video is
    /// playing". mpv is the authority for that state, so the fixture reaches
    /// it with the same inputs the ingest layer would post rather than by
    /// writing the snapshot.
    pub(crate) fn video_playing_coordinator() -> PlaybackCoordinator {
        let mut coord = PlaybackCoordinator::new();
        coord.start();
        let (tx, rx) = mpsc::channel::<()>();
        coord.add_event_sink(Box::new(move |_ev| {
            let _ = tx.send(());
        }));
        coord.enqueue(Input::MediaType(MediaType::Video));
        coord.enqueue(Input::FileLoaded);
        coord.enqueue(Input::CoreIdle(false));
        coord.enqueue(Input::PauseChanged(false));
        wait_for(&rx, || {
            let s = coord.snapshot();
            s.media_type == MediaType::Video
                && s.presence == PlayerPresence::Present
                && s.phase != PlaybackPhase::Stopped
        });
        coord
    }

    /// Poll `done` between worker wakeups until it holds or the timeout runs
    /// out.
    pub(crate) fn wait_for(rx: &mpsc::Receiver<()>, done: impl Fn() -> bool) {
        let deadline = Instant::now() + WORKER_TIMEOUT;
        loop {
            if done() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the coordinator worker never reached the expected state"
            );
            let _ = rx.recv_timeout(Duration::from_millis(25));
        }
    }

    /// A snapshot as a fresh state machine publishes it.
    pub(crate) fn fresh_snapshot() -> PlaybackSnapshot {
        PlaybackSnapshot::fresh()
    }
}
