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
- Design calls from that audit, decided 2026-09-10 (docs/test-plan.md §6):
  - The connect probe follows a redirect only if it stays on the same host
    and either keeps scheme and port or upgrades http to https; any other
    redirect fails the probe with the target named ("server redirected to
    ...; enter that address instead"). The saved URL is the one asked for,
    or the https-upgraded one.
  - A bare host with no scheme is probed over https first (2 s), then http;
    when http is what worked, the connect screen shows a one-line "Not
    encrypted" note. A typed scheme is never changed, so `http://host:8096`
    behaves exactly as before.
  - The About page's "open path" no longer goes through a shell: the path is
    one argument to the platform opener, and only paths under the config
    dir, cache dir or the log file's directory may be opened.
  - `window.jmpNative` is bound per origin, not per browser: only the saved
    server's origin (scheme, host, port) and the app's own `app://` pages get
    it, with one warn line per refused origin.
  - Every page-supplied string in a log line has newlines and control
    characters escaped.
  - `openConfigDir` and `appExit` ignore a repeat within 250 ms.
  - Second-instance IPC: a failing `accept()` backs off from 10 ms to a
    1 s cap; at most 8 connections are served at once; a connection that
    completes no frame in 5 s is closed; dropping the listener ends its
    connections. On Windows the pipe name also carries the user's SID, so
    another user cannot squat the instance name, and a name still taken
    right after a shutdown is retried as stale instead of reported as
    "already running".
  - `settings.json` reads are capped at 1 MiB; `windowScale` is clamped to
    0.5..=4.0; the decoration accessors fall back instead of panicking when
    no platform backend is installed.
  - Legacy-profile import skips symlinks on Windows with a warning, and is
    skipped entirely when the source exceeds 2 GiB or the destination has
    less free space than that.
  - A relative `--config-dir`/`ASTROFIN_CONFIG_DIR` is resolved once against
    the launch directory and the absolute path is what the CEF helpers see;
    a failed directory creation is logged instead of swallowed.
  - A secret split across two log records is documented as a known
    redaction limit.
- `Mailbox::wait` wakes waiters when the `take` closure drains the slot, so
  a two-sided handshake no longer needs both sides to re-publish.

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
- Frontend tests to one test per public function (docs/test-plan.md phase 3):
  every `src/web/` module, plus the three `dev/tools/brand/*.mjs` scripts, has
  a `node:test` suite beside it, sharing the fakes in `src/web/test/` (a fake
  DOM, fake `playbackManager`/`Events`/`ApiClient`, a `jmpNative` recorder, and
  a `loadModule()` that evaluates an injected script into a fresh window). 113
  -> 605 JS cases at merge. The source changes are test hooks only: a guarded
  `module.exports` shim per module, `resolveLanguage()` split out of
  `overlay.lang.js` unchanged, and the brand scripts' top-level work moved
  behind an entry-point guard so importing them is side-effect free.
- E2E smoke suite (docs/e2e.md, docs/test-plan.md phase 4): `just e2e`
  (`node dev/e2e/run.mjs`) drives the staged `build/` tree over the Chrome
  DevTools Protocol against a `node:http` mock of the Jellyfin server, which
  also serves a pinned jellyfin-web 10.11.11 and a clip encoded by the
  submodule mpv. Ten tests in six scenarios, about 20 s: launch and
  `--version`; connect -> sign in -> Home with no page errors and no
  unmodelled requests; play -> pause -> seek -> stop, asserted from mpv's own
  pushes and the `/Sessions` reports; a `settings.json` round-trip read back by
  a second process; second-instance forwarding; and a clean quit with zero
  `ERROR` log lines. Every launch is muted (`ao=null` in a throwaway profile;
  `E2E_AUDIO=1` opts back in) and takes the desktop, so
  `.gitea/workflows/e2e-windows.yml` is `workflow_dispatch`-only.
- Platform glue brought under test (docs/test-plan.md phase 5): the pure logic
  behind 24 exempt files moved into sibling `*_logic.rs` modules, or private
  helpers where there was less of it, without changing behaviour. What came
  out, across `windows`, `windows_sink`, `x11`, `wayland`, `macos`,
  `macos_sink`, `jfn_cef`, `gpu_paint` and `xtask`: WPARAM and LPARAM decoding,
  modifier tables, rect and DPI maths, WndProc and event routing, cursor and
  keysym tables, Chromium switch construction, popup replay and resize
  coalescing, X11 framing and geometry, Wayland scroll and repeat arithmetic,
  NSEvent tables, SMTC and now-playing projections, and release/CEF/libmpv
  naming. Those files left `dev/test-exempt.txt` (136 -> 112), which now holds
  only glue that needs a display server, a GPU, a live CEF/mpv process, a
  session bus or a real syscall, with a reason on each line. Ratio 2485 tests /
  1116 public functions = 2.23; floor 1.50 -> 2.20.
- `build-linux-appimage.yml` and `build-macos.yml` run clippy and `cargo test`
  once their dependencies are in place: on Linux inside the same
  `astrofin-appimage:base` container that builds the workspace, on macOS
  natively against the meson libmpv and the unpacked CEF. The Linux- and
  macOS-only test modules are now compiled and run by CI, not only by the
  Gitea Windows runner.

- Library grid restyled to the design canvas (docs/design/canvas/Main.dc.html,
  docs/design/theme-injection.md "Library grid"): on the movies, tv, music and
  list views (`html.af-library`) the poster grid is eight columns at the 72px
  gutter with 24px gaps, six below 1600px and five below 1280px; the toolbar
  is a right-aligned row of glass icon discs with the paging count in mono;
  the alphabet picker is a fixed rail on the right with the selected letter
  as a lit cyan disc; unfocused labels recede as on Home; and the focused
  item's art bleeds in behind the grid through the same crossfade Home uses.
  Two stock `!important` card margins are overridden so rows and columns
  share the 24px gap.
- Item detail pages restyled to the design canvas (docs/design/canvas/
  MovieDetail.dc.html and SeriesDetail.dc.html, docs/design/theme-injection.md
  "Item detail"): on `#/details` (`html.af-detail`, with the item type from
  the server on `data-af-detail-type`) the page is the "game hub" layout:
  the item's own backdrop full-bleed through Astrofin's crossfade at a
  sharper, brighter treatment than Home (both scrims kept; muted text still
  measures at least 4.5:1 over a white frame), no poster, the logo top-left
  with the title in display type when there is none, an eyebrow naming the
  type, metadata as mono chips, the vertical action stack directly under
  them with Resume as the accent pill carrying the remaining time, the
  secondary actions as glass discs, a two-column layout with Cast, More like
  this and Seasons as 130px tile shelves on the right under a synthesized
  glass facts panel (video, audio, subtitles, mode, size; next up, network
  and status for a series; episode counts for a season), and season pages
  as episode rows with 16:9 stills, played ticks, progress bars and the
  focused row lifted. jellyfin-web's theme painted both column containers
  opaque over the art; those are made transparent under the gate.

### Fixed
- Every complaint a `settings.json` read can raise now reaches the log. The
  read has to run before logging is initialized, because the log level is one
  of the settings it reads, so the `windowScale` clamp, the "not a usable
  scale" line, the over-1-MiB refusal, an unreadable file and a whole-document
  parse failure all warned into no subscriber and were dropped: the file was
  handled correctly and the user was never told. The read buffers its lines
  instead and `jfn_app_main` replays them right after `init_logging`, the same
  way the legacy-profile import's lines already were.
- The Windows pipe-name test in jfn-paths passes on any account again. It
  still spelled out the unscoped `\\.\pipe\astrofin-<id>` from before the name
  carried a per-user key, so it failed on every machine; it now asserts the
  prefix, the id suffix and that the middle is a non-empty run of the
  sanitised alphabet.
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
- A volume dragged to zero is saved as zero: `mpv-player-base.js` treated 0
  as "unset" and stored full volume, so a deliberate mute came back at
  100% on the next start. Only null, undefined, an empty string and
  non-finite numbers count as unset now.
- Language tags with an underscore (`ur_PK`, `es_419`, a POSIX `LANG`)
  resolve to their table or bare language instead of English; the
  resolver folds `_` to `-` on both sides, which also makes jellyfin-web's
  four underscored tables (bn_BD, es_419, es_DO, ur_PK) reachable.
- The "Reset Saved Server" button in client settings is `type="button"`,
  so it no longer submits the settings form it sits in.
- A connectivity result that arrives while no probe is pending is ignored;
  before, a null-url result matched the null initial state and called a
  resolver that did not exist.
- The A-B loop band and pins drop their inline `left`/`width` when the
  loop is cleared, not only their visibility, so nothing can slide in from
  the previous loop's points if the band is ever shown before a render.
- The Flatpak build no longer fails in `appstreamcli compose`: the app icon
  SVGs carried a long XML comment before the root tag, which pushed `<svg`
  past the 256 bytes gdk-pixbuf sniffs for the image format, so the
  scalable icon read as "Unrecognized image file format". The comment now
  sits inside the root element. (freedesktop.org serves the uchardet
  tarball again, which is what let the job get this far.)
- Double-clicks reach the page on every platform. The shared mouse-button
  dispatcher stamped every click as a single, so Chromium never produced a
  `dblclick` (double-click to toggle fullscreen in the player, for one). It
  now recovers the count from the press stream on Windows, X11 and Wayland
  (Chromium's own thresholds: under 500 ms, within 4 px, same button, up to
  three) and macOS passes the `NSEvent` click count through, clamped.
- Double-clicking the video no longer toggles fullscreen twice and cancels
  itself. Once the dispatcher above started delivering a real `dblclick`,
  jellyfin-web's own handler toggled fullscreen — and the shim's leftover
  mousedown-pair detector toggled it again on the same gesture, so the log
  showed `fullscreen=true` then `fullscreen=false` on every double-click. The
  JS detector is removed; the platform-delivered `dblclick` is now the one
  path.
- X11 glue hardened (docs/test-plan.md §6): the SHM allocator rejects a
  non-positive or overflowing extent itself instead of relying on checks two
  modules away; the mpv-proxy no-op rewrite no longer indexes past an empty
  request; the `_NET_WM_SYNC_REQUEST` handshake disarms when the counter is
  written, so a later reconcile cannot re-apply a stale latch. The clipboard
  read on the platform trait is renamed from `_async`: three of the four
  backends always ran the callback inline, and the trait now says so.

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
