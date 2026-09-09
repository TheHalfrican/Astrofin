//! Post-init mpv handle accessors used by sibling crates.
//!
//! All entry points borrow the global handle published by
//! [`crate::boot::jfn_mpv_handle_init`]. They no-op silently if the
//! handle has not yet been initialized or has already been terminated.
//!
//! Property writes and commands go through libmpv's async API
//! (`reply_userdata == 0`, fire-and-forget). Property reads are
//! synchronous and must only be issued from non-event contexts; observed
//! properties should be read from the `jfn_playback_*` atomics instead.
//!
//! Stateful helpers — `LoadFile` / `ApplyPendingTrackSelectionAndPlay`
//! / `SetAspectMode` — live here too. The pending-track state is
//! single-threaded by usage but guarded by a Mutex so callers from any
//! thread stay safe.
//!
//! # Safety
//!
//! Every `pub unsafe fn` in this module accepts raw C-string / raw struct
//! pointers preserved from the original FFI surface. Callers must ensure
//! all `*const c_char` arguments point to NUL-terminated UTF-8 (or are
//! null where the function documents tolerance), and that struct
//! pointers reference live values for the duration of the call.

#![allow(clippy::missing_safety_doc)]

use parking_lot::Mutex;
use std::ffi::{CStr, CString, c_char};
use std::os::raw::c_void;

use crate::sys;

// =============================================================================
// Internal helpers
// =============================================================================

fn raw() -> *mut sys::mpv_handle {
    crate::boot::current_raw_handle().unwrap_or(std::ptr::null_mut())
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a CStr> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) })
    }
}

// =============================================================================
// Generic property R/W + command
// =============================================================================

/// Async (`reply_userdata == 0`) flag write. No-op if the handle is
/// missing or `name` is NULL.
pub unsafe fn jfn_mpv_set_property_flag_async(name: *const c_char, value: bool) {
    let h = raw();
    if h.is_null() {
        return;
    }
    let Some(n) = (unsafe { cstr(name) }) else {
        return;
    };
    let mut flag: i32 = if value { 1 } else { 0 };
    unsafe {
        sys::mpv_set_property_async(
            h,
            0,
            n.as_ptr(),
            sys::mpv_format::MPV_FORMAT_FLAG,
            &mut flag as *mut _ as *mut c_void,
        );
    }
}

pub unsafe fn jfn_mpv_set_property_double_async(name: *const c_char, value: f64) {
    let h = raw();
    if h.is_null() {
        return;
    }
    let Some(n) = (unsafe { cstr(name) }) else {
        return;
    };
    let mut v = value;
    unsafe {
        sys::mpv_set_property_async(
            h,
            0,
            n.as_ptr(),
            sys::mpv_format::MPV_FORMAT_DOUBLE,
            &mut v as *mut _ as *mut c_void,
        );
    }
}

pub unsafe fn jfn_mpv_set_property_int_async(name: *const c_char, value: i64) {
    let h = raw();
    if h.is_null() {
        return;
    }
    let Some(n) = (unsafe { cstr(name) }) else {
        return;
    };
    let mut v = value;
    unsafe {
        sys::mpv_set_property_async(
            h,
            0,
            n.as_ptr(),
            sys::mpv_format::MPV_FORMAT_INT64,
            &mut v as *mut _ as *mut c_void,
        );
    }
}

pub unsafe fn jfn_mpv_set_property_string_async(name: *const c_char, value: *const c_char) {
    let h = raw();
    if h.is_null() {
        return;
    }
    let Some(n) = (unsafe { cstr(name) }) else {
        return;
    };
    let Some(v) = (unsafe { cstr(value) }) else {
        return;
    };
    let mut ptr = v.as_ptr();
    unsafe {
        sys::mpv_set_property_async(
            h,
            0,
            n.as_ptr(),
            sys::mpv_format::MPV_FORMAT_STRING,
            &mut ptr as *mut _ as *mut c_void,
        );
    }
}

/// Sync int property read. Writes the value into `*out` and returns
/// libmpv's error code (0 on success, negative on failure). NULL `out`
/// or missing handle returns `MPV_ERROR_INVALID_PARAMETER` (-4).
pub unsafe fn jfn_mpv_get_property_int(name: *const c_char, out: *mut i64) -> i32 {
    let h = raw();
    if h.is_null() || out.is_null() {
        return -4;
    }
    let Some(n) = (unsafe { cstr(name) }) else {
        return -4;
    };
    unsafe {
        sys::mpv_get_property(
            h,
            n.as_ptr(),
            sys::mpv_format::MPV_FORMAT_INT64,
            out as *mut c_void,
        )
    }
}

/// Sync string property read. Returns a malloc'd UTF-8 C string the
/// caller must free with [`jfn_mpv_free_string`], or NULL on failure.
pub unsafe fn jfn_mpv_get_property_string(name: *const c_char) -> *mut c_char {
    let h = raw();
    if h.is_null() {
        return std::ptr::null_mut();
    }
    let Some(n) = (unsafe { cstr(name) }) else {
        return std::ptr::null_mut();
    };
    let p = unsafe { sys::mpv_get_property_string(h, n.as_ptr()) };
    if p.is_null() {
        return std::ptr::null_mut();
    }
    // libmpv owns p (mpv_free required). Copy into a Rust-allocated
    // CString so the caller's free pairs with `jfn_mpv_free_string`.
    let out = unsafe { CStr::from_ptr(p) }.to_owned();
    unsafe { sys::mpv_free(p as *mut c_void) };
    out.into_raw()
}

pub unsafe fn jfn_mpv_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Async command. `args` is a `const char* const*` table of length `n`
/// (no NULL terminator required — the wrapper appends one). No-op on
/// missing handle, empty argv, or NULL entries.
pub unsafe fn jfn_mpv_command_async(args: *const *const c_char, n: usize) {
    let h = raw();
    if h.is_null() || args.is_null() || n == 0 {
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts(args, n) };
    if slice.iter().any(|p| p.is_null()) {
        return;
    }
    let mut argv: Vec<*const c_char> = slice.to_vec();
    argv.push(std::ptr::null());
    unsafe { sys::mpv_command_async(h, 0, argv.as_ptr() as *mut _) };
}

