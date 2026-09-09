// JfnCefLayer is an opaque internal handle; callers within this crate
// pass it back unchanged. Marking each consumer unsafe would cascade
// without adding type safety, so the lint is suppressed module-wide.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! WebBrowser business logic.
//!
//! Routes the ~20 jellyfin-web IPC names to mpv, settings, theme color,
//! and the playback coordinator. The web layer's exec_js sink for the
//! playback coordinator is exposed as [`jfn_web_exec_js`] for boot wiring.

use parking_lot::Mutex;
use serde_json::Value;
use std::ffi::c_char;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::browsers::{jfn_browsers_active, jfn_browsers_set_active};
use crate::business_common::{apply_setting_value, js_cstr_or_warn, reject_double_init};
use crate::client::{Inner, JfnCefLayer, jfn_cef_layer_inner, jfn_cef_layer_set_name};
use crate::ipc::{
    ArgList, BrowserMessage, list_bool, list_double, list_int, list_opt_string, list_string,
};
use jfn_color::jfn_cef_parse_color;
use jfn_color::theme::{jfn_theme_color_on_color, jfn_theme_color_set_video_mode};
use jfn_mpv::api::{
    jfn_mpv_audio_add, jfn_mpv_load_file, jfn_mpv_pause, jfn_mpv_play, jfn_mpv_seek_absolute,
    jfn_mpv_set_aspect_mode, jfn_mpv_set_audio_delay, jfn_mpv_set_audio_track, jfn_mpv_set_muted,
    jfn_mpv_set_speed, jfn_mpv_set_subtitle_delay, jfn_mpv_set_subtitle_track, jfn_mpv_set_volume,
    jfn_mpv_stop, jfn_mpv_sub_add,
};
use jfn_mpv::boot::jfn_mpv_handle_get;
use jfn_playback::ab_loop::{jfn_playback_ab_loop_action, jfn_playback_clear_ab_loop};
use jfn_playback::ingest_driver::jfn_playback_fullscreen;
use jfn_playback::shutdown::jfn_shutdown_initiate;
use jfn_playback::{Input as PbInput, MediaType as PbMediaType, post as pb_post};

use jfn_mpv::api::JfnMpvLoadOptions;

// MediaType matching jfn-playback's enum: Unknown=0, Audio=1, Video=2.
const MT_UNKNOWN: u8 = 0;
const MT_AUDIO: u8 = 1;
const MT_VIDEO: u8 = 2;

#[derive(Default, Debug, PartialEq)]
struct MediaMetadata {
    id: String,
    title: String,
    artist: String,
    album: String,
    track_number: i32,
    duration_us: i64,
    media_type: u8,
}

/// `playerLoad`'s arguments, as read off the page-controlled list. Every
/// field has a defined value for a call of any arity — including
/// `jmpNative.playerLoad()` with none at all.
#[derive(Default, Debug, PartialEq)]
struct PlayerLoad {
    url: String,
    start_ms: i32,
    video_idx: i64,
    audio_idx: i64,
    sub_idx: i64,
    metadata_json: String,
    external_audio_url: String,
    external_sub_url: String,
    is_infinite_stream: bool,
}

struct WebState {
    layer: Arc<Inner>,
    was_fullscreen_before_osd: bool,
}

static INSTANCE: Mutex<Option<WebState>> = Mutex::new(None);

pub fn jfn_web_init(layer: *mut JfnCefLayer) {
    if layer.is_null() {
        return;
    }
    // Reject double-init: prior INSTANCE would be silently overwritten and
    // its `was_fullscreen_before_osd` state lost.
    if reject_double_init(&INSTANCE.lock(), "jfn_web_init") {
        return;
    }

    let name = c"web";
    unsafe { jfn_cef_layer_set_name(layer, name.as_ptr()) };

    let inner = unsafe { jfn_cef_layer_inner(layer) };
    install_handlers(layer, Arc::clone(&inner));

    *INSTANCE.lock() = Some(WebState {
        layer: inner,
        was_fullscreen_before_osd: false,
    });
}

