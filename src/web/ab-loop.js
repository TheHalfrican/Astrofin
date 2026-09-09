// A-B repeat loop — "repeat the selected bit".
//
// Set a begin point (A), set an end point (B), and mpv loops between them.
// The loop is mpv's own `ab-loop-a` / `ab-loop-b`, which means:
//   - passing B seeks back to A, and mpv does the seek, not this file;
//   - *seeking* past B does not loop. That is mpv's documented, deliberate
//     behaviour (DOCS/man/options.rst, `--ab-loop-a`), so scrubbing out of
//     the loop is how you leave it without clearing it;
//   - with either end unset — mpv spells that "no" — looping is off.
//
// State flows one way, per CLAUDE.md's "mpv Event Flow": this file asks the
// native side to set the points, the native side writes them to mpv, mpv
// reports the two properties back, and the native side pushes them here as
// `window._nativeAbLoop(a, b)` in seconds. Nothing below ever writes `state`
// from a keystroke or a click — it sends the request and waits for mpv to
// say what happened. A point the user asks for that mpv refuses simply never
// appears.
//
// Everything is decoration over playback and the whole module shares one
// execute_java_script call with the mpv shims, so it is wrapped in try/catch,
// resolves `document` lazily, and never throws at top level: an uncaught
// exception here would take the player shims down with it.
//
// Unit tests: src/web/ab-loop.test.js (node --test).
(function () {
    'use strict';

    try {
        // ---- constants ----------------------------------------------------

        // A loop shorter than this is almost certainly a double-tap on `]`
        // rather than an intent, and mpv would spend the whole time seeking.
        var MIN_SPAN_S = 0.5;

        // The OSD mounts a beat after `playbackstart`, so the (idempotent)
        // render is retried until all three pieces have a home. Same shape
        // and budget as playback-source.js's header retry.
        var RETRY_MS = 250;
        var RETRY_MAX = 40;

        var TOAST_IN_MS = 300;
        var TOAST_HOLD_MS = 3500;
        var TOAST_OUT_MS = 300;

        var BTN_CLASS = 'af-abloop-btn';
        var OVERLAY_CLASS = 'af-abloop-overlay';
        var BAND_CLASS = 'af-abloop-band';
        var PIN_CLASS = 'af-abloop-pin';
        var READOUT_CLASS = 'af-abloop-readout';

        // Verified live in docs/design/theme-injection.md, "Player OSD": the
        // scrubber is `.sliderContainer.mdl-slider-container` inside a flex
        // row with `.startTimeText` and `.endTimeText`, and the control row
        // is `.buttons.focuscontainer-x`.
        var SLIDER_SELECTOR = '.videoOsdBottom .sliderContainer';
        var BUTTONS_SELECTOR = '.videoOsdBottom .buttons';
        var END_TIME_SELECTOR = '.videoOsdBottom .endTimeText';
        var VIDEO_SELECTOR = '.videoPlayerContainer';

        // The button cycles exactly like mpv's own `ab-loop` command, and
        // says which step it is on rather than what it is.
        var BTN_LABELS = {
            setA: 'Set loop start (A)',
            setB: 'Set loop end (B)',
            clear: 'Clear the A-B loop'
        };

        // playbackManager.duration() is in Jellyfin ticks (100 ns), while
        // currentTime() is in milliseconds. input-plugin.js's seekPercent
        // math relies on the same asymmetry.
        var TICKS_PER_SECOND = 10000000;

        // ---- environment ---------------------------------------------------

        function win() {
            return typeof window !== 'undefined' ? window : null;
        }

        function doc() {
            var w = win();
            return w && w.document ? w.document : null;
        }

        function native() {
            var w = win();
            return w && w.jmpNative ? w.jmpNative : null;
        }

        function debug(msg) {
            try {
                console.debug('[ABLoop] ' + msg);
            } catch (e) {
                /* console can be absent under a bare node harness */
            }
        }

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

        // ---- pure helpers ---------------------------------------------------

        // m:ss under an hour, h:mm:ss over it — the same shape jellyfin-web's
        // own time texts use, so the readout reads as one of them.
        function fmtTime(sec) {
            var s = num(sec);
            if (s === null || s < 0) s = 0;
            s = Math.floor(s);
            var h = Math.floor(s / 3600);
            var m = Math.floor((s % 3600) / 60);
            var r = s % 60;
            var mm = h > 0 && m < 10 ? '0' + m : String(m);
            var ss = r < 10 ? '0' + r : String(r);
            return (h > 0 ? h + ':' : '') + mm + ':' + ss;
        }

        // '' when there is nothing to say, which is also what hides the chip.
        function readoutText(st) {
            var a = num(st && st.a);
            if (a === null) return '';
            var b = num(st && st.b);
            if (b === null) return 'A ' + fmtTime(a);
            var lo = Math.min(a, b);
            var hi = Math.max(a, b);
            return 'A↔B ' + fmtTime(lo) + '–' + fmtTime(hi);
        }

        // The cycle mpv's own `ab-loop` command walks: A, then B, then off.
        function nextAction(st) {
            if (num(st && st.a) === null) return 'setA';
            if (num(st && st.b) === null) return 'setB';
            return 'clear';
        }

        // null when B may be set at `posSec`, otherwise the reason it may not.
        function checkB(st, posSec) {
            var a = num(st && st.a);
            if (a === null) return 'no-a';
            var p = num(posSec);
            if (p === null) return 'no-position';
            if (p <= a) return 'before-a';
            if (p - a < MIN_SPAN_S) return 'too-short';
            return null;
        }

        function rejectText(reason) {
            if (reason === 'before-a') return 'Loop end must come after the start';
            if (reason === 'too-short') return 'Loop must be at least half a second long';
            if (reason === 'no-a') return 'Set the loop start (A) first';
            return 'No playback position yet';
        }

        // Percentages of the duration for the band and the two pins. null
        // when there is nothing to draw (no A, or no usable duration). With
        // A only the band has zero width and only the A pin is placed.
        function bandGeometry(st, durationSec) {
            var dur = num(durationSec);
            if (dur === null || dur <= 0) return null;
            var a = num(st && st.a);
            if (a === null) return null;
            var pct = function (v) {
                return Math.max(0, Math.min(100, (v / dur) * 100));
            };
            var aPct = pct(a);
            var b = num(st && st.b);
            if (b === null) {
                return { aPct: aPct, bPct: null, leftPct: aPct, widthPct: 0 };
            }
            var bPct = pct(b);
            return {
                aPct: aPct,
                bPct: bPct,
                leftPct: Math.min(aPct, bPct),
                widthPct: Math.abs(bPct - aPct)
            };
        }

        // Native pushes arrive as seconds or null; anything else (a NaN, a
        // string) is treated as unset rather than drawn.
        function normalizePush(a, b) {
            return { a: num(a), b: num(b) };
        }

        // ---- state -----------------------------------------------------------

        var pm = null;
        var onStart = null;
        var onStop = null;
        var keyHandler = null;
        var retryTimer = null;
        var retriesLeft = 0;

        // The whole of it. Written only by applyPush.
        var state = { a: null, b: null };

        // ---- reading the player ----------------------------------------------

        function player() {
            if (!pm) return null;
            var p = typeof pm.getCurrentPlayer === 'function' ? pm.getCurrentPlayer() : pm._currentPlayer;
            return p || null;
        }

        // Position in ms, or null when there is no player to ask.
        // playbackManager.currentTime() defaults its argument to
        // `_currentPlayer` and throws "player cannot be null" underneath, so
        // the player is always named and always checked first — see the note
        // in input-plugin.js.
        function positionMs() {
            var p = player();
            if (!pm || !p || typeof pm.currentTime !== 'function') return null;
            var t = guard('currentTime', function () {
                return pm.currentTime(p);
            });
            var ms = num(t);
            return ms !== null && ms >= 0 ? ms : null;
        }

        function positionSec() {
            var ms = positionMs();
            return ms === null ? null : ms / 1000;
        }

        // Seconds, from the same duration the OSD's own percentage scrubber
        // is driven by. Null while the item is still loading.
        function durationSec() {
            var p = player();
            if (!pm || !p || typeof pm.duration !== 'function') return null;
            var ticks = num(
                guard('duration', function () {
                    return pm.duration(p);
                })
            );
            return ticks !== null && ticks > 0 ? ticks / TICKS_PER_SECOND : null;
        }

        // Seeking goes through playbackManager where it can, so jellyfin-web's
        // own progress reporting stays in step; the native shim is the last
        // resort. Either way mpv decides where the playhead lands and the OSD
        // follows its `time-pos` observation.
        function seekToMs(ms) {
            var p = player();
            if (pm && p && typeof pm.seekMs === 'function') {
                guard('seekMs', function () {
                    pm.seekMs(ms, p);
                });
                return;
            }
            if (pm && p && typeof pm.seek === 'function') {
                guard('seek', function () {
                    pm.seek(Math.round(ms * (TICKS_PER_SECOND / 1000)), p);
                });
                return;
            }
            var n = native();
            if (n && typeof n.playerSeek === 'function') {
                guard('playerSeek', function () {
                    n.playerSeek(Math.round(ms));
                });
            }
        }

        function resumeIfPaused() {
            var p = player();
            if (!pm || !p) return;
            if (typeof pm.paused !== 'function' || typeof pm.unpause !== 'function') return;
            var paused = guard('paused', function () {
                return pm.paused(p);
            });
            if (!paused) return;
            guard('unpause', function () {
                pm.unpause(p);
            });
        }

        // ---- talking to mpv ---------------------------------------------------

        // Always both ends; -1 is "unset". Deliberately does not touch
        // `state` — that is mpv's to report.
        function send(aSec, bSec) {
            var n = native();
            if (!n || typeof n.playerSetAbLoop !== 'function') {
                debug('playerSetAbLoop is not available');
                return false;
            }
            var toMs = function (v) {
                var s = num(v);
                return s === null ? -1 : Math.round(s * 1000);
            };
            guard('playerSetAbLoop', function () {
                n.playerSetAbLoop(toMs(aSec), toMs(bSec));
            });
            return true;
        }

        // ---- actions -----------------------------------------------------------

        // Setting A always begins a fresh loop: A alone means mpv is not
        // looping yet, so there is no B worth keeping.
        function setA() {
            var pos = positionSec();
            if (pos === null) {
                toast(rejectText('no-position'));
                return;
            }
            if (!send(pos, null)) return;
            toast('Loop start (A) at ' + fmtTime(pos));
        }

        function setB() {
            var pos = positionSec();
            var reason = checkB(state, pos);
            if (reason) {
                toast(rejectText(reason));
                return;
            }
            if (!send(state.a, pos)) return;
            toast('Looping ' + fmtTime(state.a) + '–' + fmtTime(pos));
        }

        function clearPoints() {
            if (state.a === null && state.b === null) return;
            if (!send(null, null)) return;
            toast('A-B loop cleared');
        }

        function cycle() {
            var action = nextAction(state);
            if (action === 'setA') setA();
            else if (action === 'setB') setB();
            else clearPoints();
        }

        // `[` with A already set is "back to A": the one thing that is worth
        // a key of its own while rehearsing a passage. Resuming on the way is
        // deliberate — you press it to hear the bit again.
        function jumpToA() {
            if (state.a === null) {
                setA();
                return;
            }
            seekToMs(Math.round(state.a * 1000));
            resumeIfPaused();
        }

        // ---- keyboard -----------------------------------------------------------

        function videoActive() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return false;
            return !!d.querySelector(VIDEO_SELECTOR);
        }

        function isTypingTarget(t) {
            if (!t) return false;
            var tag = String(t.tagName || '').toUpperCase();
            if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
            return !!t.isContentEditable;
        }

        // Capture phase, so jellyfin-web's own key handling cannot swallow
        // these first. preventDefault only when the key was actually acted
        // on, so `[` outside playback still reaches the page.
        function onKeyDown(e) {
            guard('keydown', function () {
                if (!e || e.altKey || e.ctrlKey || e.metaKey) return;
                if (!videoActive() || isTypingTarget(e.target)) return;
                var handled = true;
                if (e.key === '[') jumpToA();
                else if (e.key === ']') setB();
                else if (e.key === '\\') clearPoints();
                else handled = false;
                if (handled && typeof e.preventDefault === 'function') e.preventDefault();
            });
        }

        // ---- toast ---------------------------------------------------------------

        // Same synthesized markup playback-source.js uses: jellyfin-web's
        // toast module is ESM and cannot be imported from an injected script,
        // and the classes are what toast.scss and our own theme already
        // style. Kept local rather than shared, so nothing about the source
        // badge's behaviour depends on this file loading.
        function toast(text) {
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

        // ---- DOM -------------------------------------------------------------------

        // Idempotent. Sits with the right-hand utility cluster, before the
        // subtitle button, and is created once per OSD instance.
        function ensureButton() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return null;
            var host = d.querySelector(BUTTONS_SELECTOR);
            if (!host) return null;
            var existing = d.querySelector('.' + BTN_CLASS);
            if (existing && existing.parentNode === host) return existing;
            if (existing && existing.parentNode) existing.parentNode.removeChild(existing);
            var btn = d.createElement('button');
            btn.setAttribute('type', 'button');
            btn.className = 'paper-icon-button-light ' + BTN_CLASS;
            var icon = d.createElement('span');
            icon.className = 'material-icons xlargePaperIconButton';
            icon.setAttribute('aria-hidden', 'true');
            icon.textContent = 'repeat';
            btn.appendChild(icon);
            if (typeof btn.addEventListener === 'function') {
                btn.addEventListener('click', function (e) {
                    guard('button click', function () {
                        if (e && typeof e.preventDefault === 'function') e.preventDefault();
                        cycle();
                    });
                });
            }
            var anchor = host.querySelector('.btnSubtitles') || host.querySelector('.btnVideoOsdSettings');
            if (anchor && typeof host.insertBefore === 'function') host.insertBefore(btn, anchor);
            else host.appendChild(btn);
            return btn;
        }

        // A chip beside the "ends at" text, in the same mono face the time
        // readouts use.
        function ensureReadout() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return null;
            var end = d.querySelector(END_TIME_SELECTOR);
            if (!end || !end.parentNode) return null;
            var existing = d.querySelector('.' + READOUT_CLASS);
            if (existing && existing.parentNode === end.parentNode) return existing;
            if (existing && existing.parentNode) existing.parentNode.removeChild(existing);
            var el = d.createElement('div');
            el.className = READOUT_CLASS;
            end.parentNode.insertBefore(el, end.nextSibling);
            return el;
        }

        // A click-through band over the scrubber. It is a child of
        // .mdl-slider-container (which the theme gives `position: relative`
        // for exactly this) and is inset like jf-web's own
        // .sliderMarkerContainer so its percentages land on the same track
        // the chapter stars do.
        function ensureOverlay() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return null;
            var host = d.querySelector(SLIDER_SELECTOR);
            if (!host) return null;
            var existing = d.querySelector('.' + OVERLAY_CLASS);
            if (existing && existing.parentNode === host) return existing;
            if (existing && existing.parentNode) existing.parentNode.removeChild(existing);
            var el = d.createElement('div');
            el.className = OVERLAY_CLASS;
            el.setAttribute('aria-hidden', 'true');
            var band = d.createElement('div');
            band.className = BAND_CLASS;
            var pinA = d.createElement('div');
            pinA.className = PIN_CLASS + ' ' + PIN_CLASS + '--a';
            pinA.setAttribute('data-label', 'A');
            var pinB = d.createElement('div');
            pinB.className = PIN_CLASS + ' ' + PIN_CLASS + '--b';
            pinB.setAttribute('data-label', 'B');
            el.appendChild(band);
            el.appendChild(pinA);
            el.appendChild(pinB);
            host.appendChild(el);
            return el;
        }

        function renderButton() {
            var btn = ensureButton();
            if (!btn) return false;
            var label = BTN_LABELS[nextAction(state)];
            btn.setAttribute('aria-label', label);
            btn.setAttribute('title', label);
            var looping = state.a !== null && state.b !== null;
            if (btn.classList) {
                if (looping) btn.classList.add('is-active');
                else btn.classList.remove('is-active');
            }
            return true;
        }

        function renderReadout() {
            var el = ensureReadout();
            if (!el) return false;
            var text = readoutText(state);
            el.textContent = text;
            el.hidden = !text;
            return true;
        }

        function renderOverlay() {
            var el = ensureOverlay();
            if (!el) return false;
            var dur = durationSec();
            var geo = bandGeometry(state, dur);
            if (!geo) {
                el.hidden = true;
                // Mounted, but a loop exists and the duration does not yet:
                // report "not done" so the band is not left hidden over an
                // item that is still opening.
                return !(state.a !== null && dur === null);
            }
            el.hidden = false;
            var band = el.querySelector('.' + BAND_CLASS);
            var pinA = el.querySelector('.' + PIN_CLASS + '--a');
            var pinB = el.querySelector('.' + PIN_CLASS + '--b');
            if (band) {
                band.hidden = geo.bPct === null;
                band.style.left = geo.leftPct + '%';
                band.style.width = geo.widthPct + '%';
            }
            if (pinA) {
                pinA.hidden = false;
                pinA.style.left = geo.aPct + '%';
            }
            if (pinB) {
                pinB.hidden = geo.bPct === null;
                if (geo.bPct !== null) pinB.style.left = geo.bPct + '%';
            }
            return true;
        }

        // Idempotent, and the only thing that draws. Every piece it could not
        // place yet arms a bounded retry, because the OSD mounts after
        // `playbackstart`.
        function render() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return;
            var mounted = 0;
            if (guard('render button', renderButton)) mounted += 1;
            if (guard('render readout', renderReadout)) mounted += 1;
            if (guard('render overlay', renderOverlay)) mounted += 1;
            if (mounted < 3) scheduleRetry();
        }

        function removeUi() {
            var d = doc();
            if (!d || typeof d.querySelector !== 'function') return;
            var selectors = ['.' + BTN_CLASS, '.' + READOUT_CLASS, '.' + OVERLAY_CLASS];
            for (var i = 0; i < selectors.length; i++) {
                var el = d.querySelector(selectors[i]);
                if (el && el.parentNode) el.parentNode.removeChild(el);
            }
        }

        // ---- retries ------------------------------------------------------------------

        function clearRetry() {
            var w = win();
            if (retryTimer !== null && w && typeof w.clearTimeout === 'function') w.clearTimeout(retryTimer);
            retryTimer = null;
            retriesLeft = 0;
        }

        function armRetries() {
            clearRetry();
            retriesLeft = RETRY_MAX;
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

        // ---- native push ------------------------------------------------------------------

        // The only writer of `state`. The native side clears mpv's points on
        // every load and stop, so an item change arrives here as a null/null
        // push and the UI empties itself.
        function applyPush(a, b) {
            state = normalizePush(a, b);
            debug('points a=' + state.a + ' b=' + state.b);
            armRetries();
            render();
        }

        // ---- lifecycle ------------------------------------------------------------------

        function handleStart() {
            armRetries();
            render();
        }

        function handleStop() {
            clearRetry();
            state = { a: null, b: null };
            removeUi();
        }

        function attach(playbackManager) {
            return guard('attach', function () {
                var w = win();
                if (!playbackManager || !w || !w.Events) return;
                if (pm === playbackManager) return;
                detach();
                pm = playbackManager;
                onStart = function () {
                    guard('playbackstart', handleStart);
                };
                onStop = function () {
                    guard('playbackstop', handleStop);
                };
                w.Events.on(pm, 'playbackstart', onStart);
                w.Events.on(pm, 'playbackstop', onStop);
                if (typeof w.addEventListener === 'function') {
                    keyHandler = onKeyDown;
                    w.addEventListener('keydown', keyHandler, true);
                }
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
                if (keyHandler && w && typeof w.removeEventListener === 'function') {
                    w.removeEventListener('keydown', keyHandler, true);
                }
                clearRetry();
                keyHandler = null;
                pm = null;
                onStart = null;
                onStop = null;
                state = { a: null, b: null };
            });
        }

        var api = {
            attach: attach,
            detach: detach,
            // Controls, also the surface the OSD button and the keys use.
            cycle: cycle,
            setA: setA,
            setB: setB,
            clear: clearPoints,
            jumpToA: jumpToA,
            // Testing surface: the pure helpers, then the state hooks.
            fmtTime: fmtTime,
            readoutText: readoutText,
            nextAction: nextAction,
            checkB: checkB,
            rejectText: rejectText,
            bandGeometry: bandGeometry,
            normalizePush: normalizePush,
            MIN_SPAN_S: MIN_SPAN_S,
            _state: function () {
                return state;
            },
            _render: render
        };

        var w = win();
        if (w) {
            w.AstrofinAbLoop = api;
            // The native side's only way in. Guarded there too, because the
            // first observation can land before this file has run.
            w._nativeAbLoop = function (a, b) {
                guard('_nativeAbLoop', function () {
                    applyPush(a, b);
                });
            };
        }
        // Unit tests run this file under node, where there is no window.
        if (typeof module !== 'undefined' && module.exports) module.exports = api;
    } catch (e) {
        // A top-level throw in an injected script takes every other injected
        // script in the same execute_java_script bundle with it — including
        // the mpv player shims. Swallow and carry on without the loop.
        try {
            console.error('[ABLoop] module failed to install:', e);
        } catch (ignored) {
            /* no console */
        }
    }
})();
