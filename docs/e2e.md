# E2E smoke suite

Phase 4 of the test plan (`docs/test-plan.md` §4). The unit suites cover pure
functions; this one covers what they exempt — CEF boot, the two browser layers,
the JS↔Rust bridge, libmpv, the GPU paint path, `settings.json` persistence, the
instance IPC and the shutdown sequence — by driving the **staged `build/` tree**
against a **mock Jellyfin server** over the Chrome DevTools Protocol.

Nothing here touches the user's real profile or a real Jellyfin server.

## Running it

```
just e2e                       # build, then setup + every scenario
just e2e --only playback       # one scenario file
node dev/e2e/run.mjs           # same, without rebuilding (no `just` needed)
node dev/e2e/run.mjs --list    # the scenario files, in run order
node dev/e2e/setup.mjs         # just the cache priming
```

Requirements: node ≥ 22 (global `WebSocket`; developed on 26.4), a staged
`build/astrofin.exe`, the mpv built from the submodule (once — see *Fixtures*),
and **a desktop session**: the app opens a real window and takes focus for the
length of each scenario. There are no npm dependencies; everything is node
built-ins plus Windows' own `tar.exe`.

A full run on the developer PC takes **about 20 s** for 10 tests across 6
scenario files, plus a one-off ~32 MB download the first time.

### Audio

The suite is **muted by default**. Each launch gets an mpv.conf in its
throwaway profile containing `ao=null` (plus `volume=0`/`mute=yes`); `ao` is the
lever that holds, because jellyfin-web pushes its saved volume into mpv the
moment a player is created and would undo a volume-only mute. mpv's null output
still consumes samples in real time, so playback timing — which is what the
scenarios assert on — is unchanged, and scenario 3 asserts on mpv's own
`AO: [null]` log line rather than trusting it.

```
E2E_AUDIO=1 just e2e           # opens a real audio device instead
```

### Isolation rules

- **One app instance at a time.** The app is single-instance per config dir and
  each launch owns an mpv window and a GPU context, so `run.mjs` passes
  `--test-concurrency=1` and every scenario disposes its instance in a
  `finally`. Do not run the suite next to a `just run`.
- **Throwaway profile per launch.** A fresh `mkdtemp` directory is passed as
  both `--config-dir` and `--cache-dir`, and every `ASTROFIN_*` variable is
  stripped from the child's environment, so a developer shell that exports one
  cannot redirect a run at the real `%APPDATA%\astrofin`.
- **Teardown kills the process tree** (`taskkill /T /F`) and removes the
  profile, retrying while CEF still holds the cache dir open.

## Layout

| Path | What |
|---|---|
| `dev/e2e/run.mjs` | the one command: setup, then the scenarios serially |
| `dev/e2e/setup.mjs` | downloads/verifies jellyfin-web, encodes the clip |
| `dev/e2e/jellyfin-web.json` | the pinned jellyfin-web version, URL and SHA256 |
| `dev/e2e/lib/paths.mjs` | repo locations, the mpv CLI search order |
| `dev/e2e/lib/cdp.mjs` | the DevTools Protocol client (`waitFor`, `evaluate`) |
| `dev/e2e/lib/app.mjs` | launch / profile / log / kill, `AppInstance` |
| `dev/e2e/lib/mock-server.mjs` | the mock Jellyfin server |
| `dev/e2e/lib/fixtures.mjs` | the one user, library, item and media source |
| `dev/e2e/lib/flow.mjs` | connect, sign in, Home, play — against the real UI |
| `dev/e2e/lib/harness.mjs` | `withApp`: mock + app up, torn down whatever happens |
| `dev/e2e/scenarios/*.test.mjs` | the scenarios, as `node:test` files |

Caches live in `.cache/e2e/` (already covered by the `.cache/` line in
`.gitignore`, which is also where the CEF download goes). Throwaway profiles
live under the OS temp directory, never in the repo.

## Scenarios

