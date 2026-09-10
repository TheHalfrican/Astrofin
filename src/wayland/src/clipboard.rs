//! Wayland clipboard (CLIPBOARD selection) read path via wl-clipboard-rs.
//!
//! Why not wl_data_device on the main display: wl_data_device is focus-bound,
//! and the main jellyfin wl_display competes with XWayland's clipboard bridge
//! on the same seat which CEF (running as an X11 ozone client) relies on for
//! Ctrl+V. wl-clipboard-rs speaks ext-data-control-v1 (falling back to
//! wlr-data-control-v1), which is focus-independent, and opens its own
//! wl_display per read — no shared globals with the main display.

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use parking_lot::Mutex;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use calloop::generic::Generic;
use calloop::ping::PingSource;
use calloop::{EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, Readiness};
use crossbeam_channel::{Receiver, SendError, Sender, unbounded};
use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};
use wl_clipboard_rs::utils::is_primary_selection_supported;

struct PendingCb {
    cb: Box<dyn FnOnce(&str) + Send>,
}

struct Shared {
    tx: Sender<PendingCb>,
    stop: AtomicBool,
    ping: calloop::ping::Ping,
}

struct Handle {
    shared: Arc<Shared>,
    thread: JoinHandle<()>,
}

/// A clipboard payload as text. A selection that is not valid UTF-8 reads as
/// empty rather than being lossily transcoded: the requester's promise has to
/// resolve either way, and half-decoded bytes would reach the page as real
/// content.
fn decode_text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or("")
}

fn fire(pending: PendingCb, text: &[u8]) {
    (pending.cb)(decode_text(text));
}

fn start_receive() -> Option<OwnedFd> {
    let (reader, _mime) =
        get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text).ok()?;
    let fd = OwnedFd::from(reader);
    fcntl(&fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).ok()?;
    Some(fd)
}

fn probe_supported() -> bool {
    is_primary_selection_supported().is_ok()
}

struct Worker {
    shared: Arc<Shared>,
    queued: Receiver<PendingCb>,
    signal: LoopSignal,
    loop_handle: LoopHandle<'static, Worker>,
    active: Option<(PendingCb, Vec<u8>)>,
}

impl Worker {
    fn promote_next(&mut self) {
        if self.active.is_some() {
            return;
        }
        let Ok(cb) = self.queued.try_recv() else {
            return;
        };
        let Some(fd) = start_receive() else {
            fire(cb, &[]);
            self.drain_queued();
            return;
        };
        let inserted = self.loop_handle.insert_source(
            Generic::new(fd, Interest::READ, Mode::Level),
            |readiness, fd, worker: &mut Worker| Ok(worker.on_receive_ready(readiness, fd.as_fd())),
        );
        if let Err(e) = inserted {
            tracing::warn!(target: "Main", "clipboard: receive source: {e}");
            fire(cb, &[]);
            return;
        }
        self.active = Some((cb, Vec::new()));
    }

    fn on_receive_ready(&mut self, readiness: Readiness, fd: BorrowedFd<'_>) -> PostAction {
        let Some((_, buf)) = self.active.as_mut() else {
            return PostAction::Remove;
        };
        let mut done = readiness.error;
        if readiness.readable {
            let mut tmp = [0u8; 4096];
            let mut file = unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) };
            loop {
                match file.read(&mut tmp) {
                    Ok(0) => {
                        done = true;
                        break;
                    }
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => {
                        done = true;
                        break;
                    }
                }
            }
            // Don't let File's drop close the fd — the source's OwnedFd owns it.
            let _ = file.into_raw_fd();
        }
        if !done {
            return PostAction::Continue;
        }
        if let Some((cb, buf)) = self.active.take() {
            fire(cb, &buf);
        }
        PostAction::Remove
    }

    fn drain_pending(&mut self) {
        if let Some((cb, _)) = self.active.take() {
            fire(cb, &[]);
        }
        self.drain_queued();
    }

    fn drain_queued(&mut self) {
        for cb in self.queued.try_iter() {
            fire(cb, &[]);
        }
    }
}

