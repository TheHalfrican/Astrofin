use std::ffi::c_int;
use std::sync::OnceLock;

use crate::SignalGuard;
use crate::shutdown_logic::{adopt_callback, dispatch};

// Slot stays until process exit; the guard's Drop restores the original
// dispositions.
static GUARD: OnceLock<SignalGuard> = OnceLock::new();
static SHUTDOWN_CB: OnceLock<fn()> = OnceLock::new();

pub fn install_shutdown(on_shutdown: fn()) {
    // Claimed before arming: the handler reads this slot, so it must be live
    // by the time a signal can fire. A later caller finds the disposition
    // already ours and leaves it alone — see `adopt_callback`.
    if !adopt_callback(&SHUTDOWN_CB, on_shutdown) {
        return;
    }
    let g = unsafe { SignalGuard::install(on_shutdown_signal) };
    let _ = GUARD.set(g);
}

// Runs in signal context: must stay async-signal-safe — no allocation, no
// locking, no logging. The decision it makes is `shutdown_logic::dispatch`.
extern "C" fn on_shutdown_signal(_sig: c_int) {
    let _ = dispatch(&SHUTDOWN_CB);
}
