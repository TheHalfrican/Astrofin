/*
 * Astrofin Settings page (design canvas screen 7).
 *
 * Injected into the "web" browser last, after astrofin-theme.js (see
 * WEB_SCRIPTS and build_for_kind in src/jfn_cef/src/injection.rs). It owns one
 * screen only, so it lives here rather than in astrofin-theme.js.
 *
 * client-settings.js builds the stock jellyfin-web form and then announces it:
 * `html.af-settings` goes on, `af-settings-show` fires on `document` with the
 * mounted page, and `af-settings-hide` fires just before the page is removed.
 * Everything below runs off those two events.
 *
 * What it does to the mounted page:
 *   1. builds a six-row section rail (Server / Playback / Video mode / Audio /
 *      Advanced / About) and six glass panels, one visible at a time;
 *   2. *moves* every `[data-af-setting]` container the form built into the
 *      panel its key maps to — moved, never cloned, so the change listeners
 *      client-settings.js attached keep working and every write still goes
 *      through `window.api.settings.setValue`;
 *   3. replaces the video-mode `<select>` with a four-state segmented switch.
 *      The select stays in the DOM, visually hidden, and remains the source of
 *      truth: a segment sets `selectedIndex` and dispatches a bubbling
 *      `change`, which is the same event a mouse on the select would raise;
 *   4. tags each row LIVE or RESTART from `data-af-applies` and counts the
 *      RESTART changes into a floating "Restart to apply N changes" pill;
 *   5. adds the read-only server address, the now-playing resolve card (only
 *      when a title is actually loaded and a resolution was recorded) and the
 *      About rows.
 *
 * It adds no IPC, makes no network calls and reads only what the shims already
 * put on `window` (`jmpInfo`, `__afVideoModeResolved`, `navigator`). Anything
 * it cannot source honestly is omitted rather than invented.
 *
 * Style rules are section (p) of astrofin-theme.css, all gated on
 * `html.af-settings`; with the gate absent the stock page is untouched.
 */
