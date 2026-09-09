//! Owned event types decoded from `mpv_event`.
//!
//! Raw `mpv_event` payloads (returned by `mpv_wait_event`) are only valid
//! until the next `mpv_wait_event` call on the same handle. `Event::from_raw`
//! copies the data out so the caller can drop the loan immediately.

use crate::log::LogLevel;
use crate::node::Node;
use crate::sys;
use std::ffi::CStr;

/// User-assigned ID passed as `reply_userdata` to
/// `mpv_observe_property`. Re-emitted on `Event::PropertyChange` so callers
/// can dispatch without string-comparing property names.
pub type ObserveId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum EndFileReason {
    Eof,
    Stop,
    Quit,
    Error(crate::error::Error),
    Redirect,
    Unknown(i32),
}

impl EndFileReason {
    fn from_raw(reason: sys::mpv_end_file_reason, error: i32) -> Self {
        match reason {
            sys::mpv_end_file_reason::MPV_END_FILE_REASON_EOF => Self::Eof,
            sys::mpv_end_file_reason::MPV_END_FILE_REASON_STOP => Self::Stop,
            sys::mpv_end_file_reason::MPV_END_FILE_REASON_QUIT => Self::Quit,
            sys::mpv_end_file_reason::MPV_END_FILE_REASON_ERROR => {
                Self::Error(crate::error::Error::new(error))
            }
            sys::mpv_end_file_reason::MPV_END_FILE_REASON_REDIRECT => Self::Redirect,
            // The bindgen newtype wraps c_int on MSVC but c_uint on unix
            // targets, so the cast is required on one and a no-op on the other.
            #[allow(clippy::unnecessary_cast)]
            other => Self::Unknown(other.0 as i32),
        }
    }
}

/// Property-change payload, format-typed. Mirrors what
/// `mpv_event_property::data` decodes to under each `mpv_format`.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    None,
    Flag(bool),
    Int(i64),
    Double(f64),
    String(String),
    Node(Node),
}

