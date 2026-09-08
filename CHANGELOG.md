# Changelog

All notable changes to Astrofin. The format follows Keep a Changelog; the
project uses semantic versioning.

## [0.2.0] - 2026-09-08

First versioned Astrofin release. Everything below is relative to the
Jellium Desktop code it forked from (`andrewrabert/jellium-desktop` @ 28f2cf1).

### Added
- Rebrand: `astrofin` binary, `%APPDATA%\astrofin` / `%LOCALAPPDATA%\astrofin`
  profile with a one-shot import of an existing `jellium-desktop` profile,
  app id `io.github.thehalfrican.Astrofin`, `ASTROFIN_*` environment, new
  artwork.
- UI redesign ("PS5 in space"): starfield, frosted glass, focus-first
  navigation, themed Home, item detail, library grid, settings, connect
  screen and playback OSD. See `docs/design-brief.md`.
- Video modes (Settings → Playback → Video mode): Auto / Live-Action /
  Animation / Off, switched live through libmpv; Auto resolves per title from
  tags, genres and library. See `docs/video-modes.md`.
- Transcode indicator: a Direct Play / Direct Stream / Transcoding chip in the
  OSD header with a popover of reasons, codec path and transcoder speed, a
  once-per-playback warning toast, and the *Transcode warning* setting
  (off / cpu / any). See `docs/transcode-indicator.md`.
- Playback Info panel populated with mpv's own video, audio and player stats
  (codec, hardware decoding, resolution, frame rate, dropped frames, bitrate,
  cache, A/V sync), observed on demand while the panel is open.
- Playback OSD: show/hide motion on the theme's tokens, glass volume and
  brightness popups, skip-intro pill, up-next dialog, scrub-preview bubble,
  and chapter markers drawn as stars along the timeline.
- Native Windows file dialog for the windowless browser (image upload etc.);
  macOS and Linux cancel gracefully instead of crashing.
- Windows installers: per-user NSIS setup and WiX MSI, each removing the
  other kind first (`just package`).
- Node unit tests for the injected web modules (`just test-js`).

### Changed
- CEF 151.3.24 (upstream OSR file-chooser crash fix).
- Second-instance detection on Windows recognises a taken pipe name.
- The msys64 clang bin is prepended to PATH so bindgen finds libclang with
  another mingw toolchain installed.

### Fixed
- "player cannot be null" logged on every item start after a stop, and the
  "queue invalid" warning at every stop/start boundary (input plugin asked
  the playback manager for state with no player).
- The OS media session's next/previous state now clears after a real stop.
- *Force Transcoding* did nothing: the device profile it emitted let the
  server direct-play everything. It now omits every DirectPlayProfile.
- Unchanged shader chains and background colours are no longer re-written to
  mpv (memory-growth investigation, `docs/memory-growth-findings.md`).