/// Execute JS in the main web layer. Called by the playback browser sink.
///
/// # Safety
/// `js_utf8` must be a NUL-terminated UTF-8 pointer, or null.
pub unsafe fn jfn_web_exec_js(js_utf8: *const c_char) {
    if js_utf8.is_null() {
        return;
    }
    // Clone the Arc<Inner> out under lock then release the lock before the
    // CEF call. The Arc keeps Inner alive across the call even if the layer
    // is closed mid-way; no TOCTOU window between lock-drop and use.
    let inner = match INSTANCE.lock().as_ref() {
        Some(s) => Arc::clone(&s.layer),
        None => return,
    };
    let js = unsafe { std::ffi::CStr::from_ptr(js_utf8) }.to_string_lossy();
    inner.exec_js(&js);
}

fn install_handlers(layer: *mut JfnCefLayer, inner_for_created: Arc<Inner>) {
    let l = unsafe { &*layer };

    l.set_created_callback_rust(Some(Box::new(move |_b: *mut c_void| {
        // Main browser takes input only if no other layer has already
        // claimed it (e.g. the server-selection overlay).
        if jfn_browsers_active().is_null() {
            let p = inner_for_created.layer_ptr();
            if !p.is_null() {
                jfn_browsers_set_active(p);
            }
        }
    })));

    l.set_message_handler_rust(Some(Box::new(handle_message)));

    // BeforeClose: clear INSTANCE so any post-close jfn_web_exec_js becomes
    // a no-op instead of touching a torn-down layer.
    l.set_before_close_callback_rust(Some(Box::new(|| {
        *INSTANCE.lock() = None;
    })));

    l.set_context_menu_builder_rust(Some(crate::app_menu::build_closure()));
    l.set_context_menu_dispatcher_rust(Some(crate::app_menu::dispatch_closure()));
}

fn parse_metadata_json(json: &str) -> MediaMetadata {
    let mut out = MediaMetadata::default();
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return out;
    };
    let Value::Object(d) = v else { return out };

    let get_str = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();

    out.id = get_str("Id");
    out.title = get_str("Name");
    out.artist = get_str("SeriesName");
    if out.artist.is_empty()
        && let Some(arr) = d.get("Artists").and_then(Value::as_array)
        && let Some(first) = arr.first().and_then(Value::as_str)
    {
        out.artist = first.to_string();
    }
    out.album = get_str("SeasonName");
    if out.album.is_empty() {
        out.album = get_str("Album");
    }
    if let Some(n) = d.get("IndexNumber").and_then(Value::as_i64) {
        out.track_number = n as i32;
    }
    if let Some(t) = d.get("RunTimeTicks") {
        let ticks = t
            .as_f64()
            .or_else(|| t.as_i64().map(|n| n as f64))
            .unwrap_or(0.0);
        out.duration_us = ticks as i64 / 10;
    }
    out.media_type = match get_str("Type").as_str() {
        "Audio" => MT_AUDIO,
        "Movie" | "Episode" | "Video" | "MusicVideo" => MT_VIDEO,
        _ => MT_UNKNOWN,
    };
    out
}

fn media_type_to_pb(t: u8) -> PbMediaType {
    match t {
        MT_AUDIO => PbMediaType::Audio,
        MT_VIDEO => PbMediaType::Video,
        _ => PbMediaType::Unknown,
    }
}

fn post_metadata(meta: &MediaMetadata) {
    pb_post(PbInput::Metadata(jfn_playback::MediaMetadata {
        id: meta.id.clone(),
        title: meta.title.clone(),
        artist: meta.artist.clone(),
        album: meta.album.clone(),
        track_number: meta.track_number,
        duration_us: meta.duration_us,
        art_url: String::new(),
        art_data_uri: String::new(),
        media_type: media_type_to_pb(meta.media_type),
    }));
}

/// mpv keeps `ab-loop-a` / `ab-loop-b` across files, so a loop set on one
/// episode would silently apply to the next. Both ends are dropped on every
/// load and every stop; the observation then pushes the cleared pair out to
/// the OSD, which is how the UI learns about it.
fn clear_ab_loop(reason: &str) {
    jfn_logging::log(
        jfn_logging::CATEGORY_CEF,
        jfn_logging::LEVEL_DEBUG,
        &format!("ab-loop: clearing both points ({reason})"),
    );
    jfn_playback_clear_ab_loop();
}

fn parse_player_load<A: ArgList + ?Sized>(args: &A) -> PlayerLoad {
    PlayerLoad {
        url: list_string(args, 0),
        start_ms: list_int(args, 1),
        video_idx: i64::from(list_int(args, 2)),
        audio_idx: i64::from(list_int(args, 3)),
        sub_idx: i64::from(list_int(args, 4)),
        metadata_json: list_string(args, 5),
        external_audio_url: list_string(args, 6),
        external_sub_url: list_string(args, 7),
        is_infinite_stream: list_bool(args, 8),
    }
}

