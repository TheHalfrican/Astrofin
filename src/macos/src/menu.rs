//! CEF's Alloy OSR popup renders `<select>` hover/selection highlights as
//! opaque black on macOS, so its popup runs invisibly and a native NSMenu is
//! presented in its place.

use std::ffi::c_int;

use jfn_platform_abi::{MENU_DISMISSED, MenuHost, MenuItem, MenuRequest, menu_has_selectable};

use crate::ns_menu::{MenuEntry, MenuSpec, present_on_main};

pub(crate) struct NsMenuHost;

impl MenuHost for NsMenuHost {
    fn open(&self, req: MenuRequest) {
        if !menu_has_selectable(&req.items) {
            req.on_selected.resolve(MENU_DISMISSED);
            return;
        }
        let spec = spec_from(req.items, req.initial, req.x, req.y, req.width);
        present_on_main(spec, Some(req.on_selected));
    }
}

/// The NSMenu spec one request maps onto.
///
/// Item ids become menu-item tags, so the tag the target reports on a pick is
/// the id the caller asked for. `initial` names the row to highlight *and* the
/// item the menu is positioned against, which is how AppKit puts the current
/// choice under the pointer; a separator is never highlighted even when its id
/// matches, and a negative `initial` (CEF's "no selection") positions the menu
/// at the anchor point instead. A width of zero or less leaves the menu
/// content-sized.
fn spec_from(items: Vec<MenuItem>, initial: c_int, x: c_int, y: c_int, width: c_int) -> MenuSpec {
    let entries = items
        .into_iter()
        .map(|it| MenuEntry {
            checked: it.id == initial && !it.separator,
            title: it.label,
            tag: it.id,
            enabled: it.enabled,
            separator: it.separator,
        })
        .collect();
    MenuSpec {
        entries,
        x,
        y,
        positioning_tag: (initial >= 0).then_some(initial),
        min_width: (width > 0).then_some(width),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use jfn_platform_abi::MenuSelection;

    use super::*;

    fn item(id: c_int, label: &str, enabled: bool, separator: bool) -> MenuItem {
        MenuItem {
            id,
            label: label.to_string(),
            enabled,
            separator,
        }
    }

    fn menu() -> Vec<MenuItem> {
        vec![
            item(0, "1080p", true, false),
            item(1, "", false, true),
            item(2, "720p", true, false),
            item(3, "360p", false, false),
        ]
    }

    #[test]
    fn every_item_becomes_an_entry_carrying_its_id_and_label() {
        let spec = spec_from(menu(), -1, 10, 20, 0);
        assert_eq!(spec.entries.len(), 4);
        assert_eq!(spec.entries[0].title, "1080p");
        assert_eq!(spec.entries[0].tag, 0);
        assert!(spec.entries[0].enabled);
        assert!(!spec.entries[0].separator);
        assert!(spec.entries[1].separator);
        assert!(!spec.entries[3].enabled);
    }

    #[test]
    fn the_anchor_is_carried_through_unchanged() {
        let spec = spec_from(menu(), -1, 10, 20, 0);
        assert_eq!((spec.x, spec.y), (10, 20));
    }

    #[test]
    fn the_initial_item_is_the_only_one_checked() {
        let spec = spec_from(menu(), 2, 0, 0, 0);
        assert!(!spec.entries[0].checked);
        assert!(spec.entries[2].checked);
        assert!(!spec.entries[3].checked);
        assert_eq!(spec.positioning_tag, Some(2));
    }

    #[test]
    fn a_separator_sharing_the_initial_id_is_not_checked() {
        let spec = spec_from(vec![item(7, "", true, true)], 7, 0, 0, 0);
        assert!(!spec.entries[0].checked);
    }

    #[test]
    fn no_selection_leaves_the_menu_unpositioned_and_nothing_checked() {
        let spec = spec_from(menu(), MENU_DISMISSED, 0, 0, 0);
        assert_eq!(spec.positioning_tag, None);
        assert!(spec.entries.iter().all(|e| !e.checked));
    }

    #[test]
    fn only_a_positive_width_constrains_the_menu() {
        assert_eq!(spec_from(menu(), -1, 0, 0, 240).min_width, Some(240));
        assert_eq!(spec_from(menu(), -1, 0, 0, 0).min_width, None);
        assert_eq!(spec_from(menu(), -1, 0, 0, -5).min_width, None);
    }

    #[test]
    fn a_request_with_nothing_selectable_is_dismissed_without_a_menu() {
        let (tx, rx) = mpsc::channel();
        NsMenuHost.open(MenuRequest {
            items: vec![item(0, "", true, true), item(1, "off", false, false)],
            x: 0,
            y: 0,
            width: 0,
            initial: -1,
            on_selected: MenuSelection::new(move |id| {
                let _ = tx.send(id);
            }),
        });
        assert_eq!(rx.try_recv(), Ok(MENU_DISMISSED));
    }
}
