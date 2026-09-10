//! Cross-platform one-shot event for waking poll()/WaitForMultipleObjects().
//!
//! `signal()` is async-signal-safe on POSIX (a single short `write`) so it
//! can be invoked from signal handlers.

#[cfg(target_os = "linux")]
#[path = "eventfd.rs"]
mod imp;
#[cfg(all(unix, not(target_os = "linux")))]
#[path = "pipe.rs"]
mod imp;
#[cfg(windows)]
#[path = "event.rs"]
mod imp;

#[cfg(unix)]
mod fd_wait;
#[cfg(target_os = "linux")]
mod source;

pub use imp::WakeEvent;
#[cfg(target_os = "linux")]
pub use source::{Drain, WakeSource};

/// Fully drain a level-triggered wake fd (eventfd or pipe read end) that was
/// signaled while unread, so a following `poll` won't immediately re-fire.
/// Reads until the fd would block. For raw fds published across threads whose
/// lifetime the caller manages; owned events use [`WakeEvent`].
#[cfg(unix)]
pub fn drain_raw_fd(fd: std::ffi::c_int) {
    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let mut buf = [0u8; 64];
    loop {
        match nix::unistd::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            // Retry a signal-interrupted read; stop on would-block (drained)
            // or any real error — the wake is best-effort, not worth escalating.
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
}

/// Signal an eventfd-backed wake fd: one 8-byte write of 1. Async-signal-safe.
/// The single home for the wake-signal encoding, shared with
/// [`WakeEvent::signal`].
///
/// Linux only: the encoding and the single readable-and-writable fd are both
/// eventfd properties. The other unixes back [`WakeEvent`] with a pipe, whose
/// [`WakeEvent::fd`] is the read end, so there [`WakeEvent::signal`] — which
/// owns the write end — is the only way to signal.
#[cfg(target_os = "linux")]
pub fn signal_raw_fd(fd: std::ffi::c_int) {
    let val: u64 = 1;
    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let _ = nix::unistd::write(fd, &val.to_ne_bytes());
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::c_int;
    use std::os::fd::BorrowedFd;

    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

    use super::*;

    /// Does a `poll` see the wake fd as readable right now?
    fn readable(fd: c_int) -> bool {
        // SAFETY: `fd` is owned by the caller's live `WakeEvent`.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let mut fds = [PollFd::new(borrowed, PollFlags::POLLIN)];
        poll(&mut fds, PollTimeout::ZERO).unwrap_or(0) > 0
    }

    // `signal_raw_fd` is eventfd-only, and only Linux backs `WakeEvent` with one.
    #[test]
    #[cfg(target_os = "linux")]
    fn signal_raw_fd_makes_the_wake_fd_readable() {
        let ev = WakeEvent::new().expect("wake event");
        assert!(!readable(ev.fd()), "a fresh wake fd must not be readable");
        signal_raw_fd(ev.fd());
        assert!(readable(ev.fd()));
    }

    #[test]
    fn drain_raw_fd_empties_a_signaled_fd() {
        let ev = WakeEvent::new().expect("wake event");
        ev.signal();
        drain_raw_fd(ev.fd());
        assert!(!readable(ev.fd()));
    }

    #[test]
    fn repeated_signals_drain_in_one_pass() {
        let ev = WakeEvent::new().expect("wake event");
        for _ in 0..4 {
            ev.signal();
        }
        drain_raw_fd(ev.fd());
        assert!(!readable(ev.fd()), "one drain must clear every signal");
    }

    #[test]
    fn draining_an_unsignaled_fd_returns_without_blocking() {
        let ev = WakeEvent::new().expect("wake event");
        drain_raw_fd(ev.fd());
        assert!(!readable(ev.fd()));
        // Still usable afterwards.
        ev.signal();
        assert!(readable(ev.fd()));
    }
}