fn handle_player_load<A: ArgList + ?Sized>(args: &A) {
    clear_ab_loop("playerLoad");
    let PlayerLoad {
        url,
        start_ms,
        video_idx,
        audio_idx,
        sub_idx,
        metadata_json,
        external_audio_url,
        external_sub_url,
        is_infinite_stream,
    } = parse_player_load(args);
    jfn_logging::log(
        jfn_logging::CATEGORY_CEF,
        jfn_logging::LEVEL_INFO,
        &format!(
            "playerLoad: video={video_idx} audio={audio_idx} sub={sub_idx} \
             start={start_ms}ms infinite={is_infinite_stream} \
             extAudio={external_audio_url} extSub={external_sub_url} url={url}"
        ),
    );

    // Gate before any side effect: a refused load must not leave MPRIS/JS
    // believing something started.
    if !media_url_allowed("playerLoad url", &url, false)
        || !media_url_allowed("playerLoad ext audio", &external_audio_url, true)
        || !media_url_allowed("playerLoad ext sub", &external_sub_url, true)
    {
        return;
    }

    let meta = if metadata_json.is_empty() {
        MediaMetadata::default()
    } else {
        parse_metadata_json(&metadata_json)
    };

    // Atomic pre-load posts so MPRIS/JS see start position before
    // mpv has opened the file.
    pb_post(PbInput::LoadStarting(meta.id.clone()));
    pb_post(PbInput::Position(i64::from(start_ms) * 1000));

    if !metadata_json.is_empty() {
        jfn_theme_color_set_video_mode(meta.media_type == MT_VIDEO);
        post_metadata(&meta);
    }

    let Some(url_c) = js_cstr_or_warn("playerLoad url", &url) else {
        return;
    };
    let Some(ext_audio_c) = js_cstr_or_warn("playerLoad ext audio", &external_audio_url) else {
        return;
    };
    let Some(ext_sub_c) = js_cstr_or_warn("playerLoad ext sub", &external_sub_url) else {
        return;
    };
    let opts = JfnMpvLoadOptions {
        start_secs: f64::from(start_ms) / 1000.0,
        video_track: video_idx,
        audio_track: audio_idx,
        sub_track: sub_idx,
        external_audio_url: ext_audio_c.as_ptr(),
        external_sub_url: ext_sub_c.as_ptr(),
        is_infinite_stream,
    };
    unsafe { jfn_mpv_load_file(url_c.as_ptr(), &opts) };
}

/// Run `f` if the IPC arrived with an args list. Always returns `true` —
/// every arm using this is considered "handled" even when args are
/// missing, matching the prior behaviour.
fn with_args<A: ArgList + ?Sized>(args: Option<&A>, f: impl FnOnce(&A)) -> bool {
    if let Some(a) = args {
        f(a);
    }
    true
}

/// `setPlaybackVideoMode`: the per-title mode the web layer resolved from the
/// item's tags, genres and library, applied for this playback only.
///
/// The gate — honoured only while the mode selected for this run is `auto` —
/// lives in `jfn_mpv::video_mode::apply_resolved`, so a stale resolver call
/// (the page script racing a settings change, say) and a `--video-mode`
/// override are handled in one place. Nothing here persists.
fn handle_playback_video_mode(mode: &str, reason: &str, name: &str) {
    match jfn_mpv::VideoMode::parse(mode) {
        Some(m) => {
            jfn_mpv::video_mode::apply_resolved(m, reason, name);
        }
        None => jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            &format!("setPlaybackVideoMode: unknown mode {mode:?}; leaving the chain alone"),
        ),
    }
}

