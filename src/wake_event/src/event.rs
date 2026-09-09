use core::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};

pub struct WakeEvent {
    handle: HANDLE,
}

// Win32 event HANDLEs are kernel objects; concurrent SetEvent/ResetEvent/Wait*
// are documented thread-safe.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

impl WakeEvent {
    pub fn new() -> Option<Self> {
        // manual-reset, initially non-signaled
        let h = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if h.is_null() {
            return None;
        }
        Some(WakeEvent { handle: h })
    }

    pub fn signal(&self) {
        unsafe {
            SetEvent(self.handle);
        }
    }

    pub fn drain(&self) {
        unsafe {
            ResetEvent(self.handle);
        }
    }

    /// Block until signaled. Manual-reset, so a `signal()` that lands
    /// before the call returns immediately.
    pub fn wait(&self) {
        unsafe {
            WaitForSingleObject(self.handle, INFINITE);
        }
    }
}

impl Drop for WakeEvent {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

    use super::*;

    const DELAY: Duration = Duration::from_millis(60);

    /// Bounded wait on the same handle `WakeEvent::wait` blocks on — the only
    /// way to observe "still unsignaled" without parking forever.
    fn signaled_within(ev: &WakeEvent, ms: u32) -> bool {
        unsafe { WaitForSingleObject(ev.handle, ms) == WAIT_OBJECT_0 }
    }

    fn process_handles() -> u32 {
        let mut count = 0u32;
        let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &raw mut count) };
        assert!(ok != 0, "GetProcessHandleCount failed");
        count
    }

    #[test]
    fn a_new_event_starts_unsignaled() {
        let ev = WakeEvent::new().expect("create event");
        assert!(!signaled_within(&ev, 20));
    }

    #[test]
    fn signal_releases_a_blocked_wait() {
        let ev = Arc::new(WakeEvent::new().expect("create event"));
        let signaler = Arc::clone(&ev);
        let t = std::thread::spawn(move || {
            std::thread::sleep(DELAY);
            signaler.signal();
        });
        let start = Instant::now();
        ev.wait();
        let waited = start.elapsed();
        t.join().expect("signaler thread");
        assert!(waited >= DELAY / 2, "wait returned early after {waited:?}");
    }

    #[test]
    fn a_signal_that_lands_before_the_wait_is_not_lost() {
        let ev = WakeEvent::new().expect("create event");
        ev.signal();
        // Manual-reset: the wait must return at once, not park for the next
        // signal that will never come.
        ev.wait();
        assert!(signaled_within(&ev, 0));
    }

    #[test]
    fn drain_clears_the_signal_so_the_next_wait_would_block() {
        let ev = WakeEvent::new().expect("create event");
        ev.signal();
        assert!(signaled_within(&ev, 20));
        ev.drain();
        assert!(!signaled_within(&ev, 20));
    }

    #[test]
    fn repeated_signals_coalesce_into_one_wake() {
        let ev = WakeEvent::new().expect("create event");
        for _ in 0..3 {
            ev.signal();
        }
        ev.wait();
        ev.drain();
        // Three signals did not queue three wakes: one drain clears them all.
        assert!(!signaled_within(&ev, 20));
    }

    #[test]
    fn drain_on_an_unsignaled_event_is_a_no_op() {
        let ev = WakeEvent::new().expect("create event");
        ev.drain();
        ev.drain();
        assert!(!signaled_within(&ev, 0));
        ev.signal();
        assert!(signaled_within(&ev, 0));
    }

    #[test]
    fn dropping_an_event_returns_its_handle_to_the_process() {
        let before = process_handles();
        for _ in 0..2048 {
            let ev = WakeEvent::new().expect("create event");
            ev.signal();
            drop(ev);
        }
        let after = process_handles();
        // Leaking would show 2048 extra handles; the slack absorbs handles
        // other tests in this binary hold concurrently.
        assert!(
            after < before + 256,
            "handle count grew from {before} to {after}"
        );
    }
}
