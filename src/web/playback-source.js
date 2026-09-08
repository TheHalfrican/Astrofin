// Playback source badge: is the server direct-playing, remuxing, or
// transcoding — and if it is transcoding, on a GPU or on the CPU?
//
// A CPU transcode slows the whole server down, so it is worth surfacing.
// jellyfin-web never shows it: playerstats.js has the HardwareAccelerationType
// row commented out, and TranscodeReasons only reaches this client through
// /Sessions, not through the MediaSource it already holds.
//
// Two data planes:
//   - instant, from the `playbackstart` state: PlayState.PlayMethod plus the
//     source video stream (codec, resolution, bit depth, frame rate);
//   - delayed, from `ApiClient.getSessions({deviceId})` 4 s after start and
//     every 10 s while transcoding: TranscodingInfo, which is the only place
//     HardwareAccelerationType, TranscodeReasons and the transcoder's own
//     frame rate and completion appear.
//
// Everything here is best-effort decoration over mpv's playback. The whole
// module is wrapped in try/catch and every callback is guarded, because all
// injected scripts share one execute_java_script call: a top-level throw here
// would take the mpv shims down with it and kill playback.
//
// Unit tests: src/web/playback-source.test.js (node --test).
(function () {
    'use strict';

    try {
        // ---- constants --------------------------------------------------

        var LEVEL_DIRECT = 'direct';
        var LEVEL_STREAM = 'stream';
        var LEVEL_TRANSCODE = 'transcode';
        var LEVEL_CPU = 'cpu';
        var LEVEL_STRUGGLING = 'struggling';

        var FIRST_POLL_MS = 4000;
        var POLL_INTERVAL_MS = 10000;

        // Measured transcoder throughput below this fraction of the source
        // frame rate means the server is not keeping up in real time.
        var SLOW_RATIO = 0.95;
        // A transcoder lead thinner than this, and still shrinking, is about
        // to become a stall.
        var LEAD_FLOOR_S = 20;
        // Both signals must hold on two consecutive polls: one sample is
        // noise (the transcoder ramps up, the position jumps on a seek).
        var CONSECUTIVE = 2;

        // playbackstart fires before jellyfin-web mounts the video view, so
        // .skinHeader gains .osdHeader a beat after the state we want to draw
        // is already known. Rather than reach into the view lifecycle, retry
        // the (idempotent) render until the header exists.
        var RETRY_MS = 250;
        var RETRY_MAX = 40;

        var TOAST_IN_MS = 300;
        var TOAST_HOLD_MS = 3500;
        var TOAST_OUT_MS = 300;

        var BADGE_CLASS = 'af-source-badge';
        var LEVEL_CLASSES = [
            BADGE_CLASS + '--' + LEVEL_DIRECT,
            BADGE_CLASS + '--' + LEVEL_STREAM,
            BADGE_CLASS + '--' + LEVEL_TRANSCODE,
            BADGE_CLASS + '--' + LEVEL_CPU,
            BADGE_CLASS + '--' + LEVEL_STRUGGLING
        ];

        var HEADER_SELECTOR = '.skinHeader.osdHeader .headerTop .headerLeft';

        // TranscodeReasons enum name -> the sentence jellyfin-web's own
        // strings/en-us.json carries for that key. playerstats.js translates
        // the raw enum name because the server name *is* the translation key;
        // we cannot import globalize from an injected script, so the table is
        // inlined. Anything absent falls back to the name split on capitals.
        var REASONS = {
            ContainerNotSupported: 'The container is not supported',
            VideoCodecNotSupported: 'The video codec is not supported',
            VideoCodecTagNotSupported: 'The video codec tag is not supported',
            AudioCodecNotSupported: 'The audio codec is not supported',
            SubtitleCodecNotSupported: 'The subtitle codec is not supported',
            AudioIsExternal: 'The audio stream is external',
            SecondaryAudioNotSupported: 'Secondary audio tracks are not supported',
            VideoProfileNotSupported: "The video codec's profile is not supported",
            VideoLevelNotSupported: "The video codec's level is not supported",
            VideoResolutionNotSupported: "The video's resolution is not supported",
            VideoBitDepthNotSupported: "The video's bit depth is not supported",
            VideoFramerateNotSupported: "The video's framerate is not supported",
            VideoBitrateNotSupported: "The video's bitrate is not supported",
            VideoRangeTypeNotSupported: "The video's range type is not supported",
            RefFramesNotSupported: 'Reference frames are not supported',
            AnamorphicVideoNotSupported: 'Anamorphic video is not supported',
            InterlacedVideoNotSupported: 'Interlaced video is not supported',
            AudioChannelsNotSupported: 'The number of audio channels is not supported',
            AudioProfileNotSupported: "The audio codec's profile is not supported",
            AudioSampleRateNotSupported: "The audio's sample rate is not supported",
            AudioBitDepthNotSupported: "The audio's bit depth is not supported",
            AudioBitrateNotSupported: "The audio's bitrate is not supported",
            ContainerBitrateExceedsLimit: "The video's bitrate exceeds the limit",
            StreamCountExceedsLimit: 'The number of streams exceeds the limit',
            UnknownVideoStreamInfo: 'The video stream info is unknown',
            UnknownAudioStreamInfo: 'The audio stream info is unknown',
            DirectPlayError: 'There was an error starting direct playback'
        };

        // ---- environment ------------------------------------------------

        function win() {
            return typeof window !== 'undefined' ? window : null;
        }

        function debug(msg) {
            try {
                console.debug('[Source] ' + msg);
            } catch (e) {
                /* console can be absent under a bare node harness */
            }
        }

        // Every entry point the outside world can reach goes through this:
        // jellyfin-web's Events.trigger has no try/catch of its own, so one
        // throwing handler silently skips every handler after it.
        function guard(label, fn) {
            try {
                return fn();
            } catch (e) {
                debug(label + ' failed: ' + (e && e.message ? e.message : e));
                return undefined;
            }
        }

        function num(v) {
            return typeof v === 'number' && isFinite(v) ? v : null;
        }

        function positive(v) {
            var n = num(v);
            return n !== null && n > 0 ? n : null;
        }

        // ---- classification ---------------------------------------------

        // HardwareAccelerationType's enum values are unverified against the
        // server (no OpenAPI spec or generated SDK is vendored here), so this
        // treats the field as an opaque string: anything non-empty that is
        // not "none" counts as hardware, case-insensitively. Upstream also
        // warns the field reflects the dashboard configuration rather than
        // the encoder ffmpeg actually picked — so "CPU" here is definitive
        // (software was configured) while a named accelerator is a strong
        // hint, not a guarantee.
        function hardwareName(raw) {
            if (typeof raw !== 'string') return null;
            var v = raw.trim();
            if (!v || v.toLowerCase() === 'none') return null;
            return v;
        }

        function classify(ctx) {
            var method = ctx && ctx.playMethod;
            if (method === 'DirectPlay') return LEVEL_DIRECT;
            if (method === 'DirectStream') return LEVEL_STREAM;
            if (method !== 'Transcode') return null;
            // Struggling outranks everything else about a transcode: a
            // hardware encoder that cannot keep up is the more urgent fact.
            if (ctx.struggling) return LEVEL_STRUGGLING;
            var info = ctx.transcodingInfo;
            if (!info) return LEVEL_TRANSCODE;
            return hardwareName(info.HardwareAccelerationType) ? LEVEL_TRANSCODE : LEVEL_CPU;
        }

        // Consecutive-sample tracker for the two "server can't keep up"
        // signals. Pure: the caller owns the tracker object, so the tests can
        // step it one poll at a time.
        function newTracker() {
            return { slow: 0, shrink: 0, lastLead: null };
        }

        function trackThroughput(tracker, sample) {
            var t = tracker;
            var s = sample || {};
            var srcFps = positive(s.sourceFps);
            var outFps = positive(s.transcodeFps);
            if (srcFps !== null && outFps !== null) {
                if (outFps / srcFps < SLOW_RATIO) t.slow += 1;
                else t.slow = 0;
            }
            var lead = num(s.lead);
            if (lead !== null) {
                if (t.lastLead !== null && lead < t.lastLead && lead < LEAD_FLOOR_S) t.shrink += 1;
                else t.shrink = 0;
                t.lastLead = lead;
            }
            return t.slow >= CONSECUTIVE || t.shrink >= CONSECUTIVE;
        }

        // ---- presentation -----------------------------------------------

        function upper(v) {
            return typeof v === 'string' ? v.toUpperCase() : '';
        }

        function badgeLabel(level, accel) {
            switch (level) {
                case LEVEL_DIRECT:
                    return 'DIRECT PLAY';
                case LEVEL_STREAM:
                    return 'DIRECT STREAM';
                case LEVEL_TRANSCODE:
                    return accel ? 'TRANSCODING · ' + upper(accel) : 'TRANSCODING';
                case LEVEL_CPU:
                    return 'TRANSCODING · CPU';
                case LEVEL_STRUGGLING:
                    return "SERVER CAN'T KEEP UP";
                default:
                    return '';
            }
        }

        // "VideoCodecNotSupported" -> "video codec not supported". Used for
        // the toast (which reads as a sentence) and as the fallback for an
        // enum member this build has never heard of.
        function splitCapitals(name) {
            return String(name)
                .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
                .replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
                .toLowerCase()
                .trim();
        }

        function reasonText(name) {
            if (typeof name !== 'string' || !name) return '';
            if (Object.prototype.hasOwnProperty.call(REASONS, name)) return REASONS[name];
            var phrase = splitCapitals(name);
            return phrase ? phrase.charAt(0).toUpperCase() + phrase.slice(1) : '';
        }

        function reasonList(info) {
            var raw = info && info.TranscodeReasons;
            if (typeof raw === 'string') raw = raw ? raw.split(',') : [];
            if (!Array.isArray(raw)) return [];
            var out = [];
            for (var i = 0; i < raw.length; i++) {
                var text = reasonText(String(raw[i]).trim());
                if (text) out.push(text);
            }
            return out;
        }

        // "HEVC 10-bit 1440x1080 -> H264 8.0 Mbps". Every part is optional:
        // an unknown field is left out rather than printed as a placeholder.
        function codecPath(source, info) {
            var left = [];
            if (source) {
                if (source.codec) left.push(upper(source.codec));
                if (positive(source.bitDepth) !== null && source.bitDepth > 8) {
                    left.push(source.bitDepth + '-bit');
                }
                if (positive(source.width) !== null && positive(source.height) !== null) {
                    left.push(source.width + 'x' + source.height);
                }
            }
            var right = [];
            if (info) {
                if (info.VideoCodec) right.push(upper(info.VideoCodec));
                if (positive(info.Width) !== null && positive(info.Height) !== null) {
                    right.push(info.Width + 'x' + info.Height);
                }
                var mbps = positive(info.Bitrate);
                if (mbps !== null) right.push((mbps / 1000000).toFixed(1) + ' Mbps');
            }
            if (!left.length && !right.length) return '';
            if (!right.length) return left.join(' ');
            if (!left.length) return right.join(' ');
            return left.join(' ') + ' → ' + right.join(' ');
        }

        function speedText(source, info) {
            var srcFps = source ? positive(source.fps) : null;
            var outFps = info ? positive(info.Framerate) : null;
            if (srcFps === null || outFps === null) return '';
            return (outFps / srcFps).toFixed(1) + '× realtime';
        }

        function leadText(lead) {
            var n = num(lead);
            if (n === null) return '';
            var secs = Math.round(Math.abs(n));
            return secs + ' s ' + (n < 0 ? 'behind' : 'ahead');
        }

        // The whole popover, as plain strings. Kept separate from the DOM so
        // the tests can assert content without a browser.
        function popoverLines(view) {
            var lines = [];
            var path = codecPath(view.source, view.transcodingInfo);
            if (path) lines.push(path);
            var reasons = reasonList(view.transcodingInfo);
            for (var i = 0; i < reasons.length; i++) lines.push(reasons[i]);
            var speed = speedText(view.source, view.transcodingInfo);
            if (speed) lines.push(speed);
            var lead = leadText(view.lead);
            if (lead) lines.push(lead);
            return lines;
        }

        function toastText(level, info) {
            if (level === LEVEL_STRUGGLING) return "The server can't keep up with this transcode";
            var head;
            if (level === LEVEL_CPU) {
                head = 'The server is transcoding this on its CPU';
            } else {
                var accel = hardwareName(info && info.HardwareAccelerationType);
                head = accel
                    ? 'The server is transcoding this with ' + upper(accel)
                    : 'The server is transcoding this';
            }
            var raw = info && info.TranscodeReasons;
            if (typeof raw === 'string') raw = raw ? raw.split(',') : [];
            var first = Array.isArray(raw) && raw.length ? String(raw[0]).trim() : '';
            return first ? head + ': ' + splitCapitals(first) : head;
        }

        // ---- session state ----------------------------------------------

        var pm = null;
        var onStart = null;
        var onStop = null;
        var timer = null;
        var retryTimer = null;
        var retriesLeft = 0;
        var accelLogged = false;

        function newSession() {
            return {
                player: null,
                playMethod: null,
                source: null,
                durationSec: null,
                transcodingInfo: null,
                lead: null,
                struggling: false,
                level: null,
                polled: false,
                toastShown: false,
                tracker: newTracker()
            };
        }

        var session = newSession();

        // ---- reading the player -----------------------------------------

        function playerState(player, state) {
            if (state) return state;
            if (!pm || typeof pm.getPlayerState !== 'function' || !player) return null;
            // getPlayerState defaults its argument to the manager's
            // _currentPlayer and the tick helper underneath throws
            // "player cannot be null" — always name the player, always guard.
            return guard('getPlayerState', function () {
                return pm.getPlayerState(player);
            }) || null;
        }

        function videoStream(player, state) {
            var ms = null;
            if (pm && typeof pm.currentMediaSource === 'function' && player) {
                ms = guard('currentMediaSource', function () {
                    return pm.currentMediaSource(player);
                });
            }
            if (!ms && state) ms = state.MediaSource;
            var streams = (ms && ms.MediaStreams) || null;
            if (!streams && state && state.NowPlayingItem) streams = state.NowPlayingItem.MediaStreams;
            if (!Array.isArray(streams)) return null;
            for (var i = 0; i < streams.length; i++) {
                if (streams[i] && streams[i].Type === 'Video') return streams[i];
            }
            return null;
        }

        function readSource(player, state) {
            var v = videoStream(player, state);
            if (!v) return null;
            return {
                codec: typeof v.Codec === 'string' ? v.Codec : null,
                width: num(v.Width),
                height: num(v.Height),
                bitDepth: num(v.BitDepth),
                // RealFrameRate is the container's measured rate; the average
                // is the fallback the server itself uses when it is absent.
                fps: positive(v.RealFrameRate) !== null ? v.RealFrameRate : positive(v.AverageFrameRate)
            };
        }

        function durationSeconds(state) {
            var item = state && state.NowPlayingItem;
            var ticks = item ? positive(item.RunTimeTicks) : null;
            return ticks === null ? null : ticks / 10000000;
        }

        function positionSeconds() {
            var player = session.player;
            if (!pm || !player) return null;
            if (typeof pm.currentTime === 'function') {
                var ms = guard('currentTime', function () {
                    return pm.currentTime(player);
                });
                if (positive(ms) !== null || ms === 0) return ms / 1000;
            }
            var st = playerState(player, null);
            var ticks = st && st.PlayState ? num(st.PlayState.PositionTicks) : null;
            return ticks === null ? null : ticks / 10000000;
        }

        // How far ahead of the playhead the transcoder has got, in seconds.
        function computeLead(info) {
            var pct = num(info && info.CompletionPercentage);
            var dur = positive(session.durationSec);
            if (pct === null || dur === null) return null;
            var pos = positionSeconds();
            if (num(pos) === null) return null;
            return (pct / 100) * dur - pos;
        }

        // ---- DOM ---------------------------------------------------------

        function doc() {
            var w = win();
            return w && w.document ? w.document : null;
        }

        // Idempotent. The badge is a *sibling* of h3.pageTitle, never a
        // child: LibraryMenu.setTitle does `pageTitleElement.innerText = html`
        // on every item change, which would delete anything inside it.
        function ensureBadge() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return null;
            var host = d.querySelector(HEADER_SELECTOR);
            if (!host) return null;
            var badge = d.querySelector('.' + BADGE_CLASS);
            if (badge && badge.parentNode === host) return badge;
            if (badge && badge.parentNode) {
                // The OSD view was rebuilt under a fresh header; re-home the
                // existing node rather than growing a second one.
                host.appendChild(badge);
                return badge;
            }
            badge = d.createElement('div');
            badge.className = BADGE_CLASS;
            badge.setAttribute('tabindex', '0');
            var dot = d.createElement('span');
            dot.className = BADGE_CLASS + '-dot';
            dot.setAttribute('aria-hidden', 'true');
            var label = d.createElement('span');
            label.className = BADGE_CLASS + '-label';
            var pop = d.createElement('div');
            pop.className = 'af-source-popover';
            badge.appendChild(dot);
            badge.appendChild(label);
            badge.appendChild(pop);
            host.appendChild(badge);
            return badge;
        }

        function setLevelClass(badge, level) {
            for (var i = 0; i < LEVEL_CLASSES.length; i++) badge.classList.remove(LEVEL_CLASSES[i]);
            if (level) badge.classList.add(BADGE_CLASS + '--' + level);
        }

        function fillPopover(pop, lines) {
            while (pop.firstChild) pop.removeChild(pop.firstChild);
            var d = doc();
            for (var i = 0; i < lines.length; i++) {
                var row = d.createElement('div');
                row.className = 'af-source-popover-line';
                if (i === 0) row.className += ' af-source-popover-path';
                row.textContent = lines[i];
                pop.appendChild(row);
            }
        }

        function render() {
            var level = classify({
                playMethod: session.playMethod,
                transcodingInfo: session.transcodingInfo,
                struggling: session.struggling
            });
            session.level = level;

            var badge = ensureBadge();
            if (!badge) {
                // No OSD header yet: try again shortly, a bounded number of
                // times, then give up quietly.
                scheduleRetry();
                return;
            }

            if (!level) {
                badge.hidden = true;
                return;
            }
            badge.hidden = false;

            var accel = hardwareName(session.transcodingInfo && session.transcodingInfo.HardwareAccelerationType);
            var text = badgeLabel(level, accel);
            setLevelClass(badge, level);

            var label = badge.querySelector('.' + BADGE_CLASS + '-label');
            if (label) label.textContent = text;
            badge.setAttribute('aria-label', 'Playback source: ' + text);

            var pop = badge.querySelector('.af-source-popover');
            if (pop) {
                fillPopover(
                    pop,
                    popoverLines({
                        source: session.source,
                        transcodingInfo: session.transcodingInfo,
                        lead: session.lead
                    })
                );
            }
        }

        function removeBadge() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return;
            var badge = d.querySelector('.' + BADGE_CLASS);
            if (badge && badge.parentNode) badge.parentNode.removeChild(badge);
        }

        // ---- toast --------------------------------------------------------

        // jellyfin-web's toast module is ESM and cannot be imported from an
        // injected script, so its markup is synthesized instead — the classes
        // are what toast.scss and our own theme already style.
        function showToast(text) {
            var d = doc();
            var w = win();
            if (!d || !w || typeof d.createElement !== 'function' || !d.body) return;
            var container = d.querySelector('.toastContainer');
            if (!container) {
                container = d.createElement('div');
                container.className = 'toastContainer';
                d.body.appendChild(container);
            }
            var el = d.createElement('div');
            el.className = 'toast';
            el.textContent = text;
            container.appendChild(el);
            var later = typeof w.setTimeout === 'function' ? w.setTimeout : null;
            if (!later) return;
            later(function () {
                guard('toast show', function () {
                    el.classList.add('toastVisible');
                });
            }, TOAST_IN_MS);
            later(function () {
                guard('toast hide', function () {
                    el.classList.add('toastHide');
                });
            }, TOAST_IN_MS + TOAST_HOLD_MS);
            later(function () {
                guard('toast remove', function () {
                    if (el.parentNode) el.parentNode.removeChild(el);
                });
            }, TOAST_IN_MS + TOAST_HOLD_MS + TOAST_OUT_MS);
        }

        // off | cpu (default) | any. Read at fire time so a change in Client
        // Settings takes effect on the next item without a restart.
        function noticeSetting() {
            var w = win();
            var v = w && w.jmpInfo && w.jmpInfo.settings && w.jmpInfo.settings.playback
                ? w.jmpInfo.settings.playback.transcodeNotice
                : null;
            if (v === 'off' || v === 'cpu' || v === 'any') return v;
            return 'cpu';
        }

        function maybeToast() {
            if (session.toastShown) return;
            // Nothing is said until the first poll has settled: before that,
            // "Transcode" is all that is known and the message could not tell
            // a GPU encode from a CPU one.
            if (!session.polled) return;
            var level = session.level;
            if (level !== LEVEL_TRANSCODE && level !== LEVEL_CPU && level !== LEVEL_STRUGGLING) return;
            var mode = noticeSetting();
            if (mode === 'off') return;
            if (mode === 'cpu' && level !== LEVEL_CPU && level !== LEVEL_STRUGGLING) return;
            session.toastShown = true;
            showToast(toastText(level, session.transcodingInfo));
        }

        // ---- polling -------------------------------------------------------

        function clearRetry() {
            var w = win();
            if (retryTimer !== null && w && typeof w.clearTimeout === 'function') w.clearTimeout(retryTimer);
            retryTimer = null;
            retriesLeft = 0;
        }

        function scheduleRetry() {
            var w = win();
            if (retriesLeft <= 0 || retryTimer !== null) return;
            if (!w || typeof w.setTimeout !== 'function') return;
            retryTimer = w.setTimeout(function () {
                retryTimer = null;
                retriesLeft -= 1;
                guard('render retry', render);
            }, RETRY_MS);
        }

        function clearTimer() {
            var w = win();
            if (timer !== null && w && typeof w.clearTimeout === 'function') w.clearTimeout(timer);
            timer = null;
        }

        function schedule(delay) {
            var w = win();
            if (!w || typeof w.setTimeout !== 'function') return;
            clearTimer();
            timer = w.setTimeout(function () {
                timer = null;
                if (session.playMethod !== 'Transcode') return;
                pollOnce().then(function () {
                    if (session.playMethod === 'Transcode') schedule(POLL_INTERVAL_MS);
                });
            }, delay);
        }

        // The deviceId filter does NOT leave exactly one session: measured
        // against Jellyfin 10.11, /Sessions returned three records for this
        // client's deviceId — one live, two stale from earlier app runs, whose
        // TranscodingInfo is null and whose PlayState.PlayMethod is whatever
        // it was when they died. Taking the first match (or playerstats.js's
        // unconditional sessions[0]) therefore flaps between the real answer
        // and nothing. Prefer, in order: a matching session that is actually
        // transcoding, one that at least has an item loaded, then any match.
        function pickSession(sessions, deviceId) {
            var list = Array.isArray(sessions) ? sessions : [];
            var match = null;
            var withItem = null;
            for (var i = 0; i < list.length; i++) {
                var s = list[i];
                if (!s) continue;
                if (deviceId && s.DeviceId !== deviceId) continue;
                if (s.TranscodingInfo) return s;
                if (!withItem && s.NowPlayingItem) withItem = s;
                if (!match) match = s;
            }
            return withItem || match || list[0] || null;
        }

        function applySessions(sessions, deviceId) {
            var s = pickSession(sessions, deviceId);
            var info = (s && s.TranscodingInfo) || null;
            if (!info) {
                // The server also drops TranscodingInfo once the transcoder
                // has run to completion, and the pick above can still land on
                // a stale record. The play method has not changed, so keep the
                // last thing actually measured rather than downgrading the
                // badge to a bare "TRANSCODING".
                return;
            }
            session.transcodingInfo = info;
            if (!accelLogged) {
                accelLogged = true;
                debug('HardwareAccelerationType=' + JSON.stringify(info.HardwareAccelerationType));
            }
            var lead = computeLead(info);
            session.lead = lead;
            session.struggling = trackThroughput(session.tracker, {
                transcodeFps: info.Framerate,
                sourceFps: session.source ? session.source.fps : null,
                lead: lead
            });
        }

        // Always resolves. A failing /Sessions call must never surface as an
        // unhandled rejection, and must never stop playback or the next poll.
        function pollOnce() {
            var w = win();
            var api = w && w.ApiClient;
            var deviceId = null;
            var promise = null;
            guard('getSessions', function () {
                if (!api || typeof api.getSessions !== 'function') return;
                deviceId = typeof api.deviceId === 'function' ? api.deviceId() : null;
                promise = api.getSessions(deviceId ? { deviceId: deviceId } : {});
            });
            var settle = function () {
                session.polled = true;
                guard('render', render);
                guard('toast', maybeToast);
            };
            if (!promise || typeof promise.then !== 'function') {
                settle();
                return Promise.resolve();
            }
            return promise.then(
                function (sessions) {
                    guard('applySessions', function () {
                        applySessions(sessions, deviceId);
                    });
                },
                function (err) {
                    debug('getSessions rejected: ' + (err && err.message ? err.message : err));
                }
            ).then(settle, settle);
        }

        // ---- lifecycle ------------------------------------------------------

        function handleStart(player, state) {
            var st = playerState(player, state);
            session = newSession();
            session.player = player || null;
            session.playMethod = st && st.PlayState ? st.PlayState.PlayMethod || null : null;
            session.source = readSource(player, st);
            session.durationSec = durationSeconds(st);
            debug('playbackstart method=' + session.playMethod + ' sourceFps=' + (session.source && session.source.fps));
            clearRetry();
            retriesLeft = RETRY_MAX;
            render();
            clearTimer();
            if (session.playMethod === 'Transcode') schedule(FIRST_POLL_MS);
        }

        function handleStop() {
            clearTimer();
            clearRetry();
            session = newSession();
            // The app header outlives playback (viewbeforehide only drops the
            // osdHeader class, not its children), so the badge is dropped
            // explicitly as well as being hidden by CSS outside .osdHeader.
            removeBadge();
        }

        function attach(playbackManager) {
            return guard('attach', function () {
                var w = win();
                if (!playbackManager || !w || !w.Events) return;
                if (pm === playbackManager) return;
                detach();
                pm = playbackManager;
                onStart = function (e, player, state) {
                    guard('playbackstart', function () {
                        handleStart(player, state);
                    });
                };
                onStop = function () {
                    guard('playbackstop', handleStop);
                };
                w.Events.on(pm, 'playbackstart', onStart);
                w.Events.on(pm, 'playbackstop', onStop);
                debug('attached');
            });
        }

        function detach() {
            return guard('detach', function () {
                var w = win();
                if (pm && w && w.Events) {
                    if (onStart) w.Events.off(pm, 'playbackstart', onStart);
                    if (onStop) w.Events.off(pm, 'playbackstop', onStop);
                }
                clearTimer();
                clearRetry();
                pm = null;
                onStart = null;
                onStop = null;
                session = newSession();
            });
        }

        var api = {
            attach: attach,
            detach: detach,
            // Testing surface. Pure helpers first, then the bits that need a
            // document or a clock.
            classify: classify,
            newTracker: newTracker,
            trackThroughput: trackThroughput,
            hardwareName: hardwareName,
            badgeLabel: badgeLabel,
            reasonText: reasonText,
            popoverLines: popoverLines,
            toastText: toastText,
            pollOnce: pollOnce,
            _session: function () {
                return session;
            },
            _timer: function () {
                return timer;
            },
            _retryTimer: function () {
                return retryTimer;
            }
        };

        var w = win();
        if (w) w.AstrofinPlaybackSource = api;
        // Unit tests run this file under node, where there is no window.
        if (typeof module !== 'undefined' && module.exports) module.exports = api;
    } catch (e) {
        // A top-level throw in an injected script takes every other injected
        // script in the same execute_java_script bundle with it — including
        // the mpv player shims. Swallow and carry on without the badge.
        try {
            console.error('[Source] module failed to install:', e);
        } catch (ignored) {
            /* no console */
        }
    }
})();
