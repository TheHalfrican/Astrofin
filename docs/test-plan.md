# Test-suite plan

Status: phases 0-4 done 2026-09-09; phase 5 (platform glue) in progress. Owner decisions are recorded in §1;
agents executing a phase read §3 for the working rules and §4 for the phase
they are on. Update the status line and the phase table as work lands.

## 1. Target and decisions

- **Target: one test per public function.** Every `pub fn` in a non-exempt
  Rust file and every exported function in a `src/web` module has at least one
  named test exercising it. Measured by `cargo xtask test-ratio` (§2); the
  number reported there is the project's coverage figure, not line coverage.
- **Exemptions are explicit.** Platform and FFI glue that cannot run without a
  display server, a GPU or a live CEF/mpv process is listed in
  `dev/test-exempt.txt` with a one-line reason each. Exempt files still count
  when they contain pure logic: the rule is to extract that logic into a
  testable function (pure input -> output, no handles) rather than to widen the
  exemption. The E2E smoke suite (§4, phase 4) is what covers exempt code.
- **E2E runs against a mock Jellyfin server** written for the harness, not
  the user's real server. Deterministic, credential-free, runnable on the
  self-hosted runner.
- **Security = tooling in CI plus one manual audit pass** over the
  untrusted-input surfaces (§4, phase 1). Findings become tests and fixes.
- Test frameworks stay minimal: Rust `#[test]` (plus `tokio` where the code is
  async), JS `node:test` + `node:assert`. No jsdom; the shared JS test helpers
  in `src/web/test/` provide the DOM and jellyfin-web fakes.

## 2. Measuring the ratio

`cargo xtask test-ratio` walks the workspace (excluding `third_party/` and
`src/target/`), `src/web` and `dev/tools`, and prints a per-crate table of
public functions vs. test functions, the exemption list applied, and a total.
`cargo xtask test-ratio --check` exits non-zero when the total is below the
floor in `dev/test-ratio-floor.txt`; the floor only ever moves up. Line
coverage is available separately via `just coverage` (cargo-llvm-cov, install
it with `cargo install cargo-llvm-cov`) as a diagnostic, not a gate.

Usage:

| Command | Does |
|---|---|
| `just test-ratio` | per-crate table, exemptions, total |
| `just test-ratio -- --files` | adds the per-file table |
| `just test-ratio -- --json` | the same report as JSON |
| `just test-ratio-check` | fails when the total is below the floor |

(`cargo xtask test-ratio [--files\|--json\|--check]` directly, if `just` is not
installed.) The counting rules — what makes a function public, how trait impls
and `#[cfg(test)]` are treated, how `tests/foo.rs` is attributed to
`src/**/foo.rs`, and the limits of the JS tokenizer — are documented in the
module comment of `src/xtask/src/test_ratio.rs`. The exemption file format
(globs, `!` negation, `|` reason) is documented at the top of
`dev/test-exempt.txt`.

Baseline before this plan (survey 2026-09-09): 241 Rust source files, 52 with
a test, 503 test functions; 20 JS files, 4 with a test, 113 test cases. As the
tool measures it (2026-09-09, 133 files exempt): **514 tests / 713 public
functions = 0.72** over 136 files, which is the floor in
`dev/test-ratio-floor.txt`.

## 3. Working rules for every phase

- Tests live next to the code: inline `#[cfg(test)] mod tests` for unit
  tests, `tests/*.rs` for anything that needs a process, a socket or a temp
  profile; `src/web/<name>.test.js` beside each JS module.
- A test asserts behaviour, named after the behaviour
  (`load_rejects_negative_stream_index`), not after the function. One test per
  public function is the floor, not the ceiling: error paths and boundary
  inputs count and are wanted.
- No test touches the user's real profile dirs, the real Jellyfin server or
  the installed app. Tests that need a profile use a `tempfile` dir and the
  `ASTROFIN_CONFIG_DIR`/`ASTROFIN_CACHE_DIR` flags.
