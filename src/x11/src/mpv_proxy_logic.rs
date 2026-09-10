//! Address, framing and auth-file arithmetic for the mpv X11 proxy.
//!
//! Split out of [`crate::mpv_proxy`], which keeps the sockets, the threads and
//! the request/reply rewriting. Nothing here opens or reads anything: display
//! strings, socket names, the two length fields the framers trust, and the
//! `.Xauthority` record layout are all decisions over plain values.

use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, ParsedDisplay};
use x11rb::reexports::x11rb_protocol::protocol::xproto::NO_OPERATION_REQUEST;

/// Ceiling on a single request's or reply's byte length; past it we assume a
/// parse desync and fall back to a verbatim relay rather than buffer unbounded.
pub(crate) const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// Display numbers the proxy will bind. Starts high enough to stay clear of
/// real servers (`:0`-`:63`) and of the `:1`-ish numbers Xvfb/Xephyr take.
pub(crate) const DISPLAY_NUMBERS: std::ops::Range<u32> = 64..1024;

/// Where a candidate upstream X server lives, in the order the proxy tries
/// them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum UpstreamAddr {
    Abstract(u16),
    Path(String),
    Tcp(String, u16),
}

/// The X socket name for a display number, in both the abstract and the
/// filesystem namespace — Linux spells them the same.
pub(crate) fn x_socket_name(number: u32) -> String {
    format!("/tmp/.X11-unix/X{number}")
}

/// The `DISPLAY` value pointing at the proxy. The screen suffix is carried over
/// so a `:0.1` session keeps addressing screen 1 through the proxy.
pub(crate) fn proxy_display(number: u32, screen: u16) -> String {
    if screen == 0 {
        format!(":{number}")
    } else {
        format!(":{number}.{screen}")
    }
}

/// The byte order byte a native-endian client sends in its setup request.
/// x11rb-protocol silently misparses the other one, so a foreign-endian client
/// is relayed verbatim instead.
pub(crate) fn native_byte_order() -> u8 {
    if cfg!(target_endian = "little") {
        b'l'
    } else {
        b'B'
    }
}

/// Build the ordered upstream-address candidates from a parsed `DISPLAY`,
/// reusing x11rb's resolution and prepending the Linux abstract socket (which
/// x11rb does not try) for local servers.
pub(crate) fn upstream_addresses(parsed: &ParsedDisplay) -> Vec<UpstreamAddr> {
    let candidates: Vec<ConnectAddress<'_>> = parsed.connect_instruction().collect();
    let mut addrs = Vec::new();
    if candidates
        .iter()
        .any(|c| matches!(c, ConnectAddress::Socket(_)))
    {
        addrs.push(UpstreamAddr::Abstract(parsed.display));
    }
    for c in candidates {
        match c {
            ConnectAddress::Socket(path) => addrs.push(UpstreamAddr::Path(path)),
            ConnectAddress::Hostname(host, port) => {
                addrs.push(UpstreamAddr::Tcp(host.to_string(), port));
            }
            _ => {}
        }
    }
    addrs
}

/// Total bytes of one request from its header: the length field counts 4-byte
/// units *after* the header. `None` past [`MAX_REQUEST_BYTES`] or on overflow,
/// which the caller treats as a parse desync.
pub(crate) fn request_total_len(remaining_length: u32, header_len: usize) -> Option<usize> {
    (remaining_length as usize)
        .checked_mul(4)
        .and_then(|b| b.checked_add(header_len))
        .filter(|&t| t <= MAX_REQUEST_BYTES)
}

/// Total bytes of one reply or GenericEvent: a fixed 32-byte unit plus the
/// extra length at offset 4, again in 4-byte units.
pub(crate) fn unit_total_len(words: u32) -> Option<usize> {
    (words as usize)
        .checked_mul(4)
        .and_then(|b| b.checked_add(32))
        .filter(|&t| t <= MAX_REQUEST_BYTES)
}

/// Total bytes of the server's setup reply. All three kinds (failed=0,
/// success=1, authenticate=2) carry an additional-data length in 4-byte units
/// at offset 6; `None` means byte 0 is not one of them, i.e. a desync.
pub(crate) fn setup_reply_len(first: u8, words: u16) -> Option<usize> {
    if first > 2 {
        return None;
    }
    Some(8 + 4 * usize::from(words))
}

/// Same-length rewrite to `NoOperation`, which accepts any request length and
/// has no reply, so sequence numbers stay intact.
pub(crate) fn emit_noop(raw: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(raw);
    out[start] = NO_OPERATION_REQUEST;
}