fn run_clipboard_loop(shared: Arc<Shared>, queued: Receiver<PendingCb>, wake: PingSource) {
    let mut event_loop: EventLoop<'static, Worker> = match EventLoop::try_new() {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(target: "Main", "clipboard: event loop: {e}");
            return;
        }
    };
    let handle = event_loop.handle();
    let mut worker = Worker {
        shared,
        queued,
        signal: event_loop.get_signal(),
        loop_handle: handle.clone(),
        active: None,
    };
    if let Err(e) = handle.insert_source(wake, |(), (), worker: &mut Worker| {
        if worker.shared.stop.load(Ordering::Relaxed) {
            worker.signal.stop();
        }
    }) {
        tracing::error!(target: "Main", "clipboard: wake source: {e}");
        worker.drain_pending();
        return;
    }
    // `run` calls its callback only after a dispatch, so promote once here or a
    // request queued before the loop started would wait for the first wake.
    worker.promote_next();
    if let Err(e) = event_loop.run(None, &mut worker, Worker::promote_next) {
        tracing::error!(target: "Main", "clipboard: event loop: {e}");
    }
    worker.drain_pending();
}

/// The wayland lifecycle drives init/cleanup; the read path goes through the
/// runtime's slot.
pub struct Clipboard {
    inner: Mutex<Option<Handle>>,
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn init(&self) {
        let mut g = self.inner.lock();
        if g.is_some() {
            return;
        }
        if !probe_supported() {
            return;
        }
        let Ok((ping, wake)) = calloop::ping::make_ping() else {
            return;
        };
        let (tx, rx) = unbounded::<PendingCb>();
        let shared = Arc::new(Shared {
            tx,
            stop: AtomicBool::new(false),
            ping,
        });
        let shared_w = shared.clone();
        let thread = thread::spawn(move || run_clipboard_loop(shared_w, rx, wake));
        *g = Some(Handle { shared, thread });
    }

    pub fn available(&self) -> bool {
        self.inner.lock().is_some()
    }

    pub fn read_text_async(&self, cb: Box<dyn FnOnce(&str) + Send>) {
        // Falls through to an empty read when there is no clipboard worker, or
        // when its receiver is already gone, so the caller's promise resolves.
        // Fired with the slot lock released: a callback may re-enter here.
        let undelivered = {
            let g = self.inner.lock();
            match g.as_ref() {
                Some(c) => match c.shared.tx.send(PendingCb { cb }) {
                    Ok(()) => {
                        c.shared.ping.ping();
                        return;
                    }
                    Err(SendError(pending)) => pending,
                },
                None => PendingCb { cb },
            }
        };
        fire(undelivered, &[]);
    }

    pub fn cleanup(&self) {
        let Some(handle) = self.inner.lock().take() else {
            return;
        };
        handle.shared.stop.store(true, Ordering::Relaxed);
        handle.shared.ping.ping();
        // The worker drains every still-queued callback after its loop returns,
        // so joining here is what guarantees each one ran exactly once.
        let _ = handle.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_decodes_unchanged() {
        assert_eq!(decode_text(b"hello"), "hello");
        assert_eq!(
            decode_text("caf\u{e9} \u{2014} ok".as_bytes()),
            "caf\u{e9} \u{2014} ok"
        );
    }

    #[test]
    fn an_empty_selection_decodes_to_an_empty_string() {
        assert_eq!(decode_text(b""), "");
    }

    #[test]
    fn a_non_utf8_selection_reads_as_empty_rather_than_lossy() {
        // Latin-1 bytes, a lone surrogate half, and a truncated code point.
        assert_eq!(decode_text(&[0xFF, 0xFE]), "");
        assert_eq!(decode_text(&[0xED, 0xA0, 0x80]), "");
        assert_eq!(
            decode_text("caf\u{e9}".as_bytes().split_last().unwrap().1),
            ""
        );
    }

    #[test]
    fn an_embedded_nul_is_kept_because_it_is_valid_utf8() {
        assert_eq!(decode_text(b"a\0b"), "a\0b");
    }

    #[test]
    fn a_delivered_callback_sees_the_decoded_text() {
        let (tx, rx) = std::sync::mpsc::channel();
        fire(
            PendingCb {
                cb: Box::new(move |s| {
                    let _ = tx.send(s.to_owned());
                }),
            },
            b"clipped",
        );
        assert_eq!(rx.try_recv().unwrap(), "clipped");
    }

    #[test]
    fn an_undeliverable_request_still_resolves_with_an_empty_string() {
        let (tx, rx) = std::sync::mpsc::channel();
        fire(
            PendingCb {
                cb: Box::new(move |s| {
                    let _ = tx.send(s.to_owned());
                }),
            },
            &[],
        );
        assert_eq!(rx.try_recv().unwrap(), "");
    }
}