// =============================================================================
// Event drain (wait_event / wakeup).
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum WaitEvent {
    None,
    LogMessage(crate::LogMessage),
    Event(crate::Event),
}

/// Pumps libmpv's event queue. Returns the raw `mpv_event*` libmpv owns;
/// valid only until the next call on the same handle. NULL if the handle
/// is missing.
pub fn jfn_mpv_wait_event(timeout: f64) -> *mut sys::mpv_event {
    let h = raw();
    if h.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { sys::mpv_wait_event(h, timeout) }
}

/// A missing handle and `MPV_EVENT_NONE` both collapse to [`WaitEvent::None`].
pub fn wait_event_owned(timeout: f64) -> WaitEvent {
    let ev = jfn_mpv_wait_event(timeout);
    if ev.is_null() {
        return WaitEvent::None;
    }
    match unsafe { crate::Event::from_raw(ev) } {
        crate::Event::None => WaitEvent::None,
        crate::Event::LogMessage(m) => WaitEvent::LogMessage(m),
        event => WaitEvent::Event(event),
    }
}

pub fn jfn_mpv_wakeup() {
    let h = raw();
    if !h.is_null() {
        unsafe { sys::mpv_wakeup(h) };
    }
}

/// Install a C-style wakeup callback against the singleton mpv handle. The
/// callback fires from a foreign thread whenever libmpv queues a new
/// event; per the libmpv docs it must return promptly and call no
/// blocking API.
///
/// # Safety
/// `cb` must remain valid for as long as the mpv handle is in use.
pub unsafe fn jfn_mpv_set_wakeup_callback(
    cb: unsafe extern "C" fn(*mut std::ffi::c_void),
    data: *mut std::ffi::c_void,
) {
    let h = raw();
    if !h.is_null() {
        unsafe { sys::mpv_set_wakeup_callback(h, Some(cb), data) };
    }
}

/// Clear any previously-installed wakeup callback. After this call libmpv
/// will not fire a foreign-thread notification on new events.
pub fn jfn_mpv_clear_wakeup_callback() {
    let h = raw();
    if !h.is_null() {
        unsafe { sys::mpv_set_wakeup_callback(h, None, std::ptr::null_mut()) };
    }
}

// =============================================================================
// Player API — convenience wrappers over property writes / commands.
// =============================================================================

unsafe fn set_flag(name: &CStr, v: bool) {
    #[cfg(test)]
    recorder::note(recorder::Op::Flag(name.to_string_lossy().into_owned(), v));
    unsafe { jfn_mpv_set_property_flag_async(name.as_ptr(), v) };
}
unsafe fn set_double(name: &CStr, v: f64) {
    #[cfg(test)]
    recorder::note(recorder::Op::Double(name.to_string_lossy().into_owned(), v));
    unsafe { jfn_mpv_set_property_double_async(name.as_ptr(), v) };
}
unsafe fn set_str(name: &CStr, v: &CStr) {
    #[cfg(test)]
    recorder::note(recorder::Op::Str(
        name.to_string_lossy().into_owned(),
        v.to_string_lossy().into_owned(),
    ));
    unsafe { jfn_mpv_set_property_string_async(name.as_ptr(), v.as_ptr()) };
}
fn cmd(args: &[&CStr]) {
    #[cfg(test)]
    recorder::note(recorder::Op::Cmd(
        args.iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect(),
    ));
    let ptrs: Vec<*const c_char> = args.iter().map(|s| s.as_ptr()).collect();
    unsafe { jfn_mpv_command_async(ptrs.as_ptr(), ptrs.len()) };
}

/// Test-only tap on the four helpers every player entry point funnels
/// through, so the exact property write or command each one produces can be
/// asserted without a live mpv core. Compiled out of every other build; the
/// helpers themselves are unchanged.
#[cfg(test)]
mod recorder {
    use parking_lot::{Mutex, MutexGuard};

    /// What was asked of libmpv, in the order it was asked.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Op {
        Flag(String, bool),
        Double(String, f64),
        Str(String, String),
        Cmd(Vec<String>),
    }

    static OPS: Mutex<Vec<Op>> = Mutex::new(Vec::new());
    static SERIAL: Mutex<()> = Mutex::new(());

    pub fn note(op: Op) {
        OPS.lock().push(op);
    }

    /// Take the api tests' turn and start it with an empty log.
    pub fn start() -> MutexGuard<'static, ()> {
        let guard = SERIAL.lock();
        OPS.lock().clear();
        guard
    }

    /// Everything recorded since the last call, clearing the log.
    pub fn taken() -> Vec<Op> {
        std::mem::take(&mut OPS.lock())
    }

    pub fn flag(name: &str, v: bool) -> Op {
        Op::Flag(name.to_string(), v)
    }
    pub fn double(name: &str, v: f64) -> Op {
        Op::Double(name.to_string(), v)
    }
    pub fn string(name: &str, v: &str) -> Op {
        Op::Str(name.to_string(), v.to_string())
    }
    pub fn command(args: &[&str]) -> Op {
        Op::Cmd(args.iter().map(|s| (*s).to_string()).collect())
    }
}

pub fn jfn_mpv_play() {
    unsafe { set_flag(c"pause", false) };
}
pub fn jfn_mpv_pause() {
    unsafe { set_flag(c"pause", true) };
}
pub fn jfn_mpv_toggle_pause() {
    cmd(&[c"cycle", c"pause"]);
}
pub fn jfn_mpv_stop() {
    cmd(&[c"stop"]);
}
pub fn jfn_mpv_seek_absolute(secs: f64) {
    let s = CString::new(format!("{}", secs)).unwrap_or_default();
    cmd(&[c"seek", &s, c"absolute"]);
}
pub fn jfn_mpv_set_volume(v: f64) {
    unsafe { set_double(c"volume", v) };
}
pub fn jfn_mpv_set_muted(v: bool) {
    unsafe { set_flag(c"mute", v) };
}
pub fn jfn_mpv_set_speed(v: f64) {
    unsafe { set_double(c"speed", v) };
}
pub fn jfn_mpv_set_audio_delay(s: f64) {
    unsafe { set_double(c"audio-delay", s) };
}
pub fn jfn_mpv_set_subtitle_delay(s: f64) {
    unsafe { set_double(c"sub-delay", s) };
}
pub fn jfn_mpv_set_start_position(s: f64) {
    unsafe { set_double(c"start", s) };
}