- Pure extractions to make code testable are allowed and encouraged but must
  not change behaviour; anything larger is a separate change with its own
  review.
- Every phase ends green on `just fmt`, `just lint`, `cargo test --workspace`
  and `just test-js`, committed on a `test/<phase>` branch, merged to `main`
  after review.
- Windows is the machine that builds and runs everything; Linux-only and
  macOS-only crates are compiled and tested when that platform's CI runs.

## 4. Phases

| Phase | Scope | Branch | Status |
|---|---|---|---|
| 0 | Measurement and tooling: `xtask test-ratio`, exemption list, cargo-audit + cargo-deny, CodeQL, `just test-js` and audit in CI, `just coverage` | `test/phase-0-infra` | done 2026-09-09 (baseline 514/713 = 0.72, 133 exempt files) |
| 1 | Security audit of untrusted-input surfaces, tests and fixes for findings | `test/phase-1-security` | done 2026-09-09: 15+13+20 findings, 9 fixed in code, rest recorded in §6 |
| 2 | Backend 1:1: pure and mixed crates, in the order of §5 | `test/phase-2-backend` | done 2026-09-09: every non-exempt Rust file at or above 1.0 |
| 3 | Frontend 1:1: shared helpers, then every `src/web` module | `test/phase-3-frontend` | done 2026-09-09: 20 JS files all tested, 605 JS cases, total ratio 2.32 |
| 4 | E2E smoke: mock Jellyfin server, CDP driver, bundled clip, CI job | `test/phase-4-e2e` | done 2026-09-09: 10 scenarios in ~20 s, muted by default, CI job manual-only (`docs/e2e.md`) |
| 5 | Platform glue: pure extractions in `windows`, `macos`, `wayland`, `x11`, `gpu_paint`, `jfn_cef`; finalise exemptions | `test/phase-5-platform` | planned |

### Phase 1: security audit surfaces

Reviewed in this order, each producing tests (and a fix where warranted):

1. `src/jfn_cef/src/business_web.rs`, `business_overlay.rs`, `business_common.rs`,
   `ipc.rs`: every JS -> Rust message, its argument coercion and bounds.
2. `src/instance_ipc/src/lib.rs`: malformed, oversized and partial frames from
   a second process.
3. `src/paths/src/lib.rs` and `src/paths/src/migrate.rs`: env overrides,
   atomic writes, traversal in imported profiles.
4. `src/config/src/lib.rs`: `settings.json` parsing, unknown and wrong-typed
   values, the `videoModeLibraries` map.
5. `src/js_json/src/lib.rs` and `src/jfn_cef/src/resource.rs`: JS-embedding
   escapes for every call site, `app://` lookup.
6. `src/jellyfin/src/lib.rs`: server URL normalisation and probe-response
   parsing against hostile bodies.
7. `src/logging/src/redact.rs`: token redaction completeness.

### Phase 2: backend order

Pure crates first, then the mixed ones, then the mpv control plane:
`js_json`, `mailbox`, `color`, `compositor_core`, `input`, `jellyfin`,
`paths`, `config`, `logging`, `instance_ipc`, `playback`, `platform_abi`,
`mpris`, `mpv` (pure halves: `boot`, `event`, `api` value mapping),
`jfn_cef` (non-FFI: `injection`, `browsers`, `paint_scheduler`, `resource`,
`business_*`), `jfn_rust` (`app::resolve_startup_options`, `manager`),
`linux_util` (`menu/render`, `keysym`, `dmabuf_probe` parsing), `wake_event`,
`xtask` (`version`, `fs`, `template`).

### Phase 3: frontend

`src/web/test/player-fakes.js` (fake DOM sufficient for the modules, fake
`playbackManager`/`Events`/`ApiClient`, fake `jmpNative` recorder, and
`loadModule(file, window)`, which evaluates an injected script inside
`with (window)` and returns its `module.exports`, so every test gets a fresh
window), then tests for
`native-shim`, `mpv-video-player`, `mpv-player-base`, `mpv-audio-player`,
`client-settings`, `overlay`, `connectivityHelper`, `csd`, `select-menu`,
`about`, `astrofin-theme` (idempotency across repeated context creation),
`overlay.lang`, and the three `dev/tools/brand/*.mjs` scripts. Every module
gets the `module.exports` shim the four tested ones already have.