impl PropertyValue {
    /// # Safety
    /// `p` must point to a valid `mpv_event_property` whose `data` (if
    /// non-null) matches the declared `format`.
    pub unsafe fn from_raw(p: *const sys::mpv_event_property) -> Self {
        if p.is_null() {
            return Self::None;
        }
        let p = unsafe { &*p };
        if p.data.is_null() {
            return Self::None;
        }
        match p.format {
            sys::mpv_format::MPV_FORMAT_FLAG => Self::Flag(unsafe { *(p.data as *const i32) } != 0),
            sys::mpv_format::MPV_FORMAT_INT64 => Self::Int(unsafe { *(p.data as *const i64) }),
            sys::mpv_format::MPV_FORMAT_DOUBLE => Self::Double(unsafe { *(p.data as *const f64) }),
            sys::mpv_format::MPV_FORMAT_STRING => unsafe {
                let pp = p.data as *const *const std::os::raw::c_char;
                let s = *pp;
                if s.is_null() {
                    Self::String(String::new())
                } else {
                    Self::String(CStr::from_ptr(s).to_string_lossy().into_owned())
                }
            },
            sys::mpv_format::MPV_FORMAT_NODE => {
                Self::Node(unsafe { Node::from_raw(p.data as *const sys::mpv_node) })
            }
            _ => Self::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogMessage {
    pub prefix: String,
    pub level: LogLevel,
    pub text: String,
}

/// Reply identifier carried by async command/property events.
pub type ReplyUserdata = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `MPV_EVENT_NONE` — emitted on timeout from `wait_event`.
    None,
    Shutdown,
    LogMessage(LogMessage),
    GetPropertyReply {
        reply: ReplyUserdata,
        error: i32,
        value: PropertyValue,
        name: String,
    },
    SetPropertyReply {
        reply: ReplyUserdata,
        error: i32,
    },
    CommandReply {
        reply: ReplyUserdata,
        error: i32,
    },
    StartFile,
    EndFile(EndFileReason),
    FileLoaded,
    ClientMessage(Vec<String>),
    VideoReconfig,
    AudioReconfig,
    Seek,
    PlaybackRestart,
    PropertyChange {
        id: ObserveId,
        name: String,
        value: PropertyValue,
    },
    QueueOverflow,
    Hook {
        reply: ReplyUserdata,
        name: String,
    },
    Other(u32),
}

impl Event {
    /// Decode a raw `mpv_event` borrowed from libmpv into an owned `Event`.
    ///
    /// # Safety
    /// `ev` must reference a valid `mpv_event` returned by `mpv_wait_event`.
    /// All borrowed pointers are copied; the caller may invoke
    /// `mpv_wait_event` again immediately after this returns.
    #[allow(clippy::unnecessary_cast)] // mpv_event_id.0 is i32 on windows, u32 on linux
    pub unsafe fn from_raw(ev: *const sys::mpv_event) -> Self {
        if ev.is_null() {
            return Event::None;
        }
        let ev = unsafe { &*ev };
        match ev.event_id {
            sys::mpv_event_id::MPV_EVENT_NONE => Event::None,
            sys::mpv_event_id::MPV_EVENT_SHUTDOWN => Event::Shutdown,
            sys::mpv_event_id::MPV_EVENT_LOG_MESSAGE => unsafe {
                let m = &*(ev.data as *const sys::mpv_event_log_message);
                Event::LogMessage(LogMessage {
                    prefix: cstr_to_string(m.prefix),
                    level: LogLevel::from_raw(m.log_level),
                    text: cstr_to_string(m.text),
                })
            },
            sys::mpv_event_id::MPV_EVENT_GET_PROPERTY_REPLY => unsafe {
                let p = ev.data as *const sys::mpv_event_property;
                let name = if p.is_null() {
                    String::new()
                } else {
                    cstr_to_string((*p).name)
                };
                Event::GetPropertyReply {
                    reply: ev.reply_userdata,
                    error: ev.error,
                    value: PropertyValue::from_raw(p),
                    name,
                }
            },
            sys::mpv_event_id::MPV_EVENT_SET_PROPERTY_REPLY => Event::SetPropertyReply {
                reply: ev.reply_userdata,
                error: ev.error,
            },
            sys::mpv_event_id::MPV_EVENT_COMMAND_REPLY => Event::CommandReply {
                reply: ev.reply_userdata,
                error: ev.error,
            },
            sys::mpv_event_id::MPV_EVENT_START_FILE => Event::StartFile,
            sys::mpv_event_id::MPV_EVENT_END_FILE => unsafe {
                let d = &*(ev.data as *const sys::mpv_event_end_file);
                Event::EndFile(EndFileReason::from_raw(d.reason, d.error))
            },
            sys::mpv_event_id::MPV_EVENT_FILE_LOADED => Event::FileLoaded,
            sys::mpv_event_id::MPV_EVENT_CLIENT_MESSAGE => unsafe {
                let m = &*(ev.data as *const sys::mpv_event_client_message);
                let mut args = Vec::with_capacity(m.num_args.max(0) as usize);
                for i in 0..m.num_args {
                    let p = *m.args.offset(i as isize);
                    args.push(cstr_to_string(p));
                }
                Event::ClientMessage(args)
            },
            sys::mpv_event_id::MPV_EVENT_VIDEO_RECONFIG => Event::VideoReconfig,
            sys::mpv_event_id::MPV_EVENT_AUDIO_RECONFIG => Event::AudioReconfig,
            sys::mpv_event_id::MPV_EVENT_SEEK => Event::Seek,
            sys::mpv_event_id::MPV_EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
            sys::mpv_event_id::MPV_EVENT_PROPERTY_CHANGE => unsafe {
                let p = ev.data as *const sys::mpv_event_property;
                let name = if p.is_null() {
                    String::new()
                } else {
                    cstr_to_string((*p).name)
                };
                Event::PropertyChange {
                    id: ev.reply_userdata,
                    name,
                    value: PropertyValue::from_raw(p),
                }
            },
            sys::mpv_event_id::MPV_EVENT_QUEUE_OVERFLOW => Event::QueueOverflow,
            sys::mpv_event_id::MPV_EVENT_HOOK => unsafe {
                let h = &*(ev.data as *const sys::mpv_event_hook);
                Event::Hook {
                    reply: h.id,
                    name: cstr_to_string(h.name),
                }
            },
            other => Event::Other(other.0 as u32),
        }
    }
}

unsafe fn cstr_to_string(p: *const std::os::raw::c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    fn event(id: sys::mpv_event_id) -> sys::mpv_event {
        sys::mpv_event {
            event_id: id,
            error: 0,
            reply_userdata: 0,
            data: std::ptr::null_mut(),
        }
    }

    fn property(
        name: &CString,
        format: sys::mpv_format,
        data: *mut c_void,
    ) -> sys::mpv_event_property {
        sys::mpv_event_property {
            name: name.as_ptr(),
            format,
            data,
        }
    }

    // ---- PropertyValue::from_raw ------------------------------------------

    #[test]
    fn a_null_property_pointer_decodes_to_no_value() {
        assert_eq!(
            unsafe { PropertyValue::from_raw(std::ptr::null()) },
            PropertyValue::None
        );
    }

    /// mpv sends a payload-less property change when the property became
    /// unavailable (no file loaded, for instance).
    #[test]
    fn a_property_with_no_payload_decodes_to_no_value() {
        let name = CString::new("time-pos").expect("cstring");
        let p = property(
            &name,
            sys::mpv_format::MPV_FORMAT_DOUBLE,
            std::ptr::null_mut(),
        );
        assert_eq!(unsafe { PropertyValue::from_raw(&p) }, PropertyValue::None);
    }

    #[test]
    fn a_flag_property_decodes_any_non_zero_as_true() {
        let name = CString::new("pause").expect("cstring");
        for (raw, expected) in [(0i32, false), (1, true), (-1, true), (42, true)] {
            let mut v = raw;
            let p = property(
                &name,
                sys::mpv_format::MPV_FORMAT_FLAG,
                &mut v as *mut _ as *mut c_void,
            );
            assert_eq!(
                unsafe { PropertyValue::from_raw(&p) },
                PropertyValue::Flag(expected),
                "raw {raw}"
            );
        }
    }

    #[test]
    fn int_and_double_properties_decode_by_format() {
        let name = CString::new("chapter").expect("cstring");
        let mut i = -3i64;
        let p = property(
            &name,
            sys::mpv_format::MPV_FORMAT_INT64,
            &mut i as *mut _ as *mut c_void,
        );
        assert_eq!(
            unsafe { PropertyValue::from_raw(&p) },
            PropertyValue::Int(-3)
        );

        let mut d = 1.25f64;
        let p = property(
            &name,
            sys::mpv_format::MPV_FORMAT_DOUBLE,
            &mut d as *mut _ as *mut c_void,
        );
        assert_eq!(
            unsafe { PropertyValue::from_raw(&p) },
            PropertyValue::Double(1.25)
        );
    }

    /// A STRING payload is a pointer *to* the char pointer, and libmpv may
    /// leave that inner pointer null.
    #[test]
    fn a_string_property_is_copied_out_and_a_null_inner_pointer_is_empty() {
        let name = CString::new("ab-loop-a").expect("cstring");
        let value = CString::new("no").expect("cstring");
        let mut inner: *const c_char = value.as_ptr();
        let p = property(
            &name,
            sys::mpv_format::MPV_FORMAT_STRING,
            &mut inner as *mut _ as *mut c_void,
        );
        assert_eq!(
            unsafe { PropertyValue::from_raw(&p) },
            PropertyValue::String("no".into())
        );

        let mut null_inner: *const c_char = std::ptr::null();
        let p = property(
            &name,
            sys::mpv_format::MPV_FORMAT_STRING,
            &mut null_inner as *mut _ as *mut c_void,
        );
        assert_eq!(
            unsafe { PropertyValue::from_raw(&p) },
            PropertyValue::String(String::new())
        );
    }

    #[test]
    fn a_node_property_is_deep_copied_into_an_owned_node() {
        let name = CString::new("demuxer-cache-state").expect("cstring");
        let mut node: sys::mpv_node = unsafe { std::mem::zeroed() };
        node.format = sys::mpv_format::MPV_FORMAT_INT64;
        node.u.int64 = 7;
        let p = property(
            &name,
            sys::mpv_format::MPV_FORMAT_NODE,
            &mut node as *mut _ as *mut c_void,
        );
        assert_eq!(
            unsafe { PropertyValue::from_raw(&p) },
            PropertyValue::Node(Node::Int(7))
        );
    }

    #[test]
    fn a_property_format_we_do_not_handle_decodes_to_no_value() {
        let name = CString::new("x").expect("cstring");
        let mut byte = 0u8;
        let p = property(
            &name,
            sys::mpv_format(9999),
            &mut byte as *mut _ as *mut c_void,
        );
        assert_eq!(unsafe { PropertyValue::from_raw(&p) }, PropertyValue::None);
    }

    // ---- Event::from_raw --------------------------------------------------

    #[test]
    fn a_null_event_pointer_decodes_to_none() {
        assert_eq!(unsafe { Event::from_raw(std::ptr::null()) }, Event::None);
    }

    /// The payload-free ids are pure tags; a mix-up between any two of them
    /// would silently reroute the whole playback state machine.
    #[test]
    fn the_payload_free_ids_map_one_to_one() {
        for (id, expected) in [
            (sys::mpv_event_id::MPV_EVENT_NONE, Event::None),
            (sys::mpv_event_id::MPV_EVENT_SHUTDOWN, Event::Shutdown),
            (sys::mpv_event_id::MPV_EVENT_START_FILE, Event::StartFile),
            (sys::mpv_event_id::MPV_EVENT_FILE_LOADED, Event::FileLoaded),
            (
                sys::mpv_event_id::MPV_EVENT_VIDEO_RECONFIG,
                Event::VideoReconfig,
            ),
            (
                sys::mpv_event_id::MPV_EVENT_AUDIO_RECONFIG,
                Event::AudioReconfig,
            ),
            (sys::mpv_event_id::MPV_EVENT_SEEK, Event::Seek),
            (
                sys::mpv_event_id::MPV_EVENT_PLAYBACK_RESTART,
                Event::PlaybackRestart,
            ),
            (
                sys::mpv_event_id::MPV_EVENT_QUEUE_OVERFLOW,
                Event::QueueOverflow,
            ),
        ] {
            let ev = event(id);
            assert_eq!(unsafe { Event::from_raw(&ev) }, expected, "{id:?}");
        }
    }

    #[test]
    fn an_event_id_this_build_does_not_know_keeps_its_number() {
        let ev = event(sys::mpv_event_id(9001));
        assert_eq!(unsafe { Event::from_raw(&ev) }, Event::Other(9001));
    }

    #[test]
    fn a_log_message_carries_prefix_level_and_text() {
        let prefix = CString::new("cplayer").expect("cstring");
        let level = CString::new("v").expect("cstring");
        let text = CString::new("Playing: x.mkv\n").expect("cstring");
        let mut payload = sys::mpv_event_log_message {
            prefix: prefix.as_ptr(),
            level: level.as_ptr(),
            text: text.as_ptr(),
            log_level: sys::mpv_log_level::MPV_LOG_LEVEL_V,
        };
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_LOG_MESSAGE);
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::LogMessage(LogMessage {
                prefix: "cplayer".into(),
                level: LogLevel::Verbose,
                // Trimming belongs to the forwarder, not to the decoder.
                text: "Playing: x.mkv\n".into(),
            })
        );
    }

