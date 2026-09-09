//! Pure data types shared between the state machine and coordinator.
//!
//! These are internal Rust types. The FFI-facing shapes live in `ffi.rs`
//! and are populated from these at sink-delivery time.

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum MediaType {
    #[default]
    Unknown = 0,
    Audio = 1,
    Video = 2,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum PlayerPresence {
    #[default]
    None = 0,
    Present = 1,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum PlaybackPhase {
    Starting = 0,
    Playing = 1,
    Paused = 2,
    #[default]
    Stopped = 3,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndReason {
    Eof = 0,
    Error = 1,
    Canceled = 2,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaMetadata {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: i32,
    pub duration_us: i64,
    pub art_url: String,
    pub art_data_uri: String,
    pub media_type: MediaType,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaybackBufferedRange {
    pub start_ticks: i64,
    pub end_ticks: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackSnapshot {
    pub presence: PlayerPresence,
    pub phase: PlaybackPhase,
    pub seeking: bool,
    pub buffering: bool,
    pub media_type: MediaType,
    pub position_us: i64,
    pub variant_switch_pending: bool,
    pub rate: f64,
    pub duration_us: i64,
    pub fullscreen: bool,
    pub maximized_before_fullscreen: bool,
    pub display_hz: f64,
    pub buffered: Vec<PlaybackBufferedRange>,
}

impl PlaybackSnapshot {
    pub(crate) fn fresh() -> Self {
        Self {
            rate: 1.0,
            ..Default::default()
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackEventKind {
    Started = 0,
    Paused = 1,
    Finished = 2,
    Canceled = 3,
    Error = 4,
    SeekingChanged = 5,
    BufferingChanged = 6,
    MediaTypeChanged = 7,
    TrackLoaded = 8,
    PositionChanged = 9,
    DurationChanged = 10,
    RateChanged = 11,
    FullscreenChanged = 12,
    BufferedRangesChanged = 14,
    DisplayHzChanged = 15,
    MetadataChanged = 16,
    ArtworkChanged = 17,
    QueueCapsChanged = 18,
    Seeked = 19,
}

#[derive(Clone, Debug)]
pub struct PlaybackEvent {
    pub kind: PlaybackEventKind,
    pub flag: bool,
    pub error_message: String,
    pub snapshot: PlaybackSnapshot,
    pub metadata: MediaMetadata,
    pub artwork_uri: String,
    pub can_go_next: bool,
    pub can_go_prev: bool,
}

impl PlaybackEvent {
    pub(crate) fn new(kind: PlaybackEventKind) -> Self {
        Self {
            kind,
            flag: false,
            error_message: String::new(),
            snapshot: PlaybackSnapshot::default(),
            metadata: MediaMetadata::default(),
            artwork_uri: String::new(),
            can_go_next: false,
            can_go_prev: false,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackActionKind {
    ApplyPendingTrackSelectionAndPlay = 0,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackAction {
    pub kind: PlaybackActionKind,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn a_fresh_snapshot_is_stopped_at_normal_rate() {
        let s = PlaybackSnapshot::fresh();
        assert_eq!(s.presence, PlayerPresence::None);
        assert_eq!(s.phase, PlaybackPhase::Stopped);
        assert_eq!(s.media_type, MediaType::Unknown);
        // The one field `fresh` does not take from `Default`: a rate of 0
        // would read as "stopped" to every sink.
        assert_eq!(s.rate, 1.0);
        assert_eq!(PlaybackSnapshot::default().rate, 0.0);
        assert!(!s.seeking);
        assert!(!s.buffering);
        assert!(!s.fullscreen);
        assert_eq!(s.position_us, 0);
        assert_eq!(s.duration_us, 0);
        assert!(s.buffered.is_empty());
    }

    #[test]
    fn a_new_event_carries_its_kind_and_nothing_else() {
        let ev = PlaybackEvent::new(PlaybackEventKind::Started);
        assert_eq!(ev.kind, PlaybackEventKind::Started);
        assert!(!ev.flag);
        assert!(ev.error_message.is_empty());
        assert!(ev.artwork_uri.is_empty());
        assert!(!ev.can_go_next);
        assert!(!ev.can_go_prev);
        assert_eq!(ev.metadata, MediaMetadata::default());
        // The coordinator stamps the post-transition snapshot on the way
        // out, so a bare event starts from `Default`, not from `fresh`.
        assert_eq!(ev.snapshot, PlaybackSnapshot::default());
    }

    #[test]
    fn the_default_media_and_presence_are_the_unknown_variants() {
        assert_eq!(MediaType::default(), MediaType::Unknown);
        assert_eq!(PlayerPresence::default(), PlayerPresence::None);
        assert_eq!(PlaybackPhase::default(), PlaybackPhase::Stopped);
    }

    #[test]
    fn the_ffi_discriminants_are_the_documented_wire_values() {
        // These cross the C ABI to the OS media sinks; renumbering one
        // silently reinterprets every event.
        assert_eq!(PlaybackEventKind::Started as u8, 0);
        assert_eq!(PlaybackEventKind::Seeked as u8, 19);
        assert_eq!(PlaybackEventKind::BufferedRangesChanged as u8, 14);
        assert_eq!(MediaType::Video as u8, 2);
        assert_eq!(PlaybackPhase::Stopped as u8, 3);
        assert_eq!(EndReason::Canceled as u8, 2);
        assert_eq!(
            PlaybackActionKind::ApplyPendingTrackSelectionAndPlay as u8,
            0
        );
    }
}
