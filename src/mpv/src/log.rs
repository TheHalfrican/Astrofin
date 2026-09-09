//! libmpv log-level mapping and forwarding to `tracing`.

use crate::event::LogMessage;
use crate::sys;

/// libmpv log severities, in the order libmpv defines them. `Off` disables
/// subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
#[allow(clippy::unnecessary_cast)] // bindgen emits i32 on windows, u32 on linux; cast keeps both green
pub enum LogLevel {
    Off = sys::mpv_log_level::MPV_LOG_LEVEL_NONE.0 as u32,
    Fatal = sys::mpv_log_level::MPV_LOG_LEVEL_FATAL.0 as u32,
    Error = sys::mpv_log_level::MPV_LOG_LEVEL_ERROR.0 as u32,
    Warn = sys::mpv_log_level::MPV_LOG_LEVEL_WARN.0 as u32,
    Info = sys::mpv_log_level::MPV_LOG_LEVEL_INFO.0 as u32,
    /// Maps to mpv's "v".
    Verbose = sys::mpv_log_level::MPV_LOG_LEVEL_V.0 as u32,
    /// Maps to mpv's "debug".
    Debug = sys::mpv_log_level::MPV_LOG_LEVEL_DEBUG.0 as u32,
    /// Maps to mpv's "trace".
    Trace = sys::mpv_log_level::MPV_LOG_LEVEL_TRACE.0 as u32,
}

impl LogLevel {
    /// Token accepted by `mpv_request_log_messages`.
    pub fn as_token(self) -> &'static std::ffi::CStr {
        match self {
            LogLevel::Off => c"no",
            LogLevel::Fatal => c"fatal",
            LogLevel::Error => c"error",
            LogLevel::Warn => c"warn",
            LogLevel::Info => c"info",
            LogLevel::Verbose => c"v",
            LogLevel::Debug => c"debug",
            LogLevel::Trace => c"trace",
        }
    }

    /// Inverse of `from_raw` for the libmpv enum.
    pub fn from_raw(raw: sys::mpv_log_level) -> Self {
        match raw {
            sys::mpv_log_level::MPV_LOG_LEVEL_FATAL => LogLevel::Fatal,
            sys::mpv_log_level::MPV_LOG_LEVEL_ERROR => LogLevel::Error,
            sys::mpv_log_level::MPV_LOG_LEVEL_WARN => LogLevel::Warn,
            sys::mpv_log_level::MPV_LOG_LEVEL_INFO => LogLevel::Info,
            sys::mpv_log_level::MPV_LOG_LEVEL_V => LogLevel::Verbose,
            sys::mpv_log_level::MPV_LOG_LEVEL_DEBUG => LogLevel::Debug,
            sys::mpv_log_level::MPV_LOG_LEVEL_TRACE => LogLevel::Trace,
            _ => LogLevel::Off,
        }
    }
}