    #[test]
    fn a_log_message_with_null_strings_decodes_to_empty_ones() {
        let mut payload = sys::mpv_event_log_message {
            prefix: std::ptr::null(),
            level: std::ptr::null(),
            text: std::ptr::null(),
            log_level: sys::mpv_log_level::MPV_LOG_LEVEL_NONE,
        };
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_LOG_MESSAGE);
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::LogMessage(LogMessage {
                prefix: String::new(),
                level: LogLevel::Off,
                text: String::new(),
            })
        );
    }

    #[test]
    fn a_get_property_reply_carries_the_reply_id_name_error_and_value() {
        let name = CString::new("background-color").expect("cstring");
        let value = CString::new("#FF000000").expect("cstring");
        let mut inner: *const c_char = value.as_ptr();
        let mut payload = property(
            &name,
            sys::mpv_format::MPV_FORMAT_STRING,
            &mut inner as *mut _ as *mut c_void,
        );
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_GET_PROPERTY_REPLY);
        ev.reply_userdata = 5;
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::GetPropertyReply {
                reply: 5,
                error: 0,
                value: PropertyValue::String("#FF000000".into()),
                name: "background-color".into(),
            }
        );
    }

    /// A failed async read arrives with a negative `error` and no payload at
    /// all; the name must still not be invented from a null pointer.
    #[test]
    fn a_failed_get_property_reply_keeps_its_error_and_an_empty_name() {
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_GET_PROPERTY_REPLY);
        ev.reply_userdata = 2;
        ev.error = -8;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::GetPropertyReply {
                reply: 2,
                error: -8,
                value: PropertyValue::None,
                name: String::new(),
            }
        );
    }

    #[test]
    fn set_property_and_command_replies_carry_only_the_id_and_error() {
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_SET_PROPERTY_REPLY);
        ev.reply_userdata = 11;
        ev.error = -4;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::SetPropertyReply {
                reply: 11,
                error: -4
            }
        );

        let mut ev = event(sys::mpv_event_id::MPV_EVENT_COMMAND_REPLY);
        ev.reply_userdata = 12;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::CommandReply {
                reply: 12,
                error: 0
            }
        );
    }

    #[test]
    fn end_file_maps_every_reason_libmpv_defines() {
        for (raw, expected) in [
            (
                sys::mpv_end_file_reason::MPV_END_FILE_REASON_EOF,
                EndFileReason::Eof,
            ),
            (
                sys::mpv_end_file_reason::MPV_END_FILE_REASON_STOP,
                EndFileReason::Stop,
            ),
            (
                sys::mpv_end_file_reason::MPV_END_FILE_REASON_QUIT,
                EndFileReason::Quit,
            ),
            (
                sys::mpv_end_file_reason::MPV_END_FILE_REASON_REDIRECT,
                EndFileReason::Redirect,
            ),
        ] {
            let mut payload: sys::mpv_event_end_file = unsafe { std::mem::zeroed() };
            payload.reason = raw;
            let mut ev = event(sys::mpv_event_id::MPV_EVENT_END_FILE);
            ev.data = &mut payload as *mut _ as *mut c_void;
            assert_eq!(
                unsafe { Event::from_raw(&ev) },
                Event::EndFile(expected),
                "{raw:?}"
            );
        }
    }

    /// Only the ERROR reason carries a code, and it has to survive to the UI:
    /// that is what turns a dead stream into a message rather than an EOF.
    #[test]
    fn end_file_with_an_error_keeps_the_error_code() {
        let mut payload: sys::mpv_event_end_file = unsafe { std::mem::zeroed() };
        payload.reason = sys::mpv_end_file_reason::MPV_END_FILE_REASON_ERROR;
        payload.error = -13;
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_END_FILE);
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::EndFile(EndFileReason::Error(crate::error::Error::new(-13)))
        );
    }

    #[test]
    fn an_unknown_end_file_reason_keeps_its_number() {
        let mut payload: sys::mpv_event_end_file = unsafe { std::mem::zeroed() };
        payload.reason = sys::mpv_end_file_reason(77);
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_END_FILE);
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::EndFile(EndFileReason::Unknown(77))
        );
    }

    #[test]
    fn a_client_message_copies_every_argument_in_order() {
        let a = CString::new("key-binding").expect("cstring");
        let b = CString::new("play").expect("cstring");
        let mut args: Vec<*const c_char> = vec![a.as_ptr(), b.as_ptr()];
        let mut payload = sys::mpv_event_client_message {
            num_args: 2,
            args: args.as_mut_ptr(),
        };
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_CLIENT_MESSAGE);
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::ClientMessage(vec!["key-binding".into(), "play".into()])
        );
    }

    #[test]
    fn a_client_message_with_no_arguments_decodes_to_an_empty_list() {
        for num_args in [0, -1] {
            let mut payload = sys::mpv_event_client_message {
                num_args,
                args: std::ptr::null_mut(),
            };
            let mut ev = event(sys::mpv_event_id::MPV_EVENT_CLIENT_MESSAGE);
            ev.data = &mut payload as *mut _ as *mut c_void;
            assert_eq!(
                unsafe { Event::from_raw(&ev) },
                Event::ClientMessage(Vec::new()),
                "num_args {num_args}"
            );
        }
    }

    /// The observe id is what lets a consumer dispatch without comparing
    /// property names, so it must come through untouched beside the name.
    #[test]
    fn a_property_change_carries_the_observe_id_the_name_and_the_value() {
        let name = CString::new("pause").expect("cstring");
        let mut flag = 1i32;
        let mut payload = property(
            &name,
            sys::mpv_format::MPV_FORMAT_FLAG,
            &mut flag as *mut _ as *mut c_void,
        );
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_PROPERTY_CHANGE);
        ev.reply_userdata = 42;
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::PropertyChange {
                id: 42,
                name: "pause".into(),
                value: PropertyValue::Flag(true),
            }
        );
    }

    #[test]
    fn a_property_change_with_no_payload_still_names_the_property() {
        let name = CString::new("time-pos").expect("cstring");
        let mut payload = property(
            &name,
            sys::mpv_format::MPV_FORMAT_DOUBLE,
            std::ptr::null_mut(),
        );
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_PROPERTY_CHANGE);
        ev.reply_userdata = 9;
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::PropertyChange {
                id: 9,
                name: "time-pos".into(),
                value: PropertyValue::None,
            }
        );
    }

    /// A hook takes its reply id from the payload, not from `reply_userdata`;
    /// answering with the wrong one stalls mpv's playback loop.
    #[test]
    fn a_hook_takes_its_reply_id_from_the_payload() {
        let name = CString::new("on_load").expect("cstring");
        let mut payload = sys::mpv_event_hook {
            name: name.as_ptr(),
            id: 314,
        };
        let mut ev = event(sys::mpv_event_id::MPV_EVENT_HOOK);
        ev.reply_userdata = 1;
        ev.data = &mut payload as *mut _ as *mut c_void;
        assert_eq!(
            unsafe { Event::from_raw(&ev) },
            Event::Hook {
                reply: 314,
                name: "on_load".into(),
            }
        );
    }
}
