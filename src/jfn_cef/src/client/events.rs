use std::os::raw::c_int;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use jfn_platform_abi::cursor::CursorShape;

use crate::client_logic::{console_level, format_console};

use super::{Inner, platform_ops, tasks};

impl Inner {
    pub(crate) fn on_fullscreen_mode_change(&self, fullscreen: bool) {
        if let Some(p) = platform_ops::ops() {
            p.set_fullscreen(fullscreen);
        }
    }

    pub(crate) fn emit_cursor(&self, shape: CursorShape) {
        if let Some(handle) = self.cursor_handle() {
            crate::browsers::route_cursor(handle, shape);
        }
    }

    pub(crate) fn on_console_message(&self, level: c_int, msg: &str, src: &str, line: c_int) {
        let formatted = format_console(msg, src, line);
        jfn_logging::log(jfn_logging::CATEGORY_JS, console_level(level), &formatted);
    }

    pub(crate) fn on_load_end(&self, is_main: bool, code: c_int, url: &str) {
        let formatted = format!(
            "CefLayer::OnLoadEnd name={} main={} code={} url={}",
            self.name_str(),
            if is_main { 1 } else { 0 },
            code,
            url,
        );
        jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_INFO,
            &formatted,
        );
        if is_main {
            let _g = self.load_mtx.lock();
            self.loaded.store(true, Ordering::Release);
            self.load_cv.notify_all();
        }
    }

    pub(crate) fn on_load_error(&self, code: c_int, text: &str, url: &str) {
        let formatted = format!(
            "OnLoadError name={} url={} error={} {}",
            self.name_str(),
            url,
            code,
            text,
        );
        jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_ERROR,
            &formatted,
        );
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        let surface = self.surface_handle();
        if surface.is_none() {
            return;
        }
        if let Some(p) = platform_ops::ops() {
            p.surface_set_visible(surface, visible);
        }
    }

    pub(crate) fn try_paste(self: &Arc<Self>) -> bool {
        let Some(p) = platform_ops::ops() else {
            return false;
        };
        if !p.clipboard_text_supported() {
            return false;
        }
        let inner = Arc::clone(self);
        p.clipboard_read_text(Box::new(move |text| {
            if text.is_empty() {
                return;
            }
            tasks::post_paste_js(Arc::clone(&inner), text.to_string());
        }));
        true
    }
}
