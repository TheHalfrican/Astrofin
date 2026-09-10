# Handoff — 2026-09-10, for the MacBook session

Written on the Windows PC on the morning of 2026-09-10, after the previous
session ran out of usage. Memory files do not carry between machines, so this
file plus `CLAUDE.md` is the whole briefing. Read it top to bottom, then start
at §1.

## Where things stand

- `main` = the commit that adds this file, on top of 2e6560d. Tree clean, no
  worktrees, no stashes, every `test/phase-*` branch fully merged. Pushed to
  both remotes (GitHub `TheHalfrican/Astrofin` and the Gitea over Tailscale).
- The test-suite campaign (`docs/test-plan.md`, phases 0–5) is complete and
  merged. The two Opus agents that did phase 5 finished; their output is in
  d3cdbfd / b2b7f57. Nothing is in flight.
- 2e6560d ("ci: run clippy and cargo test on the Linux and macOS GitHub
  builds") was the last thing the previous session did. Its GitHub run
  (2026-09-10 04:40 UTC) came back:

  | workflow              | result  | note                                   |
  |-----------------------|---------|----------------------------------------|
  | checks                | pass    |                                        |
  | build-windows         | pass    |                                        |
  | codeql                | pass    |                                        |
  | build-macos-legacy    | pass    | Intel, has no test step                |
  | build-macos           | FAIL    | §1 — jfn-cef test binary SIGSEGV       |
  | build-linux-appimage  | FAIL    | §2 — one jfn-paths test, fixed here    |
  | build-linux-flatpak   | FAIL    | §3 — pre-existing, not ours to fix now |

  The previous session polled those results and stopped right after, before
  touching anything. §1 is the job for the Mac.

## 1. build-macos: the jfn-cef test binary crashes with SIGSEGV

**Facts** (run 34438085535, jobs 102747304032 arm64 and 102747303929 Intel,
identical signature on both):

- Clippy passes on macOS. `cargo test --workspace` gets to `jfn_cef` and the
  harness process dies:
  `process didn't exit successfully: .../deps/jfn_cef-… (signal: 11, SIGSEGV: invalid memory reference)`.
- 248 of the 327 jfn-cef tests printed `ok`; no test printed `FAILED`.
- libtest dispatches in sorted name order. Every test after
  `resource::tests::about_js_payload_prefixes_the_static_body_with_parsable_json`
  is unprinted, and that one is unprinted too, so it is the dispatch frontier
  and the prime suspect. (`cef_string::tests::utf16_units_become_the_string_they_spell`
  and `injection::tests::device_profile_json_setter_ignores_null_empty_and_non_utf8`
  are also unprinted despite sorting earlier; both are pure Rust, so treat the
  frontier test as the culprit until `--test-threads=1` says otherwise.)

**Why it crashes on macOS only.** `about_js_payload()`
(`src/jfn_cef/src/resource.rs:106`) calls `crate::cef_version()`
(`src/jfn_cef/src/version.rs:106`), a `LazyLock` around `probe()`, which calls
`cef_version_info` and the commit-hash getter, both CEF C API. On Windows and
Linux libcef is linked directly, so that works in a bare test binary. On macOS
CEF is a framework loaded at runtime: cef-dll-sys dispatches through a thunk
table that stays NULL until `cef_load_library` runs, and in Astrofin only
`MacosCefHost::before_start` (`src/macos/src/cef_host.rs`) does that. Nothing
in the test binary loads the framework, so the first CEF call jumps through
NULL. The eight `version::tests` were never reached and two of them (lines
173–174) call `cef_version()` too.

**Reproduce on the Mac** (same env the workflow step exports; the build dir
must exist, so `just build` first):

```sh
export CARGO_TARGET_DIR="$PWD/build/cargo-target"
export CEF_PATH="$PWD/.cache/cef"
export JFN_MPV_INCLUDE_DIR="$PWD/third_party/mpv/include"
export JFN_MPV_LIB_DIR="$PWD/build/mpv-build"
export DYLD_FALLBACK_LIBRARY_PATH="$PWD/build/mpv-build:${DYLD_FALLBACK_LIBRARY_PATH:-$HOME/lib:/usr/local/lib:/usr/lib}"
cargo test --manifest-path src/Cargo.toml -p jfn-cef --lib -- --test-threads=1
```

With one thread the last `test …` line before the crash names the culprit.
`RUST_BACKTRACE=1` will not help for a SIGSEGV; use
`lldb -- build/cargo-target/debug/deps/jfn_cef-<hash> --test-threads=1` and
`bt` if the frontier guess is wrong.

**Fix, recommended.** A macOS-only, once-only framework load in
`src/jfn_cef/src/test_support.rs`, modelled on the cef crate's own test
helper (`~/.cargo/registry/src/*/cef-151.8.1+151.3.24/src/string.rs`,
`ensure_dll_loaded`, and `src/lib.rs` `test_init_cef`):

```rust
#[cfg(all(test, target_os = "macos"))]
pub(crate) fn ensure_cef_loaded() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt, sync::Once};
    static LOAD: Once = Once::new();
    LOAD.call_once(|| {
        let dir = cef::sys::get_cef_dir().expect("CEF_PATH / cef dir");
        let fw = dir.join(cef::sys::FRAMEWORK_PATH).canonicalize().expect("framework path");
        let fw = CString::new(fw.as_os_str().as_bytes()).expect("path");
        assert_eq!(unsafe { cef::sys::cef_load_library(fw.as_ptr().cast()) }, 1);
    });
}
```

`sys::get_cef_dir()` reads `CEF_PATH`, which the workflow step already sets
(`$PWD/.cache/cef`); locally `cargo xtask` uses the same variable. Call it from
`install_platform()` and from every test that reaches CEF: the resource
`about_js_payload` test and the `version::tests` that call `cef_version()`.
Add a non-macOS no-op twin so call sites stay unconditional. Mind the
workspace lints (`expect_used` is denied outside `#[cfg(test)]` modules; the
test modules already `#![allow]` it, so keep the helper inside `cfg(test)`).

Fallback if that fights the loader: make `version::probe` injectable so the
tests never touch libcef. Do not `#[ignore]` the tests on macOS; the point of
2e6560d was to compile and run exactly these.

**Then** run the whole CI step locally (`cargo clippy … && cargo test …` with
the exports above; the exact commands are the `Lint (clippy) and test` step in
`.github/workflows/build-macos.yml`). cargo stops at the first failing crate,
so `jfn_paths` and everything after it never ran on macOS; expect the §2 test
to pass with the fix already on `main`, but watch for other first-time macOS
runs. Commit, `just fmt` + `just lint`, push, and check
`gh run list -R TheHalfrican/Astrofin -L 8` (the Gitea half of the push needs
Tailscale up; GitHub works regardless).

## 2. build-linux-appimage: one jfn-paths test, fixed on Windows, confirm on CI

**Fact:** `migrate::tests::a_symlink_pointing_outside_the_profile_is_not_followed`
failed at `paths/src/migrate.rs:947`,
`assertion failed: !new_dir.join("escape").join("secret.txt").exists()`.

**Cause:** the code is right, the assertion was wrong. `copy_symlink` recreates
the link instead of following it, so `tmp/astrofin/escape -> ../outside` is a
symlink whose relative target resolves to the same `tmp/outside`, and
`exists()` through it is true. Windows passed by accident: a relative reparse
target written with forward slashes does not resolve, and the test returns
early where symlink creation is refused.

**Fix (in the commit that adds this file):** assert on
`fs::symlink_metadata(new_dir/escape)`: the entry is a symlink or absent, never
a real directory. Verified on Windows (this PC can create symlinks, so the test
ran in full: 43 jfn-paths tests pass, clippy clean with the deny flags). Not
run on Linux (no WSL here). The AppImage workflow on that commit is the
verification; if it is still red, that is the first thing to look at.

## 3. build-linux-flatpak: pre-existing, Linux-deferred

It has never passed in the last 100 runs. It dies downloading
`uchardet-0.0.8.tar.xz` from freedesktop.org, which answers HTTP 418
(`dev/linux/flatpak/io.github.thehalfrican.Astrofin.yml:152`). When Linux
work resumes, repoint that module at a mirror (the GitLab freedesktop archive
or the Debian source tarball; untested). Not a MacBook task.

## 4. If there is time left

- `CHANGELOG.md` `[Unreleased]` records phases 1–2 and the tooling but not
  phase 3 (frontend 1:1, 605 JS cases), phase 4 (`just e2e`, `docs/e2e.md`),
  phase 5 (24 platform files un-exempted via `*_logic.rs` extractions) or the
  CI change. Add them before the next release.
- Design calls left for the user in `docs/test-plan.md` §6 (phase-1 security
  items, phase-5 glue bugs such as the x11 `shm_alloc` size guard, the
  `ResizeSync` that never disarms, macOS dropping `clickCount`).
- Frontend bugs found by the phase-3 agents, pinned by tests but not fixed:
  volume 0 is saved as 100% (`mpv-player-base.js`, `(val || 100) / 100`);
  underscore locales (`ur_PK`, `es_419`, …) fall to English because the
  resolver splits on `-` only; "Reset Saved Server" lacks `type="button"`;
  `native-shim.js` Escape calls `toggleFullscreen()` unguarded;
  `connectivityHelper.js` can call a null resolver. Fix the first two first.
- Older candidates: chapter markers not yet eyeballed on a chaptered title
  (fix 21209ff); `.af-abloop-band` keeps stale inline left/width after a
  clear; design-brief screens 3–9 undesigned; `jmpInfo.videoMode` stays stale
  after a script-driven mode switch (`docs/macos-plan.md`, "Still open").

## Working on the Mac

- Build order and gotchas: `docs/macos-plan.md` "Start here" and "Verified on
  the MacBook" (`/opt/homebrew/bin` first on PATH; `just deps`, `just lint`,
  `just build`, `just test`, `just test-js`).
- Delegate builds, test runs and mechanical edits to Opus 5 subagents
  (`Agent`, `model: "opus"`); keep the diagnosis, diff review and the commit in
  the main session (`CLAUDE.md`, working rules).
- Before every commit: `just fmt` and `just lint` must pass.
- When done, replace this file's status with a short "done" note or delete it
  and update `CLAUDE.md`'s pointer, so the next Windows session is not sent
  here again.
