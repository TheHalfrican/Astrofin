//! The decisions the second-instance listener makes about *when* to act: how
//! long to wait after a failed `accept`, how many peers may be served at once,
//! how long a silent connection may live, whether a failed probe means the
//! name is stale, and how often a bind may be retried.
//!
//! They live here rather than inside the socket glue in [`crate`] so each one
//! is a plain input -> output function with a test of its own; the glue keeps
//! only the calls that need a real socket.

use std::io::ErrorKind;
use std::time::Duration;

/// First delay after a failed `accept`, doubling from there.
///
/// A persistent error — the classic one is `EMFILE`, every file descriptor in
/// the process spent — makes `accept` return immediately, so a bare `continue`
/// spins a core at millions of iterations a second and floods the log. Ten
/// milliseconds is short enough that a transient error (a peer that hung up
/// between the connect and the accept) is invisible.
pub(crate) const BACKOFF_MIN: Duration = Duration::from_millis(10);

/// Ceiling for the doubling in [`Backoff::next_delay`]. A second is long
/// enough to idle at while a descriptor frees up, and short enough that the
/// listener is usable again within one human beat of the cause going away.
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Peers served at the same time. The only legitimate peer is a second copy of
/// the app forwarding its argv and exiting, so one at a time is the real load;
/// eight leaves room for a burst of launches while capping what a local process
/// that opens connections in a loop can make us allocate.
pub(crate) const MAX_CONNECTIONS: usize = 8;

/// How long a peer that arrives at the cap is given before it is refused.
///
/// A connection whose peer has already hung up still holds its slot until its
/// task is next polled and sees the end of stream, so a burst of short-lived
/// launches — eight second instances in a row, each connecting, forwarding its
/// argv and exiting — can otherwise find the count full of connections that
/// are already dead. One short beat lets those land. A peer that is genuinely
/// holding eight sockets open frees nothing in that time and is still refused.
pub(crate) const CAP_GRACE: Duration = Duration::from_millis(25);

/// How long a connection may go without completing a frame before it is
/// closed. A real second instance sends its message immediately after
/// connecting; anything still silent after five seconds is either wedged or
/// holding a slot on purpose.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Retries after the first bind attempt when the name is stale but still held.
pub(crate) const REBIND_RETRIES: u32 = 5;

/// Delay between those retries. Five of them is a quarter of a second, which
/// is below the threshold where a user notices a slow start.
pub(crate) const REBIND_DELAY: Duration = Duration::from_millis(50);

/// Exponential backoff for a failing `accept`: [`BACKOFF_MIN`], doubling to
/// [`BACKOFF_MAX`], reset by the next successful accept.
pub(crate) struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Backoff {
    pub(crate) const fn new() -> Self {
        Self { next: BACKOFF_MIN }
    }

    /// The delay to wait before trying to accept again, doubling on every
    /// consecutive failure and never past [`BACKOFF_MAX`].
    pub(crate) fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(BACKOFF_MAX);
        delay
    }

    /// Forget the streak. Called after every accepted connection, so an error
    /// an hour later starts again at [`BACKOFF_MIN`].
    pub(crate) fn reset(&mut self) {
        self.next = BACKOFF_MIN;
    }
}

/// Whether one more peer may be served while `active` are already in flight.
pub(crate) fn has_capacity(active: usize) -> bool {
    active < MAX_CONNECTIONS
}

/// Whether a failed probe of a name somebody already holds means the holder is
/// gone, so the name may be taken over.
///
/// `ConnectionRefused` is the unix answer: the socket file outlived the
/// process that bound it. `NotFound` is the Windows one, and the reason this
/// is a function: right after `Listener::shutdown` the pipe name can still be
/// taken while the pipe itself is already gone, and `CreateFile` then reports
/// `ERROR_FILE_NOT_FOUND` rather than a refusal. Anything else — a permission
/// error, an unreachable path — is *not* evidence of death and must never
/// reclaim a name somebody may still be using.
pub(crate) fn probe_is_stale(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::ConnectionRefused | ErrorKind::NotFound)
}