### Phase 4: E2E smoke

`dev/e2e/`: a Node mock Jellyfin server (system info, auth, user views,
items, playback info, sessions, a bundled short clip generated by the
bundled mpv encoder at harness setup), a CDP driver that launches the staged
`build/astrofin.exe` with a throwaway profile and `--remote-debug-port`, and
scenarios: launch and version probe, connect overlay -> mock server ->
login -> home rendered, start playback -> mpv reports playing -> pause/seek
-> stop, settings write round-trip, second-instance forwarding, clean quit
with zero `ERROR` log lines. `just e2e` locally (`node dev/e2e/run.mjs`
without `just`); the self-hosted runner has a desktop session, so
`.gitea/workflows/e2e-windows.yml` exists but is `workflow_dispatch`-only
because the suite takes the developer's screen. Details, the pinned
jellyfin-web build and the known gaps: `docs/e2e.md`.

## 5. Out of scope for now

Fuzzing, property-testing crates, line-coverage gates, jellyfin-web itself,
tvOS. Revisit after phase 5.

## 6. Phase 1 findings left as recommendations

Fixed in code and pinned by tests: non-http(s) URLs into the main layer,
the saved server URL and the mpv media URLs; unescaped JS splices of the
server URL and settings blob; unchecked IPC list reads; unbounded probe
body and stale probe results; the `//web` base-URL collapse; redaction
gaps; log file mode; unbounded instance-ipc frames; instance-id validation;
empty `--config-dir`; silent settings reset (now logged).

Recorded, needing a design decision before they change behaviour:

- Probe redirects: the connect overlay saves and navigates to whatever host
  a redirect chain ends on; the response is not bound to the host asked for.
- Plain `http://` is the default for a bare host, with no warning that the
  token then travels in clear.
- `aboutOpenPath` launches the path through the shell unescaped; only the
  about layer binds it and there is no `OnBeforeBrowse` handler pinning
  which URLs a layer may navigate to.
- `jmpNative` is bound per browser, not per origin, so any document the
  main layer lands on gets the player IPC surface.
- Handler log lines embed page strings without newline escaping.
- `openConfigDir`, `appExit` and `setSettingValue` are unthrottled.
- instance-ipc: no backoff on a persistent `accept()` error, no cap on
  concurrent connections or idle timeout, and `Drop` does not wake a
  `serve` task blocked in `recv`.
- On Windows the pipe namespace is global: a squatter who learns the
  instance id can make the app exit as "already running".
- `settings.json`: a duplicate key or out-of-range number still fails the
  whole document (now logged); `read_to_string` has no size cap;
  `windowScale` is not validated; four accessors panic if no Platform is
  installed and the renderer links the crate.
- Legacy-profile import: symlinks are copied through on Windows without
  symlink privilege; no size or free-space check.
- `paths::ensure` swallows `create_dir_all` errors; a relative
  `ASTROFIN_CONFIG_DIR` follows each process's cwd.
- A secret split across two log records is not redacted.

Found during phase 2 (also design calls):

- instance-ipc on Windows: rebinding immediately after `Listener::shutdown()`
  can find the name still taken while the probe gets `ENOENT` rather than
  `ConnectionRefused`, so it classifies as failed instead of stale.
- `Mailbox::wait`'s `take` closure mutates under the lock but does not
  notify; a two-sided handshake must publish with `update` on both sides.
- Ratio counting: trait-impl methods (`Write`, `Drop`, `Visit`, ...) count as
  public surface; kept, since each impl is a behaviour worth a test.
- `Platform::install_shutdown_handler`'s Unix default installs real signal
  handlers and is not exercised in tests.
