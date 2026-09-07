# Video modes

**Video mode** is an in-app upscaling preset, in Settings → Playback. It
replaces the old out-of-repo workflow (a PowerShell script that rewrote
`mpv.conf` and two Start-menu shortcuts that restarted the app) with a setting
that switches **live**, mid-playback, through libmpv.

| Mode | `glsl-shaders` | `scale` | `dscale` |
| --- | --- | --- | --- |
| `movies` (default) | `FSRCNNX_x2_16-0-4-1.glsl` | `ewa_lanczossharp` | `mitchell` |
| `anime` | Anime4K v4.0.1 Mode A (HQ), six files in order | *baseline* | *baseline* |
| `off` | *baseline* | *baseline* | *baseline* |

*baseline* = whatever the user's `mpv.conf` (or mpv's own default) had at
startup; see [Baseline and restore](#baseline-and-restore).

Both shader chains gate themselves on the output/source size ratio — FSRCNNX
above ~1.3x, Anime4K above ~1.2x — so neither costs anything on content that is
already at display resolution. Leaving either mode on permanently is safe; that
is why `movies` is the default on a fresh install rather than `off`.

## Where the shaders come from

Two locations, checked per mode:

1. **User override** — `<config dir>/mpv/shaders/`, flat. Used when *every*
   file the mode needs is present there. All-or-nothing per chain, so a user's
   updated Anime4K set is never half-mixed with the bundled one.
2. **Bundled** — `<resource dir>/shaders/<anime4k|fsrcnnx>/`, shipped with the
   app.

`<config dir>` is `%APPDATA%\astrofin` (Windows), `~/.config/astrofin`
(Linux/macOS), or whatever `--config-dir` says. `<resource dir>` is the
executable's own directory on Windows and Linux, and `Contents/Resources`
inside the macOS app bundle (`jfn_paths::resource_dir`).

`cargo xtask build` stages `resources/shaders/` into `build/shaders/`;
`cargo xtask install` carries it to the install prefix, the macOS installer
moves it into `Contents/Resources/shaders`, and the AppImage script copies it
next to the binary in `usr/bin/`.

A shader file that is missing from the resolved directory is logged at `warn`
and dropped from the chain; the rest of the chain still applies. Nothing here
ever panics or aborts a launch.

Provenance and licensing of the bundled files: `resources/shaders/README.md`.

## How the chain is applied

mpv keeps the chain in the `glsl-shaders` **path-list option**, so it is set by
writing that property — no file is loaded, no restart is needed, and mpv
recompiles on the next rendered frame.

- Value syntax: entries joined by mpv's path separator, `;` on Windows and `:`
  elsewhere (`OPTION_PATH_SEPARATOR` in `third_party/mpv/options/m_option.c`).
  Windows paths are written with forward slashes. A literal separator inside a
  path is escaped as `\<sep>`, which is what `get_nextsep` expects.
- The write is `mpv_set_property_async` with `reply_userdata == 0` —
  fire-and-forget, never a sync call (see the CLAUDE.md deadlock rule).
  `change-list glsl-shaders append` would also work but takes one command per
  file, leaving a frame or two of partial chain.
- Right after each apply the code re-reads `glsl-shaders` asynchronously and
  logs what came back, so a run log proves what mpv actually accepted:

```
INFO mpv: video mode anime applied: scale=<mpv default> dscale=<mpv default> shaders=[…]
INFO mpv: video mode anime: mpv reports glsl-shaders=…
```

Boot order (`src/jfn_rust/src/app.rs::run_app`): the baseline reads and the
first apply are queued straight after `mpv_initialize`, before any file is
loaded and before the VO wait — mpv holds the chain as plain options, so it is
in force for the first frame of the first video.

### Baseline and restore

`off`, and the scalers under `anime`, mean "put back what `mpv.conf` said".
Those values are only knowable after `mpv_initialize` has parsed the config,
and reading them synchronously there would park the caller on mpv's core thread
while the VO is still coming up. So the module fires three async property reads
(`glsl-shaders`, `scale`, `dscale`) and latches the replies.

That is race-free because libmpv funnels `mpv_get_property_async` and
`mpv_set_property_async` through the same dispatch queue (`run_async` →
`mp_dispatch_enqueue` in `player/client.c`): the reads queued first are served
first, so the baseline reflects the config file even though the boot apply does
not wait for it. The replies are consumed by whichever pump is running — the
boot pump in `app.rs::consume_boot_event` before the ingest thread exists, and
`jfn_playback::ingest_driver::ingest_events` afterwards.

## Setting, CLI flag, and the web UI

- **settings.json**: `"videoMode": "movies" | "anime" | "off"`. Absent means
  "never chosen" and resolves to the default (`movies`).
- **CLI**: `--video-mode movies|anime|off`, overriding the stored value for
  that run. An unrecognised value falls back to the default with a warning
  rather than failing the launch.
- **Web UI**: `jmpInfo.settings.playback.videoMode` is readable by page
  scripts, and `jmpInfo.settingsDescriptions.playback` carries the select.
  Changing it calls `window.api.settings.setValue('playback', 'videoMode', v)`
  → IPC `setSettingValue` → `business_common::apply_setting_value`, which
  persists the value *and* applies it to the live mpv handle. It is the only
  setting that takes effect without a restart.

Switching from a page script, e.g. for testing:

```js
window.api.settings.setValue('playback', 'videoMode', 'anime');
```

## Legacy `mpv.conf` repair

The first-run import from a Jellium Desktop profile copies `mpv/mpv.conf`
byte-for-byte, so absolute paths in it still point into
`…/jellium-desktop/mpv/shaders`. Two things fix that, both in
`src/paths/src/migrate.rs`:

1. **At import time** the copied `mpv.conf` has every absolute reference to the
   legacy config directory rewritten to the new one — both slash styles, case
   insensitively on Windows, with everything else left byte-identical
   (`input-ipc-server=\\.\pipe\jellium-mpv` included; it is harmless).
2. **At every launch**, `repair_mpv_conf()` does the same for installs migrated
   before that existed. It is idempotent and conservative: it acts only when
   the file names the legacy directory *and* at least one rewritten path
   resolves to a real file under the current config directory, and it leaves a
   one-time `mpv.conf.bak` beside the file. Otherwise it does nothing and says
   so once at `info`.

Neither path ever touches the legacy tree.

Note that a repaired `mpv.conf` still sets a `glsl-shaders` chain of its own.
That becomes the *baseline*: `off` restores it, and it is what the app falls
back to when a mode's own files are missing.

## Adding a mode

1. Put the shader files under `resources/shaders/<project>/` with their
   upstream headers and license intact, and describe them in
   `resources/shaders/README.md`.
2. In `src/mpv/src/video_mode.rs`: add the variant, its arm in `as_str`,
   `parse`, `options`, `shader_files`, and `bundled_subdir`, and — if it needs
   scalers — its arm in `apply`. The unit tests in that file cover the wire
   round-trip and the chain order.
3. Add the option to the `videoMode` select in `src/web/native-shim.js`.

Nothing else needs touching: staging, the CLI flag, persistence, and the live
switch are all driven off those lists.
