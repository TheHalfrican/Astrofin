# Done-note: the Astrofin Settings page and the 0.5.0 release

Written as a handoff at the end of the 2026-09-10 Windows session; executed
in the next session on 2026-09-10/11 and shrunk to this note. The living
description of the page is `docs/design/theme-injection.md` under "Settings".

## What landed

- `83cec65` **feat(web): Astrofin Settings page** — design canvas screen 7
  implemented: `client-settings.js` stamps `data-af-setting` /
  `data-af-section` / `data-af-applies` hooks, toggles `html.af-settings` and
  fires `af-settings-show` / `af-settings-hide` on `document`; the new
  `src/web/astrofin-settings.js` (injected right after `astrofin-theme.js`)
  builds the six-section rail (Server / Playback / Video mode / Audio /
  Advanced / About), moves the stock controls into glass panels by a key→panel
  map, draws the four-state video-mode segmented switch over the real
  `<select>` (which stays the source of truth), the LIVE/RESTART tags, the
  floating "Restart to apply N changes" pill and the now-playing resolve card
  (`_vmApply` records its last resolution on `window.__afVideoModeResolved`).
  Device Name moved under Server beside the read-only saved address; "Reset
  Saved Server" became "Sign out of this server". +52 JS tests, test-ratio
  2.27.
- `2cc6e6c` **release: 0.5.0** — version bump and changelog roll.
- `7c2bfd1` **fix: Windows dropdowns open in-page; settings chevron and menu
  highlight** — found by the owner's live test of the page. No `<select>` had
  ever opened on Windows (stock jellyfin-web pages included): CEF's composited
  OSR popup is torn down by Chromium in the same UI-thread turn it opens
  (`OnPopupShow(true)`, `OnPopupSize`, `OnPopupShow(false)`, measured with a
  debug line on every step and with real and synthetic presses). Windows now
  returns `MenuDelivery::Page` and `select-menu.js` draws the list in-page as
  X11 does, tokenised with literal fallbacks. Also the select chevron centred
  (stock `.selectArrow{margin-top:1.2em}`) and the user Settings menu
  hover/focus following the pill radius with the token focus ring.
- Tag `v0.5.0` = `7c2bfd1`; installers `Astrofin-0.5.0-x64-setup.exe`
  (a22bb544…) and `Astrofin-0.5.0-x64.msi` (39406ef2…, ProductCode
  `{6D5D713B-961D-4BC7-A1FF-05E922EF070B}`) built from it on the PC into
  `dist/` with `SHA256SUMS.txt` appended; main then moved to `0.6.0-dev`.

## Lessons

- A CDP verification that sets `selectedIndex` programmatically never
  exercises the popup path; click the control with `Input.dispatchMouseEvent`
  and screenshot the open list.
- The staging copy in `build.ps1` fails if `build\astrofin.exe` is running;
  close the app before rebuilding.
- Tag after the last fix, not at the version bump: the tag must point at the
  commit the installers were built from.

## Open, not done here

- macOS/Wayland host-menu path: a late `popupOptions` reply can leak the
  previous popup's options into the next one (seen in logs while
  instrumenting; untestable from the PC).
- `--cache-dir` does not redirect the Windows log; `log_dir_path()` in
  `src/paths/src/imp_windows.rs` hard-codes `%LOCALAPPDATA%\astrofin\Logs`.
- The X11 look of the tokenised `select-menu.js` was not eyeballed.
- Next design screens: admin dashboard light pass, component sheet + logo.
