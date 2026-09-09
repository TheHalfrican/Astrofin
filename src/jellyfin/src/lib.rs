//! Jellyfin DeviceProfile JSON builder.
//!
//! Jellyfin source pinned to commit 2c62d40 (matches the
//! `third_party/jellyfin` submodule). Profile-vs-stream matching is
//! plain case-insensitive equality against ffprobe-derived names, so
//! any rename below has to mirror what the server stores on
//! `MediaSource.Container` / `MediaStream.Codec` at probe time.
//!
//! Match logic:
//!   <https://github.com/jellyfin/jellyfin/blob/2c62d40f0d13926874eef9118a95be0dcee4e659/MediaBrowser.Model/Extensions/ContainerHelper.cs#L82-L107>
//! Subtitle match in StreamBuilder:
//!   <https://github.com/jellyfin/jellyfin/blob/2c62d40f0d13926874eef9118a95be0dcee4e659/MediaBrowser.Model/Dlna/StreamBuilder.cs#L1476>
//! Container normalization at probe time (NormalizeFormat):
//!   <https://github.com/jellyfin/jellyfin/blob/2c62d40f0d13926874eef9118a95be0dcee4e659/MediaBrowser.MediaEncoding/Probing/ProbeResultNormalizer.cs#L270-L315>
//! Subtitle normalization at probe time (NormalizeSubtitleCodec):
//!   <https://github.com/jellyfin/jellyfin/blob/2c62d40f0d13926874eef9118a95be0dcee4e659/MediaBrowser.MediaEncoding/Probing/ProbeResultNormalizer.cs#L632-L652>

use serde::Serialize;
use serde_json::Value;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum MediaKind {
    Video,
    Audio,
    Subtitle,
}

#[derive(Clone, Debug)]
pub struct Codec {
    pub name: String,
    pub kind: MediaKind,
}

const SUBTITLE_RENAMES: &[(&str, &str)] = &[
    ("subrip", "srt"),
    ("ass", "ssa"),
    ("hdmv_pgs_subtitle", "PGSSUB"),
    ("dvd_subtitle", "DVDSUB"),
    ("dvb_subtitle", "DVBSUB"),
    ("dvb_teletext", "DVBTXT"),
];

const CONTAINER_RENAMES: &[(&str, &str)] =
    &[("matroska", "mkv"), ("mpegts", "ts"), ("mpegvideo", "mpeg")];

const MAX_STATIC_BITRATE: i64 = 1_000_000_000;
const MUSIC_STREAMING_TRANSCODING_BITRATE: i64 = 1_280_000;
const TIMELINE_OFFSET_SECONDS: i64 = 5;

const TRANSCODE_CONTAINER: &str = "ts";
const TRANSCODE_CONTAINER_MP4: &str = "mp4";
const TRANSCODE_PROTOCOL: &str = "hls";
const TRANSCODE_MAX_AUDIO_CHANNELS: &str = "6";

// Codec sets come from Jellyfin's StreamBuilder._supportedHls* lists (alac
// dropped to stay under the server's 40-char AudioCodec query-param
// validator, ^[a-zA-Z0-9\-\._,|]{0,40}$):
// https://github.com/jellyfin/jellyfin/blob/2c62d40f0d13926874eef9118a95be0dcee4e659/MediaBrowser.Model/Dlna/StreamBuilder.cs#L31-L33
//
// ORDER IS CRITICAL for video: the server picks the output codec straight from
// this list (StreamBuilder keeps profile order; StreamingHelpers takes the
// first entry) and does NO encoder-capability validation — it will happily
// emit `-codec:v av1_nvenc` on a GPU with no AV1 encoder and hard-fail (ffmpeg
// exit 218). So the list must be ordered by descending real-world encode
// compatibility: h264 (every server can hardware-encode it) first, then hevc
// (all recent NVENC/QSV/VAAPI/AMF). av1/vp9 are software-encode-only on the
// vast majority of servers (~0.1x realtime), so they trail as last-resort
// fallbacks only — never reached in practice, since any client that can decode
// av1/vp9 also decodes hevc, which precedes them.
const TRANSCODE_VIDEO_CODEC: &[&str] = &["h264", "hevc", "av1", "vp9"];
const TRANSCODE_AUDIO_CODEC_MP4: &[&str] =
    &["opus", "aac", "eac3", "ac3", "flac", "mp3", "dts", "truehd"];
const TRANSCODE_AUDIO_CODEC_TS: &[&str] = &["aac", "eac3", "ac3", "mp3"];

/// Profile lists this client never populates; serializes as `[]`.
type EmptyList = [(); 0];

const EMPTY_LIST: EmptyList = [];

/// Jellyfin's `DlnaProfileType`.
#[derive(Copy, Clone, Serialize)]
enum ProfileType {
    Video,
    Audio,
    Photo,
}

#[derive(Copy, Clone, Serialize)]
enum SubtitleDeliveryMethod {
    Embed,
    External,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct DirectPlayProfile<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<&'a str>,
    #[serde(rename = "Type")]
    profile_type: ProfileType,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_codec: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_codec: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TranscodingProfile<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<&'a str>,
    #[serde(rename = "Type")]
    profile_type: ProfileType,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_codec: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_codec: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_audio_channels: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SubtitleProfile<'a> {
    format: &'a str,
    method: SubtitleDeliveryMethod,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct DeviceProfile<'a> {
    name: &'a str,
    max_static_bitrate: i64,
    music_streaming_transcoding_bitrate: i64,
    timeline_offset_seconds: i64,
    direct_play_profiles: Vec<DirectPlayProfile<'a>>,
    transcoding_profiles: Vec<TranscodingProfile<'a>>,
    subtitle_profiles: Vec<SubtitleProfile<'a>>,
    response_profiles: EmptyList,
    container_profiles: EmptyList,
    codec_profiles: EmptyList,
}

