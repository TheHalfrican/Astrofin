# Windows memory growth (#643) — measurement findings

Two measurement sessions on the developer machine (i9-14900K, RTX 4090,
4K @ 300 %, Windows 11 26200), against Jellyfin 10.11.11 at
`192.168.50.76:8096`, direct-playing 1440×1080 HEVC 10-bit through
`vo=gpu-next` on `gpu-api=d3d11`:

1. **2026-09-08 00:56–01:05**, branch `exp/memory-harness` (`06ad823` off `main`
   @ `28beb98`) — the steady-playback baseline. Cut short at 8.7 minutes.
2. **2026-09-08 02:21–04:21**, branch `fix/shader-chain-vram` — seven item-transition
   runs, P0 and T1–T6, which are what this document now turns on.

> **Headline: #643 does not reproduce on this build, and the mechanism session 1
> proposed was wrong.**
>
> Session 1 concluded that dedicated VRAM stepped up by gigabytes at each item
> start because `video_mode::apply_inner` re-writes `glsl-shaders`
> unconditionally, so a binge of same-type episodes rebuilds the shader chain
> once per episode. **That claim was an inference from three transitions, and it
> failed a direct test.** T1 runs the *unfixed* build through ten same-type item
> starts and nothing grows — not private bytes, not board VRAM, not even the
> per-process counter that produced the original numbers. §5 shows why the
> inference was wrong; §6 shows why the original numbers were not measuring
> memory at all.

## 1. What the two sessions measured

Session 1 sampled one GPU signal: `\GPU Process Memory(pid_*)\Dedicated Usage`.
Session 2 samples three, deliberately:

| signal | what it is | trustworthy? |
| --- | --- | --- |
| `\GPU Process Memory(pid_*)\Dedicated Usage` | per-process, and what session 1 used | **no** — see §6 |
| `\GPU Adapter Memory(luid_*)\Dedicated Usage` | per-adapter; the number Task Manager's *Performance* pane shows | yes |
| `nvidia-smi --query-gpu=memory.used` | the driver's own board-wide accounting | yes |

The two adapter-level numbers agree with each other to within ~11 MB in every
sample of every run, which is the check that they are measuring the board and
not an accounting artefact. Private bytes and working set are sampled per
process throughout; the sampler runs at 10 s.

Every run used an isolated profile (`--config-dir`/`--cache-dir` copies) on
`--remote-debug-port 9225`, held the display awake via
`SetThreadExecutionState`, and never touched the user's real profile or
`mpv.conf`. The user's installed instance (pids 33032, 39272, 4888, 37520,
29288, 12272) sat idle on a non-player page throughout session 2 and was left
running: its rows are excluded from the per-process series by an executable-path
filter, and it contributes a constant, not a slope, to the two machine-wide
signals. Their residual noise is ±100 MB over a 15-minute run.

## 2. The runs

Each run is 10 real item starts — `loadfile`, via the details page and the play
button, never a seek — with a 120 s first item and 75 s afterwards.

| run | build | workload |
| --- | --- | --- |
| P0 | unfixed (`main`) | Auto, same-type episodes, 4 items, mpv at `debug` — probe for §5 |
| T1 | unfixed (`main`) | Auto, same-type episodes — **the control** |
| T2 | fixed | Auto, same-type episodes — the memo suppresses the re-writes |
| T3 | fixed | `videoMode` forced to alternate animation ↔ live-action each item — a **genuine** chain switch per item |
| T4 | fixed | T3 again with `interpolation-preserve=no`, which forces the `pl_renderer_flush_cache` §7 shows is otherwise skipped |
| T5 | fixed | one item, then 10 fullscreen toggles — the swapchain-resize path session 1 actually caught |
| T6 | fixed + the `background-color` memo | Auto, same-type episodes, 6 items — verifies §8's second change |

## 3. Results

