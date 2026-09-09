//! The decoration-resolution accessors.
//!
//! `window_decorations_mode` and the three accessors built on it resolve the
//! user's stored choice against the installed `Platform`, which panics if
//! absent — so they cannot be exercised from the crate's own unit tests.
//! This binary installs a stub backend once (the ABI panics on a second
//! install) and drives the store through every mode.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use jfn_platform_abi::{
    CefPaths, DecorationOptions, DisplayBackend, Instance, MediaSink, MenuDelivery, MenuKind,
    Platform, WindowDecorations, WindowSnapshot, WindowSource,
};

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

/// A backend that offers every decoration mode and defaults to the
/// server-drawn titlebar, like Windows and macOS.
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

    fn window_decoration_options(&self) -> DecorationOptions {
        DecorationOptions::all()
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
}

#[test]
fn the_decoration_accessors_resolve_the_stored_choice_against_the_platform() {
    jfn_platform_abi::install(Box::new(StubPlatform {
        media: StubMediaSink,
        window: StubWindowSource,
    }));

    // Nothing chosen: the platform's own default answers.
    jfn_config::set_window_decorations(None);
    assert!(jfn_config::configured_window_decorations().is_none());
    assert_eq!(
        jfn_config::window_decorations_mode(),
        WindowDecorations::Server
    );
    assert_eq!(jfn_config::window_decorations(), "server");
    assert!(!jfn_config::client_side_decorations());
    assert!(!jfn_config::titlebar_theme_color());

    // Client-side: the app draws its own titlebar.
    jfn_config::set_window_decorations(Some("csd"));
    assert_eq!(
        jfn_config::configured_window_decorations(),
        Some(WindowDecorations::Csd)
    );
    assert_eq!(
        jfn_config::window_decorations_mode(),
        WindowDecorations::Csd
    );
    assert_eq!(jfn_config::window_decorations(), "csd");
    assert!(jfn_config::client_side_decorations());
    assert!(!jfn_config::titlebar_theme_color());

    // Server-themed: the WM draws it, tinted from the page's theme colour.
    jfn_config::set_window_decorations(Some("serverThemed"));
    assert_eq!(
        jfn_config::window_decorations_mode(),
        WindowDecorations::ServerThemed
    );
    assert_eq!(jfn_config::window_decorations(), "serverThemed");
    assert!(!jfn_config::client_side_decorations());
    assert!(jfn_config::titlebar_theme_color());

    // An unparseable literal clears the override rather than sticking; the
    // value is hand-editable in settings.json.
    jfn_config::set_window_decorations(Some("not-a-mode"));
    assert!(jfn_config::configured_window_decorations().is_none());
    assert_eq!(jfn_config::window_decorations(), "server");
}
