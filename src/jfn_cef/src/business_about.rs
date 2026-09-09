//! AboutBrowser business logic.
//!
//! Self-managing singleton: `jfn_about_open` creates the layer via the
//! Browsers registry and installs handler closures via the JfnCefLayer
//! setters. The unified BeforeClose path in `client::handle_on_before_close`
//! auto-removes the layer from the registry; the Browsers active-stack
//! restores focus to the previous top automatically. This module just
//! tracks open/closed status via `OPEN`.

use cef::{ImplBrowser, ImplBrowserHost};
use std::os::raw::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::browsers::{jfn_browsers_create, jfn_browsers_set_active};
use crate::client::{
    jfn_cef_layer_create, jfn_cef_layer_inner, jfn_cef_layer_set_name, jfn_cef_layer_set_visible,
};
use crate::ipc::{BrowserMessage, list_string};
use crate::platform_ops;

static OPEN: AtomicBool = AtomicBool::new(false);

/// Entry point. Creates the about layer and installs all Rust handler
/// closures. Subsequent calls while the layer is alive are no-ops.
pub fn jfn_about_open() {
    if OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let kind = c"about";
    let layer = unsafe { jfn_browsers_create(kind.as_ptr()) };
    if layer.is_null() {
        OPEN.store(false, Ordering::Release);
        return;
    }

    let name = c"about";
    unsafe { jfn_cef_layer_set_name(layer, name.as_ptr()) };

    let l = unsafe { &*layer };
    let inner = unsafe { jfn_cef_layer_inner(layer) };

    // setCreatedCallback — about wins input whenever it's created.
    let inner_for_created = Arc::clone(&inner);
    l.set_created_callback_rust(Some(Box::new(move |_browser_raw: *mut c_void| {
        let p = inner_for_created.layer_ptr();
        if !p.is_null() {
            jfn_browsers_set_active(p);
        }
    })));

    // setMessageHandler — aboutDismiss / aboutOpenPath.
    l.set_message_handler_rust(Some(Box::new(handle_message)));

    // setContextMenuBuilder / dispatcher — shared app menu.
    l.set_context_menu_builder_rust(Some(crate::app_menu::build_closure()));
    l.set_context_menu_dispatcher_rust(Some(crate::app_menu::dispatch_closure()));

    // BeforeClose: clear the open-status singleton. The Browsers registry
    // removal + active-stack pop are handled unconditionally by
    // `client::handle_on_before_close`.
    l.set_before_close_callback_rust(Some(Box::new(|| {
        OPEN.store(false, Ordering::Release);
    })));

    unsafe {
        jfn_cef_layer_set_visible(layer, true);
        let url = "app://resources/about.html";
        jfn_cef_layer_create(layer, url.as_ptr() as *const _, url.len());
    }
}

fn handle_message(message: BrowserMessage) -> bool {
    let args = message.args();

    match message.name() {
        "aboutDismiss" => {
            if let Some(b) = message.browser()
                && let Some(host) = b.host()
            {
                host.close_browser(0);
            }
            true
        }
        "aboutOpenPath" => {
            let Some(args) = args else { return true };
            if let Some(url) = about_open_path_url(&list_string(args, 0))
                && let Some(p) = platform_ops::ops()
            {
                p.open_external_url(&url);
            }
            true
        }
        _ => false,
    }
}

/// The `file://` URL `aboutOpenPath` hands to the desktop's URL handler, or
/// `None` for an empty path.
///
/// Only the about layer binds `aboutOpenPath` (see
/// `injection::ABOUT_FUNCTIONS`), and that layer only ever loads
/// `app://resources/about.html`, so the path is one of the two the About box
/// itself renders — not a value a jellyfin-web page can choose. The string is
/// passed through unchanged: nothing here percent-encodes it, so a path
/// containing `?`, `#` or a space reaches the handler as written.
fn about_open_path_url(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    Some(format!("file://{path}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn about_open_path_url_prefixes_the_file_scheme() {
        assert_eq!(
            about_open_path_url("C:\\Users\\x\\AppData\\Roaming\\astrofin").as_deref(),
            Some("file://C:\\Users\\x\\AppData\\Roaming\\astrofin")
        );
    }

    #[test]
    fn about_open_path_url_ignores_an_empty_path() {
        // What a zero-argument `jmpNative.aboutOpenPath()` produces.
        assert_eq!(about_open_path_url(""), None);
    }

    #[test]
    fn about_open_path_url_does_not_sanitise_the_path() {
        // Pinned as a known gap, not as desired behaviour: the string is
        // spliced into a URL with no percent-encoding and no traversal check.
        for raw in ["../../evil.exe", "a b#c?d", "\u{2028}", "x\"y"] {
            assert_eq!(
                about_open_path_url(raw).as_deref(),
                Some(format!("file://{raw}").as_str())
            );
        }
    }
}
