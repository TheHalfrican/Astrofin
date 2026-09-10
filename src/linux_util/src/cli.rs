//! Linux-only CLI arguments, flattened into the binary's top-level `Cli`.

use clap::{Args, ValueEnum};
use jfn_platform_abi::DisplayBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Paint {
    /// Zero-copy dmabuf shared-texture path: EGL/GBM subsurface on
    /// Wayland, Vulkan external-memory import on X11. Falls back to gpu
    /// then shm if the device can't import dmabufs.
    Dmabuf,
    /// Vulkan pixel-upload via `jfn_gpu_paint`. Falls back to shm when no
    /// Vulkan adapter is usable.
    Gpu,
    /// CPU upload (`wl_shm` / MIT-SHM). The floor of the fallback chain.
    Shm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PlatformArg {
    Wayland,
    X11,
}

/// Which display backend to install, from `--platform` and the session
/// environment.
///
/// An explicit `--platform` always wins. With none, Wayland is chosen
/// whenever the session offers it, and also when neither variable is set:
/// a session with no `DISPLAY` at all has no X server to fall back to, so
/// failing on the Wayland path gives the better diagnostic.
pub fn choose_backend(
    arg: Option<PlatformArg>,
    has_wayland_display: bool,
    has_x_display: bool,
) -> DisplayBackend {
    match arg {
        Some(PlatformArg::Wayland) => DisplayBackend::Wayland,
        Some(PlatformArg::X11) => DisplayBackend::X11,
        None => {
            if has_wayland_display || !has_x_display {
                DisplayBackend::Wayland
            } else {
                DisplayBackend::X11
            }
        }
    }
}

#[derive(Args, Debug)]
pub struct LinuxArgs {
    /// Force the display backend (Linux only).
    #[arg(long, value_enum)]
    pub platform: Option<PlatformArg>,

    /// Preferred paint path (Linux only); falls back dmabuf→gpu→shm.
    /// Values not available on the active backend degrade gracefully.
    #[arg(long, value_enum)]
    pub platform_paint: Option<Paint>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use clap::error::ErrorKind;

    #[derive(Parser, Debug)]
    #[command(args_override_self = true)]
    struct TestCli {
        #[command(flatten)]
        linux: LinuxArgs,
    }

    fn ok(args: &[&str]) -> LinuxArgs {
        TestCli::try_parse_from(args.iter().copied())
            .expect("expected successful parse")
            .linux
    }

    fn err_kind(args: &[&str]) -> ErrorKind {
        TestCli::try_parse_from(args.iter().copied())
            .expect_err("expected parse error")
            .kind()
    }

    #[test]
    fn platform_values() {
        assert_eq!(
            ok(&["app", "--platform", "x11"]).platform,
            Some(PlatformArg::X11)
        );
        assert_eq!(
            ok(&["app", "--platform", "wayland"]).platform,
            Some(PlatformArg::Wayland)
        );
    }

    #[test]
    fn platform_unknown_value_errors() {
        assert_eq!(
            err_kind(&["app", "--platform=bogus"]),
            ErrorKind::InvalidValue
        );
    }

    #[test]
    fn paint_values() {
        assert_eq!(
            ok(&["app", "--platform-paint=dmabuf"]).platform_paint,
            Some(Paint::Dmabuf)
        );
        assert_eq!(
            ok(&["app", "--platform-paint=gpu"]).platform_paint,
            Some(Paint::Gpu)
        );
        assert_eq!(
            ok(&["app", "--platform-paint", "shm"]).platform_paint,
            Some(Paint::Shm)
        );
    }

    #[test]
    fn paint_unknown_value_errors() {
        assert_eq!(
            err_kind(&["app", "--platform-paint=bogus"]),
            ErrorKind::InvalidValue
        );
    }

    #[test]
    fn paint_last_wins() {
        assert_eq!(
            ok(&["app", "--platform-paint=dmabuf", "--platform-paint", "shm"]).platform_paint,
            Some(Paint::Shm)
        );
    }

    #[test]
    fn wid_is_rejected() {
        assert_eq!(
            err_kind(&["app", "--wid", "1234"]),
            ErrorKind::UnknownArgument
        );
    }
    #[test]
    fn an_explicit_platform_flag_wins_over_the_session() {
        assert_eq!(
            choose_backend(Some(PlatformArg::X11), true, false),
            DisplayBackend::X11
        );
        assert_eq!(
            choose_backend(Some(PlatformArg::Wayland), false, true),
            DisplayBackend::Wayland
        );
    }

    #[test]
    fn a_wayland_session_picks_wayland() {
        assert_eq!(choose_backend(None, true, false), DisplayBackend::Wayland);
        // XWayland sets DISPLAY too; WAYLAND_DISPLAY still decides.
        assert_eq!(choose_backend(None, true, true), DisplayBackend::Wayland);
    }

    #[test]
    fn an_x_only_session_picks_x11() {
        assert_eq!(choose_backend(None, false, true), DisplayBackend::X11);
    }

    #[test]
    fn a_session_with_neither_display_set_picks_wayland() {
        assert_eq!(choose_backend(None, false, false), DisplayBackend::Wayland);
    }
}