/// How long to wait before bind retry number `retries_done`, or `None` once
/// the retries are spent and the failure is real.
pub(crate) fn rebind_delay(retries_done: u32) -> Option<Duration> {
    (retries_done < REBIND_RETRIES).then_some(REBIND_DELAY)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        BACKOFF_MAX, BACKOFF_MIN, Backoff, CAP_GRACE, IDLE_TIMEOUT, MAX_CONNECTIONS, REBIND_DELAY,
        REBIND_RETRIES, has_capacity, probe_is_stale, rebind_delay,
    };
    use std::io::ErrorKind;
    use std::time::Duration;

    #[test]
    fn backoff_starts_at_ten_milliseconds_and_doubles() {
        let mut backoff = Backoff::new();
        assert_eq!(backoff.next_delay(), Duration::from_millis(10));
        assert_eq!(backoff.next_delay(), Duration::from_millis(20));
        assert_eq!(backoff.next_delay(), Duration::from_millis(40));
        assert_eq!(backoff.next_delay(), Duration::from_millis(80));
    }

    #[test]
    fn backoff_stops_doubling_at_one_second() {
        let mut backoff = Backoff::new();
        for _ in 0..64 {
            assert!(backoff.next_delay() <= BACKOFF_MAX);
        }
        assert_eq!(backoff.next_delay(), BACKOFF_MAX);
    }

    #[test]
    fn backoff_reset_returns_to_the_first_delay() {
        let mut backoff = Backoff::new();
        for _ in 0..5 {
            backoff.next_delay();
        }
        backoff.reset();
        assert_eq!(backoff.next_delay(), BACKOFF_MIN);
    }

    #[test]
    fn a_default_backoff_is_a_fresh_one() {
        assert_eq!(Backoff::default().next_delay(), Backoff::new().next_delay());
    }

    #[test]
    fn has_capacity_admits_up_to_eight_peers_and_no_more() {
        assert!(has_capacity(0));
        assert!(has_capacity(MAX_CONNECTIONS - 1));
        assert!(!has_capacity(MAX_CONNECTIONS));
        assert!(!has_capacity(MAX_CONNECTIONS + 100));
    }

    /// The phase-2 finding: a name that is still taken while its probe reports
    /// "not found" is a shutdown that has not finished, not a live peer.
    #[test]
    fn probe_is_stale_covers_a_refused_connection_and_a_vanished_name() {
        assert!(probe_is_stale(ErrorKind::ConnectionRefused));
        assert!(probe_is_stale(ErrorKind::NotFound));
    }

    #[test]
    fn probe_is_stale_never_reclaims_a_name_we_merely_cannot_reach() {
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::TimedOut,
            ErrorKind::WouldBlock,
            ErrorKind::AddrInUse,
            ErrorKind::Interrupted,
        ] {
            assert!(!probe_is_stale(kind), "{kind:?}");
        }
    }

    #[test]
    fn rebind_delay_allows_five_short_retries_and_then_gives_up() {
        for done in 0..REBIND_RETRIES {
            assert_eq!(rebind_delay(done), Some(REBIND_DELAY));
        }
        assert_eq!(rebind_delay(REBIND_RETRIES), None);
        assert_eq!(rebind_delay(REBIND_RETRIES + 1), None);
        // The whole retry budget stays well under a human-visible pause.
        assert!(REBIND_DELAY * REBIND_RETRIES <= Duration::from_millis(500));
    }

    /// The grace exists to absorb a scheduling beat, not to queue a peer: a
    /// refusal has to stay far quicker than the timeout that closes a
    /// connection we did accept.
    #[test]
    fn the_cap_grace_is_a_beat_not_a_queue() {
        assert!(CAP_GRACE < BACKOFF_MAX);
        assert!(CAP_GRACE * 10 < IDLE_TIMEOUT);
    }

    #[test]
    fn the_idle_timeout_is_longer_than_the_whole_rebind_budget() {
        // A connection that outlives a rebind storm is genuinely idle, not one
        // whose peer is waiting for us to finish starting up.
        assert!(IDLE_TIMEOUT > REBIND_DELAY * REBIND_RETRIES);
    }
}
