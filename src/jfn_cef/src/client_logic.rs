//! The decisions the CEF client callbacks make, without the callbacks.
//!
//! `client/**` and `client_impl/**` are vtable wiring: they unpack CEF's
//! arguments, call one of these, and act on the answer. Everything here is
//! input -> output — no browser, no host, no surface — so the rules a live
//! browser would otherwise be needed to exercise are testable on their own.

use std::os::raw::c_int;

use jfn_platform_abi::MenuItem;
use jfn_platform_abi::event_flags::EVENTFLAG_ALT_DOWN;

use crate::platform_ops::FrameSize;

// ---------------------------------------------------------------------------
// Context menu
// ---------------------------------------------------------------------------

/// The ampersand Chromium marks a menu mnemonic with.
const ACCELERATOR_MARKER: char = '&';

/// A menu label with its mnemonic markers removed. Chromium hands labels over
/// as "&Reload"; the native menus draw the marker literally.
pub(crate) fn strip_accelerator(s: &str) -> String {
    s.chars().filter(|c| *c != ACCELERATOR_MARKER).collect()
}

// ---------------------------------------------------------------------------
// <select> popup
// ---------------------------------------------------------------------------

/// The menu rows for a `<select>`'s options. The row id is the option's index,
/// which is what the replay below steps to.
pub(crate) fn options_as_items(options: &[String]) -> Vec<MenuItem> {
    options
        .iter()
        .enumerate()
        .map(|(i, label)| MenuItem {
            id: i as i32,
            label: label.clone(),
            enabled: true,
            separator: false,
        })
        .collect()
}

/// Windows virtual-key codes CEF expects in `KeyEvent::windows_key_code`.
pub(crate) const VK_RETURN: i32 = 0x0D;
/// See [`VK_RETURN`].
pub(crate) const VK_ESCAPE: i32 = 0x1B;
/// See [`VK_RETURN`].
pub(crate) const VK_UP: i32 = 0x26;
/// See [`VK_RETURN`].
pub(crate) const VK_DOWN: i32 = 0x28;

/// What to replay into CEF's still-open `<select>` popup for the row the user
/// picked in the native menu.
///
/// CEF OSR has no "set selected index" API: the popup is a real RenderWidget
/// that must be driven by forwarded input so Blink commits and closes it
/// cleanly (which is what lets it reopen).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum PopupReplay {
    /// Send Escape — the menu was dismissed, or the target is not a row Blink
    /// can move to.
    Cancel,
    /// Send `key` `repeats` times, then Return.
    Pick { key: i32, repeats: i32 },
}

/// Which keys close the popup on the option the user picked.
///
/// Arrow stepping is in selectable-option space (Blink skips disabled rows),
/// so both the popup's current highlight and the target are mapped into that
/// space and the difference is stepped. A highlight that is not itself
/// selectable is treated as the first row, which is where Blink parks it.
pub(crate) fn popup_replay(idx: i32, current: i32, selectable: &[i32]) -> PopupReplay {
    if idx < 0 {
        return PopupReplay::Cancel;
    }
    let pos = |opt: i32| selectable.iter().position(|&v| v == opt);
    let from = pos(current).unwrap_or(0) as i32;
    let Some(to) = pos(idx) else {
        return PopupReplay::Cancel;
    };
    let delta = to as i32 - from;
    PopupReplay::Pick {
        key: if delta >= 0 { VK_DOWN } else { VK_UP },
        repeats: delta.abs(),
    }
}

/// The anchor a `popupOptions` message carries, or `None` when it carries
/// none. Blink's own popup rect flips above the element near the window
/// bottom; the anchor keeps the menu under the box.
pub(crate) fn popup_anchor(x: i32, y: i32, present: bool) -> Option<(i32, i32)> {
    present.then_some((x, y))
}

// ---------------------------------------------------------------------------
// Resize and frame rate
// ---------------------------------------------------------------------------

