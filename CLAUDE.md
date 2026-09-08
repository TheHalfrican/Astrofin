# Project Notes

## Build / Run
All app code is Rust; the cargo workspace lives in `src/` and produces the `astrofin` binary. Everything is driven through `just` — recipes are OS-gated via `[macos]`/`[linux]`/`[windows]` attributes, so the same command works everywhere:
```
just deps      # one-time: submodules, CEF download, macOS brew packages
just build     # build + stage a runnable tree in build/ (+ .app bundle on macOS)
just test      # run the workspace test suite (depends on build)
just run       # run with debug logging → logs to build/run.log
just run-mpv   # run the bundled mpv CLI directly (mpv-only debugging)
just clean     # remove build/ and dist/, cargo clean
just fmt        # format the workspace (cargo fmt)
just fmt-check  # check formatting without writing
just clippy     # clippy with -D warnings + unwrap/expect/panic denies
just lint       # fmt-check + clippy
just strict-lint # lint + clippy pedantic/nursery
just appimage build # [linux] build AppImage
just flatpak build  # [linux] build Flatpak bundle
just dmg            # [macos] build Apple Disk Image (.dmg)
just package       # [windows] build NSIS setup.exe + WiX .msi into dist/ (from the staged build/)
```
Platform-specific entry points live in `dev/linux/`, `dev/macos/`, `dev/windows/`, imported by the top-level justfile.

## Before Committing
Run `just fmt` and `just lint` before every commit; both must pass clean (lint runs `fmt-check` + `clippy`, and CI rejects unformatted or lint-failing code).

## Architecture
- **CEF** (Chromium Embedded Framework) — hosts jellyfin-web as an embedded browser; handles JS-to-Rust IPC for player control commands and renders the UI as an overlay texture above the video layer. Multi-process: browser process (main app, owns CefBrowser), renderer process (V8/Blink), GPU process. IPC via `CefProcessMessage`. Bindings via `cef-dll-sys`; project glue lives in `src/jfn_cef`.
- **mpv** (fork in `third_party/mpv`) — video playback; the desktop client injects native shims to override browser media playback. mpv owns its own window + GPU; libmpv is used only for the control plane (properties/commands/events).
- Wayland subsurface for video layer (Linux). Platform crates: `src/macos`, `src/windows`, plus Wayland/X11 paths under `src/`.

## mpv Integration
- **Never call sync mpv API (`mpv_get_property`, etc.) from event callbacks** - causes deadlock during video init. Use property observation or async variants instead.

## mpv Event Flow
mpv is the authoritative source of playback state. All state (position, speed, pause, seeking, etc.) flows from mpv property observations outward to the JS UI and OS media sessions. The JS UI and MPRIS/macOS media sessions are consumers — they never determine playback state, they only reflect what mpv reports. This means things like rate changes, seek completion, and position updates come from mpv, not from JS round-trips or manual bookkeeping.

## Astrofin fork — status and working notes (2026-09-07)

This repository is **Astrofin**, a fork of andrewrabert/jellium-desktop (upstream remote `upstream`; `origin` pushes to both GitHub `TheHalfrican/Astrofin` and the self-hosted Gitea). Rebrand of the app itself (name, exe, config dirs, ids) is DONE on `feat/rebrand-astrofin` (commits ce433b5..HEAD): binary `astrofin[.exe]`, per-user dirs `%APPDATA%\astrofin` / `%LOCALAPPDATA%\astrofin` (with a one-shot import of an existing `jellium-desktop` profile), app id `io.github.thehalfrican.Astrofin`, env prefix `ASTROFIN_*`. Artwork pixels and the optional ObjC/window-class hygiene renames are still open. See `docs/rebrand-plan.md` (exhaustive checklist + migration design + orchestrator decisions in §7). UI redesign direction: `docs/design-brief.md` (PS5-in-space; tokens/CSS vars to come from Claude Design).

Done on `main`:
- Native file dialogs (`src/jfn_cef/src/client_impl/dialog.rs` → `Platform::open_file_dialog` → `src/windows/src/file_dialog.rs`), plus CEF 151.3.24 which contains the upstream OSR file-chooser crash fix. Together these resolve upstream #681. macOS/Linux fall back to a graceful cancel (no dialog yet).
- `instance_ipc`: Windows second-instance detection fixed (a taken pipe name surfaces as `PermissionDenied`, not `AddrInUse`).
- `dev/windows/env.ps1`: msys64 clang bin is prepended to PATH (a WinLibs mingw on PATH otherwise breaks bindgen's libclang load).

Windows build on this machine (no `just`; PowerShell 7 is the Store build at `%LOCALAPPDATA%\Microsoft\WindowsApps\pwsh.exe`, not Program Files): put `%USERPROFILE%\.cargo\bin` on PATH, then `pwsh -ExecutionPolicy Bypass -File dev\windows\build.ps1` (libmpv via `dev\windows\build_mpv_source.ps1 -Arch x64` and CEF via `cargo xtask fetch-cef` are done once and cached). Lint/test: dot-source `dev\windows\env.ps1` first. Details and timings: `docs/build-windows-notes.md`.

CI: `.gitea/workflows/build-windows.yml` runs on the self-hosted host-mode act_runner on the developer PC (label `windows-latest`), with `.cache/`, `build/` and `third_party/mpv-install` junction-linked to a persistent cache under `C:\Users\NoahM\act_runner\cache\astrofin`. When `.gitea/workflows` exists Gitea ignores `.github/workflows`. On the GitHub fork, push-triggered runs did not fire; `workflow_dispatch` works (`gh workflow run build-windows.yml -R TheHalfrican/Astrofin --ref main`).

Known upstream issues worth reporting/PRing: the OSR file-dialog handler + ipc fix above; CI `build-windows.yml` appends msys64 to PATH (latent libclang shadowing); both macOS workflows call a non-existent `dev/tools/version.sh`; the Flatpak bundles GPL-3 text for a GPL-2 project; Windows memory growth during long playback (upstream #643) is still open and untouched here.
