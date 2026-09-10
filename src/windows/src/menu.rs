//! Windows menus — native Win32 popups tracked on the input thread.
//!
//! `TrackPopupMenuEx` blocks its caller for the whole life of the menu, so a
//! request is parked here and handed to the input thread through a posted
//! message; the CEF UI thread that opened it returns immediately.

use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};

use jfn_platform_abi::{
    MENU_DISMISSED, MenuHost, MenuItem, MenuRequest, MenuSelection, menu_has_selectable,
};
use parking_lot::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, EndMenu, MF_SEPARATOR, PostMessageW,
    SetForegroundWindow, TrackPopupMenuEx, WM_APP, WM_NULL,
};
use windows::core::PCWSTR;

use crate::input::input_hwnd;
use crate::menu_logic::{self, Append};
use crate::platform::win_get_scale;

/// Asks the input thread to track the parked request.
pub(crate) const WM_JFN_MENU_TRACK: u32 = WM_APP + 0x100;

/// Asks the input thread to end the menu it is tracking.
pub(crate) const WM_JFN_MENU_END: u32 = WM_APP + 0x101;

struct Pending {
    items: Vec<MenuItem>,
    /// Anchor in logical (view) coordinates.
    x: c_int,
    y: c_int,
    on_selected: MenuSelection,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

static TRACKING: AtomicBool = AtomicBool::new(false);

static CANCELLED: AtomicBool = AtomicBool::new(false);

pub(crate) struct WinMenuHost;

impl MenuHost for WinMenuHost {
    fn open(&self, req: MenuRequest) {
        let (Some(hwnd), true) = (input_hwnd(), menu_has_selectable(&req.items)) else {
            req.on_selected.resolve(MENU_DISMISSED);
            return;
        };
        let displaced = PENDING.lock().replace(Pending {
            items: req.items,
            x: req.x,
            y: req.y,
            on_selected: req.on_selected,
        });
        if let Some(prev) = displaced {
            prev.on_selected.resolve(MENU_DISMISSED);
        }
        let _ = unsafe { PostMessageW(Some(hwnd), WM_JFN_MENU_TRACK, WPARAM(0), LPARAM(0)) };
    }

    fn hide(&self) {
        CANCELLED.store(true, Ordering::Release);
        let pending = PENDING.lock().take();
        if let Some(pending) = pending {
            pending.on_selected.resolve(MENU_DISMISSED);
        }
        post_end();
    }

    fn shutdown(&self) {
        let pending = PENDING.lock().take();
        if let Some(pending) = pending {
            pending.on_selected.resolve(MENU_DISMISSED);
        }
        post_end();
    }
}

fn post_end() {
    if let Some(hwnd) = input_hwnd() {
        let _ = unsafe { PostMessageW(Some(hwnd), WM_JFN_MENU_END, WPARAM(0), LPARAM(0)) };
    }
}

/// Runs on the input thread, from `input_wndproc`.
pub(crate) fn on_input_message(hwnd: HWND, msg: u32) {
    if msg == WM_JFN_MENU_END {
        let _ = unsafe { EndMenu() };
        return;
    }
    if msg != WM_JFN_MENU_TRACK {
        return;
    }
    if TRACKING.load(Ordering::Acquire) {
        let _ = unsafe { EndMenu() };
        let _ = unsafe { PostMessageW(Some(hwnd), WM_JFN_MENU_TRACK, WPARAM(0), LPARAM(0)) };
        return;
    }
    let Some(pending) = PENDING.lock().take() else {
        return;
    };
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        pending.on_selected.resolve(MENU_DISMISSED);
        return;
    };

    for item in &pending.items {
        match menu_logic::append_kind(item) {
            Append::Separator => {
                let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
            }
            Append::Skip => {}
            Append::Command(flags) => {
                let label = menu_logic::wide(&item.label);
                let _ = unsafe {
                    AppendMenuW(
                        menu,
                        flags,
                        item.id as usize,
                        PCWSTR::from_raw(label.as_ptr()),
                    )
                };
            }
        }
    }

    let (ax, ay) = menu_logic::anchor(pending.x, pending.y, win_get_scale());
    let mut pt = POINT { x: ax, y: ay };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }

    if let Some(toplevel) = crate::platform::win_hwnd() {
        let _ = unsafe { SetForegroundWindow(toplevel) };
    }

    CANCELLED.store(false, Ordering::Release);
    TRACKING.store(true, Ordering::Release);
    let picked = unsafe { TrackPopupMenuEx(menu, menu_logic::TRACK_FLAGS, pt.x, pt.y, hwnd, None) };
    TRACKING.store(false, Ordering::Release);
    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    let _ = unsafe { DestroyMenu(menu) };

    let cancelled = CANCELLED.swap(false, Ordering::AcqRel);
    pending
        .on_selected
        .resolve(menu_logic::selection(cancelled, picked.0));
}