// Expand each input through `renames` and return the deduped union of raw +
// renamed names. Inputs may be comma-joined ffmpeg aliases (e.g.
// "matroska,webm"); each piece is split before lookup.
fn expand_with_renames(inputs: &[String], renames: &[(&'static str, &'static str)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if s.is_empty() {
            return;
        }
        if !out.iter().any(|e| e == s) {
            out.push(s.to_string());
        }
    };
    for input in inputs {
        for piece in input.split(',') {
            push(piece);
            if let Some((_, renamed)) = renames.iter().find(|(from, _)| *from == piece) {
                push(renamed);
            }
        }
    }
    out
}

// Intersect `preferred` with `available`, keeping `preferred`'s order — the
// server takes the first entry as its output codec — joined as CSV.
fn preferred_csv(preferred: &[&str], available: &[String]) -> String {
    preferred
        .iter()
        .filter(|s| available.iter().any(|a| a == *s))
        .copied()
        .collect::<Vec<&str>>()
        .join(",")
}

/// Build the DeviceProfile JSON.
pub fn build_device_profile(
    decoders: &[Codec],
    demuxers: &[String],
    device_name: &str,
    _app_version: &str,
    force_transcode: bool,
) -> String {
    let mut video_codecs: Vec<String> = Vec::new();
    let mut audio_codecs: Vec<String> = Vec::new();
    let mut subtitle_codecs: Vec<String> = Vec::new();
    for c in decoders {
        match c.kind {
            MediaKind::Video => video_codecs.push(c.name.clone()),
            MediaKind::Audio => audio_codecs.push(c.name.clone()),
            MediaKind::Subtitle => subtitle_codecs.push(c.name.clone()),
        }
    }

    let video_csv = video_codecs.join(",");
    let audio_csv = audio_codecs.join(",");

    let containers = expand_with_renames(demuxers, CONTAINER_RENAMES);
    let subtitle_names = expand_with_renames(&subtitle_codecs, SUBTITLE_RENAMES);

    // TranscodingProfiles tell the server which formats it may transcode TO,
    // not what we can decode. Stick to the curated preference list intersected
    // with mpv's decoder support — adding the rest would (a) invite the server
    // to transcode to formats we don't want as targets (truehd, dts, vp9...)
    // and (b) push the AudioCodec CSV past the server's 40-char query-param
    // validator (^[a-zA-Z0-9\-\._,|]{0,40}$).
    let transcode_video_csv = preferred_csv(TRANSCODE_VIDEO_CODEC, &video_codecs);
    let transcode_audio_csv_mp4 = preferred_csv(TRANSCODE_AUDIO_CODEC_MP4, &audio_codecs);
    let transcode_audio_csv_ts = preferred_csv(TRANSCODE_AUDIO_CODEC_TS, &audio_codecs);

    // DirectPlayProfiles. ContainerHelper.ContainsContainer splits both the
    // profile's Container and the file's MediaSource.Container on comma and
    // does case-insensitive equality, so one entry with every container
    // comma-joined matches identically to N entries with one container each —
    // without repeating the codec CSV per container.
    //
    // Force Transcoding emits NO DirectPlayProfiles at all. An entry whose
    // codec CSV is the empty string does NOT mean "no codec is supported" to
    // the server — DirectPlayProfile.SupportsVideoCodec/SupportsAudioCodec
    // return true when the profile's codec list is empty, i.e. an empty list
    // means "any codec". Emitting empty-CSV entries therefore *widened* direct
    // play to everything instead of blocking it, and forced transcoding did
    // nothing. Only an empty DirectPlayProfiles list leaves the server without
    // a DirectPlay or a DirectStream candidate — StreamBuilder decides both
    // inside its per-DirectPlayProfile projection, so an empty list short-
    // circuits to TranscodeReason.DirectPlayError and falls through to the
    // TranscodingProfiles below.
    //
    // Caveat, verified live: the server may still stream-copy the video into
    // the HLS container when the source codec is one the TranscodingProfile
    // lists (TranscodingInfo.IsVideoDirect = true). That is still a server
    // transcode session on a `master.m3u8` URL rather than a direct play, so
    // the setting does what it says; denying the copy too would mean dropping
    // the source codec from the transcode targets, which is a separate call.
    let container_csv = containers.join(",");
    let mut direct_play: Vec<DirectPlayProfile> = Vec::new();
    if !force_transcode {
        if !video_csv.is_empty() {
            direct_play.push(DirectPlayProfile {
                container: Some(&container_csv),
                profile_type: ProfileType::Video,
                video_codec: Some(&video_csv),
                audio_codec: Some(&audio_csv),
            });
        }
        if !audio_csv.is_empty() {
            direct_play.push(DirectPlayProfile {
                container: Some(&container_csv),
                profile_type: ProfileType::Audio,
                video_codec: None,
                audio_codec: Some(&audio_csv),
            });
        }
        direct_play.push(DirectPlayProfile {
            container: None,
            profile_type: ProfileType::Photo,
            video_codec: None,
            audio_codec: None,
        });
    }

    // mpv handles both Embed and External natively, so no need to distinguish.
    let mut sub_profiles: Vec<SubtitleProfile> = Vec::new();
    for fmt in &subtitle_names {
        sub_profiles.push(SubtitleProfile {
            format: fmt,
            method: SubtitleDeliveryMethod::Embed,
        });
        sub_profiles.push(SubtitleProfile {
            format: fmt,
            method: SubtitleDeliveryMethod::External,
        });
    }

    // TranscodingProfiles: describes what server should produce when a
    // transcode is unavoidable. Order of VideoCodec/AudioCodec is the
    // server's preference order.
    let mut transcoding: Vec<TranscodingProfile> = vec![TranscodingProfile {
        container: None,
        profile_type: ProfileType::Audio,
        protocol: None,
        audio_codec: None,
        video_codec: None,
        max_audio_channels: None,
    }];
    if !force_transcode {
        transcoding.push(TranscodingProfile {
            container: Some(TRANSCODE_CONTAINER_MP4),
            profile_type: ProfileType::Video,
            protocol: Some(TRANSCODE_PROTOCOL),
            audio_codec: Some(&transcode_audio_csv_mp4),
            video_codec: Some(&transcode_video_csv),
            max_audio_channels: Some(TRANSCODE_MAX_AUDIO_CHANNELS),
        });
    }
    transcoding.push(TranscodingProfile {
        container: Some(TRANSCODE_CONTAINER),
        profile_type: ProfileType::Video,
        protocol: Some(TRANSCODE_PROTOCOL),
        audio_codec: Some(&transcode_audio_csv_ts),
        video_codec: Some(&transcode_video_csv),
        max_audio_channels: Some(TRANSCODE_MAX_AUDIO_CHANNELS),
    });
    transcoding.push(TranscodingProfile {
        container: Some("jpeg"),
        profile_type: ProfileType::Photo,
        protocol: None,
        audio_codec: None,
        video_codec: None,
        max_audio_channels: None,
    });

    let profile = DeviceProfile {
        name: device_name,
        max_static_bitrate: MAX_STATIC_BITRATE,
        music_streaming_transcoding_bitrate: MUSIC_STREAMING_TRANSCODING_BITRATE,
        timeline_offset_seconds: TIMELINE_OFFSET_SECONDS,
        direct_play_profiles: direct_play,
        transcoding_profiles: transcoding,
        subtitle_profiles: sub_profiles,
        response_profiles: EMPTY_LIST,
        container_profiles: EMPTY_LIST,
        codec_profiles: EMPTY_LIST,
    };
    serde_json::to_string(&profile).unwrap_or_default()
}

