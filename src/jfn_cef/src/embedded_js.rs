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
        "input-plugin.js" => include_str!("../../web/input-plugin.js"),
        "client-settings.js" => include_str!("../../web/client-settings.js"),
        "csd.js" => include_str!("../../web/csd.js"),
        "select-menu.js" => include_str!("../../web/select-menu.js"),
        "astrofin-theme.js" => include_str!("../../web/astrofin-theme.js"),
        _ => return None,
    })
}
