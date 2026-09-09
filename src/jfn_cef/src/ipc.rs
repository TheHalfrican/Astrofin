use cef::{
    Browser, CefString, Frame, ImplBrowser, ImplFrame, ImplListValue, ImplProcessMessage,
    ListValue, ProcessId, process_message_create, sys,
};

pub(crate) struct BrowserMessage {
    name: String,
    args: Option<ListValue>,
    browser: Option<Browser>,
}

impl BrowserMessage {
    pub(crate) fn new(name: String, args: Option<ListValue>, browser: Option<Browser>) -> Self {
        Self {
            name,
            args,
            browser,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn args(&self) -> Option<&ListValue> {
        self.args.as_ref()
    }

    pub(crate) fn browser(&self) -> Option<&Browser> {
        self.browser.as_ref()
    }

    pub(crate) fn main_frame(&self) -> Option<Frame> {
        self.browser.as_ref().and_then(|b| b.main_frame())
    }
}

/// The slice of `CefListValue` the message handlers read.
///
/// Every argument list that reaches a handler was filled by the renderer's
/// V8 relay (`v8_handler.rs`) out of whatever the page passed to
/// `jmpNative.*`, so its length, each slot's type and each slot's contents
/// are all page-controlled: `jmpNative.playerLoad()` produces an empty list,
/// and an argument the relay has no branch for (an object, `null`,
/// `undefined`, an array) leaves its slot unset while later slots are still
/// written. Handlers therefore index a list that is routinely shorter than
/// the arity they expect.
///
/// The accessors below are the *unchecked* ones — valid only for
/// `idx < size()`. Every read from a handler goes through the bounded
/// `list_*` helpers in this module instead, because libcef's behaviour for
/// an out-of-range index is not part of the C API contract (the generated
/// C-to-C++ wrapper forwards the index straight into `libcef.dll`).
pub(crate) trait ArgList {
    fn size(&self) -> usize;
    fn is_string(&self, idx: usize) -> bool;
    fn is_double(&self, idx: usize) -> bool;
    fn string_at(&self, idx: usize) -> String;
    fn int_at(&self, idx: usize) -> i32;
    fn double_at(&self, idx: usize) -> f64;
    fn bool_at(&self, idx: usize) -> bool;
}

impl ArgList for ListValue {
    fn size(&self) -> usize {
        ImplListValue::size(self)
    }

    fn is_string(&self, idx: usize) -> bool {
        self.get_type(idx).as_ref() == &sys::cef_value_type_t::VTYPE_STRING
    }

    fn is_double(&self, idx: usize) -> bool {
        self.get_type(idx).as_ref() == &sys::cef_value_type_t::VTYPE_DOUBLE
    }

    fn string_at(&self, idx: usize) -> String {
        crate::cef_string::userfree_to_string(&self.string(idx))
    }

    fn int_at(&self, idx: usize) -> i32 {
        self.int(idx)
    }

    fn double_at(&self, idx: usize) -> f64 {
        self.double(idx)
    }

    fn bool_at(&self, idx: usize) -> bool {
        self.bool(idx) != 0
    }
}

/// Empty string for a missing slot or a slot holding anything but a string —
/// CEF does not coerce, and neither do we.
pub(crate) fn list_string<A: ArgList + ?Sized>(args: &A, idx: usize) -> String {
    if idx >= args.size() {
        return String::new();
    }
    args.string_at(idx)
}

/// `None` unless the slot exists *and* holds a string, which is how
/// `setSettingValue` tells "clear this key" (JS `null`) from `""`.
pub(crate) fn list_opt_string<A: ArgList + ?Sized>(args: &A, idx: usize) -> Option<String> {
    if idx < args.size() && args.is_string(idx) {
        Some(args.string_at(idx))
    } else {
        None
    }
}

/// JS can send integers as `VTYPE_DOUBLE` (e.g. via `parseFloat`); round to i32 in that case.
///
/// `f64 as i32` saturates rather than wrapping and maps NaN to 0, so a page
/// sending `Infinity`, `NaN` or `1e300` gets a clamped integer, never a
/// panic. A missing slot reads as 0.
pub(crate) fn list_int<A: ArgList + ?Sized>(args: &A, idx: usize) -> i32 {
    if idx >= args.size() {
        return 0;
    }
    if args.is_double(idx) {
        args.double_at(idx).round() as i32
    } else {
        args.int_at(idx)
    }
}

/// `false` for a missing slot or a slot holding anything but a bool.
pub(crate) fn list_bool<A: ArgList + ?Sized>(args: &A, idx: usize) -> bool {
    idx < args.size() && args.bool_at(idx)
}

/// `0.0` for a missing slot. The value is passed through as-is — NaN and the
/// infinities included, which mpv rejects on its own side.
pub(crate) fn list_double<A: ArgList + ?Sized>(args: &A, idx: usize) -> f64 {
    if idx >= args.size() {
        return 0.0;
    }
    args.double_at(idx)
}

pub(crate) fn send_to_renderer<F: FnOnce(&ListValue)>(frame: &Frame, name: &str, fill: F) {
    let Some(mut msg) = process_message_create(Some(&CefString::from(name))) else {
        return;
    };
    if let Some(args) = msg.argument_list() {
        fill(&args);
    }
    frame.send_process_message(
        ProcessId::from(sys::cef_process_id_t::PID_RENDERER),
        Some(&mut msg),
    );
}

/// A stand-in for the renderer's argument list, for tests that must not link
/// a live CEF process. Mirrors `CefListValue`'s no-coercion behaviour: a slot
/// read as the wrong type yields that type's default.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ArgValue {
    /// A slot the V8 relay left unset — what an object, array, `null` or
    /// `undefined` argument produces.
    Unset,
    Bool(bool),
    Int(i32),
    Double(f64),
    Str(String),
}

#[cfg(test)]
pub(crate) struct TestArgs(pub(crate) Vec<ArgValue>);

#[cfg(test)]
impl TestArgs {
    pub(crate) fn new(values: Vec<ArgValue>) -> Self {
        Self(values)
    }

