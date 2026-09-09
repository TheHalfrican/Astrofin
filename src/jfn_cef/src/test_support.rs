//! Test-only stand-ins for the process-global singletons this crate reads.
//!
//! Several modules here (`client`, `window_controls`, `browsers`) call
//! [`jfn_platform_abi::get`], which panics before a backend is installed.
//! The unit tests do not want a real window, so [`install_platform`] installs
//! a do-nothing backend exactly once per test process. Every method that has
//! a default in the `Platform` trait keeps it; only the six required ones are
//! implemented, plus `effective_decorations`, which the CSD tests flip.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// The contents live in an inner `#[cfg(test)] mod` so the test-ratio tool
// does not count the stub backend's trait-impl methods as public surface
// (`cargo xtask test-ratio` walks files, not the module graph).
#[cfg(test)]
pub(crate) use support::{install_platform, set_client_side_decorations};

#[cfg(test)]
mod support {
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};

    use jfn_platform_abi::{
        CefPaths, DisplayBackend, EffectiveDecorations, Instance, MediaSink, MenuDelivery,
        MenuKind, Platform, WindowDecorations, WindowSnapshot, WindowSource,
    };

    /// Flipped by [`set_client_side_decorations`]; read by
    /// `StubPlatform::effective_decorations`.
    static CSD: AtomicBool = AtomicBool::new(false);

    struct StubMediaSink;

    impl MediaSink for StubMediaSink {
        fn start(&self, _instance: &Instance) {}
        fn stop(&self) {}
    }

    struct StubWindowSource;

    impl WindowSource for StubWindowSource {
        fn snapshot(&self) -> WindowSnapshot {
            WindowSnapshot {
                extent: None,
                position: None,
                maximized: false,
                fullscreen: false,
            }
        }
    }

    struct StubPlatform {
        media: StubMediaSink,
        window: StubWindowSource,
    }

    impl Platform for StubPlatform {
        fn display(&self) -> DisplayBackend {
            DisplayBackend::Windows
        }

        fn default_window_decorations(&self) -> WindowDecorations {
            WindowDecorations::Server
        }

        fn menu_delivery(&self, _kind: MenuKind) -> MenuDelivery {
            MenuDelivery::Page
        }

        fn media_session(&self) -> &dyn MediaSink {
            &self.media
        }

        fn cef_paths(&self) -> CefPaths {
            CefPaths::default()
        }

        fn window_source(&self) -> &dyn WindowSource {
            &self.window
        }

        fn effective_decorations(&self) -> EffectiveDecorations {
            if CSD.load(Ordering::Acquire) {
                EffectiveDecorations::ClientSide
            } else {
                EffectiveDecorations::ServerSide
            }
        }
    }

    /// Install the stub backend. Idempotent and safe to call from every test —
    /// `jfn_platform_abi::install` panics on a second install, so the `Once`
    /// carries the whole process.
    pub(crate) fn install_platform() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            if jfn_platform_abi::try_get().is_none() {
                jfn_platform_abi::install(Box::new(StubPlatform {
                    media: StubMediaSink,
                    window: StubWindowSource,
                }));
            }
        });
    }

    /// Make the stub report client-side (or server-side) decorations. The flag is
    /// process-global, so a test that flips it restores it before returning.
    pub(crate) fn set_client_side_decorations(on: bool) {
        install_platform();
        CSD.store(on, Ordering::Release);
    }
}
