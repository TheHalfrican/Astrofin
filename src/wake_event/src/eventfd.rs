use std::ffi::c_int;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};

use nix::sys::eventfd::{EfdFlags, EventFd};

pub struct WakeEvent {
    fd: EventFd,
}

impl WakeEvent {
    pub fn new() -> Option<Self> {
        let fd = EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
            .ok()?;
        Some(WakeEvent { fd })
    }

    pub fn fd(&self) -> c_int {
        self.fd.as_raw_fd()
    }

    pub fn signal(&self) {
        crate::signal_raw_fd(self.fd.as_raw_fd());
    }

    pub fn drain(&self) {
        let _ = self.fd.read();
    }

    /// Block until signaled. Level-triggered, so a `signal()` that lands
    /// before the call returns immediately.
    pub fn wait(&self) {
        crate::fd_wait::wait(self.fd.as_raw_fd());
    }
}

impl AsFd for WakeEvent {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

    use super::*;

    /// Does a `poll` see the event as readable right now?
    fn readable(ev: &WakeEvent) -> bool {
        let mut fds = [PollFd::new(ev.as_fd(), PollFlags::POLLIN)];
        poll(&mut fds, PollTimeout::ZERO).unwrap_or(0) > 0
    }

    #[test]
    fn a_fresh_event_has_a_real_fd_and_is_not_signaled() {
        let ev = WakeEvent::new().expect("eventfd");
        assert!(ev.fd() >= 0);
        assert!(!readable(&ev));
    }

    #[test]
    fn signal_makes_the_event_readable() {
        let ev = WakeEvent::new().expect("eventfd");
        ev.signal();
        assert!(readable(&ev));
    }

    #[test]
    fn drain_clears_a_signal() {
        let ev = WakeEvent::new().expect("eventfd");
        ev.signal();
        ev.drain();
        assert!(!readable(&ev));
    }

    #[test]
    fn one_drain_clears_every_pending_signal() {
        // An eventfd is a counter: four signals are one readable state, and
        // one read takes the whole count.
        let ev = WakeEvent::new().expect("eventfd");
        for _ in 0..4 {
            ev.signal();
        }
        ev.drain();
        assert!(!readable(&ev));
    }

    #[test]
    fn draining_an_unsignaled_event_leaves_it_usable() {
        let ev = WakeEvent::new().expect("eventfd");
        ev.drain();
        ev.signal();
        assert!(readable(&ev));
    }

    #[test]
    fn wait_returns_at_once_for_a_signal_that_already_landed() {
        // Level-triggered: the wake cannot be missed by arriving early.
        let ev = WakeEvent::new().expect("eventfd");
        ev.signal();
        ev.wait();
    }

    #[test]
    fn the_borrowed_fd_is_the_one_fd_reports() {
        let ev = WakeEvent::new().expect("eventfd");
        assert_eq!(ev.as_fd().as_raw_fd(), ev.fd());
    }
}
