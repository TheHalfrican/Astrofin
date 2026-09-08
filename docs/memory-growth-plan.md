# Windows memory growth during long playback — hypotheses and plan

Status: **open, unmeasured** (2026-09-08). Inherited from upstream
jellium-desktop issue #643; nothing in the fork has touched it. This document
is the starting point for the next session: read the symptom, run the
measurement harness in §3 first, then use §4 to decide which hypothesis in §2
survives before changing any code.

## 1. The symptom (upstream #643, sanitised)

Windows, direct-played 1080p 10-bit HEVC MKV through `gpu-next`, several
consecutive episodes, about 80 minutes of uptime:

| Process | Working set | Private | Notes |
| --- | ---: | ---: | --- |
| Main `jellium-desktop` | 2,423 MB | 2,612 MB | owns the window |
| Largest helper | 150 MB | 401 MB | no window (renderer, most likely) |
| Other four helpers | 380 MB | 471 MB | |
| **Total** | **2,953 MB** | **3,485 MB** | |

The decisive detail: sampled 15 s apart **while paused**, the main process
grew another 4.7 MB private / 4.4 MB working set. That is roughly 19 MB/min
with no decoding, no presentation of new video frames, and no position
updates. Whatever leaks is driven by something that keeps ticking while
paused, not by the decode loop.

What lives in the main process, and therefore what the 2.6 GB can be made of:

- the Rust app (`jfn_rust`, `jfn_playback`, `jfn_mpv`, `jfn_config`, …);
- **libmpv in-process** (demuxer, decoder, `vo_gpu_next`/libplacebo, its own
  window and D3D11/Vulkan device);
- the **CEF browser process** (the OSR paint pipeline lands here, plus every
  `CefProcessMessage` from the renderer);
- the **wgpu compositor** (`jfn_gpu_paint`) that paints CEF's overlay above
  the video.

The helpers are CEF's renderer, GPU, utility and crashpad processes. The 401 MB
helper is a separate question (§2.6) and must be measured separately.

## 2. Hypotheses, ranked

Each entry: mechanism, where it lives, whether it fits "grows while paused",
the cheapest test that would falsify it, and the fix if it holds.

### 2.1 The overlay paint pipeline never reclaims what it uploads (most likely)

CEF repaints the UI on every invalidation and hands us either a BGRA buffer
(`on_paint`, `src/jfn_cef/src/client/paint.rs`) or a D3D11 shared-texture
handle (`on_accelerated_paint` → `client/accel.rs::acquire`). The frame goes to
`jfn_gpu_paint::painter`, which uploads copied frames with
`queue.write_texture` (painter.rs ~789) and submits (painter.rs ~659).

Two sub-mechanisms, either sufficient:

- **wgpu is never polled.** `painter.rs` has `queue.submit` but no
  `device.poll`. wgpu reclaims staging memory and destroyed resources only when
  the device is maintained; depending on the wgpu version, `submit` may or may
  not do that implicitly. If it does not, every `write_texture` leaves a
  staging allocation behind until something polls. A 1080p overlay is 8 MB per
  full paint; a 4K@300% one (this machine) is 33 MB.
- **Shared-handle imports pile up.** On Windows `acquire` passes CEF's raw
  handle straight to the compositor, which opens it inline
  (`gpu_paint/src/shared/dx12.rs`). Every open creates a new device resource;
  if the previous frame's import is not released before the next one is
  created (or is released only on drop of a value that is cached), GPU-side
  memory and the process handle table both grow at the paint rate.

Fits paused? **Yes, if CEF keeps painting while paused.** It does, for two
reasons: jellyfin-web keeps the OSD visible while paused (it only auto-hides
during playback), and the Astrofin theme animates star layers and a 38 px
backdrop blur behind everything (`docs/design-brief.md`, redesign follow-up
F10). `PaintScheduler` (`src/jfn_cef/src/paint_scheduler.rs`) also runs an
invalidate loop after resizes (`INVALIDATE_TICK_LIMIT = 1000`) and an
`ActivePaintScheduler` in shared-texture mode. Note the upstream report
predates the theme, so the pre-theme paint rate was lower but not zero.

