use std::ffi::c_int;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::unistd::write;

pub struct WakeEvent {
    read_fd: OwnedFd,
    write_fd: OwnedFd,
}

impl WakeEvent {
    pub fn new() -> Option<Self> {
        let (read, write) = std::io::pipe().ok()?;
        let read_fd = OwnedFd::from(read);
        let write_fd = OwnedFd::from(write);
        for fd in [&read_fd, &write_fd] {
            fcntl(fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).ok()?;
        }
        Some(WakeEvent { read_fd, write_fd })
    }

    pub fn fd(&self) -> c_int {
        self.read_fd.as_raw_fd()
    }

    pub fn signal(&self) {
        let _ = write(&self.write_fd, &[1u8]);
    }

    pub fn drain(&self) {
        crate::drain_raw_fd(self.read_fd.as_raw_fd());
    }

    /// Block until signaled. Level-triggered, so a `signal()` that lands
    /// before the call returns immediately.
    pub fn wait(&self) {
        crate::fd_wait::wait(self.read_fd.as_raw_fd());
    }
}

impl AsFd for WakeEvent {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.read_fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

    use super::*;

    /// Does a `poll` see the read end as readable right now?
    fn readable(ev: &WakeEvent) -> bool {
        let mut fds = [PollFd::new(ev.as_fd(), PollFlags::POLLIN)];
        poll(&mut fds, PollTimeout::ZERO).unwrap_or(0) > 0
    }

    #[test]
    fn a_fresh_event_has_a_real_read_fd_and_is_not_signaled() {
        let ev = WakeEvent::new().expect("pipe");
        assert!(ev.fd() >= 0);
        assert!(!readable(&ev));
    }

    #[test]
    fn signal_makes_the_read_end_readable() {
        let ev = WakeEvent::new().expect("pipe");
        ev.signal();
        assert!(readable(&ev));
    }

    #[test]
    fn drain_clears_a_signal() {
        let ev = WakeEvent::new().expect("pipe");
        ev.signal();
        ev.drain();
        assert!(!readable(&ev));
    }

    #[test]
    fn one_drain_clears_every_pending_signal() {
        let ev = WakeEvent::new().expect("pipe");
        for _ in 0..4 {
            ev.signal();
        }
        ev.drain();
        assert!(!readable(&ev), "one drain must clear every signal");
    }

    #[test]
    fn draining_an_unsignaled_event_leaves_it_usable() {
        // Both ends are non-blocking, so a drain with nothing to read must
        // return rather than park the caller.
        let ev = WakeEvent::new().expect("pipe");
        ev.drain();
        ev.signal();
        assert!(readable(&ev));
    }

    #[test]
    fn wait_returns_at_once_for_a_signal_that_already_landed() {
        let ev = WakeEvent::new().expect("pipe");
        ev.signal();
        ev.wait();
    }

    #[test]
    fn the_borrowed_fd_is_the_read_end_fd_reports() {
        let ev = WakeEvent::new().expect("pipe");
        assert_eq!(ev.as_fd().as_raw_fd(), ev.fd());
    }
}
