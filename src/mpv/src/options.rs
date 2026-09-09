//! Hwdec mode policy: which mpv hardware-decode backends each OS offers.

/// mpv's `hwdec` when the user has not chosen one. VideoToolbox is the
/// platform decoder on macOS and what every other player there defaults to;
/// elsewhere the default stays software. `jfn_config` mirrors this constant
/// (it cannot depend on this crate) and a test in `jfn_rust::cli` pins both.
pub const HWDEC_DEFAULT: &str = if cfg!(target_os = "macos") {
    "videotoolbox"
} else {
    "no"
};

#[expect(
    dead_code,
    reason = "every OS row stays compiled; only CURRENT_OS's variant is constructed"
)]
enum TargetOs {
    Linux,
    Windows,
    Macos,
}

#[cfg(target_os = "linux")]
const CURRENT_OS: TargetOs = TargetOs::Linux;
#[cfg(target_os = "windows")]
const CURRENT_OS: TargetOs = TargetOs::Windows;
#[cfg(target_os = "macos")]
const CURRENT_OS: TargetOs = TargetOs::Macos;

pub fn hwdec_options() -> &'static [&'static str] {
    match CURRENT_OS {
        TargetOs::Linux => &["auto", "no", "vaapi", "nvdec", "vulkan"],
        TargetOs::Windows => &["auto", "no", "d3d11va", "nvdec", "vulkan"],
        TargetOs::Macos => &["auto", "no", "videotoolbox", "vulkan"],
    }
}

pub fn is_valid_hwdec(value: &str) -> bool {
    hwdec_options().contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_always_valid() {
        assert!(is_valid_hwdec("auto"));
        assert!(is_valid_hwdec("no"));
        assert!(is_valid_hwdec(HWDEC_DEFAULT));
    }

    #[test]
    fn rejects_garbage() {
        assert!(!is_valid_hwdec(""));
        assert!(!is_valid_hwdec("garbage"));
    }
}
