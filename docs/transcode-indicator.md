# Transcode indicator

Astrofin tells you, during playback, whether the server is sending the file
as-is or re-encoding it, and whether a re-encode is running on the server's
GPU or its CPU. On a LAN HTPC with mpv's near-universal device profile a
transcode is almost always a misconfiguration, so it is surfaced rather than
hidden in a stats panel.

## What you see

**A chip beside the title in the playback OSD header.**

| Chip | Meaning | Colour |
| --- | --- | --- |
| `DIRECT PLAY` | the file is sent untouched | accent (calm) |
| `DIRECT STREAM` | the container is rewrapped, video untouched | muted |
| `TRANSCODING` | re-encoding; hardware type not known yet | warm |
| `TRANSCODING · NVENC` (QSV, VAAPI, AMF, ...) | re-encoding with the server's configured hardware encoder | warm |
| `TRANSCODING · CPU` | the server reports software encoding | danger |
| `SERVER CAN'T KEEP UP` | measured throughput says the transcoder is falling behind | danger, pulsing dot |

Focus or hover the chip for a glass popover with the codec path
(`HEVC 10-bit 3840x2160 → H264 1280x720 2.0 Mbps`), the server's transcode
reasons in plain words, the transcoder speed (`9.6× realtime`) and its lead
over the playhead (`378 s ahead`).

**A one-time toast per playback** when the situation is worth interrupting
for, governed by *Settings → Playback → Transcode warning*:

| Value | Fires for |
| --- | --- |
| `off` | never |
| `cpu` (default) | `TRANSCODING · CPU` and `SERVER CAN'T KEEP UP` |
| `any` | any transcode, once the first poll has settled |

## Where the data comes from

- The play method (`DirectPlay` / `DirectStream` / `Transcode`) is known the
  moment play is requested: the mpv player plugin passes its play options to
  `playback-source.js`, which draws a provisional chip as soon as the OSD
  header mounts. jellyfin-web's `playbackstart` refines it in place.
- Everything else comes from the server's session API
  (`/Sessions?deviceId=...`, `TranscodingInfo`), polled about 4 s after the
  play request and then every 10 s while transcoding, stopping on
  `playbackstop`. The poll picks the session that is actually transcoding
  (the server keeps stale records per device), and keeps the last
  `TranscodingInfo` when a poll returns none, since the server drops the
  block once the transcoder finishes.
- `HardwareAccelerationType` mirrors the server's dashboard setting, not the
  encoder ffmpeg actually chose, so it alone never turns the chip red. Red is
  earned by `none`/null (software) or by measurement: transcode frame rate
  below 0.95× the source frame rate on two consecutive polls, or the lead
  shrinking to under 20 s on two consecutive polls. "Can't keep up" outranks
  "CPU".
- jellyfin-web's own Playback Info panel has the hardware-acceleration row
  commented out in 10.11, so stock clients never show GPU vs CPU.

## Likely causes of an unexpected transcode

- A bitrate cap in jellyfin-web's playback settings (`maxbitrate-Video-*`).
- Astrofin's *Force Transcoding* setting. Since 0.2.0 it really forces a
  transcode: the device profile omits every DirectPlayProfile. For a source
  codec the server can already put into HLS (h264) it stream-copies the video
  and remuxes audio and container, which is still a transcode session.
- Subtitle burn-in set to "always" in jellyfin-web.
- A server-side user policy or a codec mpv genuinely cannot play (rare).

## Files

- `src/web/playback-source.js`, tests in `playback-source.test.js`
  (`just test-js`).
- CSS: the "Playback source badge" section at the end of
  `src/web/astrofin-theme.css`. The OSD header's `contain`/`overflow` are
  relaxed while `.osdHeader` is present so the popover can escape the band.
- Setting: `transcodeNotice` in `src/config`, dispatched in
  `src/jfn_cef/src/business_common.rs`, exposed through `jmpInfo` in
  `src/web/native-shim.js`.
