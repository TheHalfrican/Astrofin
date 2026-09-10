//! Pure mapping for the SMTC sink: the button table, the data-URI split, the
//! WinRT tick arithmetic and the two enum projections.
//!
//! `lib.rs` keeps everything that needs a live
//! `SystemMediaTransportControls` — this half is input -> output and runs
//! without one.

use jfn_playback::sink_core::{MediaCommand, Phase};
use jfn_playback::{MediaMetadata, MediaType as PbMediaType};
use windows::Media::{MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControlsButton};

/// The transport command an SMTC button raises, or `None` for the buttons the
/// sink does not enable (record, channel up/down, fast-forward, ...), which
/// are ignored rather than mapped onto something else.
pub(crate) fn button_to_command(
    button: SystemMediaTransportControlsButton,
) -> Option<MediaCommand> {
    use SystemMediaTransportControlsButton as B;
    match button {
        B::Play => Some(MediaCommand::Play),
        B::Pause => Some(MediaCommand::Pause),
        B::Stop => Some(MediaCommand::Stop),
        B::Next => Some(MediaCommand::Next),
        B::Previous => Some(MediaCommand::Previous),
        _ => None,
    }
}

/// The base64 payload of an artwork `data:` URI, or `None` when there is
/// nothing to decode: an empty URI, or one with no `,` separating the media
/// type from the payload.
pub(crate) fn data_uri_payload(uri: &str) -> Option<&str> {
    if uri.is_empty() {
        return None;
    }
    let comma = uri.find(',')?;
    Some(&uri[comma + 1..])
}

/// Microseconds as WinRT ticks (100 ns each).
pub(crate) fn us_to_ticks(us: i64) -> i64 {
    us * 10
}

/// WinRT ticks as whole milliseconds — how a seek request from the OS
/// transport reaches `sink_core`. Truncates, matching the two integer
/// divisions it replaces.
pub(crate) fn ticks_to_ms(ticks: i64) -> i64 {
    ticks / 10 / 1000
}

/// True when a metadata event names the item already on the transport, which
/// is republished rather than changed. An item with no id is never a
/// duplicate — there is nothing to compare it by.
pub(crate) fn is_same_item(incoming: &MediaMetadata, current: &MediaMetadata) -> bool {
    !incoming.id.is_empty() && incoming.id == current.id
}

/// True when a timeline can be published: SMTC rejects a timeline whose end
/// time is not after its start, which is every item with no known duration.
pub(crate) fn has_timeline(metadata: &MediaMetadata) -> bool {
    metadata.duration_us > 0
}

/// The SMTC status for a playback phase.
pub(crate) fn playback_status(phase: Phase) -> MediaPlaybackStatus {
    match phase {
        Phase::Playing => MediaPlaybackStatus::Playing,
        Phase::Paused => MediaPlaybackStatus::Paused,
        Phase::Stopped => MediaPlaybackStatus::Stopped,
    }
}

/// Which set of display properties an item fills in. Everything that is not
/// audio is shown as video, so an item with an unknown media type still gets
/// a title.
pub(crate) fn playback_type(media_type: PbMediaType) -> MediaPlaybackType {
    if media_type == PbMediaType::Audio {
        MediaPlaybackType::Music
    } else {
        MediaPlaybackType::Video
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, duration_us: i64) -> MediaMetadata {
        MediaMetadata {
            id: id.to_string(),
            duration_us,
            ..MediaMetadata::default()
        }
    }

    #[test]
    fn the_five_enabled_buttons_map_onto_transport_commands() {
        use SystemMediaTransportControlsButton as B;
        assert!(button_to_command(B::Play) == Some(MediaCommand::Play));
        assert!(button_to_command(B::Pause) == Some(MediaCommand::Pause));
        assert!(button_to_command(B::Stop) == Some(MediaCommand::Stop));
        assert!(button_to_command(B::Next) == Some(MediaCommand::Next));
        assert!(button_to_command(B::Previous) == Some(MediaCommand::Previous));
    }

    #[test]
    fn an_unhandled_button_raises_nothing() {
        use SystemMediaTransportControlsButton as B;
        assert!(button_to_command(B::Record).is_none());
        assert!(button_to_command(B::FastForward).is_none());
        assert!(button_to_command(B::ChannelUp).is_none());
    }

    #[test]
    fn a_data_uri_yields_the_part_after_the_comma() {
        assert_eq!(
            data_uri_payload("data:image/jpeg;base64,QUJD"),
            Some("QUJD")
        );
    }

    #[test]
    fn a_uri_with_an_empty_payload_still_splits() {
        assert_eq!(data_uri_payload("data:image/png;base64,"), Some(""));
    }

    #[test]
    fn an_empty_or_separatorless_uri_has_no_payload() {
        assert_eq!(data_uri_payload(""), None);
        assert_eq!(data_uri_payload("https://example.invalid/art.png"), None);
    }

    #[test]
    fn a_microsecond_is_ten_ticks() {
        assert_eq!(us_to_ticks(0), 0);
        assert_eq!(us_to_ticks(1), 10);
        assert_eq!(us_to_ticks(1_500_000), 15_000_000);
    }

    #[test]
    fn ticks_round_down_to_whole_milliseconds() {
        assert_eq!(ticks_to_ms(0), 0);
        assert_eq!(ticks_to_ms(10_000), 1);
        assert_eq!(ticks_to_ms(9_999), 0);
        assert_eq!(ticks_to_ms(us_to_ticks(1_500_000)), 1_500);
    }

    #[test]
    fn the_same_id_is_the_same_item() {
        assert!(is_same_item(&item("abc", 0), &item("abc", 0)));
        assert!(!is_same_item(&item("abc", 0), &item("def", 0)));
    }

    #[test]
    fn an_item_with_no_id_is_never_a_duplicate() {
        assert!(!is_same_item(&item("", 0), &item("", 0)));
        assert!(!is_same_item(&item("", 0), &item("abc", 0)));
    }

    #[test]
    fn only_an_item_with_a_duration_gets_a_timeline() {
        assert!(has_timeline(&item("a", 1)));
        assert!(!has_timeline(&item("a", 0)));
        assert!(!has_timeline(&item("a", -1)));
    }

    #[test]
    fn each_phase_has_its_own_smtc_status() {
        assert_eq!(
            playback_status(Phase::Playing),
            MediaPlaybackStatus::Playing
        );
        assert_eq!(playback_status(Phase::Paused), MediaPlaybackStatus::Paused);
        assert_eq!(
            playback_status(Phase::Stopped),
            MediaPlaybackStatus::Stopped
        );
    }

    #[test]
    fn audio_shows_music_properties_and_everything_else_video() {
        assert_eq!(playback_type(PbMediaType::Audio), MediaPlaybackType::Music);
        assert_eq!(playback_type(PbMediaType::Video), MediaPlaybackType::Video);
        assert_eq!(
            playback_type(PbMediaType::Unknown),
            MediaPlaybackType::Video
        );
    }
}
