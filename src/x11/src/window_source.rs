//! Native [`WindowSource`]: the X11 backend owns the toplevel, so live
//! geometry comes from the geometry thread's state, not mpv ingest. The
//! snapshot mapping itself is [`crate::geometry_logic::window_snapshot`].

use jfn_platform_abi::{WindowSnapshot, WindowSource};

use crate::geometry_logic::{empty_window_snapshot, window_snapshot};

pub struct X11WindowSource;

pub static X11_WINDOW_SOURCE: X11WindowSource = X11WindowSource;

impl WindowSource for X11WindowSource {
    fn snapshot(&self) -> WindowSnapshot {
        if crate::x11_state::host().is_none() {
            return empty_window_snapshot();
        }
        window_snapshot(&crate::x11_state::parent_snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // No host window is ever created in a test binary, so this is the branch
    // that is reachable here — and the one that matters: a caller asking before
    // boot must be told "nothing known", never a default-shaped snapshot that
    // looks like a real 0x0 window at the origin.
    #[test]
    fn before_boot_the_source_reports_no_window_at_all() {
        let snap = X11_WINDOW_SOURCE.snapshot();
        assert!(snap.extent.is_none());
        assert!(snap.position.is_none());
        assert!(!snap.maximized);
        assert!(!snap.fullscreen);
    }
}
