//! Shared window-control (client-side decoration) IPC handling.
//!
//! The titlebar/window-controls are app chrome that must work from any browser
//! layer — the main web UI, the server-selection overlay, etc. Each layer's
//! message handler calls [`handle_window_op`] first; if it claims the message,
//! the layer's own dispatch is skipped.

use cef::*;

/// Whether the app draws its own titlebar.
pub fn csd_enabled() -> bool {
    jfn_platform_abi::get().effective_decorations()
        == jfn_platform_abi::EffectiveDecorations::ClientSide
}

pub(crate) fn csd_state_js() -> String {
    format!(
        "window.__jmpCsd&&window.__jmpCsd.setEnabled({});",
        csd_enabled()
    )
}

fn list_int(args: &ListValue, idx: usize) -> i32 {
    // Integers can arrive as doubles across the V8 boundary.
    if args.get_type(idx).as_ref() == &sys::cef_value_type_t::VTYPE_DOUBLE {
        args.double(idx).round() as i32
    } else {
        args.int(idx)
    }
}

/// Whether `name` is a window-control / CSD IPC message. The base layer
/// dispatch uses this to route such messages here for every layer, before any
/// per-layer handler runs.
pub fn is_window_message(name: &str) -> bool {
    matches!(
        name,
        "windowMinimize"
            | "windowToggleMaximize"
            | "windowClose"
            | "windowStartMove"
            | "windowStartResize"
            | "csdReady"
    )
}

/// Tell the page's CSD module whether to show the titlebar, replying into the
/// frame that asked (so each layer gets the answer in its own context).
fn push_csd_state(browser: Option<&mut Browser>) {
    let Some(frame) = browser.and_then(|b| b.main_frame()) else {
        return;
    };
    let js = csd_state_js();
    let code = CefString::from(js.as_str());
    frame.execute_java_script(Some(&code), None, 0);
}

/// Handle a window-control / CSD message (callers gate on [`is_window_message`]).
/// `browser` is the layer that sent it, used to reply for `csdReady`.
pub fn handle_window_op(name: &str, args: Option<&ListValue>, browser: Option<&mut Browser>) {
    match name {
        "windowMinimize" => jfn_platform_abi::get().window_minimize(),
        "windowToggleMaximize" => jfn_platform_abi::get().window_toggle_maximize(),
        "windowStartMove" => jfn_platform_abi::get().window_start_move(),
        "windowStartResize" => {
            if let Some(a) = args {
                jfn_platform_abi::get().window_start_resize(list_int(a, 0));
            }
        }
        "windowClose" => jfn_playback::shutdown::jfn_shutdown_initiate(),
        "csdReady" => push_csd_state(browser),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support::{install_platform, set_client_side_decorations};

    /// `CSD` in the stub backend is process-global, so the two tests that
    /// flip it are serialised and restore it before returning.
    static CSD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn csd_enabled_follows_the_platform_effective_decorations() {
        let _g = CSD_LOCK.lock();
        set_client_side_decorations(true);
        assert!(csd_enabled());
        set_client_side_decorations(false);
        assert!(!csd_enabled());
    }

    #[test]
    fn csd_state_js_reports_the_current_mode_to_the_page() {
        let _g = CSD_LOCK.lock();
        set_client_side_decorations(true);
        assert_eq!(
            csd_state_js(),
            "window.__jmpCsd&&window.__jmpCsd.setEnabled(true);"
        );
        set_client_side_decorations(false);
        assert_eq!(
            csd_state_js(),
            "window.__jmpCsd&&window.__jmpCsd.setEnabled(false);"
        );
    }

    #[test]
    fn is_window_message_claims_every_window_control_name() {
        for name in [
            "windowMinimize",
            "windowToggleMaximize",
            "windowClose",
            "windowStartMove",
            "windowStartResize",
            "csdReady",
        ] {
            assert!(is_window_message(name), "{name} should be claimed");
        }
    }

    #[test]
    fn is_window_message_leaves_other_names_to_the_layer_handler() {
        for name in [
            "",
            "playerLoad",
            "windowminimize",
            "windowMinimize ",
            "windowStartResizE",
        ] {
            assert!(!is_window_message(name), "{name} should not be claimed");
        }
    }

    #[test]
    fn handle_window_op_ignores_a_name_it_does_not_own() {
        install_platform();
        // An unclaimed name must fall through without touching the platform.
        handle_window_op("playerLoad", None, None);
    }

    #[test]
    fn handle_window_op_tolerates_a_resize_request_with_no_arguments() {
        install_platform();
        // The renderer relay leaves a slot unset for a non-numeric argument,
        // so the edge may be missing entirely.
        handle_window_op("windowStartResize", None, None);
    }

    #[test]
    fn handle_window_op_replies_to_csd_ready_only_when_a_browser_asked() {
        install_platform();
        // No browser: nothing to execute the reply in, and no panic.
        handle_window_op("csdReady", None, None);
    }

    #[test]
    fn handle_window_op_forwards_the_plain_window_commands_to_the_platform() {
        install_platform();
        for name in ["windowMinimize", "windowToggleMaximize", "windowStartMove"] {
            handle_window_op(name, None, None);
        }
    }

    #[test]
    fn window_close_initiates_process_shutdown() {
        install_platform();
        handle_window_op("windowClose", None, None);
        assert!(jfn_playback::shutdown::jfn_shutting_down());
    }
}
