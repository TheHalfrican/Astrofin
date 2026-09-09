//! Helper for assembling NUL-terminated command argument arrays.
//!
//! libmpv's `mpv_command` / `mpv_command_async` take a `const char*[]`
//! terminated by a null pointer. This module owns the `CString` storage so
//! the pointers remain valid until the call returns.

use std::ffi::{CString, NulError};
use std::os::raw::c_char;

/// Owned command argument vector. Borrow `as_ptrs()` to obtain the
/// null-terminated pointer array libmpv expects.
pub struct Command {
    storage: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl Command {
    pub fn new<I, S>(args: I) -> Result<Self, NulError>
    where
        I: IntoIterator<Item = S>,
        S: Into<Vec<u8>>,
    {
        let storage: Vec<CString> = args
            .into_iter()
            .map(|s| CString::new(s.into()))
            .collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = storage.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        Ok(Self { storage, ptrs })
    }

    pub fn as_ptr(&self) -> *mut *const c_char {
        self.ptrs.as_ptr() as *mut _
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }
}

// SAFETY: `Command` is logically a `Vec<CString>` plus its derived pointer
// table. Both halves are owned by the struct. The raw pointers in `ptrs`
// point into `storage`, which has stable addresses because `CString::as_ptr`
// refers to heap memory not the `CString` itself. Moving the `Command`
// preserves those addresses.
unsafe impl Send for Command {}
unsafe impl Sync for Command {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    /// Read the argv back the way libmpv does: walk pointers until the NULL.
    fn read_back(cmd: &Command) -> Vec<String> {
        let mut out = Vec::new();
        let mut p = cmd.as_ptr();
        loop {
            let s = unsafe { *p };
            if s.is_null() {
                break;
            }
            out.push(unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned());
            p = unsafe { p.add(1) };
        }
        out
    }

    #[test]
    fn new_keeps_the_arguments_in_order() {
        let cmd = Command::new(["loadfile", "http://host/a.mkv", "replace"]).unwrap();
        assert_eq!(
            read_back(&cmd),
            ["loadfile", "http://host/a.mkv", "replace"]
        );
    }

    #[test]
    fn new_rejects_an_argument_with_an_interior_nul() {
        assert!(Command::new(["seek", "1\0 2"]).is_err());
    }

    #[test]
    fn new_accepts_an_empty_argument_list() {
        let cmd = Command::new(Vec::<&str>::new()).unwrap();
        assert!(cmd.is_empty());
        assert_eq!(cmd.len(), 0);
        assert!(read_back(&cmd).is_empty());
    }

    /// libmpv reads the table until it hits the NULL; `len` must not count it.
    #[test]
    fn as_ptr_yields_a_null_terminated_table_one_longer_than_len() {
        let cmd = Command::new(["cycle", "pause"]).unwrap();
        assert_eq!(cmd.len(), 2);
        let end = unsafe { *cmd.as_ptr().add(cmd.len()) };
        assert!(end.is_null(), "argv must be NULL-terminated");
    }

    #[test]
    fn is_empty_is_false_as_soon_as_there_is_one_argument() {
        let cmd = Command::new(["stop"]).unwrap();
        assert!(!cmd.is_empty());
        assert_eq!(cmd.len(), 1);
    }

    /// The pointers point into heap storage the `Command` owns, so moving it
    /// (as `SAFETY` on the `Send` impl claims) leaves them valid.
    #[test]
    fn pointers_survive_moving_the_command() {
        let cmd = Command::new(["sub-add", "/tmp/x.srt", "select"]).unwrap();
        let moved = cmd;
        assert_eq!(read_back(&moved), ["sub-add", "/tmp/x.srt", "select"]);
    }
}
