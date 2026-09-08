//! Cheap standing counters for memory-growth investigations
//! (`docs/memory-growth-plan.md`, `docs/memory-growth-findings.md`).
//!
//! One `debug` line per 60 s on the `memprobe` target, so it costs nothing at
//! the default log level and a run that wants it can ask for exactly it with
//! `--log-level 'warn,memprobe=debug'`.
//!
//! What it answers, and what it already ruled out for #643: CEF paint rate
//! (plan §2.1 — zero paints during playback, so the overlay pipeline cannot be
//! the leak), the two unbounded channel backlogs (§2.3 — never above single
//! digits) and mpv's demuxer read-ahead (§2.2 — flat at the configured cap).
//! It deliberately says nothing about GPU memory, which is where the real
//! regression lived; that is sampled out-of-process.
//!
//! It lives in `jfn-mpv` only because that is the one crate `jfn-cef` and
//! `jfn-playback` both already depend on; nothing here is mpv-specific.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const REPORT_PERIOD: Duration = Duration::from_secs(60);

/// CEF `OnPaint` callbacks (software BGRA buffer).
static PAINT_SW: AtomicU64 = AtomicU64::new(0);
/// CEF `OnAcceleratedPaint` callbacks (shared texture handle).
static PAINT_ACCEL: AtomicU64 = AtomicU64::new(0);
/// Of those, the ones that reached `surface_present` (the gates in
/// `client/paint.rs` drop the rest).
static PRESENT_SW: AtomicU64 = AtomicU64::new(0);
static PRESENT_ACCEL: AtomicU64 = AtomicU64::new(0);
/// High-water mark of `Receiver::len()` since the last report.
static MPV_EVENT_QLEN: AtomicU64 = AtomicU64::new(0);
static COORD_QLEN: AtomicU64 = AtomicU64::new(0);
/// Latest `demuxer-cache-state` `fw-bytes`.
static FW_BYTES: AtomicU64 = AtomicU64::new(0);

/// Start the reporter thread. Idempotent; called from every producer so the
/// probe works whichever subsystem comes up first.
pub fn start() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = thread::Builder::new()
            .name("jfn-memprobe".into())
            .spawn(|| {
                loop {
                    thread::sleep(REPORT_PERIOD);
                    report();
                }
            });
    });
}

/// A CEF paint callback arrived. `accelerated` distinguishes
/// `OnAcceleratedPaint` from `OnPaint`.
pub fn note_paint(accelerated: bool) {
    let cell = if accelerated { &PAINT_ACCEL } else { &PAINT_SW };
    cell.fetch_add(1, Ordering::Relaxed);
    start();
}

/// A paint survived the drop gates and was handed to the compositor.
pub fn note_paint_presented(accelerated: bool) {
    let cell = if accelerated {
        &PRESENT_ACCEL
    } else {
        &PRESENT_SW
    };
    cell.fetch_add(1, Ordering::Relaxed);
}

/// Backlog of `jfn_mpv::event_loop`'s channel, sampled on the receiver side.
pub fn note_mpv_event_qlen(len: usize) {
    MPV_EVENT_QLEN.fetch_max(len as u64, Ordering::Relaxed);
    start();
}

/// Backlog of `jfn_playback::coordinator`'s channel, sampled on the receiver
/// side.
pub fn note_coordinator_qlen(len: usize) {
    COORD_QLEN.fetch_max(len as u64, Ordering::Relaxed);
}

/// Latest `fw-bytes` out of mpv's `demuxer-cache-state`.
pub fn note_fw_bytes(bytes: i64) {
    FW_BYTES.store(bytes.max(0) as u64, Ordering::Relaxed);
}

fn report() {
    let paint_sw = PAINT_SW.swap(0, Ordering::Relaxed);
    let paint_accel = PAINT_ACCEL.swap(0, Ordering::Relaxed);
    let present_sw = PRESENT_SW.swap(0, Ordering::Relaxed);
    let present_accel = PRESENT_ACCEL.swap(0, Ordering::Relaxed);
    let mpv_q = MPV_EVENT_QLEN.swap(0, Ordering::Relaxed);
    let coord_q = COORD_QLEN.swap(0, Ordering::Relaxed);
    let fw = FW_BYTES.load(Ordering::Relaxed);
    tracing::debug!(
        target: "memprobe",
        "paints_sw={paint_sw} paints_accel={paint_accel} \
         presented_sw={present_sw} presented_accel={present_accel} \
         mpv_event_qlen_max={mpv_q} coordinator_qlen_max={coord_q} \
         demuxer_fw_bytes={fw}"
    );
}
