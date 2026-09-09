//! Reverse-FFI exec_js callback. C++ installs a single global handler;
//! Rust-side sinks (browser_sink, the jfn-mpris sink) call it to forward JS into
//! the embedded web view.

use parking_lot::Mutex;
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::OnceLock;

type ExecJsCb = extern "C" fn(*const c_char);

fn slot() -> &'static Mutex<Option<ExecJsCb>> {
    static SLOT: OnceLock<Mutex<Option<ExecJsCb>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub(crate) fn call(js: &str) {
    let Some(cb) = *slot().lock() else {
        return;
    };
    if let Ok(c) = CString::new(js) {
        cb(c.as_ptr());
    }
}

/// Install / clear the exec_js callback. `cb == None` clears.
pub fn jfn_playback_set_web_exec_js_handler(cb: Option<ExecJsCb>) {
    *slot().lock() = cb;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;

    #[test]
    fn js_reaches_the_installed_handler_verbatim() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        call("window._nativeEmit('playing')");
        assert_eq!(js.take(), vec!["window._nativeEmit('playing')".to_string()]);
    }

    #[test]
    fn calls_made_with_no_handler_installed_are_dropped() {
        let _g = test_support::lock();
        // Install and clear, so the slot is known-empty whatever ran before.
        let js = test_support::JsRecorder::install();
        jfn_playback_set_web_exec_js_handler(None);
        call("window._nativeEmit('paused')");
        assert!(js.take().is_empty());
    }

    #[test]
    fn a_handler_can_be_replaced_and_the_new_one_receives_the_call() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        jfn_playback_set_web_exec_js_handler(None);
        let js2 = test_support::JsRecorder::install();
        call("a()");
        assert_eq!(js2.take(), vec!["a()".to_string()]);
        drop(js);
    }

    #[test]
    fn js_containing_an_interior_nul_is_dropped_rather_than_truncated() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        call("window._nativeEmit('a\0b')");
        assert!(js.take().is_empty());
    }
}
