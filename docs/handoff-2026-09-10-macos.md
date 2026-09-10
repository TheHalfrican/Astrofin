# Handoff 2026-09-10 (Windows -> MacBook): done

Written on the Windows PC on the morning of 2026-09-10 to brief the MacBook
session on the CI failures that the clippy + `cargo test` step (2e6560d)
exposed. Everything in it landed on the MacBook the same day; this is the
done-note that replaces the briefing.

## What was wrong and what fixed it

All fixes are in 05486fa, verified locally by running the exact
`build-macos.yml` lint-and-test step on the M3 Max, then by GitHub on
aca6cd9 (`checks`, `codeql`, `build-windows`, `build-macos` arm64 + Intel,
`build-macos-legacy`, `build-linux-appimage` all green).

- **build-macos, jfn-cef test binary SIGSEGV.** As diagnosed: on macOS libcef
  is a framework loaded at runtime through a thunk table that stays NULL until
  `cef_load_library` runs, and nothing loads it in a bare test binary. The
  frontier test was the one predicted (`about_js_payload_prefixes_...`).
  `src/jfn_cef/src/test_support.rs` gained `ensure_cef_loaded()`, a
  once-per-process framework load modelled on the cef crate's own test
  helper, called from `install_platform()` and from the two tests that reach
  the CEF C API directly; a no-op twin keeps the call sites unconditional
  elsewhere. jfn-cef: 327/327 with default threads and `--test-threads=1`.
- **build-linux-appimage, jfn-paths symlink test.** The 83ebd8c assertion
  rewrite was right; it passes on macOS and on the Linux runner.
- **Three more failures the symlink fix had been masking** (found from the
  83ebd8c run's logs, fixed in the same commit):
  - jfn-cef, aarch64 Linux clippy: `p as *const u8` on a `*const c_char` is a
    same-type cast where `c_char` is `u8`; both sites use `.cast::<u8>()`,
    which is right under either signedness.
  - jfn-x11: the overflow test asked the largest `i32` extent to be rejected,
    but its byte count (2^64 - 2^34 + 4) fits a 64-bit `usize`; the test is
    gated per pointer width with a 64-bit twin. (The crate is Linux-only, so
    this was verified by the AppImage run, not locally.)
  - jfn-wake-event: `signal_raw_fd` is an eventfd write; on the other unixes
    `WakeEvent::fd()` is a pipe read end, so the write failed with EBADF and
    was swallowed. It is Linux-only now, and the drain tests signal through
    the event.

## Follow-ups from §4, also done

- `CHANGELOG.md` `[Unreleased]` records phases 3, 4, 5 and the CI step.
- Frontend bugs the phase-3 tests had pinned: volume 0 saved as 100%,
  underscore locales falling to English (1120488); "Reset Saved Server"
  without `type="button"`, a null connectivity resolver (aca6cd9); the
  `.af-abloop-band` stale inline geometry after a clear (this commit). The
  Escape handler in `native-shim.js` was already guarded and tested.

## Still open

- `build-linux-flatpak`: freedesktop.org now serves the uchardet tarball, so
  on aca6cd9 the job got to the end and failed in `appstreamcli compose`
  (`file-read-error` on the app component). Root cause: the icon SVGs had a
  long XML comment before `<svg`, past the 256 bytes gdk-pixbuf sniffs; the
  comment moved inside the root element (resources/brand + resources/linux,
  commit after aca6cd9). Reproduced and verified in an ubuntu:24.04 container
  with the runner's appstream-compose 1.0.2; the GitHub run on that commit is
  the final confirmation.
- The design calls in `docs/test-plan.md` §6, the design-brief screens, the
  chapter-marker eyeball on a chaptered title, and `jmpInfo.videoMode` going
  stale after a script-driven switch (`docs/macos-plan.md`, "Still open").