    pub(crate) fn empty() -> Self {
        Self(Vec::new())
    }

    /// Panics on an out-of-range read: the bounded `list_*` helpers must
    /// never reach the unchecked accessors, and a test that regresses that
    /// should fail loudly rather than read a default.
    fn at(&self, idx: usize) -> &ArgValue {
        #[allow(clippy::panic)]
        match self.0.get(idx) {
            Some(v) => v,
            None => panic!(
                "unchecked read of arg {idx} past the end of a {}-slot list",
                self.0.len()
            ),
        }
    }
}

#[cfg(test)]
impl ArgList for TestArgs {
    fn size(&self) -> usize {
        self.0.len()
    }

    fn is_string(&self, idx: usize) -> bool {
        matches!(self.at(idx), ArgValue::Str(_))
    }

    fn is_double(&self, idx: usize) -> bool {
        matches!(self.at(idx), ArgValue::Double(_))
    }

    fn string_at(&self, idx: usize) -> String {
        match self.at(idx) {
            ArgValue::Str(s) => s.clone(),
            _ => String::new(),
        }
    }

    fn int_at(&self, idx: usize) -> i32 {
        match self.at(idx) {
            ArgValue::Int(v) => *v,
            _ => 0,
        }
    }

    fn double_at(&self, idx: usize) -> f64 {
        match self.at(idx) {
            ArgValue::Double(v) => *v,
            _ => 0.0,
        }
    }