// ---- URL helpers ----

const SCHEME_SEPARATOR: &str = "://";
const DEFAULT_SCHEME: &str = "http://";

/// Scheme prefixes normalized to lowercase; every other prefix passes through.
const LOWERCASE_SCHEMES: [&str; 2] = ["http:", "https:"];

/// The only two schemes this client will fetch from or navigate to.
const HTTP_SCHEMES: [&str; 2] = ["http://", "https://"];

const WEB_PATH_SEGMENT: &str = "/web";

/// Trim surrounding whitespace, lowercase `Http:`/`Https:` scheme prefixes,
/// and prepend `http://` when no scheme is present.
///
/// Non-ASCII input never panics: the prefix comparison is `get`-guarded.
pub fn normalize_input(user_input: &str) -> String {
    let trimmed = user_input.trim();
    let cased = LOWERCASE_SCHEMES
        .iter()
        .find(|scheme| {
            trimmed
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })
        .map_or_else(
            || trimmed.to_string(),
            |scheme| format!("{scheme}{}", &trimmed[scheme.len()..]),
        );
    if cased.contains(SCHEME_SEPARATOR) {
        cased
    } else {
        format!("{DEFAULT_SCHEME}{cased}")
    }
}

/// Byte offset of the authority (host) within `url`: just past `://` when a
/// scheme separator is present, 0 otherwise.
fn authority_start(url: &str) -> usize {
    url.find(SCHEME_SEPARATOR)
        .map_or(0, |i| i + SCHEME_SEPARATOR.len())
}

/// Reduce a URL to its server base:
///   - if the URL contains `/web` (case-insensitive) *after the scheme
///     separator*, truncate at the last occurrence;
///   - otherwise return the origin (everything up to the first `/` after
///     `://`, or the whole string if there's no path).
///
/// The search deliberately starts at the authority: `http://web.example.com/`
/// would otherwise match the `/web` inside `//web` and reduce to `http:/`.
///
/// The result is a prefix slice of `url`: no percent-encoding, no punycode,
/// no default-port stripping, no case folding of host or path.
pub fn extract_base_url(url: &str) -> &str {
    let host_start = authority_start(url);
    let lower = url[host_start..].to_ascii_lowercase();
    if let Some(rel) = lower.rfind(WEB_PATH_SEGMENT) {
        return &url[..host_start + rel];
    }
    let end = url[host_start..]
        .find('/')
        .map_or(url.len(), |rel| host_start + rel);
    &url[..end]
}

