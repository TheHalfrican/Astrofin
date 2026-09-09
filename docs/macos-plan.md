# macOS verification plan

Astrofin inherits upstream Jellium Desktop's macOS support; nothing has been
built or run on macOS since the fork (the 2026-09-07 "Windows first" decision is
recorded in `CLAUDE.md`). This is a **verification pass plus fixing fork-specific
deltas**, not a port. Everything below was read off the repo on Windows — no
macOS behaviour has been observed, so treat every expectation here as unverified
until the Mac says otherwise.

## Start here

From the repo root on the MacBook, in order. Stop at the first failure.

1. `git status` / `git log --oneline -5` — confirm `main`, clean tree.
2. `just deps` — runs `dev/macos/setup.sh` (Xcode CLT check, then the Homebrew
   formulas `cmake ninja meson pkgconf ffmpeg libplacebo libass luajit
   vulkan-loader vulkan-headers molten-vk little-cms2 libunibreak zimg
   create-dmg`), inits the `third_party/mpv` submodule, then
   `cargo xtask fetch-cef` (~600 MB on first run).
3. `just lint` — `fmt-check` + clippy with `-D warnings -D clippy::unwrap_used
   -D clippy::expect_used -D clippy::panic`. **This is the first compile of
   `src/macos` since the rebrand**; expect the first breakage here.
4. `just build` — `cargo xtask install --mpv-cli --prefix build/output`, giving
   `build/output/Astrofin.app`. It depends on `deps`, so step 2 re-runs (cheap
   once brew is satisfied). mpv is built from the submodule, not from brew.
5. `just test` (cargo workspace; depends on `build`) and `just test-js` (the four
   node suites in `src/web`). They are separate recipes — `just test` is cargo
   only.
6. `just run` — starts `build/output/Astrofin.app/Contents/MacOS/astrofin` with
   `ASTROFIN_LOG_LEVEL=debug`; the justfile points `ASTROFIN_LOG_FILE` at
   `build/run.log`. The app's own default log is `~/Library/Logs/astrofin/`.

Recipes live in `dev/macos/macos.just`. `just run-mpv` runs the standalone mpv
CLI from `build/mpv-build/mpv` for mpv-only debugging.

## Checklist

### Build

1. **`src/macos` compiles.** Never built since the rebrand renamed its ObjC
   classes: `AstrofinApplication`, `AstrofinInputView`, `AstrofinAppMenuTarget`,
   `AstrofinLifecycleObserver`, `AstrofinDisplayLinkTarget`
   (`src/macos/src/init.rs`). Same for `src/macos_sink`. Fix compile errors
   before anything else; clippy denies `unwrap`/`expect`/`panic`, so new code
   must too.
2. **Apple Silicon, not Rosetta.** `cargo xtask fetch-cef` picks its download
   from `download_cef::DEFAULT_TARGET`, i.e. the host rust target — under Rosetta
   that silently becomes `x86_64-apple-darwin`. Check `uname -m` (`arm64`),
   `rustc -vV | grep host`, then `file` on
   `build/output/Astrofin.app/Contents/MacOS/astrofin`, the CEF framework binary
   under `Contents/Frameworks/Chromium Embedded Framework.framework/`, and the
   staged libmpv. All should say `arm64`.
3. **MoltenVK bundling.** `src/xtask/src/bundle_macos.rs::bundle_moltenvk` walks
   the brew prefix (and `/opt/homebrew` vs `/usr/local`, ordered by arch) for
   `libMoltenVK.dylib` and only *warns* if it finds nothing. Watch the build log
   for `warning: MoltenVK not found`.
4. **Bundle layout.** `src/xtask/src/platform_macos.rs` stages
   `Contents/{MacOS,Frameworks,Resources}` + `Info.plist`, and (~line 140) copies
   `build/shaders/` into `Contents/Resources/shaders`. Confirm both shader
   subdirs (`anime4k`, `fsrcnnx`) landed — video modes depend on it.