/// Append one `.Xauthority` record: a big-endian family, then four
/// length-prefixed blocks (address, display number, protocol name, cookie).
pub(crate) fn write_xauth_entry(
    out: &mut Vec<u8>,
    family: u16,
    address: &[u8],
    number: &[u8],
    name: &[u8],
    data: &[u8],
) {
    fn block(out: &mut Vec<u8>, b: &[u8]) {
        out.extend_from_slice(&(b.len() as u16).to_be_bytes());
        out.extend_from_slice(b);
    }
    out.extend_from_slice(&family.to_be_bytes());
    block(out, address);
    block(out, number);
    block(out, name);
    block(out, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::reexports::x11rb_protocol::parse_display::parse_display;

    #[test]
    fn a_socket_name_is_the_display_number_under_the_x11_dir() {
        assert_eq!(x_socket_name(0), "/tmp/.X11-unix/X0");
        assert_eq!(x_socket_name(64), "/tmp/.X11-unix/X64");
    }

    #[test]
    fn the_proxy_display_keeps_a_non_default_screen() {
        assert_eq!(proxy_display(64, 0), ":64");
        assert_eq!(proxy_display(64, 2), ":64.2");
    }

    #[test]
    fn the_display_search_starts_above_the_real_servers() {
        assert_eq!(DISPLAY_NUMBERS.start, 64);
        assert!(DISPLAY_NUMBERS.contains(&1023));
        assert!(!DISPLAY_NUMBERS.contains(&1024));
        assert!(!DISPLAY_NUMBERS.contains(&0));
    }

    #[test]
    fn the_native_byte_order_byte_matches_the_target() {
        let expected = if cfg!(target_endian = "little") {
            b'l'
        } else {
            b'B'
        };
        assert_eq!(native_byte_order(), expected);
    }

    #[test]
    fn a_local_display_tries_the_abstract_socket_first() {
        let parsed = parse_display(Some(":7")).unwrap();
        let addrs = upstream_addresses(&parsed);
        assert_eq!(addrs.first(), Some(&UpstreamAddr::Abstract(7)));
        assert!(
            addrs
                .iter()
                .any(|a| matches!(a, UpstreamAddr::Path(p) if p.contains("X7"))),
            "the filesystem socket is still a candidate: {addrs:?}"
        );
    }

    #[test]
    fn a_remote_display_has_no_abstract_socket_candidate() {
        let parsed = parse_display(Some("example.test:3")).unwrap();
        let addrs = upstream_addresses(&parsed);
        assert!(!addrs.iter().any(|a| matches!(a, UpstreamAddr::Abstract(_))));
        assert_eq!(
            addrs,
            vec![UpstreamAddr::Tcp("example.test".to_string(), 6003)]
        );
    }

    #[test]
    fn a_request_length_counts_four_byte_units_after_the_header() {
        assert_eq!(request_total_len(1, 4), Some(8));
        assert_eq!(request_total_len(0, 4), Some(4));
        assert_eq!(request_total_len(3, 8), Some(20));
    }

    #[test]
    fn an_absurd_request_length_is_refused() {
        assert_eq!(request_total_len(u32::MAX, 4), None);
        assert_eq!(request_total_len((MAX_REQUEST_BYTES / 4) as u32, 4), None);
        assert_eq!(
            request_total_len((MAX_REQUEST_BYTES / 4) as u32 - 1, 4),
            Some(MAX_REQUEST_BYTES)
        );
    }

    #[test]
    fn a_reply_is_thirty_two_bytes_plus_its_extra_words() {
        assert_eq!(unit_total_len(0), Some(32));
        assert_eq!(unit_total_len(2), Some(40));
        assert_eq!(unit_total_len(u32::MAX), None);
    }

    #[test]
    fn every_setup_reply_kind_carries_its_length_at_offset_six() {
        assert_eq!(setup_reply_len(0, 2), Some(16));
        assert_eq!(setup_reply_len(1, 0), Some(8));
        assert_eq!(setup_reply_len(2, 1), Some(12));
    }

    #[test]
    fn a_byte_that_is_not_a_setup_reply_kind_is_a_desync() {
        assert_eq!(setup_reply_len(3, 2), None);
        assert_eq!(setup_reply_len(0xff, 0), None);
    }

    #[test]
    fn a_neutralized_request_keeps_its_length_and_changes_only_the_opcode() {
        let raw = [12u8, 0, 3, 0, 1, 2, 3, 4];
        let mut out = Vec::new();
        emit_noop(&raw, &mut out);
        assert_eq!(out.len(), raw.len());
        assert_eq!(out[0], NO_OPERATION_REQUEST);
        assert_eq!(out[1..], raw[1..]);
    }

    #[test]
    fn a_neutralized_request_appends_after_what_is_already_queued() {
        let mut out = vec![0xEE];
        emit_noop(&[12u8, 0, 1, 0], &mut out);
        assert_eq!(out[0], 0xEE);
        assert_eq!(out[1], NO_OPERATION_REQUEST);
    }

    #[test]
    fn an_xauth_entry_is_a_family_then_four_length_prefixed_blocks() {
        let mut out = Vec::new();
        write_xauth_entry(
            &mut out,
            256,
            b"host",
            b"64",
            b"MIT-MAGIC-COOKIE-1",
            &[1, 2],
        );
        assert_eq!(&out[..2], &256u16.to_be_bytes());
        assert_eq!(&out[2..4], &4u16.to_be_bytes());
        assert_eq!(&out[4..8], b"host");
        assert_eq!(&out[8..10], &2u16.to_be_bytes());
        assert_eq!(&out[10..12], b"64");
        assert_eq!(&out[12..14], &18u16.to_be_bytes());
        assert_eq!(&out[14..32], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&out[32..34], &2u16.to_be_bytes());
        assert_eq!(&out[34..], &[1, 2]);
    }

    #[test]
    fn xauth_entries_concatenate() {
        let mut out = Vec::new();
        write_xauth_entry(&mut out, 256, b"h", b"64", b"N", &[9]);
        let first = out.len();
        write_xauth_entry(&mut out, 256, b"h", b"0", b"N", &[9]);
        assert_eq!(out.len(), first * 2 - 1, "the second number is one shorter");
    }
}
