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

/// The interface scales the settings page offers, as the strings it sends.
/// A value outside this list is a page this build does not know, not a new
/// size to honour: `jfn_config` will clamp a hand-edited `settings.json` into
/// a wider range, but nothing arriving over IPC widens the list.
const INTERFACE_SCALES: [&str; 6] = ["0.65", "0.75", "0.85", "1", "1.15", "1.3"];

/// What an unrecognised `interfaceScale` collapses to: the size the app has
/// always rendered at.
const INTERFACE_SCALE_DEFAULT: &str = "1";

/// What one `setSettingValue` key/value pair resolves to, decided without
/// touching the config store. Page JS chooses both the key and the value, so
/// the routing and the per-key validation are pinned by tests on
/// [`classify_setting`] rather than by the side effects below.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SettingAction<'a> {
    /// The one key that accepts a null value — it clears the override — and
    /// therefore the one decided before the null check.
    WindowDecorations(Option<&'a str>),
    /// A null value for any other key: dropped with a warning, and *not*
    /// followed by a save.
    NullValue,
    /// A key this build does not store. Warned about, but still followed by
    /// a save, as it always has been.
    Unknown,
    Hwdec(&'a str),
    /// `recognised` is false when the page sent a mode this build does not
    /// know; `mode` is then the default rather than the raw value, so an
    /// unknown string is never persisted.
    VideoMode {
        mode: jfn_mpv::VideoMode,
        recognised: bool,
    },
    /// Same shape: an unrecognised notice level collapses to `cpu` instead of
    /// being stored, because the web side reads it back out of `jmpInfo` and
    /// a bad value would be a silently dead setting.
    TranscodeNotice {
        notice: &'static str,
        recognised: bool,
    },
    /// The UI scale CEF's page zoom is driven from. Same shape again: a value
    /// the page's own list does not hold collapses to
    /// [`INTERFACE_SCALE_DEFAULT`] instead of being stored, so a typo can
    /// never leave the UI at a size it cannot be changed back from.
    InterfaceScale {
        scale: &'static str,
        recognised: bool,
    },
    AudioPassthrough(&'a str),
    AudioExclusive(bool),
    AudioChannels(&'a str),
    HideScrollbar(bool),
    LogLevel(&'a str),
    ForceTranscoding(bool),
    DeviceName(&'a str),
}

/// Route one `setSettingValue` pair. Keys match exactly: the page-supplied
/// key is never trimmed or case-folded, so `" hwdec"` and `"HWDEC"` are
/// [`SettingAction::Unknown`].
pub(crate) fn classify_setting<'a>(key: &str, value: Option<&'a str>) -> SettingAction<'a> {
    if key == "windowDecorations" {
        return SettingAction::WindowDecorations(value);
    }
    let Some(value) = value else {
        return SettingAction::NullValue;
    };
    match key {
        "hwdec" => SettingAction::Hwdec(value),
        "videoMode" => {
            // Pre-rename spellings (`movies`, `anime`) still parse and are
            // stored under their new names; anything else falls back to the
            // default rather than being persisted as-is.
            let parsed = jfn_mpv::VideoMode::parse(value);
            SettingAction::VideoMode {
                mode: parsed.unwrap_or_default(),
                recognised: parsed.is_some(),
            }
        }
        "transcodeNotice" => match value {
            "off" => SettingAction::TranscodeNotice {
                notice: "off",
                recognised: true,
            },
            "cpu" => SettingAction::TranscodeNotice {
                notice: "cpu",
                recognised: true,
            },
            "any" => SettingAction::TranscodeNotice {
                notice: "any",
                recognised: true,
            },
            _ => SettingAction::TranscodeNotice {
                notice: "cpu",
                recognised: false,
            },
        },
        "interfaceScale" => match INTERFACE_SCALES.iter().find(|v| **v == value) {
            Some(scale) => SettingAction::InterfaceScale {
                scale,
                recognised: true,
            },
            None => SettingAction::InterfaceScale {
                scale: INTERFACE_SCALE_DEFAULT,
                recognised: false,
            },
        },
        "audioPassthrough" => SettingAction::AudioPassthrough(value),
        "audioExclusive" => SettingAction::AudioExclusive(value == "true"),
        "audioChannels" => SettingAction::AudioChannels(value),
        "hideScrollbar" => SettingAction::HideScrollbar(value == "true"),
        "logLevel" => SettingAction::LogLevel(value),
        "forceTranscoding" => SettingAction::ForceTranscoding(value == "true"),
        "deviceName" => SettingAction::DeviceName(value),
        _ => SettingAction::Unknown,
    }
}

/// `setSettingValue` IPC dispatch. Superset of the keys the overlay and the
/// main web UI send today — both UIs share this single source of truth so
/// new keys land in one place.
pub(crate) fn apply_setting_value(_section: &str, key: &str, value: Option<&str>) {
    match classify_setting(key, value) {
        SettingAction::WindowDecorations(v) => {
            jfn_config::set_window_decorations(v);
            jfn_config::settings_save_async();
            return;
        }
        SettingAction::NullValue => {
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_WARN,
                &format!(
                    "Null value for setting key: {}.{}",
                    jfn_logging::escape_page_string(_section),
                    jfn_logging::escape_page_string(key)
                ),
            );
            return;
        }
        SettingAction::Hwdec(v) => jfn_config::set_hwdec(v),
        // The only setting that takes effect immediately: mpv recompiles the
        // shader chain on the next frame, so the switch is visible mid-playback.
        // An unknown value is rejected to the default rather than persisted.
        SettingAction::VideoMode { mode, recognised } => {
            if !recognised {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!(
                        "unknown videoMode {}; using {}",
                        jfn_logging::escape_page_string(value.unwrap_or_default()),
                        mode.as_str()
                    ),
                );
            }
            jfn_config::set_video_mode(mode.as_str());
            jfn_mpv::video_mode::apply_current(mode);
        }
        // Which transcodes raise the one-time warning at playback start. The
        // web side reads it back out of `jmpInfo` at fire time, so a bad
        // value would be a silently dead setting — validate here instead.
        SettingAction::TranscodeNotice { notice, recognised } => {
            if !recognised {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!(
                        "unknown transcodeNotice {}; using {notice}",
                        jfn_logging::escape_page_string(value.unwrap_or_default())
                    ),
                );
            }
            jfn_config::set_transcode_notice(notice);
        }
        // The second setting that takes effect immediately: the zoom is CEF's
        // own page zoom, so Blink re-lays the page out at the new size and
        // every measurement the theme's JS takes stays truthful — which a CSS
        // transform on the document does not.
        SettingAction::InterfaceScale { scale, recognised } => {
            if !recognised {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!(
                        "unknown interfaceScale {}; using {scale}",
                        jfn_logging::escape_page_string(value.unwrap_or_default())
                    ),
                );
            }
            jfn_config::set_interface_scale(scale);
            crate::business_web::jfn_web_set_interface_scale(jfn_config::interface_scale_factor(
                scale,
            ));
        }
        SettingAction::AudioPassthrough(v) => jfn_config::set_audio_passthrough(v),
        SettingAction::AudioExclusive(v) => jfn_config::set_audio_exclusive(v),
        SettingAction::AudioChannels(v) => jfn_config::set_audio_channels(v),
        SettingAction::HideScrollbar(v) => jfn_config::set_hide_scrollbar(v),
        SettingAction::LogLevel(v) => jfn_config::set_log_level(v),
        SettingAction::ForceTranscoding(v) => jfn_config::set_force_transcoding(v),
        // Pass empty platform_default — Rust setter clears when raw equals
        // the empty string. Neither caller has the live hostname handy here.
        SettingAction::DeviceName(v) => jfn_config::set_device_name(v, ""),
        SettingAction::Unknown => jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            &format!(
                "Unknown setting key: {}.{}",
                jfn_logging::escape_page_string(_section),
                jfn_logging::escape_page_string(key)
            ),
        ),
    }
    jfn_config::settings_save_async();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use jfn_mpv::VideoMode;

    // --- reject_double_init -------------------------------------------------

    #[test]
    fn reject_double_init_passes_an_empty_slot() {
        assert!(!reject_double_init(&None::<u8>, "test_init"));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "called twice")]
    fn reject_double_init_trips_the_debug_assert_on_a_filled_slot() {
        let _ = reject_double_init(&Some(1u8), "test_init");
    }

    // --- js_cstr_or_warn ----------------------------------------------------

    #[test]
    fn js_cstr_or_warn_converts_a_plain_string() {
        let c = js_cstr_or_warn("t", "http://host/a.mkv").unwrap();
        assert_eq!(c.to_str().unwrap(), "http://host/a.mkv");
    }

    #[test]
    fn js_cstr_or_warn_keeps_an_empty_string() {
        assert_eq!(js_cstr_or_warn("t", "").unwrap().to_bytes(), b"");
    }

    #[test]
    fn js_cstr_or_warn_drops_a_string_with_an_interior_nul() {
        // A page can put a NUL anywhere in a JS string; the C side would
        // silently see a truncated URL.
        assert!(js_cstr_or_warn("t", "http://host/a\0.mkv").is_none());
        assert!(js_cstr_or_warn("t", "\0").is_none());
    }

    #[test]
    fn js_cstr_or_warn_keeps_multibyte_and_long_strings() {
        let s = format!("{}\u{2028}\u{1F600}", "a".repeat(100_000));
        assert_eq!(js_cstr_or_warn("t", &s).unwrap().to_bytes().len(), s.len());
    }

    // --- classify_setting ---------------------------------------------------

    #[test]
    fn classify_setting_routes_window_decorations_before_the_null_check() {
        assert_eq!(
            classify_setting("windowDecorations", Some("csd")),
            SettingAction::WindowDecorations(Some("csd"))
        );
        assert_eq!(
            classify_setting("windowDecorations", None),
            SettingAction::WindowDecorations(None)
        );
        // Not validated here: jfn_config parses and drops what it cannot.
        assert_eq!(
            classify_setting("windowDecorations", Some("../../etc")),
            SettingAction::WindowDecorations(Some("../../etc"))
        );
    }

    #[test]
    fn classify_setting_drops_a_null_value_for_every_other_key() {
        for key in ["hwdec", "videoMode", "interfaceScale", "deviceName", "nope"] {
            assert_eq!(
                classify_setting(key, None),
                SettingAction::NullValue,
                "{key}"
            );
        }
    }

    #[test]
    fn classify_setting_rejects_unknown_keys() {
        for key in [
            "",
            " hwdec",
            "hwdec ",
            "HWDEC",
            "hwdec\0",
            "__proto__",
            "../../settings.json",
            "hwdec\u{2028}",
        ] {
            assert_eq!(
                classify_setting(key, Some("x")),
                SettingAction::Unknown,
                "key {key:?} must not be routed"
            );
        }
        // A megabyte of key is still just an unknown key.
        assert_eq!(
            classify_setting(&"k".repeat(1 << 20), Some("x")),
            SettingAction::Unknown
        );
    }

    #[test]
    fn classify_setting_passes_free_form_string_keys_through() {
        // These four are stored verbatim; nothing here validates them.
        assert_eq!(
            classify_setting("hwdec", Some("d3d11va-copy")),
            SettingAction::Hwdec("d3d11va-copy")
        );
        assert_eq!(
            classify_setting("audioPassthrough", Some("ac3,dts")),
            SettingAction::AudioPassthrough("ac3,dts")
        );
        assert_eq!(
            classify_setting("audioChannels", Some("5.1")),
            SettingAction::AudioChannels("5.1")
        );
        assert_eq!(
            classify_setting("logLevel", Some("debug")),
            SettingAction::LogLevel("debug")
        );
        assert_eq!(
            classify_setting("deviceName", Some("Living Room")),
            SettingAction::DeviceName("Living Room")
        );
    }

    #[test]
    fn classify_setting_stores_a_hostile_free_form_value_verbatim() {
        // Documents the gap rather than hiding it: quotes, backslashes and
        // newlines survive into settings.json, and from there into the
        // `JSON.parse(__SETTINGS_JSON__)` preamble in app.rs (the blob is spliced
        // through `jfn_js_json::to_js_json`, so quotes cannot break out).
        let hostile = "');alert(1);//\\\n\u{2028}";
        assert_eq!(
            classify_setting("hwdec", Some(hostile)),
            SettingAction::Hwdec(hostile)
        );
    }

    #[test]
    fn classify_setting_parses_video_mode_and_falls_back_to_the_default() {
        for (raw, want) in [
            ("auto", VideoMode::Auto),
            ("live-action", VideoMode::LiveAction),
            ("animation", VideoMode::Animation),
            ("off", VideoMode::Off),
        ] {
            assert_eq!(
                classify_setting("videoMode", Some(raw)),
                SettingAction::VideoMode {
                    mode: want,
                    recognised: true
                },
                "{raw}"
            );
        }
        for raw in ["", "AUTO", "garbage", "live action", "\u{2028}"] {
            assert_eq!(
                classify_setting("videoMode", Some(raw)),
                SettingAction::VideoMode {
                    mode: VideoMode::default(),
                    recognised: false
                },
                "unknown mode {raw:?} must not be persisted as-is"
            );
        }
    }

    #[test]
    fn classify_setting_clamps_transcode_notice_to_the_three_known_levels() {
        for raw in ["off", "cpu", "any"] {
            assert_eq!(
                classify_setting("transcodeNotice", Some(raw)),
                SettingAction::TranscodeNotice {
                    notice: raw,
                    recognised: true
                }
            );
        }
        for raw in ["", "ANY", "on", "cpu ", "\"any\""] {
            assert_eq!(
                classify_setting("transcodeNotice", Some(raw)),
                SettingAction::TranscodeNotice {
                    notice: "cpu",
                    recognised: false
                },
                "{raw:?}"
            );
        }
    }

    #[test]
    fn classify_setting_takes_each_interface_scale_the_page_offers() {
        for raw in ["0.65", "0.75", "0.85", "1", "1.15", "1.3"] {
            assert_eq!(
                classify_setting("interfaceScale", Some(raw)),
                SettingAction::InterfaceScale {
                    scale: raw,
                    recognised: true
                },
                "{raw}"
            );
        }
    }

    /// The setting the user cannot undo if it goes wrong — a UI at 4 % is one
    /// nobody can reach the settings page in — so an unknown value is never
    /// stored, it collapses to full size and says so.
    #[test]
    fn classify_setting_collapses_an_unknown_interface_scale_to_full_size() {
        for raw in [
            "", "1.0", "0.7", "1.30", " 1", "1 ", "130%", "0", "-1", "99", "NaN", "inf", "abc",
            "\u{2028}",
        ] {
            assert_eq!(
                classify_setting("interfaceScale", Some(raw)),
                SettingAction::InterfaceScale {
                    scale: "1",
                    recognised: false
                },
                "unknown scale {raw:?} must not be persisted as-is"
            );
        }
    }

    /// Whatever `classify_setting` yields is a scale `jfn_config` resolves to
    /// a usable factor — the two halves of the contract meeting.
    #[test]
    fn every_classified_interface_scale_resolves_to_a_factor() {
        for raw in ["0.65", "1", "1.3", "garbage", ""] {
            let SettingAction::InterfaceScale { scale, .. } =
                classify_setting("interfaceScale", Some(raw))
            else {
                panic!("{raw:?} did not route to InterfaceScale");
            };
            let factor = jfn_config::interface_scale_factor(scale);
            assert!(
                (0.5..=2.0).contains(&factor),
                "{raw:?} -> {scale} -> {factor}"
            );
        }
        assert_eq!(jfn_config::interface_scale_factor("1"), 1.0);
    }

    #[test]
    fn classify_setting_reads_the_boolean_keys_as_exactly_the_string_true() {
        for (key, ctor) in [
            (
                "audioExclusive",
                SettingAction::AudioExclusive as fn(bool) -> SettingAction<'static>,
            ),
            ("hideScrollbar", SettingAction::HideScrollbar),
            ("forceTranscoding", SettingAction::ForceTranscoding),
        ] {
            assert_eq!(classify_setting(key, Some("true")), ctor(true), "{key}");
            for raw in ["false", "TRUE", "1", "", " true", "yes"] {
                assert_eq!(
                    classify_setting(key, Some(raw)),
                    ctor(false),
                    "{key} {raw:?}"
                );
            }
        }
    }
}
