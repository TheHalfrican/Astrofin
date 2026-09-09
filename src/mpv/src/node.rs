//! Owned Rust mirror of `mpv_node`.
//!
//! libmpv hands out `mpv_node` trees via `mpv_get_property(..., MPV_FORMAT_NODE)`
//! and `mpv_event_property` payloads. Both forms reference memory owned by
//! libmpv that must be freed with `mpv_free_node_contents`. Rather than carry
//! that lifetime, `Node::from_raw` copies the tree into owned Rust values so
//! the caller can drop libmpv's allocation immediately.

use crate::sys;
use std::ffi::CStr;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    None,
    String(String),
    Flag(bool),
    Int(i64),
    Double(f64),
    Array(NodeArray),
    Map(NodeMap),
    ByteArray(Vec<u8>),
}

pub type NodeArray = Vec<Node>;
pub type NodeMap = Vec<(String, Node)>;

impl Node {
    /// Deep-copy a raw `mpv_node` (from libmpv) into an owned `Node`. The
    /// caller still owns the raw node and must free it via
    /// `mpv_free_node_contents` if libmpv handed it out.
    ///
    /// # Safety
    /// `raw` must point to a valid `mpv_node` as produced by libmpv.
    pub unsafe fn from_raw(raw: *const sys::mpv_node) -> Self {
        if raw.is_null() {
            return Node::None;
        }
        let raw = unsafe { &*raw };
        match raw.format {
            sys::mpv_format::MPV_FORMAT_NONE => Node::None,
            sys::mpv_format::MPV_FORMAT_STRING => {
                let s = unsafe { raw.u.string };
                if s.is_null() {
                    Node::String(String::new())
                } else {
                    Node::String(unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned())
                }
            }
            sys::mpv_format::MPV_FORMAT_FLAG => Node::Flag(unsafe { raw.u.flag } != 0),
            sys::mpv_format::MPV_FORMAT_INT64 => Node::Int(unsafe { raw.u.int64 }),
            sys::mpv_format::MPV_FORMAT_DOUBLE => Node::Double(unsafe { raw.u.double_ }),
            sys::mpv_format::MPV_FORMAT_NODE_ARRAY => unsafe {
                let list = raw.u.list;
                if list.is_null() {
                    return Node::Array(Vec::new());
                }
                let l = &*list;
                let mut out = Vec::with_capacity(l.num.max(0) as usize);
                for i in 0..l.num {
                    let v = l.values.add(i as usize);
                    out.push(Node::from_raw(v));
                }
                Node::Array(out)
            },
            sys::mpv_format::MPV_FORMAT_NODE_MAP => unsafe {
                let list = raw.u.list;
                if list.is_null() {
                    return Node::Map(Vec::new());
                }
                let l = &*list;
                let mut out = Vec::with_capacity(l.num.max(0) as usize);
                for i in 0..l.num {
                    let k = *l.keys.add(i as usize);
                    let key = if k.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(k).to_string_lossy().into_owned()
                    };
                    let v = l.values.add(i as usize);
                    out.push((key, Node::from_raw(v)));
                }
                Node::Map(out)
            },
            sys::mpv_format::MPV_FORMAT_BYTE_ARRAY => unsafe {
                let ba = raw.u.ba;
                if ba.is_null() {
                    return Node::ByteArray(Vec::new());
                }
                let b = &*ba;
                if b.data.is_null() || b.size == 0 {
                    return Node::ByteArray(Vec::new());
                }
                let slice = std::slice::from_raw_parts(b.data as *const u8, b.size);
                Node::ByteArray(slice.to_vec())
            },
            _ => Node::None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Node::String(s) = self {
            Some(s)
        } else {
            None
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        if let Node::Int(v) = self {
            Some(*v)
        } else {
            None
        }
    }

    pub fn as_double(&self) -> Option<f64> {
        if let Node::Double(v) = self {
            Some(*v)
        } else {
            None
        }
    }

    pub fn as_flag(&self) -> Option<bool> {
        if let Node::Flag(v) = self {
            Some(*v)
        } else {
            None
        }
    }

    pub fn as_array(&self) -> Option<&NodeArray> {
        if let Node::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }

    pub fn as_map(&self) -> Option<&NodeMap> {
        if let Node::Map(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Lookup a key in a `Node::Map`. Returns `None` for non-maps or missing keys.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys;
    use std::ffi::CString;

    fn raw_none() -> sys::mpv_node {
        let mut n: sys::mpv_node = unsafe { std::mem::zeroed() };
        n.format = sys::mpv_format::MPV_FORMAT_NONE;
        n
    }

    fn raw_int(v: i64) -> sys::mpv_node {
        let mut n = raw_none();
        n.format = sys::mpv_format::MPV_FORMAT_INT64;
        n.u.int64 = v;
        n
    }

    fn raw_double(v: f64) -> sys::mpv_node {
        let mut n = raw_none();
        n.format = sys::mpv_format::MPV_FORMAT_DOUBLE;
        n.u.double_ = v;
        n
    }

    fn raw_flag(v: bool) -> sys::mpv_node {
        let mut n = raw_none();
        n.format = sys::mpv_format::MPV_FORMAT_FLAG;
        n.u.flag = if v { 1 } else { 0 };
        n
    }

    #[test]
    fn null_pointer_decodes_to_none() {
        let n = unsafe { Node::from_raw(std::ptr::null()) };
        assert_eq!(n, Node::None);
    }

    #[test]
    fn scalar_formats_round_trip() {
        let n = raw_int(42);
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::Int(42));
        let n = raw_double(2.5);
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::Double(2.5));
        let n = raw_flag(true);
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::Flag(true));
        let n = raw_flag(false);
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::Flag(false));
    }

    #[test]
    fn map_decodes_keys_and_values() -> Result<(), std::ffi::NulError> {
        // { "w": 1920, "h": 1080 }
        let mut values = vec![raw_int(1920), raw_int(1080)];
        let key_w = CString::new("w")?;
        let key_h = CString::new("h")?;
        let mut keys: Vec<*mut std::os::raw::c_char> =
            vec![key_w.as_ptr() as *mut _, key_h.as_ptr() as *mut _];
        let list = sys::mpv_node_list {
            num: 2,
            values: values.as_mut_ptr(),
            keys: keys.as_mut_ptr(),
        };
        let mut root = raw_none();
        root.format = sys::mpv_format::MPV_FORMAT_NODE_MAP;
        root.u.list = &list as *const _ as *mut _;

        let n = unsafe { Node::from_raw(&root) };
        assert_eq!(n.get("w").and_then(|v| v.as_int()), Some(1920));
        assert_eq!(n.get("h").and_then(|v| v.as_int()), Some(1080));
        assert!(n.get("missing").is_none());
        Ok(())
    }

    #[test]
    fn array_decodes_in_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut values = vec![raw_int(1), raw_int(2), raw_int(3)];
        let list = sys::mpv_node_list {
            num: 3,
            values: values.as_mut_ptr(),
            keys: std::ptr::null_mut(),
        };
        let mut root = raw_none();
        root.format = sys::mpv_format::MPV_FORMAT_NODE_ARRAY;
        root.u.list = &list as *const _ as *mut _;

        let n = unsafe { Node::from_raw(&root) };
        let arr = n.as_array().ok_or("expected array")?;
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_int(), Some(1));
        assert_eq!(arr[2].as_int(), Some(3));
        Ok(())
    }

    #[test]
    fn empty_list_decodes() {
        let list = sys::mpv_node_list {
            num: 0,
            values: std::ptr::null_mut(),
            keys: std::ptr::null_mut(),
        };
        let mut root = raw_none();
        root.format = sys::mpv_format::MPV_FORMAT_NODE_ARRAY;
        root.u.list = &list as *const _ as *mut _;
        let n = unsafe { Node::from_raw(&root) };
        assert_eq!(n.as_array().map(|a| a.len()), Some(0));
    }

    #[test]
    fn strings_are_copied_out_and_a_null_pointer_becomes_empty() {
        let owned = CString::new("ewa_lanczossharp").expect("cstring");
        let mut n = raw_none();
        n.format = sys::mpv_format::MPV_FORMAT_STRING;
        n.u.string = owned.as_ptr() as *mut _;
        assert_eq!(
            unsafe { Node::from_raw(&n) },
            Node::String("ewa_lanczossharp".into())
        );

        // libmpv can hand out a STRING node with no payload.
        n.u.string = std::ptr::null_mut();
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::String(String::new()));
    }

    #[test]
    fn byte_arrays_are_copied_and_an_absent_buffer_decodes_to_empty() {
        let bytes: Vec<u8> = vec![0xde, 0xad, 0x00, 0xbe, 0xef];
        let ba = sys::mpv_byte_array {
            data: bytes.as_ptr() as *mut _,
            size: bytes.len(),
        };
        let mut n = raw_none();
        n.format = sys::mpv_format::MPV_FORMAT_BYTE_ARRAY;
        n.u.ba = &ba as *const _ as *mut _;
        assert_eq!(
            unsafe { Node::from_raw(&n) },
            Node::ByteArray(bytes.clone())
        );

        // A NUL byte inside is data, not a terminator.
        assert_eq!(
            unsafe { Node::from_raw(&n) },
            Node::ByteArray(vec![0xde, 0xad, 0x00, 0xbe, 0xef])
        );

        let empty = sys::mpv_byte_array {
            data: bytes.as_ptr() as *mut _,
            size: 0,
        };
        n.u.ba = &empty as *const _ as *mut _;
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::ByteArray(Vec::new()));

        n.u.ba = std::ptr::null_mut();
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::ByteArray(Vec::new()));
    }

    /// A format this build of libmpv did not have when the bindings were
    /// generated must decode to `None`, never panic.
    #[test]
    fn an_unknown_format_decodes_to_none() {
        let mut n = raw_none();
        n.format = sys::mpv_format(9999);
        assert_eq!(unsafe { Node::from_raw(&n) }, Node::None);
    }

    #[test]
    fn nested_containers_decode_recursively() {
        // { "tracks": [ 1, 2 ] }
        let mut inner_values = vec![raw_int(1), raw_int(2)];
        let inner = sys::mpv_node_list {
            num: 2,
            values: inner_values.as_mut_ptr(),
            keys: std::ptr::null_mut(),
        };
        let mut array_node = raw_none();
        array_node.format = sys::mpv_format::MPV_FORMAT_NODE_ARRAY;
        array_node.u.list = &inner as *const _ as *mut _;

        let key = CString::new("tracks").expect("cstring");
        let mut keys: Vec<*mut std::os::raw::c_char> = vec![key.as_ptr() as *mut _];
        let mut outer_values = vec![array_node];
        let outer = sys::mpv_node_list {
            num: 1,
            values: outer_values.as_mut_ptr(),
            keys: keys.as_mut_ptr(),
        };
        let mut root = raw_none();
        root.format = sys::mpv_format::MPV_FORMAT_NODE_MAP;
        root.u.list = &outer as *const _ as *mut _;

        let decoded = unsafe { Node::from_raw(&root) };
        let tracks = decoded.get("tracks").and_then(|v| v.as_array());
        assert_eq!(
            tracks.map(|a| a.iter().filter_map(Node::as_int).collect::<Vec<_>>()),
            Some(vec![1, 2])
        );
    }

    #[test]
    fn as_str_only_matches_a_string_node() {
        assert_eq!(Node::String("mkv".into()).as_str(), Some("mkv"));
        assert_eq!(Node::Int(3).as_str(), None);
        assert_eq!(Node::None.as_str(), None);
    }

    #[test]
    fn as_int_only_matches_an_int_node() {
        assert_eq!(Node::Int(-7).as_int(), Some(-7));
        // No coercion: a double that happens to be whole is still not an int.
        assert_eq!(Node::Double(7.0).as_int(), None);
        assert_eq!(Node::String("7".into()).as_int(), None);
    }

    #[test]
    fn as_double_only_matches_a_double_node() {
        assert_eq!(Node::Double(1.5).as_double(), Some(1.5));
        assert_eq!(Node::Int(1).as_double(), None);
        assert_eq!(Node::Flag(true).as_double(), None);
    }

    #[test]
    fn as_flag_only_matches_a_flag_node() {
        assert_eq!(Node::Flag(false).as_flag(), Some(false));
        assert_eq!(Node::Flag(true).as_flag(), Some(true));
        // mpv's FLAG is a distinct format from INT64; 1 is not `true` here.
        assert_eq!(Node::Int(1).as_flag(), None);
    }

    #[test]
    fn as_array_only_matches_an_array_node() {
        let arr = Node::Array(vec![Node::Int(1)]);
        assert_eq!(arr.as_array().map(Vec::len), Some(1));
        assert!(Node::Map(vec![]).as_array().is_none());
        assert!(Node::None.as_array().is_none());
    }

    #[test]
    fn as_map_only_matches_a_map_node() {
        let map = Node::Map(vec![("k".into(), Node::Int(1))]);
        assert_eq!(map.as_map().map(Vec::len), Some(1));
        assert!(Node::Array(vec![]).as_map().is_none());
        assert!(Node::None.as_map().is_none());
    }

    #[test]
    fn get_returns_none_for_a_missing_key_and_for_a_non_map() {
        let map = Node::Map(vec![
            ("w".into(), Node::Int(1920)),
            ("h".into(), Node::Int(1080)),
        ]);
        assert_eq!(map.get("h").and_then(Node::as_int), Some(1080));
        assert!(map.get("depth").is_none());
        assert!(map.get("").is_none());
        assert!(Node::Array(vec![Node::Int(1)]).get("w").is_none());
        assert!(Node::None.get("w").is_none());
    }

    /// mpv node maps are a list, not a hash: a duplicate key keeps the first.
    #[test]
    fn get_returns_the_first_of_two_equal_keys() {
        let map = Node::Map(vec![("k".into(), Node::Int(1)), ("k".into(), Node::Int(2))]);
        assert_eq!(map.get("k").and_then(Node::as_int), Some(1));
    }
}
