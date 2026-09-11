//! Embedded JS shim sources, included at compile time from `src/web/*.js`.

pub fn get(name: &str) -> Option<&'static str> {
    Some(match name {
        "native-shim.js" => include_str!("../../web/native-shim.js"),
        "video-mode-resolver.js" => include_str!("../../web/video-mode-resolver.js"),
        "mpv-stats.js" => include_str!("../../web/mpv-stats.js"),
        "mpv-player-base.js" => include_str!("../../web/mpv-player-base.js"),
        "mpv-video-player.js" => include_str!("../../web/mpv-video-player.js"),
        "mpv-audio-player.js" => include_str!("../../web/mpv-audio-player.js"),
        "playback-source.js" => include_str!("../../web/playback-source.js"),
        "ab-loop.js" => include_str!("../../web/ab-loop.js"),
        "input-plugin.js" => include_str!("../../web/input-plugin.js"),
        "client-settings.js" => include_str!("../../web/client-settings.js"),
        "csd.js" => include_str!("../../web/csd.js"),
        "select-menu.js" => include_str!("../../web/select-menu.js"),
        "astrofin-theme.js" => include_str!("../../web/astrofin-theme.js"),
        "astrofin-settings.js" => include_str!("../../web/astrofin-settings.js"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Every name the injection profiles can ask for. Kept here rather than
    /// derived from `InjectedScript` so a table entry silently dropped from
    /// one side is caught by the other.
    const NAMES: &[&str] = &[
        "native-shim.js",
        "video-mode-resolver.js",
        "mpv-stats.js",
        "mpv-player-base.js",
        "mpv-video-player.js",
        "mpv-audio-player.js",
        "playback-source.js",
        "ab-loop.js",
        "input-plugin.js",
        "client-settings.js",
        "csd.js",
        "select-menu.js",
        "astrofin-theme.js",
        "astrofin-settings.js",
    ];

    #[test]
    fn every_embedded_script_resolves_to_non_empty_source() {
        for name in NAMES {
            let src = get(name).unwrap_or_else(|| panic!("{name} is not embedded"));
            assert!(!src.trim().is_empty(), "{name} embedded as empty");
        }
    }

    #[test]
    fn every_name_the_injection_profiles_declare_is_embedded() {
        use crate::injection::InjectedScript as S;
        // The renderer looks each script up by the file name the profile
        // declares; a name with no table entry would be silently skipped.
        // Listing the variants by hand makes a new one a compile error here.
        let all = [
            S::NativeShim,
            S::VideoModeResolver,
            S::MpvStats,
            S::MpvPlayerBase,
            S::MpvVideoPlayer,
            S::MpvAudioPlayer,
            S::PlaybackSource,
            S::AbLoop,
            S::InputPlugin,
            S::ClientSettings,
            S::Csd,
            S::SelectMenu,
            S::AstrofinTheme,
            S::AstrofinSettings,
        ];
        assert_eq!(all.len(), NAMES.len());
        for script in all {
            let name = script.file_name();
            assert!(get(name).is_some(), "{name} declared but not embedded");
            assert!(NAMES.contains(&name), "{name} missing from the test table");
        }
    }

    #[test]
    fn an_unknown_script_name_is_none() {
        assert!(get("").is_none());
        assert!(get("native-shim").is_none());
        assert!(get("../../secrets.js").is_none());
        assert!(get("NATIVE-SHIM.JS").is_none());
    }
}