/// One step of mpv's own `ab-loop` command: with neither point set it stamps
/// `ab-loop-a`, with only `a` set it stamps `ab-loop-b`, and with both set it
/// clears the pair (`player/command.c`, `cmd_ab_loop`).
///
/// The point is *mpv's* `get_current_time()`, which is why the loop is set
/// this way rather than by writing a time the UI sampled: mpv disarms the
/// loop at write time when the core's pts is already past the `b` being
/// written (`player/playloop.c`, `update_ab_loop_clip`), and the UI's
/// position lags the core by up to half a second.
///
/// `no-osd` (`DOCS/man/input.rst`, "Input Command Prefixes") keeps mpv's own
/// "A-B loop: …" text off the video: the OSD for this is ours.
///
/// Async, so this is safe to call from the mpv event thread.
pub fn jfn_mpv_ab_loop_cycle() {
    cmd(&[c"no-osd", c"ab-loop"]);
}

/// Drop both A-B loop points by writing mpv's sentinel string `"no"` to each;
/// with either end unset mpv does not loop (`DOCS/man/options.rst`,
/// `--ab-loop-a`).
///
/// `a` first, deliberately: the two async writes are observed separately, so
/// there is one intermediate pair either way, and `(no, b)` already draws as
/// "no loop" while `(a, no)` would flash the A-only state.
pub fn jfn_mpv_clear_ab_loop() {
    unsafe {
        set_str(c"ab-loop-a", c"no");
        set_str(c"ab-loop-b", c"no");
    }
}

/// Track id sentinel: 0 = disabled. >=1 = explicit mpv track id.
/// Mpv's auto-track-selection is globally disabled (boot applies
/// `track-auto-selection=no`); jellyfin-web is the authority.
const TRACK_DISABLE: i64 = 0;

fn track_to_mpv_str(id: i64) -> CString {
    if id == TRACK_DISABLE {
        CString::new("no").unwrap_or_default()
    } else {
        CString::new(id.to_string()).unwrap_or_default()
    }
}

pub fn jfn_mpv_set_audio_track(id: i64) {
    let s = track_to_mpv_str(id);
    unsafe { set_str(c"aid", &s) };
}

pub fn jfn_mpv_set_subtitle_track(id: i64) {
    let s = track_to_mpv_str(id);
    unsafe { set_str(c"sid", &s) };
}

pub unsafe fn jfn_mpv_sub_add(url: *const c_char) {
    let Some(u) = (unsafe { cstr(url) }) else {
        return;
    };
    cmd(&[c"sub-add", u, c"select"]);
}

pub unsafe fn jfn_mpv_audio_add(url: *const c_char) {
    let Some(u) = (unsafe { cstr(url) }) else {
        return;
    };
    cmd(&[c"audio-add", u, c"select"]);
}

// =============================================================================
// LoadFile + deferred track selection (stateful)
// =============================================================================

/// Load options for `LoadFile`. NULL string pointers are treated as empty.
#[repr(C)]
pub struct JfnMpvLoadOptions {
    pub start_secs: f64,
    pub video_track: i64,
    pub audio_track: i64,
    pub sub_track: i64,
    pub external_audio_url: *const c_char,
    pub external_sub_url: *const c_char,
    pub is_infinite_stream: bool,
}

struct PendingTrack {
    vid: i64,
    aid: i64,
    sid: i64,
    external_audio_url: String,
    external_sub_url: String,
    defer_audio_to_mpv: bool,
    valid: bool,
}

fn pending_slot() -> &'static Mutex<PendingTrack> {
    use std::sync::OnceLock;
    static SLOT: OnceLock<Mutex<PendingTrack>> = OnceLock::new();
    SLOT.get_or_init(|| {
        Mutex::new(PendingTrack {
            vid: 1,
            aid: TRACK_DISABLE,
            sid: TRACK_DISABLE,
            external_audio_url: String::new(),
            external_sub_url: String::new(),
            defer_audio_to_mpv: false,
            valid: false,
        })
    })
}

