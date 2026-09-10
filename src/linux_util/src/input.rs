use std::os::raw::c_int;

use crate::keysym::{RawKey, raw_key_action};
use jfn_input::{jfn_input_dispatch_history_nav, jfn_input_dispatch_key_full};

pub fn jfn_input_dispatch_key_raw(keysym: u32, native_code: u32, mods: u32, pressed: c_int) {
    match raw_key_action(keysym, native_code, pressed) {
        RawKey::HistoryNav(forward) => jfn_input_dispatch_history_nav(forward),
        RawKey::Swallow => {}
        RawKey::Dispatch { vkey, native } => {
            jfn_input_dispatch_key_full(pressed, vkey, native, mods, 0, 0, 0);
        }
    }
}
