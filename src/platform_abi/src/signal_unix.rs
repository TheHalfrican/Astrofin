use std::ffi::c_int;

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

pub struct SignalGuard {
    prev_int: Option<SigAction>,
    prev_term: Option<SigAction>,
}

impl Default for SignalGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalGuard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            prev_int: snapshot(Signal::SIGINT),
            prev_term: snapshot(Signal::SIGTERM),
        }
    }

    /// # Safety
    /// `handler` must be async-signal-safe: it runs from inside a `sigaction`
    /// handler installed on SIGINT/SIGTERM.
    #[must_use]
    pub unsafe fn install(handler: extern "C" fn(c_int)) -> Self {
        let sa = SigAction::new(
            SigHandler::Handler(handler),
            SaFlags::empty(),
            SigSet::empty(),
        );
        Self {
            prev_int: unsafe { sigaction(Signal::SIGINT, &sa) }.ok(),
            prev_term: unsafe { sigaction(Signal::SIGTERM, &sa) }.ok(),
        }
    }
}

// Reads a disposition the only way sigaction offers: install SIG_IGN, then put
// the reported action straight back.
fn snapshot(signal: Signal) -> Option<SigAction> {
    let probe = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    let prev = unsafe { sigaction(signal, &probe) }.ok()?;
    let _ = unsafe { sigaction(signal, &prev) };
    Some(prev)
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev_int {
            let _ = unsafe { sigaction(Signal::SIGINT, prev) };
        }
        if let Some(prev) = &self.prev_term {
            let _ = unsafe { sigaction(Signal::SIGTERM, prev) };
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{SignalGuard, snapshot};
    use nix::sys::signal::{SigHandler, Signal};
    use std::ffi::c_int;

    extern "C" fn test_handler(_sig: c_int) {}

    fn installed(signal: Signal) -> Option<usize> {
        match snapshot(signal)?.handler() {
            SigHandler::Handler(f) => Some(f as usize),
            _ => Some(0),
        }
    }

    /// The syscall half of `Platform::install_shutdown_handler`, exercised
    /// without ever delivering a signal: arm both dispositions, read them back
    /// through the same probe `snapshot` uses, then drop the guard and see the
    /// previous ones restored. Nothing is raised, so the test binary's own
    /// death is unaffected either way.
    #[test]
    fn a_guard_arms_both_shutdown_signals_and_puts_the_old_ones_back() {
        let before = (installed(Signal::SIGINT), installed(Signal::SIGTERM));
        let want = Some(test_handler as *const () as usize);
        {
            let _guard = unsafe { SignalGuard::install(test_handler) };
            assert_eq!(installed(Signal::SIGINT), want);
            assert_eq!(installed(Signal::SIGTERM), want);
        }
        assert_eq!(
            (installed(Signal::SIGINT), installed(Signal::SIGTERM)),
            before,
            "the previous dispositions were not restored"
        );

        // `new` only records: taking one, and dropping it, changes nothing.
        // Asserted here rather than in a test of its own because the
        // dispositions are process-global and two tests would race.
        let bare = SignalGuard::new();
        assert_eq!(
            (installed(Signal::SIGINT), installed(Signal::SIGTERM)),
            before
        );
        drop(bare);
        assert_eq!(
            (installed(Signal::SIGINT), installed(Signal::SIGTERM)),
            before
        );
    }
}
