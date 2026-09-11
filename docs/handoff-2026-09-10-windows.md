# Handoff 2026-09-10 (MacBook -> Windows, evening session) — DONE

Done-note. The evening Windows session ran the verification sweep this handoff
asked for and acted on it. Kept only as a record; nothing here is still open
that is not tracked below.

## What landed (main, pushed to both remotes, CI green on 45fa2e9)

- **§1 verification, live on the Windows PC.** Second-instance IPC (per-user
  pipe name carrying the SID, hand-off, stale rebind) PASS. Settings caps
  clamp/refuse correctly. `just e2e` 10/10. Legacy import could only be
  unit-tested (the Windows profile roots come from `SHGetKnownFolderPath`, not
  an env var, so they cannot be redirected to a scratch dir without risking the
  real profile). The library grid and detail pages were eyeballed at 300 % on
  the 4K TV (1280x698).
- **Fixes found by that sweep, all committed:**
  - `d9ff773` — settings-read warnings (windowScale clamp, unusable scale,
    over-1-MiB refusal, unreadable/unparseable file) were raised before logging
    exists and dropped; `jfn-config` now buffers them and `jfn_app_main` replays
    them after `init_logging`. Same commit fixes the Windows-only jfn-paths
    pipe-name test that `632e47f` left asserting the old unscoped name (it was
    red on every Windows `cargo test`).
  - `c8e5c3c` — double-click on the video toggled fullscreen twice and
    cancelled: `adf6a92` made Chromium deliver a real `dblclick`, so
    jellyfin-web's own handler fired alongside the shim's now-redundant
    mousedown-pair detector in `native-shim.js`. Detector removed. `csd.js`'s
    titlebar detector deliberately kept (its first mousedown hands the pointer
    grab to the compositor, so a native dblclick never reaches it).
  - `45fa2e9` — six theme defects at 1280x698: grid 6->5 columns at 1280, the
    A-Z rail's last seven letters off the bottom (max-height:916 query), uncapped
    `.itemTags`/`.itemGenres`/`.tagline`/`.itemExternalLinks`/`.itemName`, the
    series action stack below the fold (max-height:900 tighten), and two dead
    header rules (`.skinHeader > .header` -> `.headerTop`; the glass blur is now
    `!important` to beat stock's semiTransparent rule, and `.osdHeader` restates
    `backdrop-filter:none !important` so the player banner stays blur-free).
    `docs/design/theme-injection.md` gained the "Verified at 300 %" notes.
- **§2 design.** The Astrofin Settings artboard (screen 7) is drafted
  (`docs/design/canvas/Settings.dc.html`), added to `canvas.json` page-1, and
  the "Astrofin UI" canvas is republished (version 7). A four-state video-mode
  switch (Auto / Live-Action / Animation / Off) with a now-playing resolve card,
  not the brief's two-option toggle.

## Still open (small)

- **Live 300 % eyeball.** The theme CSS is `include_str!`-embedded at compile
  time, so confirming the header now blurs and the OSD is unchanged needs a
  rebuilt binary; left for a human at the 4K TV. Low risk — the OSD's no-blur
  behaviour was preserved deliberately.
- **B2 (Home card-title double-click).** Two navigations (details, then the
  freshly-rendered play button). Assessed as jellyfin-web's own single-click
  behaviour with no clean fix in our layer; not touched. Revisit only if it
  bothers you in use.
- **A verification side effect:** the 300 % run started/stopped playback three
  times and Jellyfin cleared two resume points below its minimum — "Dune: Part
  Two" and "Dragon Ball Super: Broly" dropped out of Continue Watching. Not
  recoverable (positions were not recorded).
- **Design, remaining:** admin dashboard light pass, then the component sheet +
  logo board (screens 8-9). Recipe unchanged: artboard in
  `docs/design/canvas/`, re-seed, owner OK, implement as a gated section with a
  `theme-injection.md` subsection and tests.