| File | Asserts |
|---|---|
| `01-launch` | `--version` prints app/CEF/mpv/FFmpeg and exits 0; a launch brings up the debugger, both browser layers, and `jmpNative` on the main layer; the run log carries the version banner |
| `02-connect-login-home` | the overlay's HEAD→GET probe hits the mock, the URL is saved to `settings.json`, jellyfin-web loads; sign-in authenticates exactly once, Home renders with the fixture library, `/Sessions/Capabilities/Full` and the `/socket` WebSocket happen, and there are **no page errors and no unmodelled requests** |
| `03-playback` | play → mpv reports position, duration and a DirectPlay `/Sessions/Playing`; mpv opened the null audio output; pause → mpv reports paused, a paused progress report follows, and the position stops moving; seek → mpv resumes past the target without overshooting; stop → `/Sessions/Playing/Stopped` past the seek point and the player drops its media |
| `04-settings` | `window.api.settings.setValue` reaches `settings.json` (string and boolean), and a **second process** on the same profile reads both back out of the injected settings blob; an unknown key is refused, logged, and leaves the file untouched |
| `05-second-instance` | a second launch on the same profile logs `Signaled existing instance, exiting`, exits 0, and leaves the owner running and debuggable; once the owner is gone the profile can be claimed again rather than handed off to a corpse |
| `06-clean-quit` | the whole journey (connect → sign in → play → stop) then `jmpNative.appExit()` → exit code 0 with **zero `ERROR` lines** anywhere in the run log |

Every playback assertion reads state the app itself published — the values mpv
pushed into `window._mpvVideoPlayerInstance` via `_nativeUpdatePosition` and the
`paused` signal, or the reports the client posted to the mock — never a timer.

## What is mocked, what is real

**Real:** the staged `astrofin.exe` and every process it spawns; CEF and its
renderer; libmpv, its decoders and its GPU output; the connect overlay; the
injected shims (`native-shim.js`, `mpv-video-player.js`, …); `settings.json`
and `instance.json` handling; the instance IPC; the shutdown path. And
jellyfin-web itself — the real, pinned, production build.

**Mocked:** the Jellyfin server. `dev/e2e/lib/mock-server.mjs` is a `node:http`
server that serves the jellyfin-web build at `/web/` and implements the API by
hand:

- `/System/Info/Public`, `/System/Info`, `/System/Endpoint`, `/Branding/*`,
  `/QuickConnect/Enabled`, `/Localization/*`, `/Plugins`, `/Auth/Keys`
- `/Users/Public` (empty, so the manual sign-in form is what shows),
  `POST /Users/AuthenticateByName` (one user, one password), `/Users/Me`
- `/UserViews`, `/Users/{id}/Views`, `/Users/{id}/Items[/Resume|/Latest]`,
  `/Items`, `/Items/{id}`, `/Items/{id}/{Similar,ThemeMedia,Intros,Ancestors}`,
  `/Shows/NextUp`, `/LiveTv/*`, `/Persons`, `/Genres`, …
- `GET|POST /DisplayPreferences/{id}` (writes are kept)
- `POST /Items/{id}/PlaybackInfo` — always offers direct play
- `/Videos/{id}/stream*` — the generated clip, with byte-range support, because
  mpv issues a ranged GET on every seek
- `/Sessions`, `/Sessions/Capabilities/Full`, `/Sessions/Playing[/Progress|/Stopped]`
  — the reports are recorded and reshape the `/Sessions` record the app's own
  transcode indicator polls
- any `*/Images/*` → a 1×1 PNG
- the `/socket` WebSocket upgrade is answered (no frames are ever pushed)

Anything else is logged in `mock.unhandled` and answered with something inert;
scenarios 2 and 3 assert that list is **empty**, so an endpoint jellyfin-web
starts calling shows up as a test failure rather than as a silent 404. That is
also how to extend the mock: run the suite, read `unhandled`, add the route.

## Fixtures