fn handle_message(message: BrowserMessage) -> bool {
    let args = message.args();

    // mpv handle not yet initialised — return false so CEF treats the message as unhandled.
    if jfn_mpv_handle_get().is_null() {
        return false;
    }

    match message.name() {
        "playerLoad" => with_args(args, handle_player_load),
        "playerStop" => {
            clear_ab_loop("playerStop");
            jfn_mpv_stop();
            true
        }
        "playerPause" => {
            jfn_mpv_pause();
            true
        }
        "playerPlay" => {
            jfn_mpv_play();
            true
        }
        "playerSeek" => with_args(args, |a| {
            jfn_mpv_seek_absolute(f64::from(list_int(a, 0)) / 1000.0);
        }),
        "playerSetVolume" => with_args(args, |a| {
            jfn_mpv_set_volume(f64::from(list_int(a, 0)));
        }),
        "playerSetMuted" => with_args(args, |a| {
            jfn_mpv_set_muted(list_bool(a, 0));
        }),
        "playerSetSpeed" => with_args(args, |a| {
            jfn_mpv_set_speed(f64::from(list_int(a, 0)) / 1000.0);
        }),
        // One of "set-a" / "set-b" / "clear" — a step, never a time. mpv
        // stamps the point itself (`jfn_playback::ab_loop`), because it
        // disarms a loop whose `b` is already behind the core's pts and the
        // web layer's position lags that by up to half a second. Nothing is
        // echoed back from here: the OSD redraws off the `ab-loop-a` /
        // `ab-loop-b` observations mpv answers with.
        "playerAbLoop" => with_args(args, |a| {
            let action = list_string(a, 0);
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_DEBUG,
                &format!("playerAbLoop: {action}"),
            );
            jfn_playback_ab_loop_action(&action);
        }),
        "playerSetSubtitle" => with_args(args, |a| {
            let id = i64::from(list_int(a, 0));
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_INFO,
                &format!("playerSetSubtitle: {id}"),
            );
            jfn_mpv_set_subtitle_track(id);
        }),
        "playerAddSubtitle" => with_args(args, |a| {
            let url = list_string(a, 0);
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_INFO,
                &format!("playerAddSubtitle: {url}"),
            );
            if !media_url_allowed("playerAddSubtitle url", &url, false) {
                return;
            }
            if let Some(c) = js_cstr_or_warn("playerAddSubtitle url", &url) {
                unsafe { jfn_mpv_sub_add(c.as_ptr()) };
            }
        }),
        "playerSetAudio" => with_args(args, |a| {
            jfn_mpv_set_audio_track(i64::from(list_int(a, 0)));
        }),
        "playerAddAudio" => with_args(args, |a| {
            let url = list_string(a, 0);
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_INFO,
                &format!("playerAddAudio: {url}"),
            );
            if !media_url_allowed("playerAddAudio url", &url, false) {
                return;
            }
            if let Some(c) = js_cstr_or_warn("playerAddAudio url", &url) {
                unsafe { jfn_mpv_audio_add(c.as_ptr()) };
            }
        }),
        "playerSetAudioDelay" => with_args(args, |a| jfn_mpv_set_audio_delay(list_double(a, 0))),
        "playerSetSubtitleDelay" => {
            with_args(args, |a| jfn_mpv_set_subtitle_delay(list_double(a, 0)))
        }
        "playerSetAspectMode" => with_args(args, |a| {
            let mode = list_string(a, 0);
            if let Some(c) = js_cstr_or_warn("playerSetAspectMode", &mode) {
                unsafe { jfn_mpv_set_aspect_mode(c.as_ptr()) };
            }
        }),
        "playerOsdActive" => with_args(args, |a| {
            let active = list_bool(a, 0);
            // The platform call is made after the guard is dropped: page JS
            // can send this at any rate, and `set_fullscreen` runs a window
            // resize whose synchronous callbacks must never come back round
            // to a held, non-reentrant INSTANCE lock.
            let leave_fullscreen = {
                let mut g = INSTANCE.lock();
                let Some(st) = g.as_mut() else { return };
                if active {
                    st.was_fullscreen_before_osd = jfn_playback_fullscreen();
                    false
                } else {
                    !st.was_fullscreen_before_osd
                }
            };
            if leave_fullscreen {
                jfn_platform_abi::get().set_fullscreen(false);
            }
        }),
        "playerStatsActive" => with_args(args, |a| {
            // jellyfin-web's Playback Info panel polls `getStats()`; the web
            // layer keeps this flag alive while it does. The stats property
            // set is observed only for that window — see
            // `jfn_playback::stats`.
            let active = list_bool(a, 0);
            if jfn_playback::stats::jfn_playback_set_stats_active(active) {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_DEBUG,
                    if active {
                        "playerStatsActive: observing mpv stats properties"
                    } else {
                        "playerStatsActive: unobserving mpv stats properties"
                    },
                );
            }
        }),
        "setPlaybackVideoMode" => with_args(args, |a| {
            handle_playback_video_mode(&list_string(a, 0), &list_string(a, 1), &list_string(a, 2));
        }),
        "toggleFullscreen" => {
            jfn_platform_abi::get().toggle_fullscreen();
            true
        }
        "saveServerUrl" => with_args(args, |a| {
            let url = list_string(a, 0);
            match crate::business_overlay::storable_server_url(&url) {
                Some(url) => {
                    jfn_config::set_server_url(url);
                    jfn_config::settings_save_async();
                }
                None => jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    "saveServerUrl: refused a non-http(s) URL",
                ),
            }
        }),
        "setSettingValue" => with_args(args, |a| {
            let section = list_string(a, 0);
            let key = list_string(a, 1);
            let value = list_opt_string(a, 2);
            apply_setting_value(&section, &key, value.as_deref());
        }),
        "themeColor" => with_args(args, |a| {
            let color = list_string(a, 0);
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_DEBUG,
                &format!("themeColor IPC: {color}"),
            );
            if let Some(c) = js_cstr_or_warn("themeColor", &color) {
                let rgb = unsafe { jfn_cef_parse_color(c.as_ptr()) };
                jfn_theme_color_on_color(rgb);
            }
        }),
        "notifyMetadata" => with_args(args, |a| {
            let meta = parse_metadata_json(&list_string(a, 0));
            jfn_theme_color_set_video_mode(meta.media_type == MT_VIDEO);
            post_metadata(&meta);
        }),
        "notifyArtwork" => with_args(args, |a| {
            pb_post(PbInput::Artwork(list_string(a, 0)));
        }),
        "notifyQueueChange" => with_args(args, |a| {
            pb_post(PbInput::QueueCaps {
                can_go_next: list_bool(a, 0),
                can_go_prev: list_bool(a, 1),
            });
        }),
        "notifyPlaybackState" => {
            // mpv is the authoritative source via coordinator; ignore JS hint.
            true
        }
        "notifySeek" => with_args(args, |a| {
            pb_post(PbInput::Seeked(i64::from(list_int(a, 0)) * 1000));
        }),
        "appExit" => {
            jfn_shutdown_initiate();
            true
        }
        "openConfigDir" => {
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_INFO,
                "Opening mpv home directory",
            );
            if let Some(p) = crate::platform_ops::ops() {
                p.open_path(&jfn_paths::mpv_home());
            }
            true
        }
        _ => false,
    }
}