/// Forward an `MPV_EVENT_LOG_MESSAGE` payload to `tracing` under target
/// `"mpv"`. mpv's `v` lands at DEBUG and mpv's `debug` lands at TRACE so
/// the console isn't flooded at default verbosity. Unknown/`trace`
/// levels surface as WARN with an unhandled-level marker.
pub fn forward_to_tracing(msg: &LogMessage) {
    let text = msg.text.trim_end_matches(['\r', '\n']);
    let prefix = msg.prefix.as_str();
    match msg.level {
        LogLevel::Fatal | LogLevel::Error => {
            tracing::event!(target: "mpv", tracing::Level::ERROR, "{}: {}", prefix, text)
        }
        LogLevel::Warn => {
            tracing::event!(target: "mpv", tracing::Level::WARN, "{}: {}", prefix, text)
        }
        LogLevel::Info => {
            tracing::event!(target: "mpv", tracing::Level::INFO, "{}: {}", prefix, text)
        }
        LogLevel::Verbose => {
            tracing::event!(target: "mpv", tracing::Level::DEBUG, "{}: {}", prefix, text)
        }
        LogLevel::Debug => {
            tracing::event!(target: "mpv", tracing::Level::TRACE, "{}: {}", prefix, text)
        }
        LogLevel::Trace | LogLevel::Off => tracing::event!(
            target: "mpv",
            tracing::Level::WARN,
            "[unhandled mpv level {:?}] {}: {}",
            msg.level,
            prefix,
            text
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::{Event as TracingEvent, Metadata, Subscriber, span};

    /// One `tracing` record as the forwarder emitted it.
    #[derive(Clone, Debug, PartialEq)]
    struct Record {
        target: String,
        level: tracing::Level,
        message: String,
    }

    struct MessageVisitor(String);

    impl Visit for MessageVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    struct Capture(Arc<Mutex<Vec<Record>>>);

    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, event: &TracingEvent<'_>) {
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().push(Record {
                target: event.metadata().target().to_string(),
                level: *event.metadata().level(),
                message: visitor.0,
            });
        }
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    fn forwarded(level: LogLevel, prefix: &str, text: &str) -> Record {
        let sink = Arc::new(Mutex::new(Vec::new()));
        tracing::subscriber::with_default(Capture(Arc::clone(&sink)), || {
            forward_to_tracing(&LogMessage {
                prefix: prefix.to_string(),
                level,
                text: text.to_string(),
            });
        });
        let mut records = sink.lock().unwrap();
        assert_eq!(records.len(), 1, "one mpv line must make one record");
        records.pop().unwrap()
    }

    #[test]
    fn as_token_is_the_word_mpv_request_log_messages_accepts() {
        assert_eq!(LogLevel::Off.as_token(), c"no");
        assert_eq!(LogLevel::Fatal.as_token(), c"fatal");
        assert_eq!(LogLevel::Error.as_token(), c"error");
        assert_eq!(LogLevel::Warn.as_token(), c"warn");
        assert_eq!(LogLevel::Info.as_token(), c"info");
        // mpv spells these two differently from the Rust variant names.
        assert_eq!(LogLevel::Verbose.as_token(), c"v");
        assert_eq!(LogLevel::Debug.as_token(), c"debug");
        assert_eq!(LogLevel::Trace.as_token(), c"trace");
    }

    #[test]
    fn from_raw_maps_every_severity_libmpv_defines() {
        for (raw, expected) in [
            (sys::mpv_log_level::MPV_LOG_LEVEL_FATAL, LogLevel::Fatal),
            (sys::mpv_log_level::MPV_LOG_LEVEL_ERROR, LogLevel::Error),
            (sys::mpv_log_level::MPV_LOG_LEVEL_WARN, LogLevel::Warn),
            (sys::mpv_log_level::MPV_LOG_LEVEL_INFO, LogLevel::Info),
            (sys::mpv_log_level::MPV_LOG_LEVEL_V, LogLevel::Verbose),
            (sys::mpv_log_level::MPV_LOG_LEVEL_DEBUG, LogLevel::Debug),
            (sys::mpv_log_level::MPV_LOG_LEVEL_TRACE, LogLevel::Trace),
        ] {
            assert_eq!(LogLevel::from_raw(raw), expected, "{raw:?}");
        }
    }

    #[test]
    fn from_raw_falls_back_to_off_for_none_and_for_an_unknown_level() {
        assert_eq!(
            LogLevel::from_raw(sys::mpv_log_level::MPV_LOG_LEVEL_NONE),
            LogLevel::Off
        );
        assert_eq!(
            LogLevel::from_raw(sys::mpv_log_level(9999)),
            LogLevel::Off,
            "a level a newer libmpv invents must not panic"
        );
    }

    #[test]
    fn fatal_and_error_both_forward_at_error_level() {
        for level in [LogLevel::Fatal, LogLevel::Error] {
            let rec = forwarded(level, "cplayer", "boom");
            assert_eq!(rec.level, tracing::Level::ERROR, "{level:?}");
            assert_eq!(rec.message, "cplayer: boom");
        }
    }

    #[test]
    fn warn_and_info_keep_the_level_mpv_gave_them() {
        assert_eq!(
            forwarded(LogLevel::Warn, "vo", "slow").level,
            tracing::Level::WARN
        );
        assert_eq!(
            forwarded(LogLevel::Info, "ao", "opened").level,
            tracing::Level::INFO
        );
    }

    /// Deliberate downgrade: mpv's `v` and `debug` are far too chatty to sit
    /// at the levels of the same name, so they land one step quieter.
    #[test]
    fn verbose_lands_at_debug_and_debug_lands_at_trace() {
        assert_eq!(
            forwarded(LogLevel::Verbose, "cplayer", "v line").level,
            tracing::Level::DEBUG
        );
        assert_eq!(
            forwarded(LogLevel::Debug, "cplayer", "d line").level,
            tracing::Level::TRACE
        );
    }

    #[test]
    fn trailing_newlines_are_trimmed_off_the_text() {
        assert_eq!(
            forwarded(LogLevel::Info, "cplayer", "line\r\n").message,
            "cplayer: line"
        );
        assert_eq!(
            forwarded(LogLevel::Info, "cplayer", "line\n\n\n").message,
            "cplayer: line"
        );
        // Only the tail: newlines inside a multi-line message stay.
        assert_eq!(
            forwarded(LogLevel::Info, "cplayer", "a\nb\n").message,
            "cplayer: a\nb"
        );
    }

    #[test]
    fn an_empty_message_still_forwards_with_its_prefix() {
        assert_eq!(
            forwarded(LogLevel::Warn, "cplayer", "\n").message,
            "cplayer: "
        );
    }

    /// `Trace` and `Off` have no forwarding rule of their own; they must be
    /// visible as a gap rather than silently dropped.
    #[test]
    fn an_unhandled_level_is_warned_with_a_marker() {
        for level in [LogLevel::Trace, LogLevel::Off] {
            let rec = forwarded(level, "cplayer", "x");
            assert_eq!(rec.level, tracing::Level::WARN, "{level:?}");
            assert!(
                rec.message.starts_with("[unhandled mpv level "),
                "{}",
                rec.message
            );
            assert!(rec.message.ends_with("cplayer: x"), "{}", rec.message);
        }
    }

    #[test]
    fn every_forwarded_line_is_tagged_with_the_mpv_target() {
        for level in [
            LogLevel::Off,
            LogLevel::Fatal,
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Verbose,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            assert_eq!(forwarded(level, "p", "t").target, "mpv", "{level:?}");
        }
    }
}
