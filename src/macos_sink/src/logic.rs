//! Pure mapping for the Now Playing sink: the info-dictionary key table, the
//! microsecond arithmetic, the artwork data-URI split and the two enum
//! projections.
//!
//! `lib.rs` keeps everything that needs a live `MPNowPlayingInfoCenter` or
//! `MPRemoteCommandCenter` — this half is input -> output and runs without
//! one. The keys are the names of MediaPlayer's `extern NSString * const`
//! symbols, which `mp_const` resolves out of the program image; they are
//! strings here only because that lookup is by name.

use std::ffi::c_int;

use jfn_playback::sink_core::Phase;
use jfn_playback::{MediaMetadata, MediaType as PbMediaType};
use objc2_media_player::{MPNowPlayingInfoMediaType, MPNowPlayingPlaybackState};

pub(crate) const KEY_TITLE: &str = "MPMediaItemPropertyTitle";
pub(crate) const KEY_ARTIST: &str = "MPMediaItemPropertyArtist";
pub(crate) const KEY_ALBUM: &str = "MPMediaItemPropertyAlbumTitle";
pub(crate) const KEY_DURATION: &str = "MPMediaItemPropertyPlaybackDuration";
pub(crate) const KEY_TRACK_NUMBER: &str = "MPMediaItemPropertyAlbumTrackNumber";
pub(crate) const KEY_ARTWORK: &str = "MPMediaItemPropertyArtwork";
pub(crate) const KEY_ELAPSED: &str = "MPNowPlayingInfoPropertyElapsedPlaybackTime";
pub(crate) const KEY_RATE: &str = "MPNowPlayingInfoPropertyPlaybackRate";
pub(crate) const KEY_MEDIA_TYPE: &str = "MPNowPlayingInfoPropertyMediaType";

/// `MRMediaRemoteSetNowPlayingVisibility` levels.
const VISIBILITY_ALWAYS: c_int = 1;
const VISIBILITY_NEVER: c_int = 3;

/// One value of the now-playing info dictionary, in the shape the
/// `NSString` / `NSNumber` constructor at the call site needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum InfoValue {
    Text(String),
    /// Seconds, or a playback rate — anything that goes in as `NSNumber`
    /// `numberWithDouble:`.
    Number(f64),
    Int(i32),
}

/// Microseconds as the seconds MediaPlayer's durations and positions take.
pub(crate) fn us_to_seconds(us: i64) -> f64 {
    us as f64 / 1_000_000.0
}

/// A seek position in seconds as the whole milliseconds `sink_core` takes.
/// Truncates towards zero, matching the cast it replaces.
pub(crate) fn seconds_to_ms(seconds: f64) -> i64 {
    (seconds * 1000.0) as i64
}

/// The base64 payload of an artwork `data:` URI, or `None` when there is
/// nothing to decode: an empty URI, or one with no `,` separating the media
/// type from the payload.
pub(crate) fn data_uri_payload(uri: &str) -> Option<&str> {
    let comma = uri.find(',')?;
    Some(&uri[comma + 1..])
}

/// True when a metadata event names the item already on the transport, which
/// is republished rather than changed. An item with no id is never a
/// duplicate — there is nothing to compare it by.
pub(crate) fn is_same_item(incoming: &MediaMetadata, current: &MediaMetadata) -> bool {
    !incoming.id.is_empty() && incoming.id == current.id
}

