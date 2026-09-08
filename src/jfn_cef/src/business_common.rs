//! Shared helpers for the three `business_*` modules.
//!
//! Two groups, separated by the dividers below:
//!   1. Generic CEF/Rust helpers — could lift into a `cef-rs-helpers` crate.
//!   2. App-specific dispatch — Astrofin config wiring.

use std::ffi::CString;

/// Returns true if the supplied `MutexGuard`-bearing `Option` already holds
/// a value — i.e. a singleton `init` is being called twice. Crashes loud in
/// debug; logs + returns true in release so a programmer error never
/// escalates. `caller` names the offending init for the log line.
pub(crate) fn reject_double_init<T>(slot: &Option<T>, caller: &str) -> bool {
    if slot.is_some() {
        debug_assert!(false, "{caller} called twice");
        jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            &format!("{caller} called twice; ignoring"),
        );
        return true;
    }
    false
}

// --- generic Rust/C interop ------------------------------------------------

/// Convert a JS-supplied string into a `CString` for FFI, logging + dropping
/// on interior NUL. `label` names the IPC arm in the warn message so the
/// log line is enough to locate the offending handler.
///
/// Avoids the prior `CString::new(x).unwrap_or_default()` pattern that
/// silently handed `""` to downstream consumers (e.g. mpv).
pub(crate) fn js_cstr_or_warn(label: &str, s: &str) -> Option<CString> {
    match CString::new(s) {
        Ok(c) => Some(c),
        Err(_) => {
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_WARN,
                &format!("{label}: interior NUL in JS string; dropping IPC"),
            );
            None
        }
    }
}

// --- app-specific dispatch -------------------------------------------------

/// `setSettingValue` IPC dispatch. Superset of the keys the overlay and the
/// main web UI send today — both UIs share this single source of truth so
/// new keys land in one place.
pub(crate) fn apply_setting_value(_section: &str, key: &str, value: Option<&str>) {
    if key == "windowDecorations" {
        jfn_config::set_window_decorations(value);
        jfn_config::settings_save_async();
        return;
    }
    let Some(value) = value else {
        jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            &format!("Null value for setting key: {_section}.{key}"),
        );
        return;
    };
    match key {
        "hwdec" => jfn_config::set_hwdec(value),
        // The only setting that takes effect immediately: mpv recompiles the
        // shader chain on the next frame, so the switch is visible mid-playback.
        // An unknown value is rejected to the default rather than persisted.
        "videoMode" => {
            // Pre-rename spellings (`movies`, `anime`) still parse and are
            // stored under their new names; anything else falls back to the
            // default rather than being persisted as-is.
            let parsed = jfn_mpv::VideoMode::parse(value);
            let mode = parsed.unwrap_or_default();
            if parsed.is_none() {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!("unknown videoMode {value:?}; using {}", mode.as_str()),
                );
            }
            jfn_config::set_video_mode(mode.as_str());
            jfn_mpv::video_mode::apply_current(mode);
        }
        // Which transcodes raise the one-time warning at playback start. The
        // web side reads it back out of `jmpInfo` at fire time, so a bad
        // value would be a silently dead setting — validate here instead.
        "transcodeNotice" => {
            let notice = match value {
                "off" | "cpu" | "any" => value,
                _ => {
                    jfn_logging::log(
                        jfn_logging::CATEGORY_CEF,
                        jfn_logging::LEVEL_WARN,
                        &format!("unknown transcodeNotice {value:?}; using cpu"),
                    );
                    "cpu"
                }
            };
            jfn_config::set_transcode_notice(notice);
        }
        "audioPassthrough" => jfn_config::set_audio_passthrough(value),
        "audioExclusive" => jfn_config::set_audio_exclusive(value == "true"),
        "audioChannels" => jfn_config::set_audio_channels(value),
        "hideScrollbar" => jfn_config::set_hide_scrollbar(value == "true"),
        "logLevel" => jfn_config::set_log_level(value),
        "forceTranscoding" => jfn_config::set_force_transcoding(value == "true"),
        // Pass empty platform_default — Rust setter clears when raw equals
        // the empty string. Neither caller has the live hostname handy here.
        "deviceName" => jfn_config::set_device_name(value, ""),
        _ => jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            &format!("Unknown setting key: {_section}.{key}"),
        ),
    }
    jfn_config::settings_save_async();
}