    fn bool_at(&self, idx: usize) -> bool {
        match self.at(idx) {
            ArgValue::Bool(v) => *v,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn args(values: Vec<ArgValue>) -> TestArgs {
        TestArgs::new(values)
    }

    #[test]
    fn list_string_reads_a_string_slot() {
        let a = args(vec![ArgValue::Str("http://host/x.mkv".into())]);
        assert_eq!(list_string(&a, 0), "http://host/x.mkv");
    }

    #[test]
    fn list_string_past_the_end_is_empty() {
        assert_eq!(list_string(&TestArgs::empty(), 0), "");
        assert_eq!(list_string(&args(vec![ArgValue::Int(1)]), 7), "");
    }

    #[test]
    fn list_string_of_a_non_string_slot_is_empty() {
        let a = args(vec![
            ArgValue::Unset,
            ArgValue::Int(3),
            ArgValue::Double(1.5),
            ArgValue::Bool(true),
        ]);
        for idx in 0..4 {
            assert_eq!(list_string(&a, idx), "", "slot {idx}");
        }
    }

    #[test]
    fn list_string_keeps_hostile_bytes_verbatim() {
        // Quotes, backslashes, NUL, a JS line separator and an oversized
        // payload all survive unchanged: sanitising is the sink's job
        // (`js_cstr_or_warn`, `to_js_json`), not the reader's.
        let hostile = "\"'\\</script>\u{2028}\0\u{1F600}";
        let a = args(vec![ArgValue::Str(hostile.into())]);
        assert_eq!(list_string(&a, 0), hostile);

        let big = "A".repeat(1 << 20);
        let a = args(vec![ArgValue::Str(big.clone())]);
        assert_eq!(list_string(&a, 0).len(), big.len());
    }

    #[test]
    fn list_opt_string_is_none_for_missing_and_non_string_slots() {
        assert_eq!(list_opt_string(&TestArgs::empty(), 0), None);
        let a = args(vec![
            ArgValue::Unset,
            ArgValue::Int(0),
            ArgValue::Double(0.0),
            ArgValue::Bool(false),
        ]);
        for idx in 0..4 {
            assert_eq!(list_opt_string(&a, idx), None, "slot {idx}");
        }
    }

    #[test]
    fn list_opt_string_distinguishes_empty_string_from_absent() {
        let a = args(vec![ArgValue::Str(String::new())]);
        assert_eq!(list_opt_string(&a, 0), Some(String::new()));
        assert_eq!(list_opt_string(&a, 1), None);
    }

    #[test]
    fn list_int_reads_an_int_slot() {
        let a = args(vec![ArgValue::Int(-42), ArgValue::Int(i32::MAX)]);
        assert_eq!(list_int(&a, 0), -42);
        assert_eq!(list_int(&a, 1), i32::MAX);
    }

    #[test]
    fn list_int_rounds_a_double_slot() {
        let a = args(vec![
            ArgValue::Double(1.4),
            ArgValue::Double(1.5),
            ArgValue::Double(-1.5),
            ArgValue::Double(-0.4),
        ]);
        assert_eq!(list_int(&a, 0), 1);
        assert_eq!(list_int(&a, 1), 2);
        assert_eq!(list_int(&a, 2), -2);
        assert_eq!(list_int(&a, 3), 0);
    }

    #[test]
    fn list_int_saturates_instead_of_panicking_on_hostile_doubles() {
        let a = args(vec![
            ArgValue::Double(f64::NAN),
            ArgValue::Double(f64::INFINITY),
            ArgValue::Double(f64::NEG_INFINITY),
            ArgValue::Double(1e300),
            ArgValue::Double(-1e300),
        ]);
        assert_eq!(list_int(&a, 0), 0, "NaN");
        assert_eq!(list_int(&a, 1), i32::MAX);
        assert_eq!(list_int(&a, 2), i32::MIN);
        assert_eq!(list_int(&a, 3), i32::MAX);
        assert_eq!(list_int(&a, 4), i32::MIN);
    }

    #[test]
    fn list_int_past_the_end_or_wrong_type_is_zero() {
        assert_eq!(list_int(&TestArgs::empty(), 0), 0);
        let a = args(vec![ArgValue::Unset, ArgValue::Str("9".into())]);
        assert_eq!(list_int(&a, 0), 0);
        assert_eq!(list_int(&a, 1), 0, "no string-to-number coercion");
        assert_eq!(list_int(&a, 2), 0);
    }

    #[test]
    fn list_bool_is_false_for_missing_and_non_bool_slots() {
        let a = args(vec![
            ArgValue::Bool(true),
            ArgValue::Int(1),
            ArgValue::Str("true".into()),
            ArgValue::Unset,
        ]);
        assert!(list_bool(&a, 0));
        assert!(!list_bool(&a, 1));
        assert!(!list_bool(&a, 2));
        assert!(!list_bool(&a, 3));
        assert!(!list_bool(&a, 4));
        assert!(!list_bool(&TestArgs::empty(), 0));
    }

    #[test]
    fn list_double_passes_values_through_and_defaults_to_zero() {
        let a = args(vec![
            ArgValue::Double(-0.5),
            ArgValue::Double(f64::NAN),
            ArgValue::Int(3),
        ]);
        assert_eq!(list_double(&a, 0), -0.5);
        assert!(list_double(&a, 1).is_nan(), "NaN reaches the sink as NaN");
        assert_eq!(list_double(&a, 2), 0.0, "no int-to-double coercion");
        assert_eq!(list_double(&a, 3), 0.0);
        assert_eq!(list_double(&TestArgs::empty(), 0), 0.0);
    }

    #[test]
    #[should_panic(expected = "unchecked read")]
    fn the_fake_catches_an_unchecked_out_of_range_read() {
        // Guards the guard: if a helper ever drops its bounds check, the
        // tests above stop proving anything.
        TestArgs::empty().string_at(0);
    }
}