5. **install_name fixups.** `bundle_macos::complete()` rewrites the CEF framework
   id and dep-walks the dylibs. `otool -L` on the main binary should show no
   absolute `/opt/homebrew` paths left.
6. For build problems generally, the Windows equivalents and their root causes
   are in `docs/build-windows-notes.md` — the MAX_PATH parts do not apply, the
   cargo/CEF parts do.

### Runtime

7. **Ad-hoc signature / Gatekeeper.** `bundle_macos.rs::sign` (~line 340) runs
   `codesign --force --sign -` — **ad-hoc** — with
   `resources/macos/entitlements.plist`, and **ignores the exit status**
   (`let _ = cmd.status()`), so a signing failure is silent. There is no Apple
   Developer certificate and no notarization, so first launch needs
   right-click > Open, and a downloaded DMG will be quarantined. Verify with
   `codesign -dv --verbose=4 build/output/Astrofin.app` (expect
   `Signature=adhoc`) and `spctl -a -vv` (expect a rejection).
8. **First-run profile import.** `src/paths/src/migrate.rs` imports a legacy
   `jellium-desktop` profile once, triggered by the new directory not existing
   yet. On macOS the config root honours `$XDG_CONFIG_HOME`
   (`src/paths/src/imp_macos.rs` — never hardcode `~/.config`); cache is
   `~/Library/Caches/astrofin`. On a Mac that never ran Jellium there is nothing
   to import; confirm that no-op path is clean. Design notes:
   `docs/rebrand-plan.md` around line 420.
9. **Server connect + login**, then play a 1080p HEVC title.
10. **Video modes** (`docs/video-modes.md`). `jfn_paths::resource_dir()`
    (`src/paths/src/lib.rs`) maps `<bundle>/Contents/MacOS` ->
    `Contents/Resources`, so bundled shaders resolve from
    `Contents/Resources/shaders/<anime4k|fsrcnnx>/`; the user override is
    `<config dir>/mpv/shaders/`. Expect
    `video mode <mode> applied: scale=… dscale=… shaders=[…]` in the log,
    followed by `mpv reports glsl-shaders=…`. Check Auto, Live-Action, Animation
    and Off.
11. **OSD over video, and fullscreen.** The CEF overlay composites above mpv's
    layer through `macos_surface_present`. Confirm the OSD, the trickplay bubble
    and the transcode chip (`docs/transcode-indicator.md`) render over video and
    survive a fullscreen toggle both ways.
12. **Media session.** `NowPlayingSink` in `src/macos/src/lib.rs` (~line 443)
    drives `MPNowPlayingInfoCenter` via `src/macos_sink`. Runtime check only:
    play/pause state, title/artwork, and the media keys in Control Centre.
13. **Second instance.** `instance_ipc` uses a unix socket under
    `$XDG_RUNTIME_DIR`, else `/tmp/astrofin-<uid>/astrofin-<uuid>`
    (`jfn_paths::instance_listener_path`). The `PermissionDenied` special case in
    `is_addr_in_use` is `cfg!(windows)`-gated, so macOS relies on plain
    `AddrInUse`. Launch twice: the second process should signal the first and
    exit, not start a second copy or hang.

### Packaging & CI

14. **`just dmg`** — `dev/macos/build_dmg.sh` runs `create-dmg` and writes
    `dist/Astrofin-<version>-macos-$(uname -m).dmg`, version from
    `cargo xtask version`. The script swallows `create-dmg`'s exit code and then
    checks the file exists, so read its output, not just its status.
15. Install that DMG to `/Applications` and launch from there, not from
    `build/`. This is the first run that exercises Gatekeeper for real.
16. **`.github/workflows/build-macos.yml`** — matrix `macos-15`/arm64 +
    `macos-15-intel`/x86_64. Never run on this fork. Note it calls
    `cargo xtask install --prefix build/output` **without** `--mpv-cli`, unlike
    `just build`, and brew-installs its own list (adds `sdl3`, omits `cmake`,
    `pkgconf`, `little-cms2`, and installs `create-dmg` later in the job).
