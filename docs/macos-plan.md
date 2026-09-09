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
