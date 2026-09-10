//! Error classification and the synthetic readiness the X connection source
//! reports.
//!
//! Split out of [`crate::conn_source`], which owns the fd, the queue and the
//! calloop registration. Both pieces here are decisions over values, and both
//! matter: misclassifying a socket error loses the `io::Error` the caller wants
//! to log, and getting the synthetic readiness wrong strands parsed events in
//! userspace while `poll(2)` sees an idle fd.

use calloop::Readiness;
use x11rb::errors::ConnectionError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum X11SourceError {
    #[error("x11 connection error: {0}")]
    Connection(#[source] ConnectionError),
    #[error("x11 socket i/o error: {0}")]
    Io(#[source] std::io::Error),
}

/// `xcb::Error`'s own `Display` names only the category, so the cause is
/// formatted with `Debug` here and carried as `source()`.
#[derive(Debug, thiserror::Error)]
#[error("xcb connection error: {0:?}")]
pub(crate) struct XcbSourceError(#[source] pub(crate) xcb::Error);

/// Split an x11rb connection error into the socket-level and protocol-level
/// cases, so a dead socket reports the `io::Error` the operator needs rather
/// than a generic protocol failure.
pub(crate) fn classify_connection_error(e: ConnectionError) -> X11SourceError {
    match e {
        ConnectionError::IoError(e) => X11SourceError::Io(e),
        e => X11SourceError::Connection(e),
    }
}

/// The readiness reported when events are already parsed into userspace.
///
/// Every X round trip drains the socket and parses whatever it finds, so the fd
/// can look idle to `poll(2)` while events sit unhandled. Reporting readable
/// here is what gets them dispatched instead of blocking on socket traffic that
/// may never come; the source is never written to or watched for errors.
pub(crate) fn queued_readiness() -> Readiness {
    Readiness {
        readable: true,
        writable: false,
        error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_failure_keeps_its_io_error() {
        let e = classify_connection_error(ConnectionError::IoError(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "gone",
        )));
        let X11SourceError::Io(io) = e else {
            panic!("expected an io error, got {e:?}");
        };
        assert_eq!(io.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn a_protocol_failure_stays_a_connection_error() {
        let e = classify_connection_error(ConnectionError::UnknownError);
        assert!(matches!(e, X11SourceError::Connection(_)));
        let e = classify_connection_error(ConnectionError::MaximumRequestLengthExceeded);
        assert!(matches!(e, X11SourceError::Connection(_)));
    }

    #[test]
    fn an_error_says_which_side_it_came_from() {
        let io = classify_connection_error(ConnectionError::IoError(std::io::Error::other("x")));
        assert!(io.to_string().starts_with("x11 socket i/o error"));
        let conn = classify_connection_error(ConnectionError::UnknownError);
        assert!(conn.to_string().starts_with("x11 connection error"));
    }

    #[test]
    fn an_xcb_error_carries_its_debug_form_in_the_message() {
        let e = XcbSourceError(xcb::Error::Connection(xcb::ConnError::Connection));
        assert!(e.to_string().starts_with("xcb connection error:"));
        assert!(e.to_string().contains("Connection"));
    }

    #[test]
    fn queued_events_report_readable_only() {
        let r = queued_readiness();
        assert!(r.readable);
        assert!(!r.writable);
        assert!(!r.error);
    }
}
