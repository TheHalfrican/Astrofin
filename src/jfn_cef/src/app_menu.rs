//! App-level CEF context-menu items appended to every browser's menu.
//!
//! The build/dispatch closures returned here are installed via
//! `JfnCefLayer::set_context_menu_builder_rust` /
//! `set_context_menu_dispatcher_rust` by each business wrapper.

use cef::rc::ConvertReturnValue;
use cef::{ImplMenuModel, MenuModel, sys};
use std::os::raw::{c_int, c_void};

// Command IDs numbered from cef_menu_id_t::MENU_ID_USER_FIRST.
const MENU_ID_USER_FIRST: c_int = sys::cef_menu_id_t::MENU_ID_USER_FIRST as c_int;
pub const MENU_ID_TOGGLE_FULLSCREEN: c_int = MENU_ID_USER_FIRST;
pub const MENU_ID_ABOUT: c_int = MENU_ID_USER_FIRST + 1;
pub const MENU_ID_EXIT: c_int = MENU_ID_USER_FIRST + 2;

use jfn_playback::shutdown::jfn_shutdown_initiate;

/// Build closure for [`JfnCefLayer::set_context_menu_builder_rust`].
/// The slot invocation adds one ref to the menu model before calling this,
/// so we adopt it via `wrap_result` (no extra add_ref needed).
pub fn build_closure() -> Box<crate::client::ContextBuilderFn> {
    Box::new(|raw: *mut c_void| {
        if raw.is_null() {
            return;
        }
        let m: MenuModel = (raw as *mut sys::_cef_menu_model_t).wrap_result();
        m.add_item(
            MENU_ID_TOGGLE_FULLSCREEN,
            Some(&cef::CefString::from("Toggle Fullscreen")),
        );
        m.add_item(MENU_ID_ABOUT, Some(&cef::CefString::from("About")));
        m.add_item(MENU_ID_EXIT, Some(&cef::CefString::from("Exit")));
    })
}

/// Dispatch closure for [`JfnCefLayer::set_context_menu_dispatcher_rust`].
pub fn dispatch_closure() -> Box<crate::client::ContextDispatcherFn> {
    Box::new(|cmd: c_int| -> bool {
        if cmd == MENU_ID_TOGGLE_FULLSCREEN {
            if let Some(p) = jfn_platform_abi::try_get() {
                p.toggle_fullscreen();
            }
            true
        } else if cmd == MENU_ID_ABOUT {
            crate::business_about::jfn_about_open();
            true
        } else if cmd == MENU_ID_EXIT {
            jfn_shutdown_initiate();
            true
        } else {
            false
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_app_menu_ids_are_distinct_and_start_at_the_user_range() {
        assert_eq!(MENU_ID_TOGGLE_FULLSCREEN, MENU_ID_USER_FIRST);
        assert_eq!(MENU_ID_ABOUT, MENU_ID_USER_FIRST + 1);
        assert_eq!(MENU_ID_EXIT, MENU_ID_USER_FIRST + 2);
        // Colliding with CEF's own ids would silently hijack a built-in item.
        let ids = [MENU_ID_TOGGLE_FULLSCREEN, MENU_ID_ABOUT, MENU_ID_EXIT];
        for id in ids {
            assert!(id >= MENU_ID_USER_FIRST, "{id} is below the user range");
        }
        assert_eq!(
            ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
            ids.len()
        );
    }

    #[test]
    fn build_closure_ignores_a_null_menu_model() {
        // The slot invocation can hand us a null model if CEF failed to
        // build one; adopting it would be a null deref.
        let build = build_closure();
        build(std::ptr::null_mut());
    }

    #[test]
    fn dispatch_closure_claims_toggle_fullscreen() {
        crate::test_support::install_platform();
        let dispatch = dispatch_closure();
        assert!(dispatch(MENU_ID_TOGGLE_FULLSCREEN));
    }

    #[test]
    fn dispatch_closure_claims_exit_and_initiates_shutdown() {
        let dispatch = dispatch_closure();
        assert!(dispatch(MENU_ID_EXIT));
        assert!(jfn_playback::shutdown::jfn_shutting_down());
    }

    #[test]
    fn dispatch_closure_leaves_every_other_command_to_cef() {
        let dispatch = dispatch_closure();
        assert!(!dispatch(0));
        assert!(!dispatch(MENU_ID_USER_FIRST - 1));
        assert!(!dispatch(MENU_ID_EXIT + 1));
    }
}