17. **`.github/workflows/build-macos-legacy.yml`** — `macos-26-intel`,
    `MACOSX_DEPLOYMENT_TARGET=12.0`, builds ffmpeg and libplacebo from source and
    caches them under `~/jellyfin-deps` (cosmetic rebrand residue; the remaining
    rows are listed in `docs/rebrand-plan.md` around lines 240-247). Also never
    run on this fork.
18. **Triggering CI.** Push-triggered runs have not fired on the GitHub fork; use
    `gh workflow run build-macos.yml -R TheHalfrican/Astrofin --ref main`. The
    self-hosted Gitea runner is Windows-only, so macOS CI is GitHub Actions only.

### Follow-up features

19. **Native file dialog (NSOpenPanel).** `Platform::open_file_dialog`
    (`src/platform_abi/src/lib.rs` ~line 657) defaults to returning `false`,
    which the CEF glue (`src/jfn_cef/src/client_impl/dialog.rs`) treats as a
    graceful cancel. `impl Platform for MacosPlatform` (`src/macos/src/lib.rs`
    ~line 455) does not override it; Windows does, in
    `src/windows/src/file_dialog.rs`. Implementing NSOpenPanel is the follow-up.
    The upstream OSR file-chooser *crash* is already fixed by the CEF 151.3.24
    pin, so this is a missing feature, not a crash.
20. Web assets (`src/web/astrofin-theme.css`, `.js`) are embedded with
    `include_str!` in `src/jfn_cef/src/embedded_css.rs`, so any UI tweak needs a
    rebuild — there is no live reload.

## Acceptance bar

"macOS works" means all of:

- `just lint` clean (fmt-check + clippy `-D warnings`).
- `just test` and `just test-js` green.
- `build/output/Astrofin.app` launches, reaches the server, logs in.
- A 1080p HEVC title plays through mpv with a video mode applied
  (`video mode … applied` in the log).
- OSD renders over video; fullscreen toggles both ways.
- `just dmg` produces a DMG that installs to `/Applications` and launches.
- Retina check (visual, not automated): Home, item detail, the playback OSD, the
  Playback Info stats panel, the transcode chip.

## Pushing from the MacBook

`origin` pushes to both GitHub and a self-hosted Gitea reachable only over
Tailscale (`ssh://git@thehalfrican-truenas.tail1cdca8.ts.net:30009`). Bring
Tailscale up or the Gitea half of the push fails; GitHub works regardless.

## Known stale notes

- `CLAUDE.md` used to claim both macOS workflows call a non-existent
  `dev/tools/version.sh`. **Fixed and removed:** both derive the version from
  `cargo run --manifest-path src/xtask/Cargo.toml -- version` (line 36 in
  `build-macos.yml`, line 37 in `build-macos-legacy.yml`). The §1i table in
  `docs/rebrand-plan.md` still lists it as a pre-existing bug; that row is
  historical.

## Verified on the MacBook — 2026-09-09

Machine: Apple M3 Max, macOS 26.6.2, Xcode 26.5 SDK, Apple Silicon Homebrew at
`/opt/homebrew`. An Intel Homebrew at `/usr/local` was dismantled first: it came
first on `PATH`, so `dev/macos/setup.sh` and `bundle_macos::brew_prefix` would
have used x86_64 libraries. Build with `/opt/homebrew/bin` ahead of
`/usr/local/bin` on any Mac that has both.

### Start here, as it went

1. Clean `main` at 14a5a81.
2. `just deps` failed twice before passing. Rust stable was 1.92 and the
   lockfile's `kstring 2.0.4` (via `gix`) needs 1.96, so nothing resolved:
   `rustup update stable` (now 1.98.1). Then every formula was present but
   `libplacebo` was 7.360.0 and the mpv fork's `meson.build` wants
   `>= 7.360.1`; `setup.sh` only checked membership, so it now also upgrades
   stale formulas from its own list (and nothing else). `cargo xtask fetch-cef`
   pulled `cef_binary_151.3.24+g2384915+chromium-151.0.7922.174_macosarm64_minimal`.