**jellyfin-web.** The app does not bundle a web client — a real Jellyfin server
serves it at `/web/` — so the mock has to. jellyfin-web's own GitHub releases
carry no built assets, so the build is taken from the Jellyfin server's Debian
package, the smallest published archive that contains a compiled client (~32 MB
against the ~168 MB Windows server zip). It is architecture-independent
(`_all.deb`) and unpacked with Windows' built-in `tar.exe`, which reads both the
`ar` container and the inner `data.tar.xz`.

Pinned in `dev/e2e/jellyfin-web.json`:

| | |
|---|---|
| version | **10.11.11** |
| url | `https://repo.jellyfin.org/files/server/debian/stable/v10.11.11/amd64/jellyfin-web_10.11.11+deb12_all.deb` |
| sha256 | `f289b91e47cb44e52630c6a4262f3bdfe67a53c05db8f560edf479300ed0a37a` |

10.11 is the line Astrofin's device profile is written against
(`src/jellyfin/src/lib.rs`). To bump: edit the version/URL, run
`node dev/e2e/setup.mjs --print-sha256`, paste the digest back. A mismatch
deletes the download and fails setup.

**The clip.** `setup.mjs` encodes it with the mpv built from the submodule — no
ffmpeg needed — from `lavfi` `testsrc2` plus a 440 Hz sine, 30 s of 640×360
h264 + AAC in mp4, ~4 MB, in well under a second. The length lives in
`lib/fixtures.mjs` as `CLIP_SECONDS` and is what the mock advertises as the
item's runtime; a stamp file re-encodes when it changes. The mpv CLI is looked
for in `build/mpv-build/` first and `third_party/mpv/build/` second; once the
clip is cached, no mpv CLI is needed again.

## CI

`.gitea/workflows/e2e-windows.yml`, **`workflow_dispatch` only**.

The runner was checked and does have a desktop: `act_runner` runs from the
"Gitea Act Runner" scheduled task as `NoahM` with `LogonType: Interactive`, in
the same session as `explorer.exe`. So the job *can* run. It is not wired to
`push` because the runner is the developer's own PC:

- the suite opens a real window and takes focus for ~20 s per launch;
- only one app instance may exist per machine, so a CI run overlapping a
  developer's `just run` breaks both (hence the `astrofin-app-instance`
  concurrency group);
- the desktop only exists while that session is logged in, so an unattended run
  after a reboot would fail for reasons unrelated to the change.

Revisit if the suite ever gets a dedicated machine or a headless GPU session.

Run it from the Gitea UI or:

```
gh workflow run e2e-windows.yml -R TheHalfrican/Astrofin --ref <branch>
```

## Known limitations

- **Windows only, in practice.** The launcher, the process-tree kill and the
  `.deb` unpack all take the Windows path; the Linux/macOS branches exist
  (`bsdtar`, process-group kill) but have never been run.
- **No transcoding.** `PlaybackInfo` always offers direct play, so the HLS
  path, the transcode indicator's "Transcoding" state and `forceTranscoding`
  are not exercised end to end. Adding them means serving a real HLS ladder
  from the mock.
- **One item, one library, one user.** Queues, next-up, episodes, audio-only
  playback, subtitles and external tracks are not covered.
- **The UI is driven by selector.** `#txtManualName`/`#txtManualPassword` and
  `.btnPlay` are jellyfin-web's, so a jellyfin-web bump can break the flow
  helpers; that is deliberate (it is the same path a user takes) but it is the
  most likely source of a false failure after a version bump.
- **Timing is machine-dependent.** The waits are generous (20–90 s) and every
  assertion polls rather than sleeping, but a much slower machine may need the
  `bootTimeoutMs`/`timeoutMs` defaults raised.
- **No screenshots on failure.** A failing scenario appends the app's last
  `ERROR` log lines to the assertion message; the profile (and its full log) is
  deleted on teardown. If that proves too thin, `withApp({ keepProfile: true })`
  is the hook.
