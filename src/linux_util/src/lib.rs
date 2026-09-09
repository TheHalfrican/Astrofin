//! Linux-only platform helpers shared by the X11 and Wayland backends.
//!
//! The whole crate is `#![cfg(target_os = "linux")]`, so it's an empty rlib
//! elsewhere and the workspace builds uniformly on every platform.

#![cfg(target_os = "linux")]

pub mod cli;
pub mod dmabuf_probe;
pub mod egl;
pub mod idle_inhibit;
pub mod input;
mod keysym;
pub mod menu;
pub mod open_url;
pub mod xkb;

use jfn_platform_abi::{CefPaths, WindowDecorations};

pub fn cef_paths() -> CefPaths {
    let exe = std::fs::canonicalize("/proc/self/exe").unwrap_or_default();
    let res_dir = option_env!("CEF_RESOURCES_DIR")
        .map(str::to_string)
        .unwrap_or_else(|| {
            exe.parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    CefPaths {
        browser_subprocess_path: Some(exe),
        resources_dir_path: Some(std::path::PathBuf::from(&res_dir)),
        locales_dir_path: Some(std::path::PathBuf::from(format!("{res_dir}/locales"))),
        ..Default::default()
    }
}

/// Default *preference*: KDE draws its own server-side decorations and lets
/// us tint them via the palette protocol; elsewhere we draw our own
/// client-side titlebar. Whether server-side decorations are available at all
/// is decided per-backend (Wayland probes the compositor's protocols), not
/// here.
pub fn default_window_decorations() -> WindowDecorations {
    let kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.split(':').any(|s| s.eq_ignore_ascii_case("KDE")))
        .unwrap_or(false);
    if kde {
        WindowDecorations::ServerThemed
    } else {
        WindowDecorations::Csd
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// `XDG_CURRENT_DESKTOP` is process-global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const DESKTOP: &str = "XDG_CURRENT_DESKTOP";

    struct DesktopEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Option<String>,
    }

    impl DesktopEnv {
        fn set(value: Option<&str>) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = std::env::var(DESKTOP).ok();
            match value {
                Some(v) => unsafe { std::env::set_var(DESKTOP, v) },
                None => unsafe { std::env::remove_var(DESKTOP) },
            }
            DesktopEnv {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for DesktopEnv {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => unsafe { std::env::set_var(DESKTOP, v) },
                None => unsafe { std::env::remove_var(DESKTOP) },
            }
        }
    }

    #[test]
    fn cef_paths_point_at_the_running_executable_and_a_locales_dir_beside_the_resources() {
        let paths = cef_paths();
        assert_eq!(
            paths.browser_subprocess_path.as_deref(),
            std::fs::canonicalize("/proc/self/exe").ok().as_deref(),
            "the subprocess path is this executable"
        );
        let resources = paths.resources_dir_path.expect("a resources dir");
        assert_eq!(
            paths.locales_dir_path.expect("a locales dir"),
            std::path::PathBuf::from(format!("{}/locales", resources.display()))
        );
        // Only macOS ships a framework dir.
        assert!(paths.framework_dir_path.is_none());
    }

    #[test]
    fn cef_resources_default_to_the_executables_own_directory() {
        if option_env!("CEF_RESOURCES_DIR").is_some() {
            return; // A build-time override replaces the exe-relative default.
        }
        let exe = std::fs::canonicalize("/proc/self/exe").expect("/proc/self/exe");
        assert_eq!(
            cef_paths().resources_dir_path.as_deref(),
            exe.parent(),
            "resources sit next to the binary"
        );
    }

    #[test]
    fn a_kde_session_prefers_server_side_decorations() {
        let _env = DesktopEnv::set(Some("KDE"));
        assert_eq!(
            default_window_decorations(),
            WindowDecorations::ServerThemed
        );
    }

    #[test]
    fn kde_is_recognised_anywhere_in_the_desktop_list_and_in_any_case() {
        for desktop in ["kde", "ubuntu:KDE", "KDE:X-Cinnamon", "Kde"] {
            let _env = DesktopEnv::set(Some(desktop));
            assert_eq!(
                default_window_decorations(),
                WindowDecorations::ServerThemed,
                "{desktop}"
            );
        }
    }

    #[test]
    fn every_other_session_draws_its_own_titlebar() {
        for desktop in [
            Some("GNOME"),
            Some("sway"),
            Some("ubuntu:GNOME"),
            Some(""),
            None,
        ] {
            let _env = DesktopEnv::set(desktop);
            assert_eq!(
                default_window_decorations(),
                WindowDecorations::Csd,
                "{desktop:?}"
            );
        }
    }

    #[test]
    fn a_desktop_name_that_merely_contains_kde_is_not_a_kde_session() {
        // The match is per `:`-separated entry, not a substring search.
        for desktop in ["KDEish", "not-KDE", "kdenlive"] {
            let _env = DesktopEnv::set(Some(desktop));
            assert_eq!(
                default_window_decorations(),
                WindowDecorations::Csd,
                "{desktop}"
            );
        }
    }
}