3. `just lint`: exactly one error, `clippy::collapsible_if` in the macOS-only
   branch of `jfn_paths::resource_dir`, never compiled on Windows. Collapsed
   into a let-chain. Everything else in `src/macos` and `src/macos_sink` was
   clean on the first compile since the rebrand.
4. `just build`: 98 s cold, 35 s warm. Checklist items 2–5 pass: main binary,
   CEF framework, libmpv and all 44 bundled dylibs are thin arm64; MoltenVK is
   bundled from the brew keg; both shader subdirectories land; `otool -L` shows
   no `/opt/homebrew` or `/usr/local` path anywhere in the bundle. No helper
   apps by design: `browser_subprocess_path` is the main executable and CEF
   runs single-process here.
5. `just test` 309 passed / 0 failed; `just test-js` 113 / 0.
6. The staged app launches to the connect screen in about 1 s warm. Item 7:
   `Signature=adhoc`, `codesign --verify --deep --strict` passes, `spctl`
   rejects. Item 8: a clean no-op import; profile at `~/.config/astrofin`
   (`settings.json`, `instance.json`, `mpv/`), cache at
   `~/Library/Caches/astrofin`, socket under `/tmp/astrofin-<uid>/`. Item 13:
   a second launch logs `Signaled existing instance, exiting` and exits 0 in
   ~0.2 s; the first logs `received Ping`. Item 14: `just dmg` writes
   `dist/Astrofin-<version>-macos-arm64.dmg` (version from `git describe`, so
   `-dirty` on an uncommitted tree); `hdiutil verify` passes and the volume
   holds the app plus the Applications symlink.

### Fixed along the way

- **Two MoltenVKs in one process.** The bundled loader is Homebrew's build and
  scans `/opt/homebrew/etc/vulkan/icd.d` next to the bundle's own manifest, so
  on a developer Mac it enumerated both and the ObjC runtime warned about
  duplicate `MVK*` classes. `MacosMpvHost::prepare` now sets `VK_DRIVER_FILES`
  to `Contents/Resources/vulkan/icd.d/MoltenVK_icd.json` unless the user set
  `VK_DRIVER_FILES`/`VK_ICD_FILENAMES` themselves (`src/macos/src/mpv_host.rs`).
  `lsof` on the running app shows only the bundled dylib.
- **Startup deadlock on warm launches.** With saved geometry the window is up
  in ~100 ms and `run_with_cef` reaches `publish_device_profile` while mpv's
  core thread is still applying the startup `background-color` write. That
  write needs the VO thread, the VO thread does `DispatchQueue.main.sync`, and
  main is parked in the profile's sync `mpv_get_property`: a three-way cycle,
  and SIGTERM is ignored. `mpv_read_off_main` (`src/jfn_rust/src/app.rs`) runs
  the boot-time sync reads (version log, demuxer list) through
  `Platform::run_blocking`, so main keeps pumping. Five consecutive warm
  launches reached the connect screen afterwards; before, zero of four did.

### Still open

- Items 9–12 (server login, 1080p HEVC playback, the four video modes, OSD and
  fullscreen, Now Playing), 15 (install the DMG and launch from
  `/Applications`), 16–18 (the GitHub macOS workflows have still never run),
  19 (NSOpenPanel), and the Retina visual checks. All need a server and eyes.
- Intermittent `EXC_BAD_ACCESS` on shutdown in CEF's `Chrome_InProcGpuThread`,
  one of six SIGTERM shutdowns, every frame inside the CEF binary. The
  truncated teardown leaves the instance socket behind; the next launch
  removes it. Report: `~/Library/Logs/DiagnosticReports/astrofin-2026-09-09-110441.ips`.
- `boot_mpv_reconcile` still does sync reads on the main thread after CEF
  init. Not seen to deadlock, but it is the same hazard class as above.
- The runtime checks above are log-based; nothing was looked at. `screencapture -x`
  failed with "could not create image from display" inside the subagent that
  ran them, but works from the main session's shell (verified at the end of
  the session, 3456x2234), so take screenshots from the main session.