/// One frame at 60 Hz, in nanoseconds — the period used when the display's
/// refresh rate is unknown.
pub(crate) const DEFAULT_FRAME_PERIOD_NS: i64 = 16_666_667;

/// The frame period for a refresh rate, falling back to 60 Hz when the rate
/// is unknown or nonsensical.
pub(crate) fn frame_period_ns(hz: f64) -> i64 {
    if hz > 0.0 {
        (1e9 / hz) as i64
    } else {
        DEFAULT_FRAME_PERIOD_NS
    }
}

/// What a resize should do to CEF now.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum ResizeAction {
    /// Enough of a frame has passed: tell CEF immediately.
    Now,
    /// Coalesce: post the resize this many milliseconds out, never less than
    /// one (a zero-delay task would spin).
    After(i64),
}

/// Whether this resize goes to CEF now or is coalesced into a later task.
pub(crate) fn resize_action(now_ns: i64, last_ns: i64, period_ns: i64) -> ResizeAction {
    let elapsed = now_ns - last_ns;
    if elapsed >= period_ns {
        return ResizeAction::Now;
    }
    ResizeAction::After(((period_ns - elapsed) / 1_000_000).max(1))
}

/// The windowless frame rate for a display refresh rate, rounded up so a
/// 59.94 Hz panel is driven at 60. `None` for a rate that says nothing.
pub(crate) fn refresh_target(hz: f64) -> Option<i32> {
    (hz > 0.0).then(|| hz.ceil() as i32)
}

/// True when a frame rate can be applied to a browser at all.
pub(crate) fn frame_rate_usable(hz: i32) -> bool {
    hz > 0
}

// ---------------------------------------------------------------------------
// Console and load events
// ---------------------------------------------------------------------------

const LOGSEVERITY_DEFAULT: c_int = 0;
const LOGSEVERITY_INFO: c_int = 2;
const LOGSEVERITY_WARNING: c_int = 3;
const LOGSEVERITY_ERROR: c_int = 4;

/// The `jfn_logging` level for a `cef_log_severity_t` from a console message.
/// ERROR and FATAL both log as errors; VERBOSE and anything below DEFAULT
/// log as debug.
pub(crate) fn console_level(severity: c_int) -> u8 {
    if severity >= LOGSEVERITY_ERROR {
        jfn_logging::LEVEL_ERROR
    } else if severity == LOGSEVERITY_WARNING {
        jfn_logging::LEVEL_WARN
    } else if severity == LOGSEVERITY_INFO || severity == LOGSEVERITY_DEFAULT {
        jfn_logging::LEVEL_INFO
    } else {
        jfn_logging::LEVEL_DEBUG
    }
}

/// One console line: the page's message with the source it came from.
pub(crate) fn format_console(msg: &str, src: &str, line: c_int) -> String {
    format!("{msg} ({src}:{line})")
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

/// The `V` key, as CEF reports it in `windows_key_code`.
const VK_V: i32 = b'V' as i32;

/// True when a key event is the paste shortcut this client handles itself.
///
/// `is_raw_key_down` separates the press CEF asks about from the repeats and
/// the release; `action_modifier` is the platform's own accelerator modifier
/// (Command on macOS, Control elsewhere). Alt disqualifies the chord — that is
/// a different shortcut, not a decorated paste.
pub(crate) fn is_paste_shortcut(
    is_raw_key_down: bool,
    modifiers: u32,
    windows_key_code: i32,
    action_modifier: u32,
) -> bool {
    is_raw_key_down
        && (modifiers & action_modifier) != 0
        && (modifiers & EVENTFLAG_ALT_DOWN) == 0
        && windows_key_code == VK_V
}

// ---------------------------------------------------------------------------
// Browser creation and lifecycle
// ---------------------------------------------------------------------------

/// The URL a browser is created with.
///
/// A browser created with no URL never commits a navigation (the main layer
/// on a fresh profile, before a server is saved). Closing such a browser
/// during `CefShutdown` crashed CEF's in-process GPU thread on macOS in 14 of
/// 44 shutdowns from the connect screen, and never from a navigated page — so
/// it gets a real, empty document instead, which the overlay's `navigateMain`
/// replaces anyway.
pub(crate) fn initial_url(url: &str) -> &str {
    if url.is_empty() { "about:blank" } else { url }
}

/// The windowless frame rate a new browser starts at: this layer's own rate,
/// else the process default, else 60 — CEF rejects a non-positive one.
pub(crate) fn windowless_frame_rate(layer: i32, default: i32) -> i32 {
    let fr = if layer > 0 { layer } else { default };
    if fr > 0 { fr } else { 60 }
}

/// What `OnAfterCreated` does with a browser, given the layer's reset state.
///
/// A reset is a close followed by a create, and the create is deferred to
/// `OnBeforeClose`. `PendingReset` means the close never happened (there was
/// no browser to close), so the browser that just arrived is the stale one and
/// has to be closed; `Recreating` means this is the replacement and the layer
/// is whole again.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct AfterCreated {
    /// The reset state to store, or `None` to leave it alone.
    pub(crate) next_state: Option<i32>,
    /// Close this browser again instead of publishing it.
    pub(crate) close_again: bool,
}