(function () {
    'use strict';

    // This file is concatenated into one execute_java_script call with every
    // other injected shim, so a throw at this level would abort the ones after
    // it. Nothing here may escape.
    try {

        if (window.__afSettings) {
            // Unit tests run this file under node: a second load against a
            // window that already has it hands back the installation that is
            // there, so no listener is registered twice.
            if (typeof module !== 'undefined' && module.exports) {
                module.exports = window.__afSettings;
            }
            return;
        }

        var doc = document;
        var SVG_NS = 'http://www.w3.org/2000/svg';

        function root() { return doc.documentElement; }

        function log(err) {
            if (window.console && console.debug) {
                console.debug('[Astrofin settings]', err);
            }
        }

        function guard(fn) {
            return function () {
                try { return fn.apply(null, arguments); } catch (e) { log(e); return undefined; }
            };
        }

        /* ---- the rail ------------------------------------------------------
         * Labels, panel headings and icon paths are the artboard's `railDefs`
         * (docs/design/canvas/Settings.dc.html). */
        var SECTIONS = [
            {
                id: 'server', label: 'Server', title: 'Jellyfin server',
                icon: 'M4 6.5h16v5H4zM4 12.5h16v5H4zM7 9h.01M7 15h.01'
            },
            {
                id: 'playback', label: 'Playback', title: 'Decoding and transcodes',
                icon: 'M5 4.5v15l14-7.5z'
            },
            {
                id: 'video', label: 'Video mode', title: 'Upscaling preset',
                icon: 'M4 5.5h16v13H4zM4 10h16M9 5.5v13M15 5.5v13'
            },
            {
                id: 'audio', label: 'Audio', title: 'Output and passthrough',
                icon: 'M4 9.5v5h3.5L12 18.5v-13L7.5 9.5zM15.5 8.5a5 5 0 0 1 0 7M18 6a8.5 8.5 0 0 1 0 12'
            },
            {
                id: 'advanced', label: 'Advanced', title: 'Window, logs and mpv',
                icon: 'M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7zM12 3.5v2M12 18.5v2M3.5 12h2'
                    + 'M18.5 12h2M6 6l1.4 1.4M16.6 16.6 18 18M6 18l1.4-1.4M16.6 7.4 18 6'
            },
            {
                id: 'about', label: 'About', title: 'This build',
                icon: 'M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 11v5.5M12 7.8v.01'
            }
        ];

        /* Which panel each setting belongs in. Owner decision: Device Name is a
         * server-facing name, so it leaves Advanced for Server. */
        var PANEL_FOR_KEY = {
            deviceName: 'server',
            hwdec: 'playback',
            transcodeNotice: 'playback',
            forceTranscoding: 'playback',
            videoMode: 'video',
            audioPassthrough: 'audio',
            audioChannels: 'audio',
            audioExclusive: 'audio',
            windowDecorations: 'advanced',
            transparentTitlebar: 'advanced',
            hideScrollbar: 'advanced',
            logLevel: 'advanced'
        };

        /* A key the map above has never heard of — a setting added to
         * native-shim.js and not to this file — still has to land somewhere
         * visible, so it falls back to its jmpInfo section. */
        var PANEL_FOR_SECTION = {
            server: 'server',
            playback: 'playback',
            transcode: 'playback',
            audio: 'audio',
            advanced: 'advanced',
            mpv: 'advanced'
        };
        var FALLBACK_PANEL = 'advanced';
        var DEFAULT_PANEL = 'video';

        function panelFor(key, section) {
            if (key && Object.prototype.hasOwnProperty.call(PANEL_FOR_KEY, key)) {
                return PANEL_FOR_KEY[key];
            }
            var s = String(section || '');
            return Object.prototype.hasOwnProperty.call(PANEL_FOR_SECTION, s)
                ? PANEL_FOR_SECTION[s]
                : FALLBACK_PANEL;
        }

        /* ---- video mode ---------------------------------------------------- */

        var MODE_LABELS = {
            'auto': 'Auto',
            'live-action': 'Live-Action',
            'animation': 'Animation',
            'anime': 'Animation',
            'off': 'Off'
        };

        var MODE_ICONS = {
            'auto': 'M12 3.5 14.4 9.6 21 10.1 16 14.3 17.6 20.5 12 17.1 6.4 20.5 8 14.3 3 10.1 9.6 9.6z',
            'live-action': 'M4 6.5h16v11H4zM4 10.5h16M8 6.5v11M16 6.5v11',
            'animation': 'M5 17.5c2.5-7 5-10 8.5-11.5-1 4-3.5 8-8.5 11.5zM13.5 6c2 .5 4 2 5.5 4.5',
            'off': 'M12 3.5v8M6.7 7.2a7.5 7.5 0 1 0 10.6 0'
        };

        /* The rule chain video-mode-resolver.js walks, in order. */
        var CHAIN = ['Tag', 'Genre', 'Library', 'Default'];

        function modeLabel(value) {
            var key = String(value == null ? '' : value).toLowerCase();
            return MODE_LABELS[key] || key;
        }

        /* The descriptors carry one string per option — "Auto — pick per title
         * from tags, genres and library". The segment shows the head, the
         * sub-line below the switch shows the tail. */
        function shortModeLabel(value, text) {
            var key = String(value == null ? '' : value).toLowerCase();
            if (MODE_LABELS[key]) { return MODE_LABELS[key]; }
            var head = String(text || '').split('—')[0];
            return head.trim() || key;
        }

        function modeDescription(text) {
            var parts = String(text || '').split('—');
            return parts.length > 1 ? parts.slice(1).join('—').trim() : '';
        }

        /* Which rule won, from the reason string video-mode-resolver.js built.
         * -1 for anything else (the post-error fallback), which lights nothing
         * rather than guessing. */
        function chainIndexFor(reason) {
            var r = String(reason || '').toLowerCase();
            if (r.indexOf('library') === 0) { return 2; }
            if (r.indexOf('tag:') !== -1) { return 0; }
            if (r.indexOf('genre:') !== -1) { return 1; }
            if (r.indexOf('default') === 0) { return 3; }
            return -1;
        }

        /* ---- small readers -------------------------------------------------- */

        /* Only ever answers in a browser that keeps the Chrome token. CEF is
         * started with `Mozilla/5.0 Astrofin/<version>` (src/jfn_cef/src/ffi.rs),
         * so in the app this yields '' and the About row is omitted; it is kept
         * because nothing else would tell us, and a UA change would light it. */
        function chromiumVersion(ua) {
            var m = /Chrome\/([0-9][0-9.]*)/.exec(String(ua || ''));
            return m ? m[1] : '';
        }

        function serverAddress() {
            var jmp = window.jmpInfo;
            var main = jmp && jmp.settings ? jmp.settings.main : null;
            var url = main ? main.userWebClient : '';
            return typeof url === 'string' ? url : '';
        }

        /* The scheme, and only the scheme. Whether the server answers is not
         * something this page can know without asking it. */
        function connectionKind(url) {
            var s = String(url || '').toLowerCase();
            if (s.indexOf('https:') === 0) { return 'https'; }
            if (s.indexOf('http:') === 0) { return 'http'; }
            return '';
        }

        function aboutRows(info, nav) {
            var rows = [];
            var version = info && info.version;
            if (version) { rows.push(['Version', String(version)]); }
            var platform = nav && nav.platform;
            if (platform) { rows.push(['Platform', String(platform)]); }
            var chromium = chromiumVersion(nav && nav.userAgent);
            if (chromium) { rows.push(['Chromium (CEF)', chromium]); }
            return rows;
        }

        /* A title is loaded when the video layer is up — the same probe
         * astrofin-theme.js's updateVideoMode() uses for `html.af-video`. */
        function isPlaying() {
            var el = root();
            if (el && el.classList && el.classList.contains('af-video')) { return true; }
            return !!doc.querySelector('.videoPlayerContainer');
        }

        /* The last per-title resolution mpv-video-player.js recorded, but only
         * while Auto is the setting and something is actually playing.
         * Otherwise there is nothing honest to show and the card is omitted. */
        function resolveInfo() {
            var jmp = window.jmpInfo;
            var playback = jmp && jmp.settings ? jmp.settings.playback : null;
            if (!playback || playback.videoMode !== 'auto') { return null; }
            if (!isPlaying()) { return null; }
            var last = window.__afVideoModeResolved;
            if (!last || !last.mode || !last.reason) { return null; }
            return { mode: last.mode, reason: last.reason, title: last.title || '' };
        }

        /* ---- DOM helpers ---------------------------------------------------- */

        function el(tag, cls, text) {
            var node = doc.createElement(tag);
            if (cls) { node.className = cls; }
            if (text != null) { node.textContent = text; }
            return node;
        }

        function icon(path) {
            if (!doc.createElementNS) { return el('span', 'af-set-icon'); }
            var svg = doc.createElementNS(SVG_NS, 'svg');
            svg.setAttribute('class', 'af-set-icon');
            svg.setAttribute('viewBox', '0 0 24 24');
            svg.setAttribute('aria-hidden', 'true');
            var p = doc.createElementNS(SVG_NS, 'path');
            p.setAttribute('d', path);
            p.setAttribute('fill', 'none');
            p.setAttribute('stroke', 'currentColor');
            p.setAttribute('stroke-width', '1.6');
            p.setAttribute('stroke-linecap', 'round');
            p.setAttribute('stroke-linejoin', 'round');
            svg.appendChild(p);
            return svg;
        }

        function all(node, selector) {
            if (!node || !node.querySelectorAll) { return []; }
            return Array.prototype.slice.call(node.querySelectorAll(selector));
        }

        function fireChange(node) {
            var Ctor = window.CustomEvent || window.Event;
            if (typeof Ctor !== 'function' || !node.dispatchEvent) { return false; }
            node.dispatchEvent(new Ctor('change', { bubbles: true }));
            return true;
        }

        function optionsOf(select) {
            return select ? all(select, 'option') : [];
        }

        /* ---- rows ------------------------------------------------------------
         * One row is label + LIVE/RESTART tag + description on the left and the
         * control on the right, with a hairline under all but the last. */
        function buildRow(opts) {
            var o = opts || {};
            var row = el('div', 'af-set-row');
            if (o.key) { row.setAttribute('data-af-row', o.key); }
            if (o.applies) { row.setAttribute('data-af-applies', o.applies); }

            var text = el('div', 'af-set-row-text');
            var head = el('div', 'af-set-row-head');
            head.appendChild(el('span', 'af-set-row-label', o.label || ''));
            if (o.applies === 'live') {
                head.appendChild(el('span', 'af-set-tag af-set-tag-live', 'LIVE'));
            } else if (o.applies === 'restart') {
                head.appendChild(el('span', 'af-set-tag af-set-tag-restart', 'RESTART'));
            }
            text.appendChild(head);
            if (o.description) {
                text.appendChild(el('div', 'fieldDescription af-set-row-desc', o.description));
            } else if (o.descriptionNode) {
                o.descriptionNode.classList.add('af-set-row-desc');
                text.appendChild(o.descriptionNode);
            }
            row.appendChild(text);

            var control = el('div', 'af-set-row-control');
            if (o.control) { control.appendChild(o.control); }
            row.appendChild(control);
            return row;
        }

        /* The label jellyfin-web gave the control, wherever it put it: an
         * .inputLabel for text/textarea/codecList, a .checkboxLabel inside the
         * <label> for a checkbox, the `label` attribute for a select (which
         * emby-select later copies into a .selectLabel of its own). */
        function labelOf(container) {
            var node = container.querySelector('.inputLabel')
                || container.querySelector('.checkboxLabel');
            if (node && node.textContent) { return node.textContent; }
            var select = container.querySelector('select');
            var attr = select && select.getAttribute('label');
            return attr || '';
        }

        function rowFor(container) {
            return buildRow({
                key: container.getAttribute('data-af-setting'),
                applies: container.getAttribute('data-af-applies'),
                label: labelOf(container),
                descriptionNode: container.querySelector('.fieldDescription'),
                control: container
            });
        }

        /* ---- the segmented switch -------------------------------------------
         * `select` stays in the DOM and stays authoritative: a segment writes
         * selectedIndex and raises `change`, which is what client-settings.js
         * listens on, so the write still goes through
         * window.api.settings.setValue. Nothing here talks to native. */
        function buildModeSwitch(select, onSync) {
            var node = el('div', 'af-mode-switch');
            node.setAttribute('role', 'radiogroup');
            node.setAttribute('aria-label', 'Video mode');

            var options = optionsOf(select);
            var segs = [];

            function sync() {
                var idx = select ? select.selectedIndex : -1;
                segs.forEach(function (seg, i) {
                    var on = i === idx;
                    seg.classList.toggle('af-on', on);
                    seg.setAttribute('aria-checked', on ? 'true' : 'false');
                    seg.setAttribute('tabindex', on ? '0' : '-1');
                });
                if (onSync) { onSync(idx >= 0 ? options[idx] : null); }
                return idx;
            }

            function pick(idx, focus) {
                if (idx < 0 || idx >= segs.length) { return -1; }
                if (!select) { return -1; }
                if (select.selectedIndex !== idx) {
                    select.selectedIndex = idx;
                    fireChange(select);
                }
                sync();
                if (focus && segs[idx].focus) { segs[idx].focus(); }
                return idx;
            }

            options.forEach(function (opt, i) {
                var seg = doc.createElement('button');
                seg.type = 'button';
                seg.className = 'af-mode-seg';
                seg.setAttribute('role', 'radio');
                seg.setAttribute('data-af-mode', opt.value || '');
                seg.setAttribute('title', opt.textContent || '');
                seg.appendChild(icon(MODE_ICONS[String(opt.value || '').toLowerCase()]
                    || MODE_ICONS.off));
                seg.appendChild(el('span', 'af-mode-seg-label',
                    shortModeLabel(opt.value, opt.textContent)));
                seg.addEventListener('click', guard(function () { pick(i, false); }));
                seg.addEventListener('keydown', guard(function (e) {
                    var step = 0;
                    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { step = 1; }
                    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { step = -1; }
                    else { return; }
                    if (e.preventDefault) { e.preventDefault(); }
                    pick(i + step, true);
                }));
                node.appendChild(seg);
                segs.push(seg);
            });

            // Changed from somewhere else (a script, or the hidden select
            // itself reached by keyboard): follow it.
            if (select) { select.addEventListener('change', guard(sync)); }
            sync();
            return { node: node, segs: segs, sync: sync, pick: pick };
        }

        /* ---- the now-playing resolve card ----------------------------------- */

        function renderResolveCard() {
            var info = resolveInfo();
            if (!info) { return null; }

            var card = el('div', 'af-resolve');
            var head = el('div', 'af-resolve-head');
            head.appendChild(el('span', 'af-resolve-eyebrow', 'Now playing'));
            if (info.title) {
                head.appendChild(el('span', 'af-resolve-title', info.title));
            }
            card.appendChild(head);

            var line = el('div', 'af-resolve-line');
            line.appendChild(el('span', null, 'Resolved to'));
            line.appendChild(el('span', 'af-resolve-mode', modeLabel(info.mode)));
            card.appendChild(line);

            var winner = chainIndexFor(info.reason);
            var chain = el('div', 'af-resolve-chain');
            CHAIN.forEach(function (label, i) {
                var step = el('div', 'af-resolve-step');
                if (i === winner) { step.classList.add('af-on'); }
                if (winner >= 0 && i < winner) { step.classList.add('af-past'); }
                step.appendChild(el('span', 'af-resolve-dot'));
                step.appendChild(el('span', 'af-resolve-step-label', label));
                chain.appendChild(step);
            });
            card.appendChild(chain);
            card.appendChild(el('div', 'af-resolve-reason', info.reason));
            return card;
        }

        /* ---- panels ---------------------------------------------------------- */

        function buildPanel(def) {
            var panel = el('section', 'af-set-panel');
            panel.setAttribute('data-af-panel', def.id);
            panel.hidden = true;

            var head = el('div', 'af-set-head');
            var heading = el('div', 'af-set-heading');
            heading.appendChild(el('div', 'af-set-eyebrow', def.label));
            heading.appendChild(el('h2', 'af-set-panel-title', def.title));
            head.appendChild(heading);
            if (def.id === 'video') {
                head.appendChild(el('span', 'af-set-tag af-set-tag-live af-set-head-tag',
                    'LIVE · APPLIES MID-PLAYBACK'));
            }
            panel.appendChild(head);

            var card = el('div', 'af-set-card');
            panel.appendChild(card);
            return { panel: panel, card: card };
        }

        /* Video mode is the headline: the switch, the selected mode's own
         * description, the resolve card and the setting's help text, with the
         * real <select> kept (visually hidden) at the end. */
        function buildVideoPanel(card, container) {
            var block = el('div', 'af-video-block');
            var select = container.querySelector('select');

            var grid = el('div', 'af-video-grid');
            var detail = el('div', 'af-mode-detail');
            var detailTitle = el('div', 'af-mode-detail-title');
            var detailBody = el('div', 'af-mode-detail-body');
            detail.appendChild(detailTitle);
            detail.appendChild(detailBody);

            var sw = buildModeSwitch(select, function (opt) {
                detailTitle.textContent = opt ? shortModeLabel(opt.value, opt.textContent) : '';
                detailBody.textContent = opt ? modeDescription(opt.textContent) : '';
            });
            block.appendChild(sw.node);

            grid.appendChild(detail);
            var resolve = renderResolveCard();
            if (resolve) { grid.appendChild(resolve); }
            block.appendChild(grid);

            var help = container.querySelector('.fieldDescription');
            if (help) {
                help.classList.add('af-mode-note');
                block.appendChild(help);
            }
            block.appendChild(container);
            card.appendChild(block);
            return sw;
        }

        /* The address this client signs in to, read-only: changing it is the
         * sign-out row below, not an edit here. */
        function buildServerAddressRow() {
            var address = serverAddress();
            if (!address) { return null; }
            var chip = el('div', 'af-set-address');
            chip.appendChild(el('span', 'af-set-address-dot'));
            chip.appendChild(el('span', 'af-set-address-url', address));
            var wrap = el('div', 'af-set-address-wrap');
            wrap.appendChild(chip);
            var kind = connectionKind(address);
            if (kind) { wrap.appendChild(el('span', 'af-set-address-kind', kind.toUpperCase())); }
            return buildRow({
                key: 'serverAddress',
                label: 'Server address',
                description: 'Where this client signs in. Plain http on the LAN is fine; '
                    + 'a bare host name tries https first.',
                control: wrap
            });
        }

        /* The wordmark's comet, same path as the artboard and
         * resources/brand/astrofin-icon.svg. */
        function cometMark() {
            var mark = el('div', 'af-about-mark');
            if (!doc.createElementNS) { return mark; }
            var svg = doc.createElementNS(SVG_NS, 'svg');
            svg.setAttribute('class', 'af-about-comet');
            svg.setAttribute('viewBox', '0 0 28 28');
            svg.setAttribute('aria-hidden', 'true');
            var p = doc.createElementNS(SVG_NS, 'path');
            p.setAttribute('d', 'M4 22.5 C11 21 18.5 15 23.5 4.5 C22.5 14 19 20 14.5 22.5 Z');
            p.setAttribute('fill', 'currentColor');
            svg.appendChild(p);
            mark.appendChild(svg);
            return mark;
        }

        function buildAboutPanel(card) {
            var identity = el('div', 'af-about-identity');
            identity.appendChild(cometMark());
            var names = el('div', 'af-about-names');
            names.appendChild(el('div', 'af-about-wordmark', 'Astrofin'));
            var jmp = window.jmpInfo;
            var version = (jmp && jmp.version) || '';
            if (version) { names.appendChild(el('div', 'af-about-version', version)); }
            identity.appendChild(names);
            card.appendChild(identity);

            card.appendChild(el('div', 'af-about-blurb',
                'A desktop client for Jellyfin with mpv underneath the web UI. '
                + 'Based on Jellium Desktop, GPL-2.0.'));

            var rows = aboutRows(jmp, window.navigator);
            var list = el('div', 'af-about-rows');
            rows.forEach(function (pair) {
                var row = el('div', 'af-about-row');
                row.appendChild(el('span', null, pair[0]));
                row.appendChild(el('span', 'af-about-value', pair[1]));
                list.appendChild(row);
            });
            card.appendChild(list);
            return rows.length;
        }

        /* ---- the rail --------------------------------------------------------- */

        function buildRail(onPick) {
            var rail = doc.createElement('nav');
            rail.className = 'af-set-rail';
            rail.setAttribute('aria-label', 'Settings sections');
            var tabs = {};
            SECTIONS.forEach(function (def, i) {
                var tab = doc.createElement('button');
                tab.type = 'button';
                tab.className = 'af-set-tab';
                // `data-af-tab`, not `data-af-panel`: the panel it shows
                // carries that one, and one selector must never find both.
                tab.setAttribute('data-af-tab', def.id);
                tab.setAttribute('tabindex', '-1');
                tab.appendChild(icon(def.icon));
                tab.appendChild(el('span', 'af-set-tab-label', def.label));
                tab.addEventListener('click', guard(function () { onPick(def.id, false); }));
                tab.addEventListener('keydown', guard(function (e) {
                    var step = 0;
                    if (e.key === 'ArrowDown') { step = 1; }
                    else if (e.key === 'ArrowUp') { step = -1; }
                    else { return; }
                    if (e.preventDefault) { e.preventDefault(); }
                    var next = SECTIONS[i + step];
                    if (next) { onPick(next.id, true); }
                }));
                rail.appendChild(tab);
                tabs[def.id] = tab;
            });
            return { node: rail, tabs: tabs };
        }

        /* ---- state ------------------------------------------------------------ */

        var view = null;
        var installed = false;

        function selectPanel(id, focus) {
            if (!view || !view.panels[id]) { return null; }
            Object.keys(view.panels).forEach(function (key) {
                var on = key === id;
                var panel = view.panels[key].panel;
                panel.classList.toggle('af-on', on);
                panel.hidden = !on;
                var tab = view.tabs[key];
                if (!tab) { return; }
                tab.classList.toggle('af-on', on);
                tab.setAttribute('tabindex', on ? '0' : '-1');
                if (on) { tab.setAttribute('aria-current', 'true'); }
                else { tab.removeAttribute('aria-current'); }
            });
            view.current = id;
            if (focus && view.tabs[id] && view.tabs[id].focus) { view.tabs[id].focus(); }
            return id;
        }

        function updatePill() {
            if (!view || !view.pill) { return 0; }
            var n = view.pending.size;
            view.pill.hidden = n === 0;
            view.pillLabel.textContent = 'Restart to apply ' + n
                + (n === 1 ? ' change' : ' changes');
            return n;
        }

        function onFormChange(e) {
            if (!view) { return; }
            var target = e && e.target;
            if (!target || !target.closest) { return; }
            var container = target.closest('[data-af-setting]');
            if (!container || container.getAttribute('data-af-applies') !== 'restart') { return; }
            view.pending.add(container.getAttribute('data-af-setting'));
            updatePill();
        }

        function buildPill() {
            var pill = el('div', 'af-set-pill');
            pill.hidden = true;
            pill.appendChild(icon('M16 10a6 6 0 1 1-1.8-4.3M16 3.5v3h-3'));
            var label = el('span', 'af-set-pill-label', '');
            pill.appendChild(label);
            return { pill: pill, label: label };
        }

        /* ---- decoration -------------------------------------------------------- */

        function decorate(page) {
            if (!page || page.__afSettingsDecorated) { return null; }
            var container = page.querySelector('.settingsContainer');
            var form = page.querySelector('form');
            if (!container || !form) { return null; }
            page.__afSettingsDecorated = true;

            var rail = buildRail(function (id, focus) { selectPanel(id, focus); });
            var host = el('div', 'af-set-panels');
            var panels = {};
            SECTIONS.forEach(function (def) {
                var built = buildPanel(def);
                panels[def.id] = built;
                host.appendChild(built.panel);
            });

            var pill = buildPill();
            view = {
                page: page,
                form: form,
                panels: panels,
                tabs: rail.tabs,
                current: null,
                pending: new Set(),
                pill: pill.pill,
                pillLabel: pill.label,
                modeSwitch: null
            };

            // Move, never clone: the change listeners client-settings.js
            // attached are the only path to window.api.settings.setValue.
            all(form, '[data-af-setting]').forEach(function (setting) {
                var key = setting.getAttribute('data-af-setting');
                var target = panels[panelFor(key, setting.getAttribute('data-af-section'))]
                    || panels[FALLBACK_PANEL];
                if (key === 'videoMode') {
                    view.modeSwitch = buildVideoPanel(target.card, setting);
                } else {
                    target.card.appendChild(rowFor(setting));
                }
            });

            var configBtn = form.querySelector('[data-af-action="open-config-dir"]');
            if (configBtn) {
                panels.advanced.card.appendChild(buildRow({
                    key: 'open-config-dir',
                    label: 'mpv configuration',
                    description: 'Your own mpv.conf and shaders live here and layer under '
                        + 'the video mode.',
                    control: configBtn
                }));
            }

            var addressRow = buildServerAddressRow();
            if (addressRow) {
                panels.server.card.insertBefore(addressRow, panels.server.card.firstChild);
            }

            var resetBtn = form.querySelector('[data-af-action="reset-server"]');
            if (resetBtn) {
                panels.server.card.appendChild(buildRow({
                    key: 'reset-server',
                    label: 'Change server',
                    description: 'Forgets the saved address and returns to the connect '
                        + 'screen. Your library and watch history stay on the server.',
                    control: resetBtn
                }));
            }

            buildAboutPanel(panels.about.card);

            // Whatever the stock form has left — a .verticalSection emptied of
            // its controls is nothing but its heading.
            all(form, '.verticalSection').forEach(function (group) {
                if (!group.querySelector('[data-af-setting]')
                    && !group.querySelector('[data-af-action]')) {
                    group.hidden = true;
                    group.classList.add('af-set-emptied');
                }
            });

            // The banner becomes the rail's footnote; one line, not a wall.
            var notice = form.querySelector('[data-af-notice]');
            if (notice) {
                notice.textContent = 'Live settings apply at once. The ones marked RESTART '
                    + 'wait for the next launch.';
                notice.classList.add('af-set-note');
                rail.node.appendChild(notice);
            }

            form.appendChild(rail.node);
            form.appendChild(host);
            form.addEventListener('change', guard(onFormChange));
            page.appendChild(view.pill);

            selectPanel(DEFAULT_PANEL, false);
            updatePill();
            return view;
        }

        function onShow(e) {
            var page = e && e.detail ? e.detail.page : null;
            return decorate(page);
        }

        function onHide() {
            view = null;
            return true;
        }

        function install() {
            if (installed) { return false; }
            installed = true;
            doc.addEventListener('af-settings-show', guard(onShow));
            doc.addEventListener('af-settings-hide', guard(onHide));
            return true;
        }

        function state() {
            return { view: view, installed: installed };
        }

        var api = {
            panelFor: panelFor,
            modeLabel: modeLabel,
            shortModeLabel: shortModeLabel,
            modeDescription: modeDescription,
            chainIndexFor: chainIndexFor,
            chromiumVersion: chromiumVersion,
            connectionKind: connectionKind,
            serverAddress: serverAddress,
            aboutRows: aboutRows,
            resolveInfo: resolveInfo,
            selectPanel: selectPanel,
            updatePill: updatePill,
            decorate: decorate,
            install: install,
            state: state
        };
        window.__afSettings = api;

        install();

        if (typeof module !== 'undefined' && module.exports) {
            module.exports = api;
        }
    } catch (err) {
        if (window.console && console.warn) {
            console.warn('[Astrofin settings] initialisation failed', err);
        }
    }
}());
