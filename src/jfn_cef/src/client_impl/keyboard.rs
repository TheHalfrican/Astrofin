use cef::*;
use std::os::raw::c_int;
use std::sync::Arc;

use crate::client::Inner;
use crate::client_impl::os_ffi::OsKeyEvent;
use crate::client_logic;
use jfn_platform_abi::event_flags::EVENTFLAG_CONTROL_DOWN;

fn action_modifier() -> u32 {
    jfn_platform_abi::try_get()
        .map(|p| p.display().action_modifier_flag())
        .unwrap_or(EVENTFLAG_CONTROL_DOWN)
}

fn is_paste_shortcut(e: &KeyEvent) -> bool {
    let kt: sys::cef_key_event_type_t = e.type_.into();
    if kt != sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN {
        return false;
    }
    client_logic::is_paste_shortcut(true, e.modifiers, e.windows_key_code, action_modifier())
}

wrap_keyboard_handler! {
    pub struct JfnKeyboardHandlerBuilder {
        inner: Arc<Inner>,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: OsKeyEvent<'_>,
            _is_keyboard_shortcut: Option<&mut c_int>,
        ) -> c_int {
            let Some(e) = event else { return 0 };
            if !is_paste_shortcut(e) {
                return 0;
            }
            if self.inner.try_paste() { 1 } else { 0 }
        }
    }
}
