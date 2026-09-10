# Changelog

All notable changes to Astrofin. The format follows Keep a Changelog; the
project uses semantic versioning.

## [Unreleased]

### Security
- Audit of the untrusted-input surfaces (docs/test-plan.md phase 1), with
  tests pinning each fix:
  - Only `http(s)` URLs may load into the main web layer, be saved as the
    server URL, be probed by the connect overlay, or be handed to mpv as a
    media/track URL. `file://`, `app://`, `chrome://` and friends were
    accepted before, and the main layer carries the native bridge.
  - The saved server URL and the settings blob are now spliced into the
    injected shim as escaped JS literals; a quote in either could run
    script in the renderer.
  - Every `jmpNative` argument is bounds-checked before it is read from the
    CEF list (a zero-argument `playerLoad` read past the end).
  - The connect-overlay probe body is capped at 64 KiB and results are bound
    to the request that started them; `//web` inside a host name no longer
    collapses the base URL.
  - Log redaction now covers every occurrence on a line, is case-insensitive,
    and knows `X-Emby-Token`, `Authorization: MediaBrowser ... Token=`,
    `Bearer`, pretty-printed JSON tokens, passwords and URL userinfo. Log
    files are created owner-only on Unix.
  - Second-instance frames are capped at 64 KiB (an endless line grew the
    process without bound); instance ids are validated before they become a
    pipe or socket name; an empty `--config-dir` no longer splits the profile
    between the browser and CEF helper processes.
  - A `settings.json` that fails to parse as a whole (duplicate key,
    out-of-range number) is logged at warn instead of silently resetting
    every setting.

### Added
- Backend tests to one test per public function across every non-exempt
  Rust file (docs/test-plan.md phase 2): mpv option tables and command wire
  forms, playback sinks through recording fakes, paint-scheduler pacing,
  startup-option precedence, the platform trait defaults, wake events,
  Linux menu rendering. Workspace tests 563 -> about 1400.
- Test-suite tooling (docs/test-plan.md): `cargo xtask test-ratio` measures
  tests per public function with an explicit exemption list for platform glue
  (`dev/test-exempt.txt`) and a floor that only moves up
  (`dev/test-ratio-floor.txt`, checked in CI). `just test-js` now globs every
  `src/web/*.test.js`; `just coverage`, `just audit`, `just deny` added.
- Supply-chain checks (docs/supply-chain.md): cargo-deny policy in `deny.toml`
  (licences, advisories, bans, sources), cargo-audit, CodeQL on GitHub, and a
  hosted `checks` workflow (rustfmt, deny, audit, JS tests, ratio floor).
  Baseline: zero vulnerabilities, one unmaintained transitive crate
  (ttf-parser via cosmic-text, Linux menu renderer only).

### Fixed
- The workspace test suite now passes on the GitHub macOS and Linux CI
  runners, not only on the Windows one. The jfn-cef test binary loads the
  CEF framework once per process on macOS before its first CEF call (a bare
  test binary has no `MacosCefHost`, so `cef_version()` went through a NULL
  thunk and the harness died with SIGSEGV); the two `c_char` pointer casts
  clippy rejects on aarch64 Linux, where `c_char` is unsigned, use
  `.cast::<u8>()`; the x11 overflow test asserts per pointer width, since the
  largest `i32` extent still fits a 64-bit `usize`; and `signal_raw_fd` is
  Linux-only, because its eventfd write cannot land on the pipe read end
  the other unixes hand out (the drain tests signal through the event).

## [0.4.0] - 2026-09-09

### Added
- macOS: native file chooser. `<input type=file>` in jellyfin-web now opens an
  `NSOpenPanel` sheet on the player window (type filters, multi-select,
  cancel) instead of resolving to a silent cancel. Linux still cancels.
- macOS: hardware decoding defaults to VideoToolbox. Windows and Linux keep
  `no`; the settings page shows the real default instead of "auto".
- `just deps` on macOS upgrades stale Homebrew formulas from its own list;
  a too-old libplacebo used to fail inside the mpv meson configure.

### Fixed
- macOS: warm launches could deadlock right after platform init (main thread
  in a synchronous mpv read, mpv core applying the startup background colour,
  the VO waiting on the main queue). Boot-time synchronous mpv reads now run
  off the main thread while it keeps pumping.
- macOS: quitting from the connect screen crashed CEF's in-process GPU thread
  about one time in three; the never-navigated main web layer is now created
  at `about:blank`.
- macOS: two copies of MoltenVK were loaded on machines with the Homebrew
  `molten-vk` formula; the Vulkan loader is pinned to the bundled one.
- The settings page claimed hardware decoding was "auto" while mpv ran with
  none; the display fallback now follows the real default.

### Docs
- `docs/macos-plan.md`: the plan as executed on an M3 Max, with every item
  verified except the human DMG install, and the fixes above.

## [0.3.0] - 2026-09-08

### Added
- A-B repeat loop: set a begin and an end point and playback loops between
  them. `[` / `]` / `\` on the keyboard, a `repeat` button in the OSD control
  row, a band and A/B pins on the scrubber and a readout beside the "ends at"
  time. The loop is mpv's own `ab-loop-a` / `ab-loop-b`, so seeking past B
  deliberately does not loop; the points reset with every item and are not
  persisted. See `docs/ab-loop.md`.

### Fixed
- Chapter-marker stars now sit on the scrubber line. jellyfin-web hangs its
  ticks off the top of the slider, which only meets the track at its own font
  size; the OSD's larger type had floated the stars above the line.

### Docs
- `docs/macos-plan.md`: the checklist for verifying the macOS build.

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
