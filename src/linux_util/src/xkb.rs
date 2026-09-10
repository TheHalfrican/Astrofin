//! Shared xkb modifier-state → CEF `EVENTFLAG_*` translation, used by both
//! the X11 and Wayland input backends (their xkb state is queried identically).

use xkbcommon::xkb;

use crate::keysym::cef_mods;

/// Map the effective xkb modifier state to CEF event-flag bits.
pub fn to_cef_mods(st: &xkb::State) -> u32 {
    cef_mods(
        st.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE),
        st.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE),
        st.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE),
    )
}
