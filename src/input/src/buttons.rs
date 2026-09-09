//! linux/input-event-codes.h pointer button codes — the cross-platform
//! "button code" currency passed to [`crate::jfn_input_dispatch_mouse_button`].
//! Defined here so every backend (Wayland, X11, macOS, Windows) shares one
//! definition instead of redeclaring the literals.

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
pub const BTN_SIDE: u32 = 0x113;
pub const BTN_EXTRA: u32 = 0x114;
pub const BTN_FORWARD: u32 = 0x115;
pub const BTN_BACK: u32 = 0x116;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_codes_match_linux_input_event_codes() {
        // Pinned against linux/input-event-codes.h: every backend translates
        // its native buttons into these, so a drift here would be silent.
        assert_eq!(
            [
                BTN_LEFT,
                BTN_RIGHT,
                BTN_MIDDLE,
                BTN_SIDE,
                BTN_EXTRA,
                BTN_FORWARD,
                BTN_BACK,
            ],
            [0x110, 0x111, 0x112, 0x113, 0x114, 0x115, 0x116]
        );
    }
}