unsafe fn cstr_to_string(p: *const c_char) -> String {
    unsafe { cstr(p) }
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether mpv, not Jellyfin, picks the audio track for this load.
///
/// Only for an unprobed live stream that carries no explicit choice and no
/// external audio file: there, jellyfin-web had no track list to choose from,
/// while mpv's demuxer knows the container's own default (HLS `DEFAULT=YES`,
/// the first MPEG-TS PMT entry, and so on).
fn should_defer_audio(
    is_infinite_stream: bool,
    audio_track: i64,
    external_audio_url: &str,
) -> bool {
    is_infinite_stream && audio_track == TRACK_DISABLE && external_audio_url.is_empty()
}

/// The per-file option string handed to mpv's `loadfile`.
///
/// Always paused: the intended track ids are applied by property write after
/// FILE_LOADED (mpv drops loadfile's own selectors under
/// `track-auto-selection=no`), and playback starts once those have landed.
fn load_file_options(start_secs: f64, defer_audio: bool) -> String {
    let mut opts = format!("start={start_secs},pause=yes");
    if defer_audio {
        // Per-file enable so mpv's demuxer picks the format-correct
        // audio track. We explicitly write `sid=no` after FILE_LOADED
        // to keep subs off.
        opts.push_str(",track-auto-selection=yes");
    }
    opts
}

pub unsafe fn jfn_mpv_load_file(path: *const c_char, opts: *const JfnMpvLoadOptions) {
    let Some(path_c) = (unsafe { cstr(path) }) else {
        return;
    };
    let Some(o) = (unsafe { opts.as_ref() }) else {
        return;
    };

    let ext_audio = unsafe { cstr_to_string(o.external_audio_url) };
    let ext_sub = unsafe { cstr_to_string(o.external_sub_url) };
    let defer_audio = should_defer_audio(o.is_infinite_stream, o.audio_track, &ext_audio);

    // Track selection is owned by Jellyfin. With track-auto-selection=no,
    // mpv silently drops aid/vid/sid in loadfile options (loadfile.c
    // skips select_default_track entirely). Load the file *paused* with
    // no selectors, stash the intended ids, and apply them via property
    // writes after FILE_LOADED. The async writes + final pause=false are
    // FIFO-ordered on mpv's core thread, so playback only begins after
    // track-switch reinits land.
    {
        let mut s = pending_slot().lock();
        s.vid = o.video_track;
        s.aid = o.audio_track;
        s.sid = o.sub_track;
        s.external_audio_url = ext_audio;
        s.external_sub_url = ext_sub;
        s.defer_audio_to_mpv = defer_audio;
        s.valid = true;
    }

    let opts_c = CString::new(load_file_options(o.start_secs, defer_audio)).unwrap_or_default();
    cmd(&[c"loadfile", path_c, c"replace", c"-1", &opts_c]);
}

pub fn jfn_mpv_apply_pending_track_selection_and_play() {
    let snapshot = {
        let mut s = pending_slot().lock();
        if !s.valid {
            return;
        }
        let snap = (
            s.vid,
            s.aid,
            s.sid,
            std::mem::take(&mut s.external_audio_url),
            std::mem::take(&mut s.external_sub_url),
            s.defer_audio_to_mpv,
        );
        s.valid = false;
        s.defer_audio_to_mpv = false;
        snap
    };
    let (vid, aid, sid, ext_audio, ext_sub, defer_audio) = snapshot;

    let vid_s = track_to_mpv_str(vid);
    unsafe { set_str(c"vid", &vid_s) };
    if !defer_audio {
        // Normal path: jellyfin-web is authoritative. Skipped only for
        // the unprobed-live case (track-auto-selection=yes was set
        // per-file in load_file so mpv's demuxer already picked).
        let aid_s = track_to_mpv_str(aid);
        unsafe { set_str(c"aid", &aid_s) };
    }
    let sid_s = track_to_mpv_str(sid);
    unsafe { set_str(c"sid", &sid_s) };
    if !ext_audio.is_empty() {
        match CString::new(ext_audio) {
            Ok(u) => cmd(&[c"audio-add", &u, c"select"]),
            Err(e) => tracing::warn!("ext audio path has interior NUL: {e}"),
        }
    }
    if !ext_sub.is_empty() {
        match CString::new(ext_sub) {
            Ok(u) => cmd(&[c"sub-add", &u, c"select"]),
            Err(e) => tracing::warn!("ext sub path has interior NUL: {e}"),
        }
    }
    unsafe { set_flag(c"pause", false) };
}

// =============================================================================
// Aspect-mode helper
// =============================================================================

/// mpv's two knobs for an aspect mode: `keepaspect` and `panscan`.
/// `None` for a mode this build does not know, which the caller ignores
/// silently (matching the legacy log-and-skip).
fn aspect_mode_options(mode: &[u8]) -> Option<(bool, f64)> {
    match mode {
        b"auto" => Some((true, 0.0)),
        b"cover" => Some((true, 1.0)),
        b"fill" => Some((false, 0.0)),
        _ => None,
    }
}

pub unsafe fn jfn_mpv_set_aspect_mode(mode: *const c_char) {
    let Some(m) = (unsafe { cstr(mode) }) else {
        return;
    };
    let Some((keepaspect, panscan)) = aspect_mode_options(m.to_bytes()) else {
        return;
    };
    unsafe { set_flag(c"keepaspect", keepaspect) };
    unsafe { set_double(c"panscan", panscan) };
}

// =============================================================================
// Window / display
// =============================================================================

pub fn jfn_mpv_set_fullscreen(v: bool) {
    unsafe { set_flag(c"fullscreen", v) };
}
pub fn jfn_mpv_toggle_fullscreen() {
    cmd(&[c"cycle", c"fullscreen"]);
}
pub fn jfn_mpv_set_window_minimized(v: bool) {
    unsafe { set_flag(c"window-minimized", v) };
}
pub fn jfn_mpv_set_window_maximized(v: bool) {
    unsafe { set_flag(c"window-maximized", v) };
}
pub fn jfn_mpv_set_force_window_position(v: bool) {
    unsafe { set_flag(c"force-window-position", v) };
}
pub unsafe fn jfn_mpv_set_geometry(g: *const c_char) {
    let Some(g) = (unsafe { cstr(g) }) else {
        return;
    };
    unsafe { set_str(c"geometry", g) };
}

/// Reply id carried by the [`crate::Event::GetPropertyReply`] answering
/// [`jfn_mpv_request_background_color`].
pub const BACKGROUND_COLOR_REPLY: crate::event::ReplyUserdata = 1;

/// Enqueues an async read of mpv's `background-color` on the core dispatch
/// queue; never parks the caller on mpv's core lock. No-op without a live
/// handle. The answer arrives as a `GetPropertyReply` tagged
/// [`BACKGROUND_COLOR_REPLY`]; decode it with
/// [`background_color_from_reply`].
pub fn jfn_mpv_request_background_color() {
    let h = raw();
    if h.is_null() {
        return;
    }
    // Boot-time entry point for this property: a fresh handle is back on
    // mpv.conf's value, so forget what the previous one was told.
    *background_color_memo().lock() = None;
    unsafe {
        sys::mpv_get_property_async(
            h,
            BACKGROUND_COLOR_REPLY,
            c"background-color".as_ptr(),
            sys::mpv_format::MPV_FORMAT_STRING,
        )
    };
}

/// Packed 0x00RRGGBB parsed from a [`BACKGROUND_COLOR_REPLY`] payload.
/// `None` when the reply carried no string value.
pub fn background_color_from_reply(value: &crate::PropertyValue) -> Option<u32> {
    match value {
        crate::PropertyValue::String(s) => Some(crate::color::parse(s)),
        _ => None,
    }
}

/// The last `background-color` written, so writing it again is free.
///
/// mpv keeps `background-color` in `gl_video_conf` — the very option group
/// `vo_gpu_next`'s `update_options` watches — so any write it counts as a
/// change re-runs `update_render_options`, which walks the whole user-shader
/// chain through `load_hook` again and re-decides whether to flush the
/// renderer cache. The app writes this on every player open and every close,
/// always the same two colours, so most of that work is for nothing.
fn background_color_memo() -> &'static Mutex<Option<CString>> {
    use std::sync::OnceLock;
    static SLOT: OnceLock<Mutex<Option<CString>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub unsafe fn jfn_mpv_set_background_color_hex(hex: *const c_char) {
    let Some(h) = (unsafe { cstr(hex) }) else {
        return;
    };
    let mut last = background_color_memo().lock();
    if last.as_deref() == Some(h) {
        return;
    }
    unsafe { set_str(c"background-color", h) };
    *last = Some(h.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use recorder::{command, double, flag, string};
    use std::ptr;

    fn ops() -> Vec<recorder::Op> {
        recorder::taken()
    }

    fn load_options(start_secs: f64, audio_track: i64, sub_track: i64) -> JfnMpvLoadOptions {
        JfnMpvLoadOptions {
            start_secs,
            video_track: 1,
            audio_track,
            sub_track,
            external_audio_url: ptr::null(),
            external_sub_url: ptr::null(),
            is_infinite_stream: false,
        }
    }

    // =====================================================================
    // Generic property writes and commands: the guards, since the payload
    // needs a live core to go anywhere.
    // =====================================================================

    /// Every entry point is reachable before `jfn_mpv_handle_init` and after
    /// terminate; the null-handle guard is what keeps that from being a
    /// null-pointer call into libmpv. (The recorder taps the helper layer
    /// above these four, so a direct call is expected to log nothing.)
    #[test]
    fn a_flag_write_without_a_handle_or_with_a_null_name_reaches_libmpv_never() {
        let _g = recorder::start();
        unsafe { jfn_mpv_set_property_flag_async(c"pause".as_ptr(), true) };
        unsafe { jfn_mpv_set_property_flag_async(ptr::null(), true) };
        assert!(ops().is_empty());
    }

    #[test]
    fn a_double_write_without_a_handle_or_with_a_null_name_reaches_libmpv_never() {
        let _g = recorder::start();
        unsafe { jfn_mpv_set_property_double_async(c"speed".as_ptr(), 1.5) };
        unsafe { jfn_mpv_set_property_double_async(ptr::null(), 1.5) };
        assert!(ops().is_empty());
    }

    #[test]
    fn an_int_write_without_a_handle_or_with_a_null_name_reaches_libmpv_never() {
        let _g = recorder::start();
        unsafe { jfn_mpv_set_property_int_async(c"chapter".as_ptr(), 3) };
        unsafe { jfn_mpv_set_property_int_async(ptr::null(), 3) };
        assert!(ops().is_empty());
    }

    /// A string write needs both halves; a null value must not reach libmpv
    /// as an empty string, which is a legal value there.
    #[test]
    fn a_string_write_needs_both_a_name_and_a_value() {
        let _g = recorder::start();
        unsafe { jfn_mpv_set_property_string_async(c"aid".as_ptr(), c"2".as_ptr()) };
        unsafe { jfn_mpv_set_property_string_async(ptr::null(), c"2".as_ptr()) };
        unsafe { jfn_mpv_set_property_string_async(c"aid".as_ptr(), ptr::null()) };
        assert!(ops().is_empty());
    }

    /// The wrapper appends the NULL terminator libmpv wants, so an argv that
    /// already holds one would truncate the command; it is dropped instead.
    #[test]
    fn a_command_with_no_arguments_or_a_null_entry_is_refused() {
        let _g = recorder::start();
        let argv: Vec<*const c_char> = vec![c"stop".as_ptr()];
        unsafe { jfn_mpv_command_async(argv.as_ptr(), argv.len()) };
        unsafe { jfn_mpv_command_async(argv.as_ptr(), 0) };
        unsafe { jfn_mpv_command_async(ptr::null(), 1) };
        let holey: Vec<*const c_char> = vec![c"seek".as_ptr(), ptr::null()];
        unsafe { jfn_mpv_command_async(holey.as_ptr(), holey.len()) };
        assert!(ops().is_empty());
    }

    /// libmpv's own "invalid parameter"; the caller cannot tell a missing
    /// handle from a bad argument, and does not need to.
    #[test]
    fn reading_an_int_property_without_a_handle_reports_invalid_parameter() {
        let mut out: i64 = 7;
        assert_eq!(
            unsafe { jfn_mpv_get_property_int(c"chapter".as_ptr(), &mut out) },
            -4
        );
        assert_eq!(out, 7, "the out parameter must be left alone");
    }

    #[test]
    fn reading_an_int_property_into_a_null_out_or_from_a_null_name_is_refused() {
        assert_eq!(
            unsafe { jfn_mpv_get_property_int(c"chapter".as_ptr(), ptr::null_mut()) },
            -4
        );
        let mut out: i64 = 0;
        assert_eq!(
            unsafe { jfn_mpv_get_property_int(ptr::null(), &mut out) },
            -4
        );
    }

    #[test]
    fn reading_a_string_property_without_a_handle_returns_null() {
        assert!(unsafe { jfn_mpv_get_property_string(c"mpv-version".as_ptr()) }.is_null());
        assert!(unsafe { jfn_mpv_get_property_string(ptr::null()) }.is_null());
    }

    /// The string handed back is Rust-allocated, so freeing it has to pair
    /// with `CString::from_raw` and not with libmpv's `mpv_free`.
    #[test]
    fn freeing_a_string_accepts_null_and_a_rust_allocated_pointer() {
        unsafe { jfn_mpv_free_string(ptr::null_mut()) };
        let owned = CString::new("mpv 0.40.0").expect("cstring").into_raw();
        unsafe { jfn_mpv_free_string(owned) };
    }

    // =====================================================================
    // Event drain
    // =====================================================================

    #[test]
    fn waiting_for_an_event_without_a_handle_returns_null_at_once() {
        assert!(jfn_mpv_wait_event(0.0).is_null());
        assert!(jfn_mpv_wait_event(10.0).is_null(), "must not park");
    }

    /// A missing handle and an idle mpv are the same thing to the caller.
    #[test]
    fn the_owned_event_drain_collapses_a_missing_handle_to_none() {
        assert_eq!(wait_event_owned(0.0), WaitEvent::None);
    }

    #[test]
    fn waking_the_event_loop_without_a_handle_does_nothing() {
        jfn_mpv_wakeup();
    }

    #[test]
    fn installing_and_clearing_a_wakeup_callback_without_a_handle_does_nothing() {
        unsafe extern "C" fn noop(_: *mut std::ffi::c_void) {}
        unsafe { jfn_mpv_set_wakeup_callback(noop, ptr::null_mut()) };
        jfn_mpv_clear_wakeup_callback();
    }

    // =====================================================================
    // Player API — the property write or command each entry point produces
    // =====================================================================

    #[test]
    fn play_and_pause_write_the_pause_flag() {
        let _g = recorder::start();
        jfn_mpv_play();
        assert_eq!(ops(), [flag("pause", false)]);
        jfn_mpv_pause();
        assert_eq!(ops(), [flag("pause", true)]);
    }

    /// `cycle pause` and not a read-then-write: mpv is the authority on the
    /// current state, so it does the flipping.
    #[test]
    fn toggling_pause_asks_mpv_to_cycle_the_property() {
        let _g = recorder::start();
        jfn_mpv_toggle_pause();
        assert_eq!(ops(), [command(&["cycle", "pause"])]);
    }

    #[test]
    fn stop_sends_mpvs_stop_command() {
        let _g = recorder::start();
        jfn_mpv_stop();
        assert_eq!(ops(), [command(&["stop"])]);
    }

    #[test]
    fn an_absolute_seek_carries_the_position_as_mpv_spells_it() {
        let _g = recorder::start();
        jfn_mpv_seek_absolute(12.5);
        assert_eq!(ops(), [command(&["seek", "12.5", "absolute"])]);
        // A whole number loses the fraction, which mpv accepts either way.
        jfn_mpv_seek_absolute(90.0);
        assert_eq!(ops(), [command(&["seek", "90", "absolute"])]);
        // Rewind past the start is mpv's problem to clamp, not ours.
        jfn_mpv_seek_absolute(-3.0);
        assert_eq!(ops(), [command(&["seek", "-3", "absolute"])]);
    }

    #[test]
    fn volume_speed_and_the_two_delays_are_double_properties() {
        let _g = recorder::start();
        jfn_mpv_set_volume(85.0);
        assert_eq!(ops(), [double("volume", 85.0)]);
        jfn_mpv_set_speed(1.25);
        assert_eq!(ops(), [double("speed", 1.25)]);
        jfn_mpv_set_audio_delay(-0.125);
        assert_eq!(ops(), [double("audio-delay", -0.125)]);
        jfn_mpv_set_subtitle_delay(0.5);
        assert_eq!(ops(), [double("sub-delay", 0.5)]);
    }

    #[test]
    fn muting_writes_the_mute_flag() {
        let _g = recorder::start();
        jfn_mpv_set_muted(true);
        assert_eq!(ops(), [flag("mute", true)]);
        jfn_mpv_set_muted(false);
        assert_eq!(ops(), [flag("mute", false)]);
    }

    /// `start` is an option mpv applies to the next loaded file, so setting
    /// it is not a seek of the current one.
    #[test]
    fn the_start_position_is_a_double_property_not_a_seek() {
        let _g = recorder::start();
        jfn_mpv_set_start_position(300.0);
        assert_eq!(ops(), [double("start", 300.0)]);
    }

    /// mpv stamps the point from its own pts; a time sampled in the UI lags
    /// the core and would disarm the loop on the spot.
    #[test]
    fn the_ab_loop_step_is_mpvs_own_command_with_its_osd_suppressed() {
        let _g = recorder::start();
        jfn_mpv_ab_loop_cycle();
        assert_eq!(ops(), [command(&["no-osd", "ab-loop"])]);
    }

    /// A first, deliberately: both writes are observed separately, and
    /// `(no, b)` already draws as "no loop" where `(a, no)` would flash the
    /// A-only state.
    #[test]
    fn clearing_the_ab_loop_writes_the_sentinel_to_a_before_b() {
        let _g = recorder::start();
        jfn_mpv_clear_ab_loop();
        assert_eq!(
            ops(),
            [string("ab-loop-a", "no"), string("ab-loop-b", "no")]
        );
    }

    /// Track id 0 is the "disabled" sentinel and has to reach mpv as its
    /// string `no`, not as the number zero (which is a real track id there).
    #[test]
    fn track_zero_becomes_mpvs_no_and_every_other_id_its_number() {
        assert_eq!(track_to_mpv_str(0).to_bytes(), b"no");
        assert_eq!(track_to_mpv_str(1).to_bytes(), b"1");
        assert_eq!(track_to_mpv_str(17).to_bytes(), b"17");
    }

    #[test]
    fn selecting_an_audio_track_writes_aid() {
        let _g = recorder::start();
        jfn_mpv_set_audio_track(2);
        assert_eq!(ops(), [string("aid", "2")]);
        jfn_mpv_set_audio_track(0);
        assert_eq!(ops(), [string("aid", "no")]);
    }

    #[test]
    fn selecting_a_subtitle_track_writes_sid() {
        let _g = recorder::start();
        jfn_mpv_set_subtitle_track(3);
        assert_eq!(ops(), [string("sid", "3")]);
        jfn_mpv_set_subtitle_track(0);
        assert_eq!(ops(), [string("sid", "no")]);
    }

    #[test]
    fn adding_an_external_subtitle_selects_it_and_a_null_url_is_refused() {
        let _g = recorder::start();
        unsafe { jfn_mpv_sub_add(c"http://host/a.srt".as_ptr()) };
        assert_eq!(
            ops(),
            [command(&["sub-add", "http://host/a.srt", "select"])]
        );
        unsafe { jfn_mpv_sub_add(ptr::null()) };
        assert!(ops().is_empty());
    }

    #[test]
    fn adding_an_external_audio_track_selects_it_and_a_null_url_is_refused() {
        let _g = recorder::start();
        unsafe { jfn_mpv_audio_add(c"http://host/a.mka".as_ptr()) };
        assert_eq!(
            ops(),
            [command(&["audio-add", "http://host/a.mka", "select"])]
        );
        unsafe { jfn_mpv_audio_add(ptr::null()) };
        assert!(ops().is_empty());
    }

    // =====================================================================
    // LoadFile and the deferred track selection
    // =====================================================================

    /// Loaded paused with no track selectors: with `track-auto-selection=no`
    /// mpv drops aid/vid/sid out of loadfile options, so they are written
    /// afterwards instead.
    #[test]
    fn load_file_asks_for_a_paused_replace_at_the_start_position() {
        let _g = recorder::start();
        let path = CString::new("http://host/a.mkv").expect("cstring");
        let opts = load_options(12.5, 2, 3);
        unsafe { jfn_mpv_load_file(path.as_ptr(), &opts) };
        assert_eq!(
            ops(),
            [command(&[
                "loadfile",
                "http://host/a.mkv",
                "replace",
                "-1",
                "start=12.5,pause=yes",
            ])]
        );
    }

    #[test]
    fn load_file_needs_both_a_path_and_an_options_struct() {
        let _g = recorder::start();
        let path = CString::new("http://host/a.mkv").expect("cstring");
        let opts = load_options(0.0, 1, 1);
        unsafe { jfn_mpv_load_file(ptr::null(), &opts) };
        unsafe { jfn_mpv_load_file(path.as_ptr(), ptr::null()) };
        assert!(ops().is_empty());
    }

    #[test]
    fn the_load_option_string_starts_at_the_requested_second_and_pauses() {
        assert_eq!(load_file_options(0.0, false), "start=0,pause=yes");
        assert_eq!(load_file_options(61.25, false), "start=61.25,pause=yes");
        assert_eq!(
            load_file_options(0.0, true),
            "start=0,pause=yes,track-auto-selection=yes"
        );
    }

    /// The one case jellyfin-web cannot answer: an unprobed live stream with
    /// no chosen track and no external file.
    #[test]
    fn only_an_unprobed_live_stream_hands_the_audio_choice_back_to_mpv() {
        assert!(should_defer_audio(true, 0, ""));
        // A chosen track, an external file, or a normal file: ours to pick.
        assert!(!should_defer_audio(true, 2, ""));
        assert!(!should_defer_audio(true, 0, "http://host/a.mka"));
        assert!(!should_defer_audio(false, 0, ""));
    }

    /// The writes are FIFO on mpv's core thread, so `pause=false` last is
    /// what makes playback begin only once the track switches have landed.
    #[test]
    fn the_pending_tracks_are_written_before_playback_starts() {
        let _g = recorder::start();
        let path = CString::new("http://host/a.mkv").expect("cstring");
        let opts = load_options(0.0, 2, 3);
        unsafe { jfn_mpv_load_file(path.as_ptr(), &opts) };
        ops();

        jfn_mpv_apply_pending_track_selection_and_play();
        assert_eq!(
            ops(),
            [
                string("vid", "1"),
                string("aid", "2"),
                string("sid", "3"),
                flag("pause", false),
            ]
        );
    }

    /// FILE_LOADED can arrive for a file we did not load (a redirect, a
    /// leftover from the previous item); applying stale ids then would switch
    /// tracks under the running one.
    #[test]
    fn pending_tracks_are_applied_once_and_only_after_a_load() {
        let _g = recorder::start();
        // Consume whatever state is pending, then assert the second run is
        // silent.
        jfn_mpv_apply_pending_track_selection_and_play();
        ops();
        jfn_mpv_apply_pending_track_selection_and_play();
        assert!(ops().is_empty());
    }

    #[test]
    fn external_audio_and_subtitle_files_are_added_after_the_track_writes() {
        let _g = recorder::start();
        let path = CString::new("http://host/a.mkv").expect("cstring");
        let audio = CString::new("http://host/a.mka").expect("cstring");
        let sub = CString::new("http://host/a.srt").expect("cstring");
        let opts = JfnMpvLoadOptions {
            start_secs: 0.0,
            video_track: 1,
            audio_track: 0,
            sub_track: 0,
            external_audio_url: audio.as_ptr(),
            external_sub_url: sub.as_ptr(),
            is_infinite_stream: false,
        };
        unsafe { jfn_mpv_load_file(path.as_ptr(), &opts) };
        ops();

        jfn_mpv_apply_pending_track_selection_and_play();
        assert_eq!(
            ops(),
            [
                string("vid", "1"),
                string("aid", "no"),
                string("sid", "no"),
                command(&["audio-add", "http://host/a.mka", "select"]),
                command(&["sub-add", "http://host/a.srt", "select"]),
                flag("pause", false),
            ]
        );
    }

    /// mpv's demuxer already picked the audio track for this one, so writing
    /// `aid` would undo it; subtitles are still forced off.
    #[test]
    fn a_deferred_audio_load_skips_the_aid_write_but_still_disables_subs() {
        let _g = recorder::start();
        let path = CString::new("http://host/live.m3u8").expect("cstring");
        let opts = JfnMpvLoadOptions {
            start_secs: 0.0,
            video_track: 1,
            audio_track: 0,
            sub_track: 0,
            external_audio_url: ptr::null(),
            external_sub_url: ptr::null(),
            is_infinite_stream: true,
        };
        unsafe { jfn_mpv_load_file(path.as_ptr(), &opts) };
        assert_eq!(
            ops(),
            [command(&[
                "loadfile",
                "http://host/live.m3u8",
                "replace",
                "-1",
                "start=0,pause=yes,track-auto-selection=yes",
            ])]
        );

        jfn_mpv_apply_pending_track_selection_and_play();
        assert_eq!(
            ops(),
            [
                string("vid", "1"),
                string("sid", "no"),
                flag("pause", false)
            ]
        );
    }

    // =====================================================================
    // Aspect mode
    // =====================================================================

    #[test]
    fn each_aspect_mode_maps_to_a_keepaspect_and_panscan_pair() {
        assert_eq!(aspect_mode_options(b"auto"), Some((true, 0.0)));
        assert_eq!(aspect_mode_options(b"cover"), Some((true, 1.0)));
        assert_eq!(aspect_mode_options(b"fill"), Some((false, 0.0)));
    }

    #[test]
    fn an_unknown_aspect_mode_writes_nothing_at_all() {
        let _g = recorder::start();
        assert_eq!(aspect_mode_options(b""), None);
        assert_eq!(aspect_mode_options(b"Cover"), None);
        assert_eq!(aspect_mode_options(b"stretch"), None);
        unsafe { jfn_mpv_set_aspect_mode(c"stretch".as_ptr()) };
        unsafe { jfn_mpv_set_aspect_mode(ptr::null()) };
        assert!(ops().is_empty());
    }

    #[test]
    fn setting_the_aspect_mode_writes_both_properties() {
        let _g = recorder::start();
        unsafe { jfn_mpv_set_aspect_mode(c"cover".as_ptr()) };
        assert_eq!(ops(), [flag("keepaspect", true), double("panscan", 1.0)]);
        unsafe { jfn_mpv_set_aspect_mode(c"fill".as_ptr()) };
        assert_eq!(ops(), [flag("keepaspect", false), double("panscan", 0.0)]);
    }

    // =====================================================================
    // Window and display
    // =====================================================================

    #[test]
    fn fullscreen_is_a_flag_and_toggling_it_cycles_the_property() {
        let _g = recorder::start();
        jfn_mpv_set_fullscreen(true);
        assert_eq!(ops(), [flag("fullscreen", true)]);
        jfn_mpv_toggle_fullscreen();
        assert_eq!(ops(), [command(&["cycle", "fullscreen"])]);
    }

    #[test]
    fn minimising_and_maximising_write_their_window_flags() {
        let _g = recorder::start();
        jfn_mpv_set_window_minimized(true);
        assert_eq!(ops(), [flag("window-minimized", true)]);
        jfn_mpv_set_window_maximized(false);
        assert_eq!(ops(), [flag("window-maximized", false)]);
    }

    #[test]
    fn forcing_the_window_position_writes_its_flag() {
        let _g = recorder::start();
        jfn_mpv_set_force_window_position(true);
        assert_eq!(ops(), [flag("force-window-position", true)]);
    }

    #[test]
    fn the_geometry_string_is_passed_through_and_a_null_one_is_refused() {
        let _g = recorder::start();
        unsafe { jfn_mpv_set_geometry(c"1280x720+10+20".as_ptr()) };
        assert_eq!(ops(), [string("geometry", "1280x720+10+20")]);
        unsafe { jfn_mpv_set_geometry(ptr::null()) };
        assert!(ops().is_empty());
    }

    // =====================================================================
    // Background colour
    // =====================================================================

    #[test]
    fn a_background_colour_reply_is_parsed_and_anything_else_is_ignored() {
        let parsed = background_color_from_reply(&crate::PropertyValue::String("#010203".into()));
        assert_eq!(parsed, Some(crate::color::parse("#010203")));
        assert_ne!(parsed, Some(0), "a real colour must not parse as black");

        for other in [
            crate::PropertyValue::None,
            crate::PropertyValue::Int(1),
            crate::PropertyValue::Flag(true),
            crate::PropertyValue::Double(1.0),
        ] {
            assert_eq!(background_color_from_reply(&other), None, "{other:?}");
        }
    }

    /// mpv keeps `background-color` in the very option group `vo_gpu_next`
    /// watches, so an identical re-write costs a walk of the whole shader
    /// chain for nothing.
    #[test]
    fn the_same_background_colour_is_not_written_twice_in_a_row() {
        let _g = recorder::start();
        let first = c"#FF102030";
        let second = c"#FF405060";
        unsafe { jfn_mpv_set_background_color_hex(first.as_ptr()) };
        unsafe { jfn_mpv_set_background_color_hex(first.as_ptr()) };
        unsafe { jfn_mpv_set_background_color_hex(second.as_ptr()) };
        unsafe { jfn_mpv_set_background_color_hex(first.as_ptr()) };
        assert_eq!(
            ops(),
            [
                string("background-color", "#FF102030"),
                string("background-color", "#FF405060"),
                string("background-color", "#FF102030"),
            ]
        );
    }

    #[test]
    fn a_null_background_colour_is_refused_and_leaves_the_memo_alone() {
        let _g = recorder::start();
        let colour = c"#FF0A0B0C";
        unsafe { jfn_mpv_set_background_color_hex(colour.as_ptr()) };
        ops();
        unsafe { jfn_mpv_set_background_color_hex(ptr::null()) };
        assert!(ops().is_empty());
        unsafe { jfn_mpv_set_background_color_hex(colour.as_ptr()) };
        assert!(ops().is_empty(), "the memo still holds the last colour");
    }

    /// Without a handle there is nothing to read, and the memo of what mpv is
    /// running must survive: clearing it here would make the next write of
    /// the same colour a needless round trip.
    #[test]
    fn requesting_the_background_colour_without_a_handle_forgets_nothing() {
        let _g = recorder::start();
        let colour = c"#FF0D0E0F";
        unsafe { jfn_mpv_set_background_color_hex(colour.as_ptr()) };
        assert_eq!(ops(), [string("background-color", "#FF0D0E0F")]);
        jfn_mpv_request_background_color();
        unsafe { jfn_mpv_set_background_color_hex(colour.as_ptr()) };
        assert!(ops().is_empty());
    }
}