MB per item start (or per switch, or per fullscreen toggle), least squares
against the item index, from the value sampled 60 s into each item (15 s for
T5's toggles):

| run | glsl writes | mpv rebuilds | main private | main WS | GPU ded (per-proc) | adapter ded | nvidia-smi |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| T1 (unfixed, same-type) | 11 | 23 | **+0.4** | +3.3 | −2.5 | +7.1 | +7.1 |
| T2 (fixed, same-type) | 2 | 23 | **+2.0** | +3.7 | −1.6 | +17.0 | +16.9 |
| T3 (fixed, real switch/item) | 11 | 32 | **+4.9** | +4.6 | +1.1 | +14.0 | +14.0 |
| T4 (T3, `interpolation-preserve=no`) | 11 | 32 | **+2.0** | +4.3 | −2.0 | +8.3 | +8.4 |
| T5 (fullscreen toggles) | 2 | 5 | **+3.8** | +3.5 | **+3599** | −23.6 | −23.6 |
| T6 (fixed + bg memo, 6 items) | 2 | 15 | **−3.6** | +3.0 | −6.0 | +51.6 | +51.5 |

Totals over each whole run, for the same columns:

| run | main private | adapter ded | nvidia-smi | GPU ded (per-proc) |
| --- | ---: | ---: | ---: | ---: |
| T1 | −15 MB | +102 | +102 | −46 |
| T2 | +33 MB | +295 | +294 | +1 |
| T3 | +63 MB | +53 | +53 | +5 |
| T4 | +10 MB | +101 | +102 | −41 |
| T5 | +57 MB | −576 | −576 | **+36 388** |
| T6 | −39 MB | +339 | +339 | −51 |

Nothing real grows in any run. Private bytes move by tens of MB with no
monotonic trend (T1's ten items read 1634, 1635, 1600, 1640, 1594, 1600, 1648,
1611, 1651, 1619 — oscillation, not a ramp). Board VRAM ends every run within
noise of where it started, and was back to 5 609 MiB with only the user's idle
instance left afterwards.

Read the two board-VRAM columns as noise, not signal: the machine-wide numbers
include the user's idle instance and the desktop, and their per-item slopes are
dominated by a single warm-up step at the first transition rather than by any
ramp. T6's +51.6 MB/item, the largest in the table, is one 7 359 → 7 759 step
between items 1 and 2 followed by 7 723, 7 749, 7 788, 7 697.

## 4. The binge case never leaked (T1 vs T2)

T1 is the unfixed build: `apply_inner` writes `glsl-shaders`, `scale` and
`dscale` on all eleven applies. T2 is the fixed build: the memo skips nine of
them, leaving two.

Private bytes, board VRAM and the per-process counter are flat in **both**.
Suppressing nine chain re-writes changed no memory number, because there was
nothing to suppress: §5 shows those writes never reached the renderer in the
first place.

## 5. Why the session-1 mechanism was wrong

An identical `glsl-shaders` write is already a no-op inside mpv. In
`third_party/mpv`:

- `video/out/gpu/video.c:547` — `{"glsl-shaders", OPT_PATHLIST(user_shaders),
  .flags = M_OPT_FILE}`, with no `.force_update`.
- `options/m_config_frontend.c`, `m_config_set_option_raw` notifies only
  `if (m_config_cache_write_opt(config->cache, co->data))`.
- `options/m_config_core.c:802` — `bool changed = !m_option_equal(...) ||
  opt->force_update;` and the group timestamp `gsrc->ts` is bumped only inside
  `if (changed)`.
- `options/m_option.c:1626` — `str_list_equal` compares the path list element by
  element with `strcmp`.
- `video/out/vo_gpu_next.c:887` — `update_options` calls `update_render_options`
  only when `m_config_cache_update` reports a change.

`scale` and `dscale` are `OPT_CHOICE_C` (`video.c:503`, `:505`) with
`int_equal`, so the same holds for them.

**The measurement agrees.** `update_render_options` logs "Render options
updated, flushing renderer cache." at mpv's `debug` level *only* when it
actually runs, so counting that line counts real rebuilds:

| run | glsl-shaders writes | rebuilds |
| --- | ---: | ---: |
| T1 (11 writes, all identical after the first) | 11 | 23 |
| T2 (memo: 2 writes) | 2 | **23** |
| T3 (11 writes, every one a real switch) | 11 | **32** |

Removing nine identical writes left the rebuild count **exactly unchanged**
(23 → 23). Making nine of them real switches added nine rebuilds (23 → 32).
Identical writes cost nothing; real ones cost one rebuild each, and leak
nothing (T3).

So what *are* those 23 rebuilds? Our own `background-color` writes. mpv
declares it at `video/out/gpu/video.c:537`, inside the same `gl_video_conf`
group, and the app writes it on every player open and every close — black for
playback, the theme colour otherwise. All three runs issue exactly **22**
`background-color` writes across ten items, and the arithmetic closes:

| run | background-color writes | real chain switches | rebuilds |
| --- | ---: | ---: | ---: |
| T1 | 22 | 1 (boot) | 23 |
| T2 | 22 | 1 (boot) | 23 |
| T3 | 22 | 1 + 9 | 32 |

21 of those 22 writes genuinely change the colour, because the two values
alternate; only the duplicated `#101010` at startup is redundant. §8 memoises
that one. Removing the other 21 would mean not repainting mpv's background on
every player open and close, which is a behaviour change rather than a guard,
and is left as a follow-up — it is wasted work, not a leak.

## 6. What session 1 actually measured: a documented bad counter

Microsoft KB4490156, *"GPU Process Memory counters report incorrect value"*
(Windows 10 1709 and later), states that the **GPU Process Memory** counter set
and Task Manager's *Details*-pane "Dedicated GPU memory" column "appear to show
memory leaks for running applications". The documented repro is an application
that discards GPU resources and re-creates them: the counter climbs without
bound while the *Performance* pane, WPR and WPA keep reporting the true value.

T5 demonstrates this on this machine beyond argument. Ten fullscreen toggles —
each a swapchain resize and a `gpu-next` reconfigure, i.e. exactly a
destroy-and-recreate cycle:

| toggle | GPU ded (per-proc) | adapter ded | nvidia-smi | private |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 3 182 | 7 665 | 7 654 | 1 661 |
| 3 | 10 193 | 6 966 | 6 956 | 1 670 |
| 5 | 18 692 | 7 102 | 7 091 | 1 684 |
| 7 | 25 641 | 7 027 | 7 016 | 1 688 |
| 10 | **36 033** | 6 964 | 6 953 | 1 693 |

The per-process counter grows ~3.5 GB per toggle and reaches **36 033 MB on a
24 564 MiB card** — 50 % more than the board physically has. It is fabricating.
Over the same toggles the adapter counter and nvidia-smi are flat (and drift
slightly *down*), and private bytes move 32 MB.

That is the same shape, on the same path, as session 1's "+1 202 / +5 867 /
+2 741 MB" steps. Re-reading session 1's log with this in hand: its third and
largest step carried **no shader write at all** — it landed on
`vo/gpu-next/d3d11: Disabling full-screen exclusive mode while leaving
fullscreen` and a 3840×2160 → 3840×2093 resize. The first two steps each
coincided with a genuine chain change *and* a VO reconfigure to a new source
size *and* a fullscreen transition. The correlation with `glsl-shaders` was
coincidental; the correlation with destroy-and-recreate was the real one, and it
was a counter artefact.

## 7. The mpv/libplacebo side, for the record

Investigated because the brief called for it. Nothing here needs changing.

- **A real chain switch does not flush the renderer cache during playback.**
  `vo_gpu_next.c:2703` ends `update_render_options` with
  `p->flush_cache = p->paused || !p->next_opts->inter_preserve;`, and
  `--interpolation-preserve` defaults to `true` (`vo_gpu_next.c:231`). T4 forces
  the flush with `interpolation-preserve=no` and is flat, exactly like T3 — so
  the skipped flush costs nothing. It would matter little anyway: in libplacebo
  7.360.1 `pl_renderer_flush_cache` (`src/renderer.c:193`) only destroys the
  frame-mixing textures and resets peak detection.
- **The FBO pool is bounded.** `get_fbo` (`src/renderer.c:447`) picks the
  best-fitting existing texture for the stage and appends a slot only when every
  slot is already live in this pass, then `pl_tex_recreate`s it. Slot count
  saturates at the heaviest chain's concurrent-FBO count; a chain switch resizes
  slots rather than adding them.
- **Parsed user shaders are cached, not re-parsed.** `load_hook`
  (`vo_gpu_next.c:2372`) keys `pl_hook`s by path for the VO's lifetime.
- **D3D11's deferred destruction is resolved every frame.**
  `pl_d3d11_tex_destroy` only releases, but `d3d11_gpu_flush` calls
  `ID3D11DeviceContext_Flush` (`src/d3d11/gpu.c:203`) and `vo_gpu_next` calls
  `pl_gpu_flush` at the end of every `draw_frame` (`vo_gpu_next.c:1506`). Per
  Microsoft's `Flush` documentation that destroys objects whose destruction was
  deferred, so no backlog accumulates.
- `gpu-api` is **d3d11** here, from the run log itself
  (`vo/gpu-next/d3d11: Initializing GPU context 'd3d11'`, RTX 4090), which is
  the autoprobe order in `video/out/gpu/context.c`. The user's `mpv.conf` was
  not modified, and no libplacebo change is warranted: 7.360.1 is the newest
  release and its release notes carry no renderer/hook VRAM fix.

## 8. What was changed, and why

Three changes, all small, none of them a leak fix — because there is no leak to
fix. They are justified as hygiene and by the rebuild counts above.

1. **`src/mpv/src/video_mode.rs`** — `apply_inner` memoises the last successful
   `(chain, scale, dscale)` and writes nothing when a resolution lands on the
   same triple, logging "unchanged, not re-applied". mpv would absorb the
   redundant writes anyway (§5); this makes the behaviour ours rather than
   borrowed from an mpv implementation detail, makes the log say what happened,
   and saves one libmpv dispatch round trip plus a read-back per item start.
   The chain is also normalised through `normalise_entry`, so the applied line
   and mpv's read-back line are the same spelling and can be diffed.
2. **`src/mpv/src/api.rs`** — `jfn_mpv_set_background_color_hex` memoises the
   last colour. Marginal by itself: the two colours alternate, so 21 of the 22
   writes per ten items are genuine changes and only the duplicated `#101010`
   at startup is removed. It is kept because it is free, because it guards any
   future caller that repeats a colour, and because it is where the finding in
   §5 belongs — these writes, not the shader chain, are what re-runs
   `update_render_options` twice per item.

   Verified by T6, the same workload as T2 on a build carrying this change: 13
   `background-color` writes across 6 items with **no consecutive duplicates**,
   against T2's 22 across 10 (2.2/item plus the startup duplicate). Exactly one
   write removed, as predicted, and memory flat as everywhere else.
3. **`src/mpv/src/memprobe.rs`** — kept from session 1, downgraded from `info`
   to `debug`. It is what eliminated the overlay paint pipeline (zero paints
   during playback), the unbounded channels and the demuxer cache, and it is the
   cheapest way to re-check them.

## 9. Status of #643

**Not reproducible on this build with these workloads.** The upstream report is
about *host* memory — 2 612 MB private in the main process after ~80 minutes and
several episodes — and:

- steady playback converges rather than grows (session 1: main private +9.92
  MB/min over the first 5 minutes but +1.79 MB/min over the last 2, with GPU
  dedicated bit-identical across eleven consecutive samples);
- item starts do not ratchet it (T1–T4: −15 to +63 MB total across ten items);
- genuine chain switches do not (T3), and neither do fullscreen/VO reconfigures
  (T5);
- the GPU evidence that suggested otherwise was a counter Microsoft documents as
  wrong for precisely this workload (§6).

The one thing sessions 1 and 2 could not say anything about was uptime: their
longest run was 21 minutes against the report's ~80. **§11 is that run**, and it
does not grow either.

## 10. Raw data

Session 2, under
`C:\Users\NoahM\AppData\Local\Temp\claude\C--Users-NoahM-Documents-RustProjects-Astrofin\d6d26212-bbc7-4b66-91f1-4e3b2f601c5b\scratchpad\mem2\`:

| path | contents |
| --- | --- |
| `runs\{P0,T1,T2,T3,T4,T5,T6}\samples.csv` | 10 s samples: per-process private/WS/handles/threads + all three GPU signals + a foreign-instance count |
| `runs\*\events.csv` | item starts, mode switches, fullscreen toggles |
| `runs\*\app.log` | the run's own log at `info,mpv=trace`, so "Render options updated" is countable |
| `runs\*\summary.md`, `gpu.png`, `host.png` | generated tables and plots |
| `sampler.ps1`, `driver.mjs`, `afm.mjs`, `setup-run.ps1`, `run-one.ps1`, `run-matrix.ps1`, `analyze.py`, `combine.py` | the harness |
| `foreign-instance.json` | the user's idle instance pids, excluded from the per-process series |

Session 1's data and harness are under `…\scratchpad\mem\runs\A\`; that
session's own writeup is commit `0b8269f` on `exp/memory-harness`, superseded by
this file.

## 11. Long run (90/30/10) — the uptime experiment

Run **L1**, 2026-09-08 04:24–06:35 local, on `main` @ `6829ded`
(`build\astrofin.exe`, `0.1.0-dev+578c5c5-dirty` — the memo build of §8, not
rebuilt for this run). Auto mode, isolated profile, port 9225, sampling every
30 s, `--log-level 'warn,memprobe=debug,mpv=info'`. `mpv=info` is added to the
level the plan asked for because the `video mode … applied` /
`unchanged, not re-applied` lines this section counts are `info` on target
`mpv`; the memprobe line is `debug` on target `memprobe` and the JS console's
`ERROR` lines pass `warn`, so nothing else is turned up.

Workload: Dragon Ball Z Kai, 1440×1080 HEVC 10-bit, ~23 min/episode, played by
**jellyfin-web's own auto-play** — 5 `loadfile`s over the 90 minutes, at t = 0,
22, 45, 69 and 92 min, only the first of which the driver issued. Then 30
minutes paused, then the player closed and 10 minutes idle on Home. 1 566
process-sample rows.

### Growth rate per phase (MB/min, least squares, first 60 s of each phase dropped)

| process | play (90 min) | paused (30 min, clean window) | closed (10 min) | whole run |
| --- | ---: | ---: | ---: | ---: |
| **main (29808)** | **−0.03** | **+0.03** | **−1.43** | −1.10 |
| gpu-process (31564) | −5.51 | +0.04 | +0.40 | −0.44 |
| renderer#1 (5412) — the player page | +0.05 | +0.02 | −0.09 | −0.25 |
| renderer#2 (7444) | +0.00 | +0.03 | +0.07 | −0.00 |
| utility (20352, storage) | +0.00 | +0.00 | −0.00 | +0.00 |
| utility (22468, network) | −0.00 | −0.02 | −0.01 | −0.00 |
| (machine-wide) adapter dedicated | −5.08 | +0.02 | −0.01 | −0.62 |
| (machine-wide) nvidia-smi board | −5.15 | +0.03 | +0.00 | −0.63 |

The gpu-process/adapter −5 MB/min in the play column is not a decay of anything
we did: it is one step down (901 → 414 MB) as CEF's GPU process releases its
startup allocations around t = 25 min, then flat for the remaining hour. Read
the play phase as two windows instead:

| window | main private MB/min | first → last |
| --- | ---: | --- |
| play 0–10 min (warm-up) | **+4.88** | 1 622 → 1 685 MB |
| play 10–91 min | **−0.03** | 1 685 → 1 698 MB |

That is the §4 "converging warm-up", now confirmed over 80 minutes rather than
inferred from 5: the ramp is spent inside the first ten minutes and the next
eighty are flat to within ±0.03 MB/min. No process anywhere in the tree exceeds
±0.06 MB/min while paused, against the report's ~19 MB/min.

### Absolute numbers, against the report

| | upstream #643 | run L1 (peak over 131 min) |
| --- | ---: | ---: |
| main process, private | 2 612 MB | **1 701 MB** |
| whole tree, private | 3 485 MB | **3 073 MB** (at t = 0; 2 608 MB at play-end) |
| largest helper, private | 401 MB | 209 → 213 MB (renderer#1) |
| growth while paused | +4.7 MB / 15 s | **+2 MB / 27 min** |

The tree's private total *falls* across the run — 3 011 MB at t = 0, 2 608 MB
at play-end, 2 775 MB after the player is closed. Main handles fall too: 2 624
at t = 0, never higher, 2 498 at the end.

### The paint counter kills hypothesis §2.1 outright

`memprobe`, per phase (`paints_sw` is 0 for the entire run — this build never
takes the software path):

| phase | minutes | accelerated paints | per minute |
| --- | ---: | ---: | ---: |
| play | 90 | 2 022 | 22.5 |
| paused (clean window) | 27 | 54 | **2.0** |
| closed, idle on Home | 10 | 68 138 | **6 814** |

Plan §2.1 predicted the leak was the overlay upload path and that it would show
while paused *because CEF keeps painting the OSD and the animated theme*. The
opposite is true on both halves. A paused player paints **two frames a minute**,
so there is nothing to leak; and the Home screen — which does paint, at 114
frames a second, 68 000 paints in ten minutes — is the phase in which main
private **falls** 1.43 MB/min and then sits flat (1 334 → 1 319, then 1 317,
1 317, 1 318, 1 318, 1 318, 1 319 MB). 68 000 uploads with no accumulation is a
direct falsification, not an absence of evidence.

The other two probes agree with sessions 1–2: the mpv event queue peaked at 8
and the coordinator queue at 4 across 130 minutes (§2.3 dead), and
`demuxer_fw_bytes` never exceeded the configured 150 MB cap (§2.2's cache
sub-mechanism dead).

### Renderer heap (§2.6, the 401 MB helper)

`HeapProfiler.takeHeapSnapshot` at t = 0 and t = 90 min, plus
`Runtime.getHeapUsage` every 5 minutes:

| | t = 0 | t = 90 min |
| --- | ---: | ---: |
| nodes | 461 898 | **415 123** |
| edges | 1 514 105 | **1 342 117** |
| snapshot size | 31.7 MB | 28.5 MB |
| `usedSize` | 15.3 MB | 13.8 MB |

The player page's heap is 10 % *smaller* after 90 minutes and five episodes.
`usedSize` oscillates 11.1–15.4 MB for the whole run with no trend, and the
renderer's private bytes move 209 → 213 MB. There is no 401 MB helper here to
explain.

### Video-mode memo, over five real item starts

6 applies: **2 `applied`, 4 `unchanged, not re-applied`**. The two that ran are
the boot apply (live-action, before any title is known) and the first
resolution to `animation`; every subsequent episode of the same series hits the
memo. That is §8.1 doing exactly what it was written to do on the binge case,
and — as §5 established — costing nothing either way. 0 `ERROR` lines in the
whole run.

### Verdict

**Converging warm-up, not sustained growth.** Against the plan's §4 thresholds:
the play phase is −0.03 MB/min in the main process (threshold: > 2 MB/min
sustained) and every process is within ±0.06 MB/min while paused (threshold:
any slope at all). Both fail by two orders of magnitude, and they fail on the
one axis sessions 1 and 2 could not test. #643 does not reproduce on this build
at the report's uptime, with the report's codec, at the report's episode count.

### Harness note: the pause guard was wrong for the first 3.4 minutes

`window.api.player.paused` is not a state getter. `native-shim.js`'s
`createSignal` makes it a callable *signal emitter* that returns nothing, so the
driver's `!!p.paused()` read `false` even while paused and it re-clicked
`.btnPause` every 30 s — which toggles. Between 09:55:03 and 09:57:34 UTC the
player therefore alternated play/pause and auto-play advanced one more episode
(the t = 92 min `loadfile`, and the five `re-pause` rows in `events.csv`). It
was corrected live over a second CDP connection: the signal was wrapped so it
still emits but returns `true`, and `window.jmpNative.playerPause()` was called
directly; position then held at 1:44 across four 15 s polls. **The paused column
above is fitted over the clean window only, t = 93.7 → 121 min, 54 samples**,
and the phase-column fit that includes the toggling (+0.81 MB/min for main) is
the toggling, not a pause. `runs\L1\intervention.md` has the detail.

Worth keeping separately: *calling* `window.api.player.paused()` emits the
`paused` signal into jellyfin-web. That is where the
`ERROR [JS] [Media] [Signal] paused error: Error: player cannot be null` lines
in the session-2 logs come from whenever anything polls it (memory
`bug-player-cannot-be-null`).

### The next experiment, if the report persists

Not another run on this machine — three sessions and 131 minutes of uptime have
now failed to reproduce it, and the remaining candidates are all *differences
from the reporter*, not hypotheses about our code. The cheapest discriminating
experiment is to run this same harness against **upstream jellium-desktop's own
release**, the version the report was filed on, on this hardware. That is one
bit and it is the useful one: if upstream grows here, the fork's CEF 151.3.24
and mpv-fork bumps fixed it by accident and the issue can be closed with the
plot attached; if upstream is flat here too, the variable is the reporter's
environment (GPU driver, `mpv.conf`, `hwdec`, display scale) and the next move
is to ask them for `--log-level 'warn,memprobe=debug'` plus the two
*adapter-level* GPU signals — because §6 makes it likely they are reading the
same fabricating per-process counter Task Manager's Details pane shows.

### Raw data

`…\scratchpad\mem3\runs\L1\`: `samples.csv` (30 s, six processes ×
private/WS/handles/threads + all three GPU signals), `events.csv`, `phases.csv`,
`heapusage.csv`, `heap-t0.heapsnapshot`, `heap-play-end.heapsnapshot`,
`app.log`, `driver.log`, `intervention.md`, `summary.md`, and the plots
`private_bytes.png`, `gpu.png`, `handles.png`. Harness alongside in
`…\scratchpad\mem3\`: `run-long.ps1`, `setup-run.ps1`, `sampler.ps1`,
`driver.mjs`, `afm.mjs`, `fix-pause.mjs`, `analyze.py` (the paused-window fit
takes the clean-window start as `argv[2]`, here 5623).
