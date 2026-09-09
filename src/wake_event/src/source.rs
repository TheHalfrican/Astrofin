use std::ffi::c_int;
use std::os::fd::{AsRawFd as _, BorrowedFd};

use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

pub enum Drain {
    /// The fd stays readable; a level-triggered fan-out lets several loops
    /// observe one signal.
    Never,
    BeforeCallback,
}

pub struct WakeSource {
    fd: BorrowedFd<'static>,
    drain: Drain,
    token: Option<Token>,
}

impl WakeSource {
    /// `fd` must outlive the source; the caller owns the [`WakeEvent`].
    ///
    /// [`WakeEvent`]: crate::WakeEvent
    pub fn new(fd: c_int, drain: Drain) -> WakeSource {
        // SAFETY: the caller keeps the owning `WakeEvent` alive for at least as
        // long as this source.
        let fd = unsafe { BorrowedFd::borrow_raw(fd) };
        WakeSource {
            fd,
            drain,
            token: None,
        }
    }
}

impl EventSource for WakeSource {
    type Event = ();
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    fn process_events<F: FnMut((), &mut ())>(
        &mut self,
        _readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> std::io::Result<PostAction> {
        if self.token != Some(token) {
            return Ok(PostAction::Continue);
        }
        if matches!(self.drain, Drain::BeforeCallback) {
            crate::drain_raw_fd(self.fd.as_raw_fd());
        }
        callback((), &mut ());
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        let token = factory.token();
        self.token = Some(token);
        // SAFETY: the fd outlives this source, and unregistration always
        // happens before the source is dropped.
        unsafe { poll.register(self.fd, Interest::READ, Mode::Level, token) }
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        let token = factory.token();
        self.token = Some(token);
        poll.reregister(self.fd, Interest::READ, Mode::Level, token)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.token = None;
        poll.unregister(self.fd)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use calloop::EventLoop;

    use super::*;
    use crate::WakeEvent;

    const TIMEOUT: Duration = Duration::from_millis(200);

    #[test]
    fn a_signaled_event_fires_the_callback() {
        let ev = WakeEvent::new().expect("wake event");
        let mut evl: EventLoop<'_, u32> = EventLoop::try_new().expect("event loop");
        evl.handle()
            .insert_source(
                WakeSource::new(ev.fd(), Drain::BeforeCallback),
                |_, _, fired: &mut u32| *fired += 1,
            )
            .expect("insert source");
        let mut fired = 0u32;
        ev.signal();
        evl.dispatch(TIMEOUT, &mut fired).expect("dispatch");
        assert_eq!(fired, 1);
    }

    #[test]
    fn draining_before_the_callback_consumes_the_signal() {
        let ev = WakeEvent::new().expect("wake event");
        let mut evl: EventLoop<'_, u32> = EventLoop::try_new().expect("event loop");
        evl.handle()
            .insert_source(
                WakeSource::new(ev.fd(), Drain::BeforeCallback),
                |_, _, fired: &mut u32| *fired += 1,
            )
            .expect("insert source");
        let mut fired = 0u32;
        ev.signal();
        evl.dispatch(TIMEOUT, &mut fired).expect("dispatch");
        // The fd was drained, so the next dispatch has nothing to report and
        // returns on its timeout instead.
        evl.dispatch(Duration::from_millis(20), &mut fired)
            .expect("dispatch");
        assert_eq!(fired, 1);
    }

    #[test]
    fn a_never_drained_signal_stays_readable_for_the_next_dispatch() {
        let ev = WakeEvent::new().expect("wake event");
        let mut evl: EventLoop<'_, u32> = EventLoop::try_new().expect("event loop");
        evl.handle()
            .insert_source(
                WakeSource::new(ev.fd(), Drain::Never),
                |_, _, fired: &mut u32| *fired += 1,
            )
            .expect("insert source");
        let mut fired = 0u32;
        ev.signal();
        evl.dispatch(TIMEOUT, &mut fired).expect("dispatch");
        evl.dispatch(TIMEOUT, &mut fired).expect("dispatch");
        assert_eq!(fired, 2, "a level-triggered fan-out must re-fire");
        ev.drain();
        evl.dispatch(Duration::from_millis(20), &mut fired)
            .expect("dispatch");
        assert_eq!(fired, 2);
    }

    #[test]
    fn reregistering_keeps_the_source_live() {
        let ev = WakeEvent::new().expect("wake event");
        let mut evl: EventLoop<'_, u32> = EventLoop::try_new().expect("event loop");
        let token = evl
            .handle()
            .insert_source(
                WakeSource::new(ev.fd(), Drain::BeforeCallback),
                |_, _, fired: &mut u32| *fired += 1,
            )
            .expect("insert source");
        evl.handle().update(&token).expect("reregister");
        let mut fired = 0u32;
        ev.signal();
        evl.dispatch(TIMEOUT, &mut fired).expect("dispatch");
        assert_eq!(fired, 1);
    }

    #[test]
    fn a_removed_source_stops_reporting() {
        let ev = WakeEvent::new().expect("wake event");
        let mut evl: EventLoop<'_, u32> = EventLoop::try_new().expect("event loop");
        let token = evl
            .handle()
            .insert_source(
                WakeSource::new(ev.fd(), Drain::Never),
                |_, _, fired: &mut u32| *fired += 1,
            )
            .expect("insert source");
        let mut fired = 0u32;
        ev.signal();
        evl.dispatch(TIMEOUT, &mut fired).expect("dispatch");
        assert_eq!(fired, 1);
        evl.handle().remove(token);
        ev.signal();
        evl.dispatch(Duration::from_millis(20), &mut fired)
            .expect("dispatch");
        assert_eq!(fired, 1, "an unregistered source must not be polled");
    }
}