/// True when `url` is an absolute `http://`/`https://` URL (scheme compared
/// case-insensitively) with a non-empty authority and no whitespace or
/// control character anywhere in it.
///
/// This is the gate for every URL the connect overlay hands to CEF — the
/// probe request and the main browser's `load_url`. Anything else
/// (`file:`, `data:`, `javascript:`, `app:`, `chrome:`, a bare `//host`)
/// is rejected: the main layer has the native bridge injected into it, so a
/// non-http(s) document loaded there would run with app privileges. The
/// control-character rule additionally keeps a `\n` out of the log line that
/// records the navigation.
///
/// Userinfo (`http://user:pw@host/`) is *not* rejected: servers behind basic
/// auth are a supported deployment.
pub fn is_http_url(url: &str) -> bool {
    let Some(rest) = HTTP_SCHEMES.iter().find_map(|scheme| {
        url.get(..scheme.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
            .map(|_| &url[scheme.len()..])
    }) else {
        return false;
    };
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    !rest[..authority_end].is_empty()
}

/// Validate that a Jellyfin `/System/Info/Public` response body is a JSON
/// object with a non-empty string `Id` field.
pub fn is_valid_public_info(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(o) = v.as_object() else { return false };
    o.get("Id")
        .and_then(Value::as_str)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

// ---- tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn parse(s: &str) -> Result<Value, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(s)?)
    }

    fn codec(name: &str, kind: MediaKind) -> Codec {
        Codec {
            name: name.to_string(),
            kind,
        }
    }

    #[test]
    fn empty_capabilities_emits_photo_only_direct_play() -> TestResult {
        let s = build_device_profile(&[], &[], "dev", "1.0", false);
        let v = parse(&s)?;
        let dp = v["DirectPlayProfiles"].as_array().ok_or("expected array")?;
        assert_eq!(dp.len(), 1);
        assert_eq!(dp[0]["Type"], "Photo");
        Ok(())
    }

    #[test]
    fn force_transcode_drops_the_fmp4_transcoding_profile() -> TestResult {
        let decoders = vec![
            codec("h264", MediaKind::Video),
            codec("aac", MediaKind::Audio),
        ];
        let s = build_device_profile(&decoders, &["matroska".into()], "dev", "1.0", true);
        let v = parse(&s)?;
        let tp = v["TranscodingProfiles"]
            .as_array()
            .ok_or("expected array")?;
        // Audio + Video (ts) + Photo. No fmp4 entry under force_transcode.
        assert!(!tp.iter().any(|e| e["Container"] == "mp4"));
        Ok(())
    }

    #[test]
    fn force_transcode_emits_no_direct_play_profiles_but_keeps_a_transcode_target() -> TestResult {
        // An empty codec CSV reads as "any codec" on the server, so the only
        // way to deny DirectPlay *and* DirectStream is to ship no
        // DirectPlayProfile at all — while still offering somewhere to
        // transcode to, or the server has nothing left to pick.
        let decoders = vec![
            codec("h264", MediaKind::Video),
            codec("aac", MediaKind::Audio),
            codec("subrip", MediaKind::Subtitle),
        ];
        let s = build_device_profile(&decoders, &["matroska".into()], "dev", "1.0", true);
        let v = parse(&s)?;
        let dp = v["DirectPlayProfiles"].as_array().ok_or("expected array")?;
        assert!(dp.is_empty(), "expected no DirectPlayProfiles, got {dp:?}");

        let tp = v["TranscodingProfiles"]
            .as_array()
            .ok_or("expected array")?;
        let video: Vec<&Value> = tp.iter().filter(|e| e["Type"] == "Video").collect();
        assert!(!video.is_empty(), "no video TranscodingProfile to fall to");
        assert!(video.iter().any(|e| e["Container"] == "ts"
            && e["Protocol"] == "hls"
            && e["VideoCodec"] == "h264"));

        // The non-forced profile of the same client still direct-plays.
        let unforced = parse(&build_device_profile(
            &decoders,
            &["matroska".into()],
            "dev",
            "1.0",
            false,
        ))?;
        let dp = unforced["DirectPlayProfiles"]
            .as_array()
            .ok_or("expected array")?;
        assert!(dp.iter().any(|e| e["Type"] == "Video"));
        Ok(())
    }

    #[test]
    fn container_rename_expands_and_dedupes() -> TestResult {
        // Container CSV is only emitted when there are video/audio decoders.
        let decoders = vec![codec("h264", MediaKind::Video)];
        let s = build_device_profile(&decoders, &["matroska,webm".into()], "dev", "1.0", false);
        let v = parse(&s)?;
        let dp = v["DirectPlayProfiles"].as_array().ok_or("expected array")?;
        let video = dp
            .iter()
            .find(|e| e["Type"] == "Video")
            .ok_or("no Video entry")?;
        let container = video["Container"].as_str().ok_or("expected string")?;
        let parts: Vec<&str> = container.split(',').collect();
        assert!(parts.contains(&"matroska"));
        assert!(parts.contains(&"webm"));
        assert!(parts.contains(&"mkv"));
        // No duplicates.
        let mut sorted = parts.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), parts.len());
        Ok(())
    }

    #[test]
    fn subtitle_rename_emits_both_methods() -> TestResult {
        let decoders = vec![codec("subrip", MediaKind::Subtitle)];
        let s = build_device_profile(&decoders, &[], "dev", "1.0", false);
        let v = parse(&s)?;
        let sp = v["SubtitleProfiles"].as_array().ok_or("expected array")?;
        let formats: Vec<&str> = sp
            .iter()
            .map(|e| e["Format"].as_str().unwrap_or_default())
            .collect();
        assert!(formats.contains(&"subrip"));
        assert!(formats.contains(&"srt"));
        // Each format appears with both Embed and External.
        for fmt in ["subrip", "srt"] {
            let methods: Vec<&str> = sp
                .iter()
                .filter(|e| e["Format"] == fmt)
                .map(|e| e["Method"].as_str().unwrap_or_default())
                .collect();
            assert!(methods.contains(&"Embed"));
            assert!(methods.contains(&"External"));
        }
        Ok(())
    }

    #[test]
    fn transcode_video_prefers_h264_first_regardless_of_decoder_order() -> TestResult {
        // The server picks the first VideoCodec with no encoder validation, so
        // h264 (universally hardware-encodable) must lead even when the decoder
        // list enumerates other codecs first. av1/vp9 trail as last resort.
        let decoders = vec![
            codec("av1", MediaKind::Video),
            codec("vp9", MediaKind::Video),
            codec("hevc", MediaKind::Video),
            codec("h264", MediaKind::Video),
        ];
        let s = build_device_profile(&decoders, &["matroska".into()], "dev", "1.0", false);
        let v = parse(&s)?;
        let tp = v["TranscodingProfiles"]
            .as_array()
            .ok_or("expected array")?;
        for e in tp.iter().filter(|e| e["Type"] == "Video") {
            assert_eq!(e["VideoCodec"], "h264,hevc,av1,vp9");
        }
        Ok(())
    }

    #[test]
    fn transcode_audio_csv_uses_curated_order_not_decoder_order() -> TestResult {
        let decoders = vec![
            codec("h264", MediaKind::Video),
            codec("mp3", MediaKind::Audio),
            codec("aac", MediaKind::Audio),
            codec("opus", MediaKind::Audio),
        ];
        let s = build_device_profile(&decoders, &["matroska".into()], "dev", "1.0", false);
        let v = parse(&s)?;
        let tp = v["TranscodingProfiles"]
            .as_array()
            .ok_or("expected array")?;
        let fmp4 = tp
            .iter()
            .find(|e| e["Container"] == "mp4")
            .ok_or("no fmp4 entry")?;
        assert_eq!(fmp4["AudioCodec"], "opus,aac,mp3");
        Ok(())
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_input("  http://example.com  "),
            "http://example.com"
        );
        assert_eq!(normalize_input("\thttps://host\n"), "https://host");
    }

    #[test]
    fn normalize_lowercases_scheme() {
        assert_eq!(normalize_input("HTTP://example.com"), "http://example.com");
        assert_eq!(
            normalize_input("HTTPS://example.com"),
            "https://example.com"
        );
        assert_eq!(normalize_input("Http://example.com"), "http://example.com");
        assert_eq!(
            normalize_input("Https://example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn normalize_prepends_http_when_no_scheme() {
        assert_eq!(normalize_input("example.com"), "http://example.com");
        assert_eq!(
            normalize_input("example.com:8096"),
            "http://example.com:8096"
        );
        assert_eq!(normalize_input("192.168.1.10"), "http://192.168.1.10");
    }

    #[test]
    fn normalize_trims_whitespace_before_prepending_scheme() {
        // Trim must happen first: otherwise a leading space would get trapped
        // between the prepended scheme and the host, producing "http:// host".
        assert_eq!(normalize_input(" example.com"), "http://example.com");
        assert_eq!(normalize_input("\texample.com\n"), "http://example.com");
        assert_eq!(normalize_input("   example.com  "), "http://example.com");
    }

    #[test]
    fn normalize_leaves_well_formed_input_unchanged() {
        assert_eq!(normalize_input("http://example.com"), "http://example.com");
        assert_eq!(
            normalize_input("https://example.com/jellyfin"),
            "https://example.com/jellyfin"
        );
    }

    #[test]
    fn normalize_passes_non_http_schemes_through() {
        // Only Http:/Https: prefixes are touched; anything else passes through.
        assert_eq!(normalize_input("FTP://example.com"), "FTP://example.com");
    }

    #[test]
    fn extract_base_truncates_at_web() {
        assert_eq!(
            extract_base_url("https://host/web/index.html"),
            "https://host"
        );
        assert_eq!(extract_base_url("https://host/web"), "https://host");
    }

    #[test]
    fn extract_base_preserves_prefix_before_web() {
        assert_eq!(
            extract_base_url("https://host/jellyfin/web/index.html"),
            "https://host/jellyfin"
        );
        assert_eq!(
            extract_base_url("https://host:8096/jellyfin/web/"),
            "https://host:8096/jellyfin"
        );
    }

    #[test]
    fn extract_base_uses_last_web_when_multiple() {
        assert_eq!(
            extract_base_url("https://host/web/app/web/index.html"),
            "https://host/web/app"
        );
    }

    #[test]
    fn extract_base_case_insensitive_web() {
        assert_eq!(
            extract_base_url("https://host/WEB/index.html"),
            "https://host"
        );
        assert_eq!(
            extract_base_url("https://host/Web/index.html"),
            "https://host"
        );
        assert_eq!(
            extract_base_url("https://host/wEb/index.html"),
            "https://host"
        );
    }

    #[test]
    fn extract_base_returns_origin_when_no_web() {
        assert_eq!(extract_base_url("https://host/"), "https://host");
        assert_eq!(extract_base_url("https://host"), "https://host");
        assert_eq!(extract_base_url("https://host/foo"), "https://host");
        assert_eq!(extract_base_url("http://host:8096/foo"), "http://host:8096");
    }

    #[test]
    fn extract_base_handles_port_in_origin() {
        assert_eq!(
            extract_base_url("http://host:8096/web/index.html"),
            "http://host:8096"
        );
        assert_eq!(
            extract_base_url("http://localhost:8096/web/"),
            "http://localhost:8096"
        );
        assert_eq!(
            extract_base_url("http://192.168.1.100:8096/web/"),
            "http://192.168.1.100:8096"
        );
        assert_eq!(
            extract_base_url("http://[::1]:8096/web/"),
            "http://[::1]:8096"
        );
    }

    #[test]
    fn extract_base_strips_query_and_fragment_after_web() {
        assert_eq!(
            extract_base_url("https://host/web/?foo=bar"),
            "https://host"
        );
        assert_eq!(
            extract_base_url("https://host/web/#section"),
            "https://host"
        );
        assert_eq!(
            extract_base_url("https://host/jellyfin/web/?foo=bar#section"),
            "https://host/jellyfin"
        );
    }

    #[test]
    fn extract_base_treats_website_and_webdav_as_web_match() {
        // Matches Qt behavior: substring match on "/web" does not distinguish
        // these longer path segments. Locked in here so a future fix is deliberate.
        assert_eq!(extract_base_url("https://host/website/"), "https://host");
        assert_eq!(extract_base_url("https://host/webdav/"), "https://host");
    }

    #[test]
    fn extract_base_handles_degenerate_urls() {
        assert_eq!(extract_base_url("https://"), "https://");
        assert_eq!(extract_base_url("https:///web/"), "https://");
    }

    #[test]
    fn idn_hosts_survive_unchanged() {
        assert_eq!(
            normalize_input("http://example.みんな"),
            "http://example.みんな"
        );
        assert_eq!(normalize_input("example.みんな"), "http://example.みんな");
        assert_eq!(
            normalize_input("  HTTPS://example.みんな/web "),
            "https://example.みんな/web"
        );

        assert_eq!(
            extract_base_url("http://example.みんな/web/"),
            "http://example.みんな"
        );
        assert_eq!(
            extract_base_url("https://example.みんな/jellyfin/web"),
            "https://example.みんな/jellyfin"
        );
        assert_eq!(
            extract_base_url("http://example.みんな/"),
            "http://example.みんな"
        );
    }

    #[test]
    fn public_info_accepts_object_with_non_empty_id() {
        assert!(is_valid_public_info(br#"{"Id":"abc","ServerName":"x"}"#));
        assert!(is_valid_public_info(br#"{"ServerName":"x","Id":"zzz"}"#));
    }

    #[test]
    fn public_info_rejects_empty_or_missing_id() {
        assert!(!is_valid_public_info(br#"{"Id":""}"#));
        assert!(!is_valid_public_info(br#"{"ServerName":"x"}"#));
        assert!(!is_valid_public_info(br#"{}"#));
    }

    #[test]
    fn public_info_rejects_non_string_id() {
        assert!(!is_valid_public_info(br#"{"Id":null}"#));
        assert!(!is_valid_public_info(br#"{"Id":123}"#));
        assert!(!is_valid_public_info(br#"{"Id":true}"#));
    }

    #[test]
    fn public_info_rejects_non_object_json() {
        assert!(!is_valid_public_info(br#"["Id"]"#));
        assert!(!is_valid_public_info(br#""Id""#));
        assert!(!is_valid_public_info(b"null"));
    }

    #[test]
    fn public_info_rejects_invalid_json() {
        assert!(!is_valid_public_info(b""));
        assert!(!is_valid_public_info(b"not json"));
        assert!(!is_valid_public_info(br#"{"Id":"abc""#));
    }

    #[test]
    fn public_info_no_false_positive_on_substring() {
        // Regression: the old C++ code string-matched "Id" which matched any
        // body containing that substring. Real JSON parse must reject these.
        assert!(!is_valid_public_info(
            br#"<html>oops "Id" lives here</html>"#
        ));
    }

    const EMPTY_PROFILE_JSON: &str = r#"{"Name":"dev","MaxStaticBitrate":1000000000,"MusicStreamingTranscodingBitrate":1280000,"TimelineOffsetSeconds":5,"DirectPlayProfiles":[{"Type":"Photo"}],"TranscodingProfiles":[{"Type":"Audio"},{"Container":"mp4","Type":"Video","Protocol":"hls","AudioCodec":"","VideoCodec":"","MaxAudioChannels":"6"},{"Container":"ts","Type":"Video","Protocol":"hls","AudioCodec":"","VideoCodec":"","MaxAudioChannels":"6"},{"Container":"jpeg","Type":"Photo"}],"SubtitleProfiles":[],"ResponseProfiles":[],"ContainerProfiles":[],"CodecProfiles":[]}"#;

    const TYPICAL_PROFILE_JSON: &str = r#"{"Name":"dev","MaxStaticBitrate":1000000000,"MusicStreamingTranscodingBitrate":1280000,"TimelineOffsetSeconds":5,"DirectPlayProfiles":[{"Container":"matroska,mkv,webm,mpegts,ts","Type":"Video","VideoCodec":"h264,hevc","AudioCodec":"aac,opus"},{"Container":"matroska,mkv,webm,mpegts,ts","Type":"Audio","AudioCodec":"aac,opus"},{"Type":"Photo"}],"TranscodingProfiles":[{"Type":"Audio"},{"Container":"mp4","Type":"Video","Protocol":"hls","AudioCodec":"opus,aac","VideoCodec":"h264,hevc","MaxAudioChannels":"6"},{"Container":"ts","Type":"Video","Protocol":"hls","AudioCodec":"aac","VideoCodec":"h264,hevc","MaxAudioChannels":"6"},{"Container":"jpeg","Type":"Photo"}],"SubtitleProfiles":[{"Format":"subrip","Method":"Embed"},{"Format":"subrip","Method":"External"},{"Format":"srt","Method":"Embed"},{"Format":"srt","Method":"External"}],"ResponseProfiles":[],"ContainerProfiles":[],"CodecProfiles":[]}"#;

    const FORCED_TRANSCODE_PROFILE_JSON: &str = r#"{"Name":"dev","MaxStaticBitrate":1000000000,"MusicStreamingTranscodingBitrate":1280000,"TimelineOffsetSeconds":5,"DirectPlayProfiles":[],"TranscodingProfiles":[{"Type":"Audio"},{"Container":"ts","Type":"Video","Protocol":"hls","AudioCodec":"aac","VideoCodec":"h264,hevc","MaxAudioChannels":"6"},{"Container":"jpeg","Type":"Photo"}],"SubtitleProfiles":[{"Format":"subrip","Method":"Embed"},{"Format":"subrip","Method":"External"},{"Format":"srt","Method":"Embed"},{"Format":"srt","Method":"External"}],"ResponseProfiles":[],"ContainerProfiles":[],"CodecProfiles":[]}"#;

    fn byte_exact_decoders() -> Vec<Codec> {
        vec![
            codec("h264", MediaKind::Video),
            codec("hevc", MediaKind::Video),
            codec("aac", MediaKind::Audio),
            codec("opus", MediaKind::Audio),
            codec("subrip", MediaKind::Subtitle),
        ]
    }

    #[test]
    fn empty_capabilities_profile_is_byte_exact() {
        assert_eq!(
            build_device_profile(&[], &[], "dev", "1.0", false),
            EMPTY_PROFILE_JSON
        );
    }

    #[test]
    fn typical_capabilities_profile_is_byte_exact() {
        let demuxers = vec!["matroska,webm".to_string(), "mpegts".to_string()];
        assert_eq!(
            build_device_profile(&byte_exact_decoders(), &demuxers, "dev", "1.0", false),
            TYPICAL_PROFILE_JSON
        );
    }

    #[test]
    fn forced_transcode_profile_is_byte_exact() {
        let demuxers = vec!["matroska".to_string()];
        assert_eq!(
            build_device_profile(&byte_exact_decoders(), &demuxers, "dev", "1.0", true),
            FORCED_TRANSCODE_PROFILE_JSON
        );
    }

    #[test]
    fn normalize_survives_multibyte_char_across_scheme_prefix() {
        assert_eq!(normalize_input("htt\u{2028}p"), "http://htt\u{2028}p");
        assert_eq!(normalize_input("みんな"), "http://みんな");
    }

    // ---- security audit, phase 1: hostile server-URL input ----

    #[test]
    fn normalize_passes_dangerous_schemes_through_untouched() {
        // `normalize_input` is a formatter, not a validator: anything with a
        // `://` survives verbatim. `is_http_url` is the gate that rejects
        // these before they reach a CEF request or `load_url`.
        for hostile in [
            "file:///C:/Windows/win.ini",
            "FILE://///attacker/share",
            "app://resources/overlay.html",
            "chrome://settings",
            "devtools://devtools/bundled/inspector.html",
            "data://x",
        ] {
            assert_eq!(normalize_input(hostile), hostile, "input {hostile:?}");
            assert!(!is_http_url(hostile), "input {hostile:?} must not pass");
        }
    }

    #[test]
    fn normalize_turns_schemeless_hostile_input_into_an_http_url() {
        // No `://` means the whole string becomes an http *host*, which is
        // inert — `javascript:` and `data:` never survive as schemes.
        assert_eq!(
            normalize_input("javascript:alert(1)"),
            "http://javascript:alert(1)"
        );
        assert_eq!(
            normalize_input("data:text/html,<script>alert(1)</script>"),
            "http://data:text/html,<script>alert(1)</script>"
        );
        assert!(!normalize_input("javascript:alert(1)").starts_with("javascript:"));
    }

    #[test]
    fn normalize_keeps_userinfo_port_and_ipv6_literals() {
        // Userinfo is preserved on purpose (basic-auth deployments); the
        // phishing shape `real-host@evil-host` is a known accepted risk.
        assert_eq!(
            normalize_input("http://user:pw@host:8096/jellyfin"),
            "http://user:pw@host:8096/jellyfin"
        );
        assert_eq!(normalize_input("user:pw@host"), "http://user:pw@host");
        assert_eq!(normalize_input("[::1]:8096"), "http://[::1]:8096");
        assert_eq!(
            normalize_input("HTTPS://[2001:db8::1]:8920/web"),
            "https://[2001:db8::1]:8920/web"
        );
    }

    #[test]
    fn normalize_preserves_dot_segments_and_control_characters() {
        // Neither is canonicalised here; `is_http_url` rejects the control
        // characters and CEF's GURL resolves the dot segments.
        assert_eq!(
            normalize_input("http://host/a/../../etc/passwd"),
            "http://host/a/../../etc/passwd"
        );
        assert_eq!(normalize_input("http://host/\u{0}x"), "http://host/\u{0}x");
        // Interior whitespace survives trimming of the ends.
        assert_eq!(normalize_input("  http://ho st  "), "http://ho st");
    }

    #[test]
    fn normalize_handles_degenerate_and_very_long_input() {
        assert_eq!(normalize_input(""), "http://");
        assert_eq!(normalize_input("   "), "http://");
        assert_eq!(normalize_input("://"), "://");
        assert_eq!(normalize_input("http:"), "http://http:");
        assert_eq!(
            normalize_input("HTTP:/example.com"),
            "http://http:/example.com"
        );
        let long = "a".repeat(200_000);
        assert_eq!(normalize_input(&long).len(), long.len() + "http://".len());
        let long_scheme = format!("HTTP://{}", "b".repeat(200_000));
        assert!(normalize_input(&long_scheme).starts_with("http://b"));
    }

    #[test]
    fn normalize_keeps_punycode_and_idn_bytes_verbatim() {
        assert_eq!(
            normalize_input("http://xn--n3h.example.com/web"),
            "http://xn--n3h.example.com/web"
        );
        // No IDN folding: the unicode form is *not* converted to punycode, so
        // the two spellings stay distinguishable in settings.json.
        assert_ne!(
            normalize_input("http://☃.example.com"),
            "http://xn--n3h.example.com"
        );
    }

    #[test]
    fn is_http_url_accepts_only_absolute_http_and_https() {
        assert!(is_http_url("http://host"));
        assert!(is_http_url("https://host:8096/jellyfin/"));
        assert!(is_http_url("HTTP://HOST/Web"));
        assert!(is_http_url("http://[::1]:8096/"));
        assert!(is_http_url("http://user:pw@host/"));
        assert!(is_http_url("http://example.みんな/web"));
    }

    #[test]
    fn is_http_url_rejects_other_schemes_and_relative_forms() {
        for bad in [
            "",
            "host",
            "//host",
            "/web/index.html",
            "file:///C:/Windows/win.ini",
            "file://host/share",
            "app://resources/overlay.html",
            "chrome://settings",
            "devtools://devtools",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "ftp://host",
            "httpx://host",
            "https:/host",
            "http:host",
        ] {
            assert!(!is_http_url(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn is_http_url_rejects_empty_authority_and_embedded_whitespace() {
        assert!(!is_http_url("http://"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url("http:///web"));
        assert!(!is_http_url("http://?x=1"));
        assert!(!is_http_url("http://#frag"));
        // Log forging / header smuggling shapes.
        assert!(!is_http_url("http://host/a\nERROR [Main] fake"));
        assert!(!is_http_url("http://host/a\r\nX: y"));
        assert!(!is_http_url("http://ho st/"));
        assert!(!is_http_url("http://host/\u{0}"));
        assert!(!is_http_url("http://host/\t"));
    }

    #[test]
    fn extract_base_does_not_mistake_a_web_host_for_a_web_path() {
        // Regression: the `/web` search used to run over the whole string, so
        // the `//web` of `http://web.example.com` matched and the base
        // collapsed to `http:/`.
        assert_eq!(
            extract_base_url("http://web.example.com/"),
            "http://web.example.com"
        );
        assert_eq!(
            extract_base_url("https://web.example.com:8096/jellyfin/web/index.html"),
            "https://web.example.com:8096/jellyfin"
        );
        assert_eq!(
            extract_base_url("https://WEB.example.com"),
            "https://WEB.example.com"
        );
        assert_eq!(
            extract_base_url("http://webmail.corp/"),
            "http://webmail.corp"
        );
    }

    #[test]
    fn extract_base_keeps_hostile_authority_and_dot_segments_intact() {
        // No canonicalisation happens here; whatever the redirect chain
        // resolved to is what gets probed. Pinned so a change is deliberate.
        assert_eq!(
            extract_base_url("http://real.example.com@evil.example/web/"),
            "http://real.example.com@evil.example"
        );
        assert_eq!(
            extract_base_url("http://host/a/../../web/x"),
            "http://host/a/../.."
        );
        assert_eq!(extract_base_url(""), "");
        assert_eq!(extract_base_url("/web"), "");
    }

    #[test]
    fn public_info_survives_hostile_bodies_without_panicking() {
        // Non-UTF-8 bytes.
        assert!(!is_valid_public_info(&[0xff, 0xfe, 0x00, 0x01]));
        // HTML error page from a captive portal / reverse proxy.
        assert!(!is_valid_public_info(
            b"<!doctype html><html><body>404</body></html>"
        ));
        // Right shape, wrong types.
        assert!(!is_valid_public_info(br#"{"Id":{"Id":"abc"}}"#));
        assert!(!is_valid_public_info(br#"{"Id":["abc"]}"#));
        assert!(
            !is_valid_public_info(br#"{"id":"abc"}"#),
            "Id is case-sensitive"
        );
        // Deep nesting must hit serde_json's recursion limit, not the stack.
        let deep = format!("{}{}", "[".repeat(50_000), "]".repeat(50_000));
        assert!(!is_valid_public_info(deep.as_bytes()));
        // A huge but well-formed body is merely rejected (the caller caps the
        // number of bytes it will ever accumulate).
        let huge = format!(r#"{{"Id":"abc","pad":"{}"}}"#, "p".repeat(1_000_000));
        assert!(is_valid_public_info(huge.as_bytes()));
        let truncated = &huge.as_bytes()[..64 * 1024];
        assert!(!is_valid_public_info(truncated));
    }

    #[test]
    fn public_info_accepts_a_server_id_that_differs_from_the_one_asked_for() {
        // Documented gap: the probe only asserts "some Jellyfin server lives
        // here", never that it is the server the user typed. A redirect chain
        // can therefore move the client to a different server id.
        assert!(is_valid_public_info(
            br#"{"Id":"0000000000000000000000000000ffff","ServerName":"somebody else"}"#
        ));
    }

    #[test]
    fn device_profile_escapes_a_hostile_device_name() {
        // `device_name` is a fixed literal today, but the JSON this builds is
        // spliced straight into injected JS, so the escaping has to hold.
        let hostile = "a\"b\\c\nd</script><script>alert(1)</script>\u{0}e";
        let s = build_device_profile(&[], &[], hostile, "1.0", false);
        assert!(!s.contains("a\"b"), "raw quote survived: {s}");
        assert!(!s.contains('\n'), "raw newline survived: {s}");
        assert!(!s.contains('\u{0}'), "raw NUL survived: {s}");
        let v: Value = serde_json::from_str(&s).unwrap_or(Value::Null);
        assert_eq!(v["Name"].as_str(), Some(hostile));
    }

    #[test]
    fn device_profile_never_emits_a_truncated_document() {
        // `serde_json::to_string` cannot fail for this struct, but the call
        // site swallows an error into `String::default()`; assert the happy
        // path stays parseable for pathological codec names too.
        let decoders = vec![
            codec("h264\",\"evil\":\"", MediaKind::Video),
            codec("\u{2028}aac", MediaKind::Audio),
        ];
        let s = build_device_profile(&decoders, &["mat\"roska".into()], "dev", "1.0", false);
        assert!(!s.is_empty());
        let v: Value = serde_json::from_str(&s).unwrap_or(Value::Null);
        assert!(v.is_object(), "unparseable profile: {s}");
        assert!(v.get("evil").is_none(), "codec name broke out: {s}");
    }
}