Falsify: log a counter of `on_paint`/`on_accelerated_paint` calls per minute
alongside the memory samples. If memory growth tracks the paint count (not
wall time), this is it. Then run once with `--disable-gpu-compositing` (the
software path) and once with shared textures forced off; if only one of the
two grows, the sub-mechanism is identified.

Fix: (a) `device.poll(wgpu::Maintain::Poll)` after each present, or use a
`StagingBelt` recycled per frame; (b) hold at most one imported shared
texture per surface and release the previous import explicitly before opening
the next; (c) independent of the leak, stop repainting the whole overlay when
nothing changes — throttle the theme animations while the player is up (that
is redesign follow-up F10 anyway).

### 2.2 libmpv / libplacebo growth under `gpu-next`

mpv's `vo_gpu_next` keeps a shader cache and libplacebo's own GPU allocator
pools; several mpv 0.37–0.39 bug reports describe steady growth on Windows
with D3D11 and Vulkan, some fixed upstream, some not. Our mpv is a fork at
`third_party/mpv` (0.41.0-UNKNOWN) built against MSYS2's libplacebo
(7.360.x) and ffmpeg 9.

Fits paused? **Partly.** A paused mpv does not decode, but it *does* redraw the
last frame whenever the window is invalidated, and every redraw goes through
the full libplacebo pipeline. On Windows a window under an always-repainting
overlay may get WM_PAINT more often than expected. Also, `demuxer-cache-state`
keeps changing while the demuxer prefetches after a pause until the cache
(150 MB forward + 50 MB back by default; nothing overrides it in the user's
mpv.conf) is full, so growth *should* plateau within a minute of pausing —
if it does not, the cache is not the answer.

Falsify: run the harness with `vo=gpu` instead of `gpu-next` (mpv.conf
override), then with `hwdec=no` instead of `auto-copy`. Growth that disappears
under one of them lives in mpv, not in us. Separately, `just run-mpv` plays
the same file in the standalone mpv built from the submodule; if that grows
alone, it is purely an mpv/libplacebo issue and we should bisect the
submodule or the MSYS2 libplacebo version.

Fix: pin/upgrade libplacebo or mpv in `build_mpv_source.ps1`, or carry an mpv
patch; none of it in our Rust code.

### 2.3 Unbounded event channels backing up

`jfn_mpv::event_loop` and `jfn_playback::coordinator` both use
`crossbeam_channel::unbounded`. The ingest driver observes fourteen properties
(`ingest_driver.rs` ~180), including `time-pos` (every frame while playing)
and `demuxer-cache-state` as an `MPV_FORMAT_NODE` (deep-copied per event,
digested by `ingest.rs::digest_cache_state`, which extracts seekable ranges —
a list that grows over a long file). If a consumer stalls — a JS round trip
under load, a blocked `send_process_message`, an SMTC call — the queue grows
without bound and nothing ever shrinks it.

Fits paused? **Weakly.** `time-pos` stops while paused; `demuxer-cache-state`
does not until the cache fills.

Falsify: log `rx.len()` of both channels once a minute. A queue that is not
near zero is the bug.

Fix: bounded channels with coalescing for the high-rate properties (only the
latest `time-pos` matters), and observe `demuxer-cache-state` only while
something consumes it.

### 2.4 Logging

`--log-level debug` writes every position tick and every CEF console line.
If the tracing writer is non-blocking with an unbounded buffer, or the log
file grows in a memory-mapped way, memory follows the log.

Fits paused? Only if something logs while paused; with `demuxer-cache-state`
at debug it does.

Falsify: run the harness at `--log-level warn`. Trivial to check, so do it in
the first matrix.