/// Whether a page-supplied media URL may be handed to mpv. Jellyfin only ever
/// streams over http(s), but mpv understands `file:`, `edl:`, `smb:` and
/// plain paths, and its error text is echoed back to the page through
/// `_nativeEmit('error')`, so an unfiltered URL would let a page read local
/// files or probe the LAN from the client's position. `optional` allows the
/// empty string (no external track).
fn media_url_allowed(label: &str, url: &str, optional: bool) -> bool {
    if (optional && url.is_empty()) || jfn_jellyfin::is_http_url(url) {
        return true;
    }
    jfn_logging::log(
        jfn_logging::CATEGORY_CEF,
        jfn_logging::LEVEL_WARN,
        &format!("{label}: refused a non-http(s) media URL"),
    );
    false
}

#[cfg(test)]
mod media_url_tests {
    use super::media_url_allowed;

    #[test]
    fn media_url_allowed_accepts_http_and_https() {
        assert!(media_url_allowed(
            "t",
            "http://jf.example.com/Videos/1/stream",
            false
        ));
        assert!(media_url_allowed(
            "t",
            "HTTPS://10.0.0.5:8920/x.m3u8?api_key=k",
            false
        ));
    }

    #[test]
    fn media_url_allowed_refuses_every_other_scheme_and_bare_paths() {
        for bad in [
            "file:///C:/Windows/win.ini",
            "edl://http://a/;http://b/",
            "smb://nas/share/movie.mkv",
            r"C:\Users\x\secret.mkv",
            "/etc/passwd",
            "ftp://host/x",
            "http://",
            "",
        ] {
            assert!(!media_url_allowed("t", bad, false), "{bad}");
        }
    }

