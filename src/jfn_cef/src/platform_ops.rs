//! Thin re-export shim over [`jfn_platform_abi`].

#[cfg(target_os = "linux")]
pub use jfn_gpu_paint::{DmabufFormat, DmabufPlane};
pub use jfn_gpu_paint::{FrameSize, SharedTexture};
pub use jfn_platform_abi::{
    DisplayBackend, FileDialogFilter, FileDialogKind, FileDialogRequest, JfnRect, MENU_DISMISSED,
    MenuDelivery, MenuItem, MenuKind, MenuRequest, MenuSelection, PaintFrame, PhysicalSize,
    Platform, SurfaceHandle, SurfaceSize,
};

/// Returns the installed platform backend, or `None` if no backend has
/// been installed yet (e.g. early CEF helper-process boot before
/// `jfn_app_main` runs).
pub fn ops() -> Option<&'static dyn Platform> {
    jfn_platform_abi::try_get()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn ops_resolves_the_installed_backend() {
        crate::test_support::install_platform();
        let p = ops().expect("a backend is installed");
        // The one call every early-boot caller makes is cheap and total.
        assert_eq!(p.display(), DisplayBackend::Windows);
    }

    #[test]
    fn ops_is_the_non_panicking_accessor() {
        crate::test_support::install_platform();
        // `jfn_platform_abi::get()` panics before install; `ops()` is the
        // form the CEF helper-process paths use, and it agrees once a
        // backend exists.
        assert!(ops().is_some());
        assert!(jfn_platform_abi::try_get().is_some());
    }
}