/// See [`AfterCreated`]. The three state values are `client.rs`'s
/// `STATE_NORMAL` / `STATE_PENDING_RESET` / `STATE_RECREATING`.
pub(crate) fn after_created(
    state: i32,
    normal: i32,
    pending_reset: i32,
    recreating: i32,
) -> AfterCreated {
    if state == pending_reset {
        return AfterCreated {
            next_state: Some(recreating),
            close_again: true,
        };
    }
    if state == recreating {
        return AfterCreated {
            next_state: Some(normal),
            close_again: false,
        };
    }
    AfterCreated {
        next_state: None,
        close_again: false,
    }
}

// ---------------------------------------------------------------------------
// Context-menu results
// ---------------------------------------------------------------------------

/// What a menu id picked in the native menu means.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum MenuResult {
    /// Nothing was picked: resolve CEF's callback as cancelled and stop.
    Dismissed,
    /// One of CEF's own commands (Reload, Copy, ...). Only `cont()` runs
    /// those, so the callback is continued with the id rather than cancelled.
    Builtin,
    /// An id the app added to the model: CEF's callback is cancelled and the
    /// command is dispatched to the app's own handler.
    AppCommand,
}

/// Classify one menu result. `user_first` is CEF's `MENU_ID_USER_FIRST`,
/// the floor of the range a client may add its own commands in.
pub(crate) fn menu_result(id: c_int, user_first: c_int) -> MenuResult {
    if id < 0 {
        return MenuResult::Dismissed;
    }
    if id < user_first {
        return MenuResult::Builtin;
    }
    MenuResult::AppCommand
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// The script that pastes `text` into the focused editable element.
///
/// The text is embedded as a JSON literal; a value that cannot be encoded at
/// all pastes as the empty string rather than splicing anything unescaped into
/// the page.
pub(crate) fn paste_js(text: &str) -> String {
    let literal = jfn_js_json::to_js_json(text).unwrap_or_else(|| "\"\"".to_string());
    format!("document.execCommand('insertText',false,{literal});")
}

// ---------------------------------------------------------------------------
// Paint
// ---------------------------------------------------------------------------

/// The byte length of a tightly packed BGRA frame, or `None` when the frame
/// is unusable — no buffer, a degenerate size, or a size whose byte length
/// does not fit in a `usize`.
pub(crate) fn software_buffer_len(has_buffer: bool, w: i32, h: i32) -> Option<usize> {
    if !has_buffer || w <= 0 || h <= 0 {
        return None;
    }
    (w as usize).checked_mul(h as usize)?.checked_mul(4)
}

/// The device scale factor CEF is told about: the ratio of the physical
/// surface to the logical view. Either side being unknown means 1:1.
pub(crate) fn screen_scale(physical_w: i32, logical_w: i32) -> f32 {
    if physical_w > 0 && logical_w > 0 {
        physical_w as f32 / logical_w as f32
    } else {
        1.0
    }
}

/// Which of a browser's two surfaces a paint addresses: `Some(true)` the
/// dropdown popup, `Some(false)` the view. `None` for a paint element type
/// this client does not render, which is dropped.
pub(crate) fn paint_is_popup(kind: cef::sys::cef_paint_element_type_t) -> Option<bool> {
    use cef::sys::cef_paint_element_type_t as T;
    match kind {
        T::PET_POPUP => Some(true),
        T::PET_VIEW => Some(false),
        _ => None,
    }
}

/// The coded and visible extents of one accelerated-paint frame, or `None`
/// when the coded size is not presentable. A visible rect CEF leaves unset
/// comes through as zero and is clamped rather than trusted.
pub(crate) fn accel_extents(
    coded_w: i32,
    coded_h: i32,
    visible_w: i32,
    visible_h: i32,
) -> Option<(FrameSize, FrameSize)> {
    if coded_w <= 0 || coded_h <= 0 {
        return None;
    }
    Some((
        FrameSize {
            w: coded_w,
            h: coded_h,
        },
        FrameSize {
            w: visible_w.max(0),
            h: visible_h.max(0),
        },
    ))
}

/// How many memory planes of an accelerated frame to import, or `None` when
/// there is nothing importable.
///
/// Every plane the modifier uses counts — a DCC/CCS modifier adds an
/// auxiliary plane beyond the color plane — but never more than the fixed
/// array CEF hands over holds.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn plane_count(reported: i32, available: usize) -> Option<usize> {
    let n = reported.clamp(0, available as i32) as usize;
    (n >= 1).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mnemonic_marker_is_stripped_from_a_label() {
        assert_eq!(strip_accelerator("&Reload"), "Reload");
        assert_eq!(strip_accelerator("Save &As"), "Save As");
    }

    #[test]
    fn a_label_without_a_marker_is_unchanged() {
        assert_eq!(strip_accelerator("Reload"), "Reload");
        assert_eq!(strip_accelerator(""), "");
    }

    #[test]
    fn stripping_keeps_non_ascii_labels_intact() {
        assert_eq!(strip_accelerator("&Rückgängig"), "Rückgängig");
        assert_eq!(strip_accelerator("再&読み込み"), "再読み込み");
    }

    #[test]
    fn options_become_rows_numbered_by_their_index() {
        let items = options_as_items(&["One".into(), "Two".into()]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 0);
        assert_eq!(items[0].label, "One");
        assert_eq!(items[1].id, 1);
        assert!(items[1].enabled);
        assert!(!items[1].separator);
    }

    #[test]
    fn a_select_with_no_options_has_no_rows() {
        assert!(options_as_items(&[]).is_empty());
    }

    #[test]
    fn a_dismissed_menu_replays_an_escape() {
        assert_eq!(popup_replay(-1, 0, &[0, 1, 2]), PopupReplay::Cancel);
    }

    #[test]
    fn a_pick_below_the_highlight_steps_down_and_commits() {
        assert_eq!(
            popup_replay(2, 0, &[0, 1, 2]),
            PopupReplay::Pick {
                key: VK_DOWN,
                repeats: 2
            }
        );
    }

    #[test]
    fn a_pick_above_the_highlight_steps_up() {
        assert_eq!(
            popup_replay(0, 2, &[0, 1, 2]),
            PopupReplay::Pick {
                key: VK_UP,
                repeats: 2
            }
        );
    }

    #[test]
    fn picking_the_row_already_highlighted_only_commits() {
        assert_eq!(
            popup_replay(1, 1, &[0, 1, 2]),
            PopupReplay::Pick {
                key: VK_DOWN,
                repeats: 0
            }
        );
    }

    #[test]
    fn stepping_counts_only_the_selectable_rows() {
        // Option 3 is disabled, so 0 -> 4 is two steps, not four.
        assert_eq!(
            popup_replay(4, 0, &[0, 2, 4]),
            PopupReplay::Pick {
                key: VK_DOWN,
                repeats: 2
            }
        );
    }

    #[test]
    fn a_highlight_that_is_not_selectable_counts_as_the_first_row() {
        assert_eq!(
            popup_replay(4, 9, &[0, 2, 4]),
            PopupReplay::Pick {
                key: VK_DOWN,
                repeats: 2
            }
        );
    }

    #[test]
    fn a_target_that_is_not_selectable_cancels_instead_of_guessing() {
        assert_eq!(popup_replay(3, 0, &[0, 2, 4]), PopupReplay::Cancel);
    }

    #[test]
    fn an_anchor_is_only_taken_when_the_message_carries_one() {
        assert_eq!(popup_anchor(10, 20, true), Some((10, 20)));
        assert_eq!(popup_anchor(10, 20, false), None);
    }

    #[test]
    fn a_known_refresh_rate_sets_the_frame_period() {
        assert_eq!(frame_period_ns(100.0), 10_000_000);
        assert_eq!(frame_period_ns(60.0), 16_666_666);
    }

    #[test]
    fn an_unknown_refresh_rate_falls_back_to_sixty_hertz() {
        assert_eq!(frame_period_ns(0.0), DEFAULT_FRAME_PERIOD_NS);
        assert_eq!(frame_period_ns(-1.0), DEFAULT_FRAME_PERIOD_NS);
    }

    #[test]
    fn a_resize_a_full_frame_after_the_last_one_goes_straight_through() {
        assert_eq!(resize_action(20_000_000, 0, 16_666_667), ResizeAction::Now);
        assert_eq!(resize_action(16_666_667, 0, 16_666_667), ResizeAction::Now);
    }

    #[test]
    fn a_resize_inside_the_frame_is_coalesced_into_a_later_task() {
        // 6.67 ms left of a 16.67 ms frame.
        assert_eq!(
            resize_action(10_000_000, 0, 16_666_667),
            ResizeAction::After(6)
        );
    }

    #[test]
    fn a_coalesced_resize_never_posts_a_zero_delay_task() {
        assert_eq!(
            resize_action(16_666_666, 0, 16_666_667),
            ResizeAction::After(1)
        );
    }

    #[test]
    fn a_fractional_refresh_rate_rounds_up_to_the_next_frame_rate() {
        assert_eq!(refresh_target(59.94), Some(60));
        assert_eq!(refresh_target(120.0), Some(120));
    }

    #[test]
    fn a_refresh_rate_that_says_nothing_sets_no_frame_rate() {
        assert_eq!(refresh_target(0.0), None);
        assert_eq!(refresh_target(-60.0), None);
    }

    #[test]
    fn only_a_positive_frame_rate_is_usable() {
        assert!(frame_rate_usable(60));
        assert!(!frame_rate_usable(0));
        assert!(!frame_rate_usable(-1));
    }

    #[test]
    fn console_errors_and_fatals_log_as_errors() {
        assert_eq!(console_level(4), jfn_logging::LEVEL_ERROR);
        assert_eq!(console_level(5), jfn_logging::LEVEL_ERROR);
    }

    #[test]
    fn console_warnings_info_and_default_keep_their_own_levels() {
        assert_eq!(console_level(3), jfn_logging::LEVEL_WARN);
        assert_eq!(console_level(2), jfn_logging::LEVEL_INFO);
        assert_eq!(console_level(0), jfn_logging::LEVEL_INFO);
    }

    #[test]
    fn console_verbose_logs_as_debug() {
        assert_eq!(console_level(1), jfn_logging::LEVEL_DEBUG);
        assert_eq!(console_level(-1), jfn_logging::LEVEL_DEBUG);
    }

    #[test]
    fn a_console_line_names_the_source_and_line_it_came_from() {
        assert_eq!(
            format_console("boom", "app://web/x.js", 42),
            "boom (app://web/x.js:42)"
        );
    }

    #[test]
    fn the_action_modifier_plus_v_on_a_key_down_is_a_paste() {
        let ctrl = jfn_platform_abi::event_flags::EVENTFLAG_CONTROL_DOWN;
        assert!(is_paste_shortcut(true, ctrl, VK_V, ctrl));
    }

    #[test]
    fn paste_needs_the_press_the_modifier_and_the_v() {
        let ctrl = jfn_platform_abi::event_flags::EVENTFLAG_CONTROL_DOWN;
        assert!(!is_paste_shortcut(false, ctrl, VK_V, ctrl));
        assert!(!is_paste_shortcut(true, 0, VK_V, ctrl));
        assert!(!is_paste_shortcut(true, ctrl, b'C' as i32, ctrl));
    }

    #[test]
    fn alt_disqualifies_the_paste_chord() {
        let ctrl = jfn_platform_abi::event_flags::EVENTFLAG_CONTROL_DOWN;
        assert!(!is_paste_shortcut(
            true,
            ctrl | EVENTFLAG_ALT_DOWN,
            VK_V,
            ctrl
        ));
    }

    #[test]
    fn the_two_paint_element_types_name_the_two_surfaces() {
        use cef::sys::cef_paint_element_type_t as T;
        assert_eq!(paint_is_popup(T::PET_POPUP), Some(true));
        assert_eq!(paint_is_popup(T::PET_VIEW), Some(false));
    }

    #[test]
    fn a_frame_with_area_reports_both_extents() {
        assert_eq!(
            accel_extents(1920, 1088, 1920, 1080),
            Some((
                FrameSize { w: 1920, h: 1088 },
                FrameSize { w: 1920, h: 1080 }
            ))
        );
    }

    #[test]
    fn a_frame_with_no_coded_area_is_dropped() {
        assert_eq!(accel_extents(0, 1080, 1920, 1080), None);
        assert_eq!(accel_extents(1920, 0, 1920, 1080), None);
        assert_eq!(accel_extents(-1, -1, 0, 0), None);
    }

    #[test]
    fn a_negative_visible_rect_is_clamped_rather_than_trusted() {
        assert_eq!(
            accel_extents(64, 64, -5, -5),
            Some((FrameSize { w: 64, h: 64 }, FrameSize { w: 0, h: 0 }))
        );
    }

    #[test]
    fn every_plane_the_modifier_uses_is_imported() {
        assert_eq!(plane_count(1, 4), Some(1));
        assert_eq!(plane_count(2, 4), Some(2));
    }

    #[test]
    fn a_plane_count_beyond_the_array_is_clamped_to_it() {
        assert_eq!(plane_count(9, 4), Some(4));
    }

    #[test]
    fn a_frame_with_no_planes_is_dropped() {
        assert_eq!(plane_count(0, 4), None);
        assert_eq!(plane_count(-1, 4), None);
        assert_eq!(plane_count(2, 0), None);
    }
    #[test]
    fn a_negative_menu_id_is_a_dismissal() {
        assert_eq!(menu_result(-1, 26500), MenuResult::Dismissed);
    }

    #[test]
    fn an_id_below_the_user_range_is_one_of_cefs_own_commands() {
        assert_eq!(menu_result(0, 26500), MenuResult::Builtin);
        assert_eq!(menu_result(26499, 26500), MenuResult::Builtin);
    }

    #[test]
    fn an_id_in_the_user_range_is_the_apps_own_command() {
        assert_eq!(menu_result(26500, 26500), MenuResult::AppCommand);
        assert_eq!(menu_result(26501, 26500), MenuResult::AppCommand);
    }

    #[test]
    fn pasted_text_reaches_the_page_as_a_json_literal() {
        assert_eq!(
            paste_js("hi"),
            "document.execCommand('insertText',false,\"hi\");"
        );
    }

    #[test]
    fn a_quote_in_pasted_text_cannot_break_out_of_the_literal() {
        let js = paste_js("a\"b");
        assert!(js.contains("\\\""), "{js}");
        assert!(!js.contains("a\"b"), "{js}");
    }

    #[test]
    fn pasting_nothing_is_an_empty_literal() {
        assert_eq!(
            paste_js(""),
            "document.execCommand('insertText',false,\"\");"
        );
    }

    #[test]
    fn a_frame_with_pixels_has_a_four_byte_per_pixel_length() {
        assert_eq!(software_buffer_len(true, 1920, 1080), Some(1920 * 1080 * 4));
        assert_eq!(software_buffer_len(true, 1, 1), Some(4));
    }

    #[test]
    fn a_frame_with_no_buffer_or_no_area_has_no_length() {
        assert_eq!(software_buffer_len(false, 1920, 1080), None);
        assert_eq!(software_buffer_len(true, 0, 1080), None);
        assert_eq!(software_buffer_len(true, 1920, 0), None);
        assert_eq!(software_buffer_len(true, -1, -1), None);
    }

    #[test]
    fn an_enormous_frame_cannot_overflow_its_length() {
        // The checked multiplies matter on a 32-bit host, where i32::MAX
        // pixels of BGRA do not fit in a usize at all; on a 64-bit host they
        // do, and the length must still be exact rather than wrapped.
        let expected = (i32::MAX as u64)
            .checked_mul(i32::MAX as u64)
            .and_then(|v| v.checked_mul(4))
            .and_then(|v| usize::try_from(v).ok());
        assert_eq!(software_buffer_len(true, i32::MAX, i32::MAX), expected);
    }

    #[test]
    fn the_screen_scale_is_the_physical_to_logical_ratio() {
        assert_eq!(screen_scale(2560, 1280), 2.0);
        assert_eq!(screen_scale(1920, 1920), 1.0);
        assert_eq!(screen_scale(1920, 1280), 1.5);
    }

    #[test]
    fn an_unknown_side_of_the_screen_scale_means_one_to_one() {
        assert_eq!(screen_scale(0, 1280), 1.0);
        assert_eq!(screen_scale(2560, 0), 1.0);
        assert_eq!(screen_scale(-1, -1), 1.0);
    }

    #[test]
    fn a_browser_with_no_url_gets_a_real_empty_document() {
        assert_eq!(initial_url(""), "about:blank");
    }

    #[test]
    fn a_browser_with_a_url_is_created_on_it() {
        assert_eq!(
            initial_url("https://jellyfin.invalid/web"),
            "https://jellyfin.invalid/web"
        );
        assert_eq!(initial_url("about:blank"), "about:blank");
    }

    #[test]
    fn a_layers_own_frame_rate_wins_over_the_process_default() {
        assert_eq!(windowless_frame_rate(120, 60), 120);
    }

    #[test]
    fn a_layer_with_no_frame_rate_takes_the_process_default() {
        assert_eq!(windowless_frame_rate(0, 144), 144);
        assert_eq!(windowless_frame_rate(-1, 144), 144);
    }

    #[test]
    fn a_browser_never_starts_at_a_non_positive_frame_rate() {
        assert_eq!(windowless_frame_rate(0, 0), 60);
        assert_eq!(windowless_frame_rate(-1, -1), 60);
    }

    #[test]
    fn a_browser_created_in_the_normal_state_is_published_as_is() {
        assert_eq!(
            after_created(0, 0, 1, 2),
            AfterCreated {
                next_state: None,
                close_again: false
            }
        );
    }

    #[test]
    fn a_browser_that_arrives_during_a_pending_reset_is_closed_again() {
        assert_eq!(
            after_created(1, 0, 1, 2),
            AfterCreated {
                next_state: Some(2),
                close_again: true
            }
        );
    }

    #[test]
    fn the_replacement_browser_ends_the_reset() {
        assert_eq!(
            after_created(2, 0, 1, 2),
            AfterCreated {
                next_state: Some(0),
                close_again: false
            }
        );
    }
}