    #[test]
    fn media_url_allowed_optional_permits_only_the_empty_string() {
        assert!(media_url_allowed("t", "", true));
        assert!(!media_url_allowed("t", "file:///x.srt", true));
        assert!(media_url_allowed("t", "https://jf.example.com/x.srt", true));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::ipc::{ArgValue, TestArgs};

    fn args(values: Vec<ArgValue>) -> TestArgs {
        TestArgs::new(values)
    }

    fn s(v: &str) -> ArgValue {
        ArgValue::Str(v.to_string())
    }

    // --- parse_player_load --------------------------------------------------

    #[test]
    fn parse_player_load_defaults_every_field_when_the_page_passes_nothing() {
        // `jmpNative.playerLoad()` — an empty list. Slots 2..4 used to be
        // read without a bounds check.
        assert_eq!(parse_player_load(&TestArgs::empty()), PlayerLoad::default());
    }

    #[test]
    fn parse_player_load_reads_a_full_argument_list() {
        let a = args(vec![
            s("http://host/Videos/1/stream.mkv"),
            ArgValue::Int(90_000),
            ArgValue::Int(1),
            ArgValue::Int(2),
            ArgValue::Int(-1),
            s("{\"Id\":\"abc\"}"),
            s("http://host/audio.mka"),
            s("http://host/subs.srt"),
            ArgValue::Bool(true),
        ]);
        assert_eq!(
            parse_player_load(&a),
            PlayerLoad {
                url: "http://host/Videos/1/stream.mkv".into(),
                start_ms: 90_000,
                video_idx: 1,
                audio_idx: 2,
                sub_idx: -1,
                metadata_json: "{\"Id\":\"abc\"}".into(),
                external_audio_url: "http://host/audio.mka".into(),
                external_sub_url: "http://host/subs.srt".into(),
                is_infinite_stream: true,
            }
        );
    }

    #[test]
    fn parse_player_load_ignores_slots_of_the_wrong_type() {
        // The V8 relay leaves a slot unset for an object/array/null/undefined
        // argument, and never coerces a string to a number.
        let a = args(vec![
            ArgValue::Int(7),
            s("90000"),
            ArgValue::Unset,
            ArgValue::Bool(true),
            ArgValue::Unset,
            ArgValue::Int(5),
            ArgValue::Unset,
            ArgValue::Double(1.0),
            ArgValue::Int(1),
        ]);
        assert_eq!(parse_player_load(&a), PlayerLoad::default());
    }

    #[test]
    fn parse_player_load_saturates_hostile_numbers() {
        let a = args(vec![
            s(""),
            ArgValue::Double(f64::INFINITY),
            ArgValue::Double(f64::NAN),
            ArgValue::Double(-1e300),
            ArgValue::Double(2.147_483_9e9),
        ]);
        let got = parse_player_load(&a);
        assert_eq!(got.start_ms, i32::MAX);
        assert_eq!(got.video_idx, 0, "NaN");
        assert_eq!(got.audio_idx, i64::from(i32::MIN));
        assert_eq!(got.sub_idx, i64::from(i32::MAX));
        // The start position is milliseconds; both derived values must stay
        // finite rather than overflow.
        assert_eq!(i64::from(got.start_ms) * 1000, 2_147_483_647_000);
        assert!(f64::from(got.start_ms) / 1000.0 < 2_147_484.0);
    }

    #[test]
    fn parse_player_load_passes_hostile_strings_through_untouched() {
        let hostile = "http://h/a'\";\u{2028}</script>\u{0}b";
        let a = args(vec![s(hostile), ArgValue::Int(0), ArgValue::Int(0)]);
        assert_eq!(parse_player_load(&a).url, hostile);
    }

    // --- parse_metadata_json ------------------------------------------------

    #[test]
    fn parse_metadata_json_reads_the_jellyfin_item_fields() {
        let got = parse_metadata_json(
            r#"{"Id":"abc","Name":"Ep 1","SeriesName":"Show","SeasonName":"S1",
                "IndexNumber":3,"RunTimeTicks":12000000,"Type":"Episode"}"#,
        );
        assert_eq!(
            got,
            MediaMetadata {
                id: "abc".into(),
                title: "Ep 1".into(),
                artist: "Show".into(),
                album: "S1".into(),
                track_number: 3,
                duration_us: 1_200_000,
                media_type: MT_VIDEO,
            }
        );
    }

