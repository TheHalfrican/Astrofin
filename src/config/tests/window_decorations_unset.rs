//! The decoration accessors in a process with no `Platform`.
//!
//! `window_decorations.rs` installs a stub backend, and a backend can be
//! installed only once per process, so the other half of the contract needs a
//! binary of its own: the CEF renderer links this crate to build the injected
//! settings blob and installs nothing, and the four accessors used to panic
//! there rather than answer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use jfn_platform_abi::WindowDecorations;

#[test]
fn the_decoration_accessors_fall_back_to_csd_with_no_backend_installed() {
    assert!(
        jfn_platform_abi::try_get().is_none(),
        "this binary must not install a backend"
    );

    // Nothing chosen: client-side decorations, the one mode every backend
    // supports, instead of a panic.
    jfn_config::set_window_decorations(None);
    assert!(jfn_config::configured_window_decorations().is_none());
    assert_eq!(
        jfn_config::window_decorations_mode(),
        WindowDecorations::Csd
    );
    assert_eq!(jfn_config::window_decorations(), "csd");
    assert!(jfn_config::client_side_decorations());
    assert!(!jfn_config::titlebar_theme_color());

    // An explicit choice is still reported as made; there is nobody to ask
    // whether this machine can honour it.
    jfn_config::set_window_decorations(Some("serverThemed"));
    assert_eq!(
        jfn_config::window_decorations_mode(),
        WindowDecorations::ServerThemed
    );
    assert_eq!(jfn_config::window_decorations(), "serverThemed");
    assert!(!jfn_config::client_side_decorations());
    assert!(jfn_config::titlebar_theme_color());

    jfn_config::set_window_decorations(Some("server"));
    assert_eq!(
        jfn_config::window_decorations_mode(),
        WindowDecorations::Server
    );
    assert!(!jfn_config::client_side_decorations());
}