/// The now-playing entries for one metadata snapshot, in insertion order.
///
/// Empty strings and non-positive numbers are left out entirely rather than
/// published as blanks, so the OS falls back to showing nothing for that field
/// instead of an empty line. Elapsed time and rate are always published: they
/// are what makes the transport's scrubber live.
pub(crate) fn now_playing_entries(
    metadata: &MediaMetadata,
    position_us: i64,
    rate: f64,
) -> Vec<(&'static str, InfoValue)> {
    let mut out: Vec<(&'static str, InfoValue)> = Vec::new();
    if !metadata.title.is_empty() {
        out.push((KEY_TITLE, InfoValue::Text(metadata.title.clone())));
    }
    if !metadata.artist.is_empty() {
        out.push((KEY_ARTIST, InfoValue::Text(metadata.artist.clone())));
    }
    if !metadata.album.is_empty() {
        out.push((KEY_ALBUM, InfoValue::Text(metadata.album.clone())));
    }
    if metadata.duration_us > 0 {
        out.push((
            KEY_DURATION,
            InfoValue::Number(us_to_seconds(metadata.duration_us)),
        ));
    }
    if metadata.track_number > 0 {
        out.push((KEY_TRACK_NUMBER, InfoValue::Int(metadata.track_number)));
    }
    out.push((KEY_ELAPSED, InfoValue::Number(us_to_seconds(position_us))));
    out.push((KEY_RATE, InfoValue::Number(rate)));
    out
}

/// The Now Playing media type for a playback media type. Anything that is not
/// known to be audio is published as video, which is what an unknown item on a
/// video client most likely is.
pub(crate) fn media_type_value(media_type: PbMediaType) -> MPNowPlayingInfoMediaType {
    if media_type == PbMediaType::Audio {
        MPNowPlayingInfoMediaType::Audio
    } else {
        MPNowPlayingInfoMediaType::Video
    }
}

/// The Now Playing playback state for a transport phase.
pub(crate) fn playback_state(phase: Phase) -> MPNowPlayingPlaybackState {
    match phase {
        Phase::Playing => MPNowPlayingPlaybackState::Playing,
        Phase::Paused => MPNowPlayingPlaybackState::Paused,
        Phase::Stopped => MPNowPlayingPlaybackState::Stopped,
    }
}

/// Whether the private MediaRemote origin should advertise us in Now Playing.
/// A stopped transport hides itself so the Control Center tile goes back to
/// whatever else is playing instead of showing a dead player.
pub(crate) fn visibility_for_phase(phase: Phase) -> c_int {
    if phase == Phase::Stopped {
        VISIBILITY_NEVER
    } else {
        VISIBILITY_ALWAYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> MediaMetadata {
        MediaMetadata {
            id: "item-1".to_string(),
            title: "Pilot".to_string(),
            artist: "Some Show".to_string(),
            album: "Season 1".to_string(),
            track_number: 3,
            duration_us: 2_700_000_000,
            media_type: PbMediaType::Video,
            ..MediaMetadata::default()
        }
    }

    #[test]
    fn microseconds_become_seconds() {
        assert_eq!(us_to_seconds(0), 0.0);
        assert_eq!(us_to_seconds(1_000_000), 1.0);
        assert_eq!(us_to_seconds(1_500_000), 1.5);
        assert_eq!(us_to_seconds(-1_000_000), -1.0);
    }

    #[test]
    fn a_seek_position_truncates_to_whole_milliseconds() {
        assert_eq!(seconds_to_ms(0.0), 0);
        assert_eq!(seconds_to_ms(1.5), 1500);
        assert_eq!(seconds_to_ms(0.0009), 0);
        assert_eq!(seconds_to_ms(12.3456), 12345);
    }

    #[test]
    fn a_data_uri_yields_the_part_after_the_comma() {
        assert_eq!(
            data_uri_payload("data:image/jpeg;base64,QUJD"),
            Some("QUJD")
        );
        assert_eq!(data_uri_payload("data:image/png;base64,"), Some(""));
    }

    #[test]
    fn an_empty_or_separatorless_uri_has_no_payload() {
        assert_eq!(data_uri_payload(""), None);
        assert_eq!(data_uri_payload("https://example.invalid/art.png"), None);
    }

    #[test]
    fn an_item_repeats_only_when_it_names_the_same_non_empty_id() {
        let current = metadata();
        assert!(is_same_item(&metadata(), &current));
        let mut other = metadata();
        other.id = "item-2".to_string();
        assert!(!is_same_item(&other, &current));
    }

    #[test]
    fn an_item_without_an_id_is_never_a_repeat() {
        let mut blank = metadata();
        blank.id = String::new();
        let mut current = metadata();
        current.id = String::new();
        assert!(!is_same_item(&blank, &current));
    }

    #[test]
    fn a_full_item_publishes_every_field_in_order() {
        let entries = now_playing_entries(&metadata(), 30_000_000, 1.0);
        assert_eq!(
            entries,
            vec![
                (KEY_TITLE, InfoValue::Text("Pilot".to_string())),
                (KEY_ARTIST, InfoValue::Text("Some Show".to_string())),
                (KEY_ALBUM, InfoValue::Text("Season 1".to_string())),
                (KEY_DURATION, InfoValue::Number(2700.0)),
                (KEY_TRACK_NUMBER, InfoValue::Int(3)),
                (KEY_ELAPSED, InfoValue::Number(30.0)),
                (KEY_RATE, InfoValue::Number(1.0)),
            ]
        );
    }

    #[test]
    fn empty_text_fields_are_left_out() {
        let mut meta = metadata();
        meta.title = String::new();
        meta.artist = String::new();
        meta.album = String::new();
        let keys: Vec<&str> = now_playing_entries(&meta, 0, 0.0)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            [KEY_DURATION, KEY_TRACK_NUMBER, KEY_ELAPSED, KEY_RATE]
        );
    }

    #[test]
    fn an_unknown_duration_and_track_number_are_left_out() {
        let mut meta = metadata();
        meta.duration_us = 0;
        meta.track_number = 0;
        let keys: Vec<&str> = now_playing_entries(&meta, 0, 0.0)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            [KEY_TITLE, KEY_ARTIST, KEY_ALBUM, KEY_ELAPSED, KEY_RATE]
        );
        // A negative duration is as unusable as a missing one.
        meta.duration_us = -1;
        meta.track_number = -1;
        assert_eq!(
            now_playing_entries(&meta, 0, 0.0)
                .into_iter()
                .map(|(k, _)| k)
                .collect::<Vec<_>>(),
            [KEY_TITLE, KEY_ARTIST, KEY_ALBUM, KEY_ELAPSED, KEY_RATE]
        );
    }

    #[test]
    fn an_empty_item_still_publishes_a_live_timeline() {
        let entries = now_playing_entries(&MediaMetadata::default(), 500_000, 2.0);
        assert_eq!(
            entries,
            vec![
                (KEY_ELAPSED, InfoValue::Number(0.5)),
                (KEY_RATE, InfoValue::Number(2.0)),
            ]
        );
    }

    #[test]
    fn only_audio_is_published_as_audio() {
        assert_eq!(
            media_type_value(PbMediaType::Audio),
            MPNowPlayingInfoMediaType::Audio
        );
        assert_eq!(
            media_type_value(PbMediaType::Video),
            MPNowPlayingInfoMediaType::Video
        );
        assert_eq!(
            media_type_value(PbMediaType::Unknown),
            MPNowPlayingInfoMediaType::Video
        );
    }

    #[test]
    fn each_phase_projects_onto_its_now_playing_state() {
        assert_eq!(
            playback_state(Phase::Playing),
            MPNowPlayingPlaybackState::Playing
        );
        assert_eq!(
            playback_state(Phase::Paused),
            MPNowPlayingPlaybackState::Paused
        );
        assert_eq!(
            playback_state(Phase::Stopped),
            MPNowPlayingPlaybackState::Stopped
        );
    }

    #[test]
    fn only_a_stopped_transport_hides_itself() {
        assert_eq!(visibility_for_phase(Phase::Stopped), VISIBILITY_NEVER);
        assert_eq!(visibility_for_phase(Phase::Playing), VISIBILITY_ALWAYS);
        assert_eq!(visibility_for_phase(Phase::Paused), VISIBILITY_ALWAYS);
    }
}