    #[test]
    fn parse_metadata_json_falls_back_to_artists_and_album() {
        let got = parse_metadata_json(
            r#"{"Artists":["A","B"],"Album":"Rec","Type":"Audio","RunTimeTicks":10}"#,
        );
        assert_eq!(got.artist, "A");
        assert_eq!(got.album, "Rec");
        assert_eq!(got.media_type, MT_AUDIO);
        assert_eq!(got.duration_us, 1);
    }

    #[test]
    fn parse_metadata_json_returns_defaults_for_junk() {
        for json in [
            "",
            "not json",
            "null",
            "[]",
            "[{\"Id\":\"x\"}]",
            "\"string\"",
            "12",
            "{",
            "{\"Id\":",
        ] {
            assert_eq!(
                parse_metadata_json(json),
                MediaMetadata::default(),
                "input {json:?}"
            );
        }
    }

    #[test]
    fn parse_metadata_json_ignores_wrong_typed_fields() {
        let got = parse_metadata_json(
            r#"{"Id":{"a":1},"Name":[1,2],"SeriesName":7,"Artists":"A",
                "IndexNumber":"3","RunTimeTicks":"x","Type":42}"#,
        );
        assert_eq!(got, MediaMetadata::default());
    }

    #[test]
    fn parse_metadata_json_survives_out_of_range_numbers() {
        // No panic and no overflow trap on any of these.
        let huge =
            parse_metadata_json(r#"{"RunTimeTicks":1e308,"IndexNumber":9223372036854775807}"#);
        assert_eq!(huge.duration_us, i64::MAX / 10);
        let negative = parse_metadata_json(r#"{"RunTimeTicks":-1e308,"IndexNumber":-1}"#);
        assert_eq!(negative.duration_us, i64::MIN / 10);
        assert_eq!(negative.track_number, -1);
        let fractional = parse_metadata_json(r#"{"RunTimeTicks":15.9}"#);
        assert_eq!(fractional.duration_us, 1);
    }

    #[test]
    fn parse_metadata_json_keeps_hostile_strings_verbatim() {
        // The sinks (MPRIS, `to_js_json`) escape; the parser must not
        // silently truncate or mangle.
        let got = parse_metadata_json(
            "{\"Id\":\"a\\u2028b\",\"Name\":\"</script>\",\"SeriesName\":\"q\\\"q\"}",
        );
        assert_eq!(got.id, "a\u{2028}b");
        assert_eq!(got.title, "</script>");
        assert_eq!(got.artist, "q\"q");

        let big = format!("{{\"Name\":\"{}\"}}", "x".repeat(200_000));
        assert_eq!(parse_metadata_json(&big).title.len(), 200_000);
    }

    #[test]
    fn parse_metadata_json_maps_every_known_type() {
        for (ty, want) in [
            ("Audio", MT_AUDIO),
            ("Movie", MT_VIDEO),
            ("Episode", MT_VIDEO),
            ("Video", MT_VIDEO),
            ("MusicVideo", MT_VIDEO),
            ("audio", MT_UNKNOWN),
            ("Photo", MT_UNKNOWN),
            ("", MT_UNKNOWN),
        ] {
            let json = format!("{{\"Type\":{}}}", serde_json::to_string(ty).unwrap());
            assert_eq!(parse_metadata_json(&json).media_type, want, "type {ty:?}");
        }
    }

    // --- media_type_to_pb ---------------------------------------------------

    #[test]
    fn media_type_to_pb_maps_known_values_and_defaults_the_rest() {
        assert_eq!(media_type_to_pb(MT_AUDIO), PbMediaType::Audio);
        assert_eq!(media_type_to_pb(MT_VIDEO), PbMediaType::Video);
        assert_eq!(media_type_to_pb(MT_UNKNOWN), PbMediaType::Unknown);
        assert_eq!(media_type_to_pb(u8::MAX), PbMediaType::Unknown);
    }

    // --- with_args ----------------------------------------------------------

    #[test]
    fn with_args_skips_the_body_when_the_message_carries_no_list() {
        let mut ran = false;
        assert!(with_args(None::<&TestArgs>, |_| ran = true));
        assert!(!ran, "a message without an argument list must not run");
    }

    #[test]
    fn with_args_runs_the_body_and_still_claims_the_message() {
        let a = args(vec![s("x")]);
        let mut seen = String::new();
        assert!(with_args(Some(&a), |v| seen = list_string(v, 0)));
        assert_eq!(seen, "x");
    }
}
