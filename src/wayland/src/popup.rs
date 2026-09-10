use jfn_platform_abi::{
    Generation, MenuClose, MenuMetrics, MenuPaint, MenuPlacement, PopupSurface,
};

use crate::root_window::PopupCommand;
use crate::runtime::WlRuntime;

pub(crate) struct WlPopupSurface {
    pub(crate) rt: &'static WlRuntime,
}

impl PopupSurface for WlPopupSurface {
    fn metrics(&self) -> MenuMetrics {
        let extent = self.rt.window().window_extent();
        MenuMetrics {
            scale: extent.map_or_else(|| self.rt.window().cached_scale(), |e| e.scale()),
            clamp_ph: extent.map(|e| e.physical().h()),
        }
    }

    fn create(&self, generation: Generation, place: MenuPlacement, serial: u32) {
        let serial = if serial != 0 {
            serial
        } else {
            self.rt.seat().last_input_serial()
        };
        crate::root_window::popup(
            self.rt,
            PopupCommand::Create {
                generation,
                place,
                serial,
            },
        );
    }

    fn reposition(&self, generation: Generation, place: MenuPlacement) {
        crate::root_window::popup(self.rt, PopupCommand::Reposition { generation, place });
    }

    fn present(&self, paint: MenuPaint) {
        crate::root_window::popup(self.rt, PopupCommand::Paint(paint));
    }

    fn destroy(&self, generation: Generation, reason: MenuClose) {
        match reason {
            // The keyboard-leave swallowed at arm time was our own grab; the
            // window still holds focus and teardown returns the keyboard.
            MenuClose::Speculative => self.rt.seat().discard_suppressed_focus_loss(),
            // A compositor-initiated dismissal means focus left the window, so
            // the swallowed keyboard-leave was real and no re-enter follows.
            MenuClose::External => self.rt.seat().flush_suppressed_focus_loss(),
            MenuClose::Finished => {}
        }
        crate::root_window::popup(self.rt, PopupCommand::Destroy { generation });
    }
}

pub(crate) fn surface_matches(rt: &'static WlRuntime, surface_id: u32) -> bool {
    is_menu_surface(rt, surface_id) && rt.menu().has_menu()
}

/// Unlike [`surface_matches`], true also when no menu is shown: the teardown
/// keyboard-leave arrives after the menu is already cleared.
pub(crate) fn is_menu_surface(rt: &'static WlRuntime, surface_id: u32) -> bool {
    surface_id != 0 && matches_menu_surface(surface_id, rt.root().menu_surface_id())
}

/// Whether a `wl_surface` protocol id names the menu popup. Zero is "no
/// surface" on the wire, so it never matches — not even the initial menu id,
/// which is also zero until the first popup is created.
pub(crate) fn matches_menu_surface(surface_id: u32, menu_surface_id: u32) -> bool {
    surface_id != 0 && menu_surface_id == surface_id
}

#[cfg(test)]
mod tests {
    use super::matches_menu_surface;

    #[test]
    fn the_menus_own_surface_matches() {
        assert!(matches_menu_surface(42, 42));
    }

    #[test]
    fn another_surface_does_not_match() {
        assert!(!matches_menu_surface(41, 42));
        assert!(!matches_menu_surface(42, 41));
    }

    #[test]
    fn a_null_surface_id_never_matches_even_before_any_menu_existed() {
        // Both sides start at zero; an event naming no surface must not be
        // mistaken for the menu.
        assert!(!matches_menu_surface(0, 0));
        assert!(!matches_menu_surface(0, 42));
    }

    #[test]
    fn a_real_surface_does_not_match_an_unset_menu() {
        assert!(!matches_menu_surface(42, 0));
    }
}
