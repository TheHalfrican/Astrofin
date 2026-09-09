//! Live window geometry, sourced from whichever component owns the window,
//! plus the payload-free change wakeup. Producers update their source and
//! call [`notify_window_changed`]; consumers subscribe and pull a
//! [`WindowSnapshot`].

use std::sync::Arc;

use parking_lot::Mutex;

use crate::geometry::{WindowExtent, WindowPos};

#[derive(Clone, Copy)]
pub struct WindowSnapshot {
    pub extent: Option<WindowExtent>,
    pub position: Option<WindowPos>,
    pub maximized: bool,
    pub fullscreen: bool,
}

pub trait WindowSource: Send + Sync {
    fn snapshot(&self) -> WindowSnapshot;
}

static WINDOW_SUBSCRIBERS: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(Vec::new());

/// Register a window-changed subscriber for the life of the process.
/// Subscribers must not depend on invocation order.
pub fn subscribe_window_changed<F: Fn() + Send + Sync + 'static>(cb: F) {
    WINDOW_SUBSCRIBERS.lock().push(Arc::new(cb));
}

/// Wake every subscriber; each pulls the current snapshot itself. Callers
/// must have already committed the state a pull would read.
pub fn notify_window_changed() {
    let subs: Vec<_> = WINDOW_SUBSCRIBERS.lock().clone();
    for cb in subs {
        cb();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    // The subscriber list is process-wide and append-only, so the tests that
    // touch it run one at a time and assert on their own deltas.
    static SERIAL: Mutex<()> = Mutex::new(());
    static FIRST: AtomicUsize = AtomicUsize::new(0);
    static SECOND: AtomicUsize = AtomicUsize::new(0);
    static LATE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn a_notification_wakes_every_subscriber_once() {
        let _serial = SERIAL.lock();
        subscribe_window_changed(|| {
            FIRST.fetch_add(1, Ordering::Relaxed);
        });
        subscribe_window_changed(|| {
            SECOND.fetch_add(1, Ordering::Relaxed);
        });
        let (first, second) = (
            FIRST.load(Ordering::Relaxed),
            SECOND.load(Ordering::Relaxed),
        );
        notify_window_changed();
        assert_eq!(FIRST.load(Ordering::Relaxed), first + 1);
        assert_eq!(SECOND.load(Ordering::Relaxed), second + 1);
    }

    #[test]
    fn a_subscriber_registered_later_is_woken_too() {
        let _serial = SERIAL.lock();
        notify_window_changed();
        let before = LATE.load(Ordering::Relaxed);
        subscribe_window_changed(|| {
            LATE.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(LATE.load(Ordering::Relaxed), before, "subscribing is quiet");
        notify_window_changed();
        assert_eq!(LATE.load(Ordering::Relaxed), before + 1);
    }

    #[test]
    fn a_snapshot_carries_the_sources_own_view_of_the_window() {
        struct Source;
        impl WindowSource for Source {
            fn snapshot(&self) -> WindowSnapshot {
                WindowSnapshot {
                    extent: Some(WindowExtent::new(
                        crate::geometry::PhysicalSize { w: 1920, h: 1080 },
                        crate::geometry::Scale(1.0),
                    )),
                    position: Some(WindowPos { x: 10, y: 20 }),
                    maximized: true,
                    fullscreen: false,
                }
            }
        }
        let snap = (&Source as &dyn WindowSource).snapshot();
        assert_eq!(
            snap.extent.map(|e| e.physical()),
            Some(crate::geometry::PhysicalSize { w: 1920, h: 1080 })
        );
        assert_eq!(snap.position, Some(WindowPos { x: 10, y: 20 }));
        assert!(snap.maximized);
        assert!(!snap.fullscreen);
    }
}