### 2.5 Windows media transport (SMTC) sink

`src/windows_sink` rebuilds the thumbnail stream only when the item changes
and caches it; timeline updates are throttled. Nothing here should grow, but
every position tick still crosses into WinRT.

Falsify: not worth a dedicated run; if 2.1–2.4 are ruled out, run once with
the sink disabled.

### 2.6 The renderer process (the 401 MB helper)

jellyfin-web's player page attaches listeners and updates the DOM on every
position tick; our injected scripts (native shim, the theme, the video-mode
resolver's session cache) add their own. Growth there is a JS heap problem,
not ours to fix in Rust, but it is ours to detect.

Falsify: with `--remote-debug-port 9223`, take a DevTools heap snapshot at
t=0 and t=2 h (`HeapProfiler.takeHeapSnapshot` over CDP; the capture harness in
the previous session's scratchpad already speaks CDP) and diff retained
sizes by constructor.

## 3. Measurement harness — run this before touching code

One session on the user's 4K@300% machine, a copied profile
(`--config-dir`/`--cache-dir`, see `worktree-build-recipe` memory), the same
kind of file as the report (1080p HEVC, direct play). Timeline: 90 min
playing across at least two episodes, then 30 min paused, then 10 min on the
Home screen with the player closed.

Sample every 30 s for every process in the app's tree
(`Get-CimInstance Win32_Process | ? { $_.Name -like 'astrofin*' }` gives the
parent/child set): private bytes, working set, handle count, GDI/USER objects,
thread count, and GPU memory from the
`\GPU Process Memory(pid_*)\Dedicated Usage` and `Shared Usage` counters.
Write CSV; plot private bytes and GPU dedicated per process against time and
annotate the play/pause/close boundaries.

Add three app-side counters to the same clock (temporary `info` logs are
fine): paints per minute (from `on_paint`/`on_accelerated_paint`), the two
channel lengths (§2.3), and mpv's `demuxer-cache-state` `fw-bytes`.

Toggle matrix, one run each, only after the baseline run is plotted:

| Run | Change | Isolates |
| --- | --- | --- |
| A | baseline (`gpu-next`, `hwdec=auto-copy`, debug log, shared textures) | — |
| B | `--log-level warn` | §2.4 |
| C | `--disable-gpu-compositing` (software overlay path) | §2.1 sub-mechanism |
| D | mpv.conf `vo=gpu` | §2.2 |
| E | mpv.conf `hwdec=no` | §2.2 / decoder pools |
| F | `just run-mpv` on the same file, no app at all | §2.2 vs. everything else |

Run B–F only as long as the baseline still grows; stop as soon as one run is
flat, that is the culprit's neighbourhood.

## 4. Decision tree

- **Main process grows, helpers flat, growth tracks paint count** → §2.1.
  Confirm with run C; fix in `jfn_gpu_paint` and throttle theme animations.
- **Main grows, flat under run D or E or F** → §2.2; fix in the mpv build,
  not in Rust.
- **Main grows linearly with time regardless of paints, channel lengths
  non-zero** → §2.3.
- **Flat under run B** → §2.4 (and stop shipping debug logging by default).
- **A helper grows, main flat** → §2.6; heap-snapshot diff, then fix in the
  injected JS or report to jellyfin-web.
- **Everything flat in the fork** → the upstream bug predates the redesign and
  the fork's CEF/mpv bumps may have fixed it by accident; close with the plot
  attached.

## 5. Notes for whoever runs this

- The paint counter is the single most informative number; add it first.
- Do not measure with the capture harness's 1280×720 viewport override, it
  changes the paint size and the poster fetches (see `ui-redesign-branches`
  memory).
- Use a copied profile and a dedicated debug port so the user's real profile
  and any other running instance are untouched; stop instances by PID.
- Two hours of samples at 30 s is 240 rows; plot before reading the numbers.
