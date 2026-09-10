//! The per-user value spliced into the Windows pipe name (see
//! `crate::pipe_name`). Nothing here is a decision — the decisions are the
//! sanitising and the name shape, which are pure and tested in `lib.rs`; this
//! is only the token lookup, which needs a real process token.

use std::ffi::c_void;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The current user's SID in string form (`S-1-5-21-…-1001`), falling back to
/// `%USERNAME%` and then to an empty string, which the caller turns into a
/// constant. Both fallbacks are weaker — a user name is not unique across
/// domains — but every one of them is stable for the life of the login, which
/// is what the name needs: the second instance has to derive the same string
/// as the first.
pub(crate) fn current_user_key() -> String {
    if let Some(sid) = current_user_sid() {
        return sid;
    }
    std::env::var("USERNAME").unwrap_or_default()
}

fn current_user_sid() -> Option<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
    // close, and `token` is a live out-parameter of the right type.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    // SAFETY: `token` was just opened and is closed exactly once below.
    let sid = unsafe { token_user_sid(token) };
    // SAFETY: same handle, not used after this point.
    unsafe { CloseHandle(token) };
    sid
}

/// # Safety
/// `token` must be an open process token with `TOKEN_QUERY` access.
unsafe fn token_user_sid(token: HANDLE) -> Option<String> {
    let mut needed: u32 = 0;
    // The documented two-call shape: the first call always fails and reports
    // the buffer size it wanted.
    // SAFETY: a null buffer of length zero is what the size query takes.
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    // `TOKEN_USER` holds a pointer, so the buffer has to be pointer-aligned; a
    // `Vec<u8>` is only byte-aligned and reading the struct out of it would be
    // undefined. A `Vec<u64>` is aligned for every field on both x64 and arm64.
    let words = (needed as usize).div_ceil(size_of::<u64>());
    let mut buf = vec![0u64; words];
    let Ok(len) = u32::try_from(words * size_of::<u64>()) else {
        return None;
    };
    let mut written = len;
    // SAFETY: `buf` is `len` bytes long and suitably aligned for TOKEN_USER.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            len,
            &mut written,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: on success the call wrote a TOKEN_USER at the start of the
    // buffer, whose `Sid` points into that same allocation and outlives the
    // string conversion below.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    unsafe { sid_to_string(user.User.Sid) }
}

/// # Safety
/// `sid` must be a valid SID that stays alive for the length of the call.
unsafe fn sid_to_string(sid: PSID) -> Option<String> {
    let mut raw: *mut u16 = ptr::null_mut();
    // SAFETY: `raw` is a live out-parameter; on success it owns a
    // LocalAlloc'd, NUL-terminated wide string.
    if unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 || raw.is_null() {
        return None;
    }
    let mut len = 0usize;
    // SAFETY: the buffer is NUL-terminated by contract.
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units of a live buffer, read before it is freed.
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
    // SAFETY: the string was allocated by `ConvertSidToStringSidW` and is not
    // used again.
    unsafe { LocalFree(raw.cast::<c_void>()) };
    Some(text)
}
