/*
 * Astrofin theme runtime for jellyfin-web.
 *
 * Injected into the "web" browser after native-shim.js (see WEB_SCRIPTS in
 * src/jfn_cef/src/injection.rs). Runs at OnContextCreated — before
 * jellyfin-web's own bundles and before DOMContentLoaded — and again on every
 * navigation that creates a fresh V8 context, so everything here is
 * idempotent.
 *
 * Responsibilities:
 *   1. keep <style id="af-theme"> (installed by the Rust preamble) last in
 *      <head>, because jellyfin-web appends chunk stylesheets lazily;
 *   2. build and maintain #af-space — starfield, nebula and the two crossfaded
 *      backdrop art layers;
 *   3. track the focused/hovered card and drive the backdrop from it — on
 *      Home, where it also drives #af-spotlight, #af-server-panel and
 *      #af-hint, and on the library grids, where it drives the art alone;
 *   3b. gate the item detail pages: fetch the item once per route, stamp
 *      html[data-af-detail-type], drive the backdrop from it and synthesise
 *      the #af-detail-panel facts panel;
 *   4. pin <meta name="theme-color"> to --af-bg-base so the native chrome and
 *      the mpv letterbox match;
 *   5. gate every Astrofin layer off while video is playing.
 *
 * It must never throw, never call preventDefault, and never interfere with
 * jellyfin-web's own focus management. Selectors marked `jf-web 10.11.11` were
 * read off the running bundle; see docs/design/theme-injection.md.
 */
(function () {
    'use strict';

    // This file shares a single execute_java_script call with csd.js and
    // select-menu.js, which jellyfin-web's browser profile appends after it.
    // A throw at this level would abort those too, so the whole body is
    // wrapped: the theme may fail, the shims may not.
    try {

        if (window.__afTheme) {
            try { window.__afTheme.refresh(); } catch (e) { /* ignore */ }
            // Unit tests run this file under node: re-entry hands back the
            // installation that is already there, so a second load is visibly
            // a refresh and not a second install.
            if (typeof module !== 'undefined' && module.exports) {
                module.exports = window.__afTheme;
            }
            return;
        }

        var BG_BASE = '#070A14';
        var HOVER_DEBOUNCE_MS = 120;

        /* Types the spotlight treats as containers rather than something to play:
         * no NEW / remaining-time chips, no Play, a single Browse action. */
        var FOLDER_TYPES = {
            CollectionFolder: 1,
            UserView: 1,
            Folder: 1,
            BoxSet: 1,
            Season: 1,
            Playlist: 1
        };

        function isFolderLike(item) {
            return !!(item && FOLDER_TYPES[item.Type]);
        }

        var doc = document;
        // Resolved lazily: at OnContextCreated there is no <html> element yet.
        function root() { return doc.documentElement; }

        function log(err) {
            if (window.console && console.debug) {
                console.debug('[Astrofin theme]', err);
            }
        }

        function guard(fn) {
            return function () {
                try { return fn.apply(null, arguments); } catch (e) { log(e); return undefined; }
            };
        }

        /* Read a duration token so JS timing follows the sheet — including the
         * prefers-reduced-motion collapse in astrofin-tokens.css. */
        function tokenMs(name, fallback) {
            try {
                var el = root();
                var v = el && getComputedStyle(el).getPropertyValue(name).trim();
                if (v) {
                    if (v.slice(-2) === 'ms') { return parseFloat(v); }
                    if (v.slice(-1) === 's') { return parseFloat(v) * 1000; }
                }
            } catch (e) { /* ignore */ }
            return fallback;
        }

        /* ------------------------------------------------------------------ */
        /* 1. Stylesheet ordering                                              */
        /* ------------------------------------------------------------------ */

        // Cascade ordering. Two things load after us at equal specificity:
        //   - a <link> per lazily loaded webpack chunk, appended to <head>;
        //   - jf-web 10.11.11 puts themes/<name>/theme.css in a <div> inside
        //     <body>, which no position in <head> can ever beat.
        // So the sheet lives at the end of <body> and is moved back there whenever
        // another stylesheet node ends up after it. Only stylesheet nodes count,
        // which keeps this from ping-ponging with #af-spotlight and friends.
        function keepThemeLast() {
            var el = doc.getElementById('af-theme');
            if (!el) {
                if (typeof window.__afInstallTheme === 'function') {
                    el = window.__afInstallTheme();
                }
                if (!el) { return; }
            }
            var host = doc.body || doc.head;
            if (!host) { return; }
            if (el.parentNode !== host) {
                host.appendChild(el);
                return;
            }
            var sheets = doc.querySelectorAll('link[rel="stylesheet"], style');
            var last = sheets[sheets.length - 1];
            if (last && last !== el
                && (el.compareDocumentPosition(last) & Node.DOCUMENT_POSITION_FOLLOWING)) {
                host.appendChild(el);
            }
        }

        function watchStylesheets() {
            keepThemeLast();
            var opts = { childList: true };
            if (doc.head) {
                new MutationObserver(guard(keepThemeLast)).observe(doc.head, opts);
            }
            if (doc.body) {
                new MutationObserver(guard(keepThemeLast)).observe(doc.body, opts);
            }
        }

        /* ------------------------------------------------------------------ */
        /* 2. Native tint                                                      */
        /* ------------------------------------------------------------------ */

        // native-shim.js mirrors <meta name="theme-color"> to the native titlebar
        // and the mpv letterbox fill. jellyfin-web's themeManager rewrites it on
        // theme change, so hold it at the Astrofin base colour.
        function pinThemeColor() {
            var meta = doc.querySelector('meta[name="theme-color"]');
            if (!meta) {
                if (!doc.head) { return; }
                meta = doc.createElement('meta');
                meta.setAttribute('name', 'theme-color');
                meta.setAttribute('content', BG_BASE);
                doc.head.appendChild(meta);
            } else if (meta.getAttribute('content') !== BG_BASE) {
                meta.setAttribute('content', BG_BASE);
            }
            if (meta.__afPinned) { return; }
            meta.__afPinned = true;
            new MutationObserver(guard(function () {
                if (meta.getAttribute('content') !== BG_BASE) {
                    meta.setAttribute('content', BG_BASE);
                }
            })).observe(meta, { attributes: true, attributeFilter: ['content'] });
        }

        /* ------------------------------------------------------------------ */
        /* 3. #af-space                                                        */
        /* ------------------------------------------------------------------ */

        var space = null;
        var backdropLayers = [];
        var backdropSlot = 0;
        var currentBackdropUrl = null;

        function div(className) {
            var el = doc.createElement('div');
            if (className) { el.className = className; }
            return el;
        }

        function ensureSpace() {
            if (!doc.body) { return; }
            if (space && space.isConnected) {
                // .videoPlayerContainer is also inserted at body.firstChild; only
                // stacking order matters, so leave whichever is first alone.
                return;
            }
            space = doc.getElementById('af-space');
            if (!space) {
                space = div('');
                space.id = 'af-space';
                space.setAttribute('aria-hidden', 'true');

                var backdrop = div('');
                backdrop.id = 'af-backdrop';
                backdropLayers = [div('af-backdrop-layer'), div('af-backdrop-layer')];
                backdrop.appendChild(backdropLayers[0]);
                backdrop.appendChild(backdropLayers[1]);
                backdrop.appendChild(div('af-backdrop-scrim'));

                space.appendChild(backdrop);
                space.appendChild(div('af-stars af-stars-1'));
                space.appendChild(div('af-stars af-stars-2'));
                space.appendChild(div('af-nebula'));
            } else {
                backdropLayers = Array.prototype.slice.call(
                    space.querySelectorAll('.af-backdrop-layer')
                );
            }
            doc.body.insertBefore(space, doc.body.firstChild);
        }

        /* Video mode. Two independent gates, either of which hides every Astrofin
         * layer (the CSS keys off both):
         *   - html.transparentDocument, set by jellyfin-web's
         *     setBackdropTransparency(1|2) — jf-web 10.11.11;
         *   - html.af-video, set here while a .videoPlayerContainer exists
         *     (inserted by src/web/mpv-video-player.js at body.firstChild).
         */
        function updateVideoMode() {
            var playing = !!doc.querySelector('.videoPlayerContainer');
            root().classList.toggle('af-video', playing);
            if (playing) {
                clearBackdrop();
            }
        }

        function setBackdrop(url) {
            if (!backdropLayers.length || url === currentBackdropUrl) { return; }
            currentBackdropUrl = url;
            if (!url) { clearBackdrop(); return; }

            var img = new Image();
            img.onload = guard(function () {
                if (currentBackdropUrl !== url) { return; }
                var next = backdropLayers[backdropSlot];
                var prev = backdropLayers[1 - backdropSlot];
                next.style.backgroundImage = 'url("' + url.replace(/"/g, '%22') + '")';
                // Force a reflow so the reset transform animates from the scaled
                // start position on every swap, not only the first.
                next.classList.remove('af-on');
                void next.offsetWidth;
                next.classList.add('af-on');
                prev.classList.remove('af-on');
                backdropSlot = 1 - backdropSlot;
                root().classList.add('af-backdrop');
            });
            img.onerror = guard(function () {
                if (currentBackdropUrl === url) { clearBackdrop(); }
            });
            img.src = url;
        }

        function clearBackdrop() {
            currentBackdropUrl = null;
            for (var i = 0; i < backdropLayers.length; i++) {
                backdropLayers[i].classList.remove('af-on');
            }
            root().classList.remove('af-backdrop');
        }

        /* ------------------------------------------------------------------ */
        /* 4. Route detection                                                  */
        /* ------------------------------------------------------------------ */

        // The hash is authoritative whenever there is one. jf-web 10.11.11 leaves
        // the previous view (and therefore #homeTab) in the DOM after a route
        // change, so a DOM probe alone stays true forever once Home has rendered.
        function isHomeRoute() {
            var hash = String(location.hash || '').replace(/^#!?/, '');
            if (hash) { return /^\/home(\.html)?([?/]|$)/.test(hash); }
            return !!doc.querySelector('#homeTab, .homePage');
        }

        // The library grids: #moviesPage, #tvPage, #musicPage and the generic
        // list view, all of which render .libraryPage > .itemsContainer.
        // Hash-first for the same reason as isHomeRoute, and the DOM fallback
        // excludes .homePage because jf-web 10.11.11 puts *both* classes on
        // Home - a bare .libraryPage probe would call Home a library.
        function isLibraryRoute() {
            var hash = String(location.hash || '').replace(/^#!?/, '');
            if (hash) { return /^\/(movies|tv|music|list)(\.html)?([?/]|$)/.test(hash); }
            return !!doc.querySelector('.libraryPage:not(.homePage)');
        }

        // The item detail page: #/details?id=<guid>&serverId=<guid>[&context=…].
        // Hash-only, with no DOM fallback: jf-web 10.11.11 keeps the outgoing
        // view in the DOM, duplicate id and all, so a `.itemDetailPage` probe
        // stays true forever once one has rendered. Everything downstream reads
        // `.itemDetailPage:not(.hide)` rather than #itemDetailPage for the same
        // reason.
        function isDetailRoute() {
            var hash = String(location.hash || '').replace(/^#!?/, '');
            return /^\/details([?/]|$)/.test(hash);
        }

        // The item the route is about. A season page is a details route of its
        // own, so this changing is what makes the gate re-fetch.
        function detailIdFromHash() {
            var m = /[?&]id=([^&]*)/.exec(String(location.hash || ''));
            if (!m || !m[1]) { return null; }
            try { return decodeURIComponent(m[1]); } catch (e) { return m[1]; }
        }

        /* ------------------------------------------------------------------ */
        /* 5. Spotlight, server panel, controller hint                         */
        /* ------------------------------------------------------------------ */

        var ui = null;

        function buildUi() {
            if (ui || !doc.body) { return; }
            // Idempotent across a re-run in the same document: drop any panels a
            // previous execution left behind rather than shadowing them.
            ['af-spotlight', 'af-server-panel', 'af-hint'].forEach(function (id) {
                var old = doc.getElementById(id);
                if (old && old.parentNode) { old.parentNode.removeChild(old); }
            });

            var spotlight = doc.createElement('section');
            spotlight.id = 'af-spotlight';
            spotlight.hidden = true;
            spotlight.tabIndex = -1;

            // Everything that changes with the item lives in one wrapper so an
            // item swap is a single opacity transition.
            var body = div('af-sp-body');

            var title = doc.createElement('h2');
            title.className = 'af-sp-title';
            var chips = div('af-sp-chips');
            var overview = doc.createElement('p');
            overview.className = 'af-sp-overview';
            var actions = div('af-sp-actions');

            // jf-web 10.11.11's focusManager builds its focusable set from
            //   INPUT/TEXTAREA/SELECT/BUTTON/A + ':not([tabindex="-1"]):not(:disabled)'
            //   plus '.focusable'
            // and autoFocus() additionally skips anything classed 'noautofocus'.
            // tabindex -1 is therefore what keeps the panel out of controller and
            // keyboard spatial navigation between the rails; noautofocus stops it
            // being chosen as a view's initial focus. Both buttons stay fully
            // usable with the mouse.
            function panelButton(className, label) {
                var b = doc.createElement('button');
                b.type = 'button';
                b.className = className + ' noautofocus';
                b.tabIndex = -1;
                if (label) { b.textContent = label; }
                return b;
            }

            var play = panelButton('af-btn af-btn-primary', null);
            var glyph = doc.createElement('span');
            glyph.className = 'af-btn-glyph';
            var playLabel = doc.createElement('span');
            playLabel.textContent = 'Play';
            play.appendChild(glyph);
            play.appendChild(playLabel);

            var details = panelButton('af-btn af-btn-secondary', 'Details');

            actions.appendChild(play);
            actions.appendChild(details);
            body.appendChild(title);
            body.appendChild(chips);
            body.appendChild(overview);
            body.appendChild(actions);
            spotlight.appendChild(body);

            var server = doc.createElement('aside');
            server.id = 'af-server-panel';
            server.hidden = true;
            server.setAttribute('aria-hidden', 'true');

            var hint = div('');
            hint.id = 'af-hint';
            hint.hidden = true;
            hint.setAttribute('aria-hidden', 'true');
            ['◀ ▶ MOVE', '✕ SELECT', '◯ BACK', 'OPTIONS ⋯'].forEach(function (t) {
                var s = doc.createElement('span');
                s.textContent = t;
                hint.appendChild(s);
            });

            // The spotlight is placed in the home sections container by
            // placeSpotlight(); body is only its parking spot until then.
            doc.body.appendChild(spotlight);
            doc.body.appendChild(server);
            doc.body.appendChild(hint);

            ui = {
                spotlight: spotlight,
                body: body,
                title: title,
                chips: chips,
                overview: overview,
                glyph: glyph,
                playLabel: playLabel,
                play: play,
                details: details,
                server: server,
                hint: hint
            };
            spotlightPainted = false;

            play.addEventListener('click', guard(onPlayClick));
            details.addEventListener('click', guard(onDetailsClick));
        }

        // Video-mode wire value -> panel label. The pre-rename spellings are
        // still accepted because a settings.json normalised at startup only
        // reaches jmpInfo on the next launch.
        var VIDEO_MODE_LABELS = {
            'auto': 'Auto',
            'live-action': 'Live-Action',
            'movies': 'Live-Action',
            'animation': 'Animation',
            'anime': 'Animation',
            'off': 'Off'
        };

        function videoModeLabel(value) {
            var key = String(value || '').toLowerCase();
            return VIDEO_MODE_LABELS[key] || key;
        }

        function renderServerPanel() {
            if (!ui) { return; }
            var api = window.ApiClient;
            var name = '';
            try {
                name = (api && api.serverName && api.serverName()) || '';
            } catch (e) { /* ignore */ }
            if (!name) {
                try {
                    var info = api && api.serverInfo && api.serverInfo();
                    name = (info && (info.Name || info.name)) || '';
                } catch (e2) { /* ignore */ }
            }
            if (!name) { ui.server.hidden = true; return; }

            var frag = doc.createDocumentFragment();
            var eyebrow = div('af-srv-eyebrow');
            eyebrow.textContent = 'Server';
            var nameEl = div('af-srv-name');
            nameEl.textContent = name;
            frag.appendChild(eyebrow);
            frag.appendChild(nameEl);

            // Rows only for values native-shim.js actually exposes; anything we
            // cannot source honestly is omitted rather than invented.
            var rows = [];
            var jmp = window.jmpInfo;
            if (jmp && jmp.settings) {
                if (jmp.settings.playback && jmp.settings.playback.videoMode) {
                    rows.push(['Mode', videoModeLabel(jmp.settings.playback.videoMode)]);
                }
                if (jmp.settings.playback && jmp.settings.playback.hwdec) {
                    rows.push(['Decode', String(jmp.settings.playback.hwdec)]);
                }
            }
            if (rows.length) {
                frag.appendChild(div('af-srv-rule'));
                rows.forEach(function (r) {
                    var row = div('af-srv-row');
                    var k = doc.createElement('span');
                    k.textContent = r[0];
                    var v = doc.createElement('span');
                    v.textContent = r[1];
                    row.appendChild(k);
                    row.appendChild(v);
                    frag.appendChild(row);
                });
            }

            ui.server.textContent = '';
            ui.server.appendChild(frag);
        }

        /* ------------------------------------------------------------------ */
        /* 6. Item metadata                                                    */
        /* ------------------------------------------------------------------ */

        var TICKS_PER_MINUTE = 600000000;
        var itemCache = Object.create(null);
        var itemCacheKeys = [];
        var ITEM_CACHE_MAX = 64;
        var requestToken = 0;

        function cacheItem(id, item) {
            if (!itemCache[id]) {
                itemCacheKeys.push(id);
                while (itemCacheKeys.length > ITEM_CACHE_MAX) {
                    delete itemCache[itemCacheKeys.shift()];
                }
            }
            itemCache[id] = item;
        }

        function minutes(ticks) {
            return Math.round((Number(ticks) || 0) / TICKS_PER_MINUTE);
        }

        function formatRuntime(ticks) {
            var m = minutes(ticks);
            if (!m) { return null; }
            var h = Math.floor(m / 60);
            return h ? h + 'h ' + (m % 60) + 'm' : m + 'm';
        }

        function videoStream(item) {
            var streams = item && item.MediaStreams;
            if (!streams || !streams.length) { return null; }
            for (var i = 0; i < streams.length; i++) {
                if (streams[i].Type === 'Video') { return streams[i]; }
            }
            return null;
        }

        function resolutionLabel(stream) {
            var w = stream && Number(stream.Width);
            if (!w) { return null; }
            if (w >= 3600) { return '4K'; }
            if (w >= 1800) { return '1080p'; }
            if (w >= 1200) { return '720p'; }
            return 'SD';
        }

        function chipsFor(item) {
            var out = [];

            // Containers get a child count and nothing else: a library has no
            // year, runtime, rating, resume position or release date worth
            // showing, and "New" on a library folder is meaningless.
            if (isFolderLike(item)) {
                if (item.ChildCount) {
                    out.push(item.ChildCount + (item.ChildCount === 1 ? ' item' : ' items'));
                }
                return out;
            }

            if (item.Type === 'Episode' && item.Name) { out.push(item.Name); }
            if (item.ParentIndexNumber != null && item.IndexNumber != null) {
                out.push('S' + item.ParentIndexNumber + ' · E' + item.IndexNumber);
            }
            if (item.ProductionYear) { out.push(String(item.ProductionYear)); }

            var rt = formatRuntime(item.RunTimeTicks);
            if (rt) { out.push(rt); }

            var vs = videoStream(item);
            var res = resolutionLabel(vs);
            if (res) { out.push(res); }
            var range = vs && (vs.VideoRangeType || vs.VideoRange);
            if (range && range !== 'SDR') { out.push(String(range).replace(/_/g, ' ')); }

            if (item.CommunityRating) {
                out.push('★ ' + Number(item.CommunityRating).toFixed(1));
            }

            var ud = item.UserData || {};
            if (ud.PlaybackPositionTicks > 0 && item.RunTimeTicks > 0) {
                var left = minutes(item.RunTimeTicks - ud.PlaybackPositionTicks);
                if (left > 0) { out.push('left ' + left + ' m'); }
            } else if (item.DateCreated) {
                var age = Date.now() - Date.parse(item.DateCreated);
                if (age >= 0 && age < 14 * 864e5) { out.push('New'); }
            }
            return out;
        }

        function titleFor(item) {
            if (item.Type === 'Episode' && item.SeriesName) { return item.SeriesName; }
            return item.Name || '';
        }

        /* Which branch of backdropUrlFor()'s fallback chain an item lands on.
         * Pure, and kept beside it so the two cannot drift. Only the detail
         * gate reads it: `primary` means the art is a 2:3 poster cropped to a
         * 16:9 canvas, which section (o) blurs back towards a wash. */
        function backdropSourceFor(item) {
            if (!item) { return null; }
            if (item.BackdropImageTags && item.BackdropImageTags.length) {
                return 'backdrop';
            }
            if (item.ParentBackdropItemId
                && item.ParentBackdropImageTags
                && item.ParentBackdropImageTags.length) {
                return 'parent';
            }
            if (item.ImageTags && item.ImageTags.Primary) { return 'primary'; }
            return null;
        }

        function backdropUrlFor(item) {
            var api = window.ApiClient;
            if (!api || !api.getScaledImageUrl) { return null; }
            var maxWidth = Math.min(
                1920,
                Math.round((window.innerWidth || 1280) * (window.devicePixelRatio || 1))
            );
            var opts;
            if (item.BackdropImageTags && item.BackdropImageTags.length) {
                opts = { type: 'Backdrop', maxWidth: maxWidth, tag: item.BackdropImageTags[0] };
                return api.getScaledImageUrl(item.Id, opts);
            }
            if (item.ParentBackdropItemId
                && item.ParentBackdropImageTags
                && item.ParentBackdropImageTags.length) {
                opts = { type: 'Backdrop', maxWidth: maxWidth, tag: item.ParentBackdropImageTags[0] };
                return api.getScaledImageUrl(item.ParentBackdropItemId, opts);
            }
            if (item.ImageTags && item.ImageTags.Primary) {
                opts = { type: 'Primary', maxWidth: maxWidth, tag: item.ImageTags.Primary };
                return api.getScaledImageUrl(item.Id, opts);
            }
            return null;
        }

        function fetchItem(id) {
            if (itemCache[id]) { return Promise.resolve(itemCache[id]); }
            var api = window.ApiClient;
            if (!api || !api.getItem || !api.getCurrentUserId) {
                return Promise.reject(new Error('no ApiClient'));
            }
            return Promise.resolve(api.getItem(api.getCurrentUserId(), id)).then(function (item) {
                if (item) { cacheItem(id, item); }
                return item;
            });
        }

        /* ------------------------------------------------------------------ */
        /* 7. Focus tracking                                                   */
        /* ------------------------------------------------------------------ */

        var focusedCard = null;
        var focusedItem = null;
        /* The card the panel is currently *showing*. setFocusedCard runs
         * synchronously on hover/focus but the panel only repaints once
         * fetchItem() resolves, so the buttons must act on this, never on
         * focusedCard, or a slow server lets Play start the wrong item. */
        var shownCard = null;
        var shownItem = null;

        var spotlightPainted = false;
        var swapTimer = 0;
        var pendingItem = null;
        var pendingCard = null;

        function setFocusedCard(card) {
            if (card === focusedCard) { return; }
            if (focusedCard) { focusedCard.classList.remove('af-focused'); }
            focusedCard = card;
            if (!card) { return; }
            card.classList.add('af-focused');
            /* A library grid drives the same art bleed as Home, but not the
             * spotlight: placeSpotlight() inserts the panel into #homeTab's
             * section stream, and a library page has no such stream. Resolve
             * the route once, synchronously - the fetch below can outlive a
             * navigation, and the Home path must behave exactly as it did when
             * this was a bare `if (!isHomeRoute()) return`. */
            var home = isHomeRoute();
            if (!home && !isLibraryRoute()) { return; }

            var id = card.getAttribute('data-id');
            if (!id) { return; }
            var token = ++requestToken;
            fetchItem(id).then(guard(function (item) {
                if (token !== requestToken || !item) { return; }
                focusedItem = item;
                if (home) { renderSpotlight(item, card); }
                setBackdrop(backdropUrlFor(item));
            })).catch(function (e) { log(e); });
        }

        /* ------------------------------------------------------------------ */
        /* 7b. In-flow placement                                               */
        /*                                                                     */
        /* jellyfin-web stacks several rails where the design has one, so a    */
        /* viewport-fixed panel always covers a rail. The spotlight is a normal */
        /* block inserted after the section holding the focused card instead;   */
        /* only the backdrop stays fixed and full-bleed.                        */
        /* ------------------------------------------------------------------ */

        function homeSectionsContainer() {
            return doc.querySelector('#homeTab .homeSectionsContainer')
                || doc.querySelector('#homeTab');
        }

        function sectionOf(card) {
            return card && card.closest ? card.closest('#homeTab .verticalSection') : null;
        }

        /* jf-web 10.11.11 renders every home section up front and hides the empty
         * ones with .hide, so pick the first that is actually showing cards. */
        function defaultSection() {
            var container = homeSectionsContainer();
            if (!container) { return null; }
            var kids = container.children;
            var fallback = null;
            for (var i = 0; i < kids.length; i++) {
                var el = kids[i];
                if (!el.classList || !el.classList.contains('verticalSection')) { continue; }
                if (!fallback) { fallback = el; }
                if (!el.classList.contains('hide') && el.querySelector('.card')) { return el; }
            }
            return fallback;
        }

        // Moving the panel reflows the rails under a stationary cursor, which
        // fires a fresh mouseover on whatever slid beneath it. Accepting that
        // would relocate the panel again and cascade down the page, so hover is
        // ignored until the pointer genuinely moves.
        var pointerMovedSincePlace = true;

        function placeSpotlight(section) {
            if (!ui) { return; }
            var target = (section && section.parentNode) ? section : defaultSection();
            if (!target || !target.parentNode) {
                if (!ui.spotlight.isConnected && doc.body) {
                    doc.body.appendChild(ui.spotlight);
                }
                return;
            }
            if (ui.spotlight.previousElementSibling === target) { return; }
            target.parentNode.insertBefore(ui.spotlight, target.nextSibling);
            pointerMovedSincePlace = false;
        }

        /* ------------------------------------------------------------------ */
        /* 7c. Rendering                                                       */
        /* ------------------------------------------------------------------ */

        function writeSpotlight(item, card) {
            if (!ui) { return; }
            shownItem = item;
            shownCard = card;
            var folder = isFolderLike(item);

            ui.title.textContent = titleFor(item);

            ui.chips.textContent = '';
            chipsFor(item).forEach(function (label) {
                var chip = div('af-chip');
                chip.textContent = label;
                ui.chips.appendChild(chip);
            });

            var overview = folder ? '' : (item.Overview || '');
            ui.overview.textContent = overview;
            ui.overview.hidden = !overview;

            if (folder) {
                // A library, box set or season is browsed, not played.
                ui.playLabel.textContent = 'Browse';
                ui.glyph.hidden = true;
                ui.details.hidden = true;
            } else {
                var resumable = item.UserData && item.UserData.PlaybackPositionTicks > 0;
                ui.playLabel.textContent = resumable ? 'Resume' : 'Play';
                ui.glyph.hidden = false;
                ui.details.hidden = false;
            }

            // Whatever queued this write, the content on screen is now current,
            // so the fade must come off here rather than only in the timer that
            // may since have been cleared.
            ui.body.classList.remove('af-sp-swap');
        }

        function renderSpotlight(item, card) {
            ensureUi();
            if (!ui) { return; }

            placeSpotlight(sectionOf(card));

            pendingItem = item;
            pendingCard = card;
            if (!spotlightPainted) {
                writeSpotlight(pendingItem, pendingCard);
                spotlightPainted = true;
            } else if (!swapTimer) {
                // Opacity-only swap; a later hover during the fade just replaces
                // what the in-flight timer will write.
                ui.body.classList.add('af-sp-swap');
                swapTimer = setTimeout(guard(function () {
                    swapTimer = 0;
                    if (pendingItem) { writeSpotlight(pendingItem, pendingCard); }
                    if (ui) { ui.body.classList.remove('af-sp-swap'); }
                }), tokenMs('--af-dur-tile', 180));
            }

            showOverlays(true);
        }

        /* jf-web 10.11.11: the tile's own primary action button. Clicking it reuses
         * jellyfin-web's playback path (resume offsets, media source selection)
         * instead of reimplementing it. */
        function primaryCardButton(card) {
            return card.querySelector(
                '.cardOverlayButton[data-action="resume"],'
                + '.cardOverlayButton[data-action="play"],'
                + '.cardOverlayButton.cardOverlayFab-primary'
            );
        }

        /* jf-web 10.11.11 routes card navigation through a delegated click handler
         * that resolves the action from the closest .itemAction[data-action] and
         * the item from the nearest [data-id] ancestor - the card itself. */
        function cardLinkTarget(card) {
            return card.querySelector(
                '.cardImageContainer[data-action="link"],'
                + '.cardOverlayContainer[data-action="link"],'
                + '.itemAction[data-action="link"]'
            );
        }

        /* Fallback for rails whose cards carry no primary overlay button: borrow
         * the same delegation contract with a throwaway .itemAction, so the item
         * identity still comes off the card and jellyfin-web owns the playback
         * decision. Removed synchronously - the delegated handler runs during
         * dispatch. */
        function clickSyntheticAction(card, action) {
            var probe = doc.createElement('button');
            probe.type = 'button';
            probe.className = 'itemAction noautofocus';
            probe.tabIndex = -1;
            probe.setAttribute('data-action', action);
            probe.style.cssText =
                'position:absolute;width:0;height:0;opacity:0;pointer-events:none';
            card.appendChild(probe);
            try {
                probe.click();
            } finally {
                if (probe.parentNode) { probe.parentNode.removeChild(probe); }
            }
        }

        function navigateToDetails(card, item) {
            var id = (item && item.Id) || (card && card.getAttribute('data-id'));
            if (!id) { return; }
            var serverId = (card && card.getAttribute('data-serverid'))
                || (item && item.ServerId)
                || '';
            if (!serverId) {
                try {
                    var api = window.ApiClient;
                    serverId = (api && api.serverId && api.serverId()) || '';
                } catch (e) { /* ignore */ }
            }
            location.hash = '#/details?id=' + encodeURIComponent(id)
                + (serverId ? '&serverId=' + encodeURIComponent(serverId) : '');
        }

        function onPlayClick() {
            var card = shownCard;
            var item = shownItem;
            if (!card || !card.isConnected) { return; }

            if (isFolderLike(item)) {
                var link = cardLinkTarget(card);
                if (link) { link.click(); }
                return;
            }

            var btn = primaryCardButton(card);
            if (btn) { btn.click(); return; }

            var resumable = item && item.UserData && item.UserData.PlaybackPositionTicks > 0;
            clickSyntheticAction(card, resumable ? 'resume' : 'play');
        }

        function onDetailsClick() {
            navigateToDetails(shownCard, shownItem);
        }

        function cardFrom(node) {
            if (!node || !node.closest) { return null; }
            var card = node.closest('.card[data-id]');
            if (!card) { return null; }
            // Home rails and library grids only; every other page keeps plain
            // jellyfin-web behaviour.
            return card.closest('#homeTab, .homePage, .libraryPage .itemsContainer')
                ? card
                : null;
        }

        var hoverTimer = 0;

        function onFocusIn(e) {
            var card = cardFrom(e.target);
            if (card) {
                if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = 0; }
                setFocusedCard(card);
            }
        }

        function onPointerMove() {
            pointerMovedSincePlace = true;
        }

        function onPointerOver(e) {
            if (!pointerMovedSincePlace) { return; }
            var card = cardFrom(e.target);
            if (!card || card === focusedCard) { return; }
            if (hoverTimer) { clearTimeout(hoverTimer); }
            hoverTimer = setTimeout(guard(function () {
                hoverTimer = 0;
                setFocusedCard(card);
            }), HOVER_DEBOUNCE_MS);
        }

        /* ------------------------------------------------------------------ */
        /* 8. Item detail                                                      */
        /* ------------------------------------------------------------------ */

        /* jf-web 10.11.11 renders movie, series, season and episode pages from
         * one template and marks none of them — nothing in the DOM says which
         * kind of item is on screen. The item JSON does, so the gate fetches it
         * once per route and writes the answer to html[data-af-detail-type],
         * which is what the eyebrow in section (o) of the sheet keys off.
         * `attr()` could not do this: it resolves against the pseudo-element's
         * own originating element, never against <html>. */
        var DETAIL_TYPES = {
            Movie: 'movie',
            Series: 'series',
            Season: 'season',
            Episode: 'episode'
        };

        function detailTypeFor(item) {
            return (item && DETAIL_TYPES[item.Type]) || 'other';
        }

        /* The panel prefers MediaSources[0], which is what the page's own
         * version picker is showing; item.MediaStreams is the fallback for the
         * trimmed shapes /Items and /NextUp hand back. */
        function mediaStreams(item) {
            var src = item && item.MediaSources && item.MediaSources[0];
            if (src && src.MediaStreams && src.MediaStreams.length) {
                return src.MediaStreams;
            }
            return (item && item.MediaStreams) || [];
        }

        function streamOfType(streams, type) {
            for (var i = 0; i < streams.length; i++) {
                if (streams[i] && streams[i].Type === type) { return streams[i]; }
            }
            return null;
        }

        function formatBytes(value) {
            var n = Number(value);
            if (!n || n <= 0) { return null; }
            var units = ['B', 'KB', 'MB', 'GB', 'TB'];
            var i = 0;
            while (n >= 1024 && i < units.length - 1) { n /= 1024; i += 1; }
            return (i >= 2 ? n.toFixed(1) : String(Math.round(n))) + ' ' + units[i];
        }

        function videoFact(streams) {
            var s = streamOfType(streams, 'Video');
            if (!s) { return null; }
            var head = [
                s.Codec ? String(s.Codec).toUpperCase() : null,
                resolutionLabel(s)
            ].filter(Boolean).join(' ');
            var range = s.VideoRangeType || s.VideoRange;
            range = range && String(range).toUpperCase() !== 'SDR'
                ? String(range).replace(/_/g, ' ')
                : null;
            return [head, range].filter(Boolean).join(' · ') || null;
        }

        function audioFact(streams) {
            var s = streamOfType(streams, 'Audio');
            if (!s) { return null; }
            return [
                s.Codec ? String(s.Codec).toUpperCase() : null,
                s.ChannelLayout || (s.Channels ? s.Channels + 'ch' : null)
            ].filter(Boolean).join(' ') || null;
        }

        /* Language codes only. A subtitle track's DisplayTitle is a sentence
         * ("English - Forced - SRT") and would blow the 300px panel apart. */
        function subtitleFact(streams) {
            var seen = Object.create(null);
            var codes = [];
            for (var i = 0; i < streams.length; i++) {
                if (!streams[i] || streams[i].Type !== 'Subtitle') { continue; }
                if (!streams[i].Language) { continue; }
                var code = String(streams[i].Language).toUpperCase();
                if (seen[code]) { continue; }
                seen[code] = 1;
                codes.push(code);
            }
            if (!codes.length) { return null; }
            if (codes.length <= 3) { return codes.join(' · '); }
            return codes.slice(0, 3).join(' · ') + ' +' + (codes.length - 3);
        }

        function resumeLabel(item) {
            var ud = item && item.UserData;
            var pos = Number(ud && ud.PlaybackPositionTicks) || 0;
            var total = Number(item && item.RunTimeTicks) || 0;
            if (pos <= 0 || total <= 0 || pos >= total) { return null; }
            var left = formatRuntime(total - pos);
            return left ? left + ' left' : null;
        }

        function nextUpLine(ep) {
            if (!ep) { return null; }
            var bits = [];
            if (ep.ParentIndexNumber != null && ep.IndexNumber != null) {
                bits.push('S' + ep.ParentIndexNumber + ' E' + ep.IndexNumber);
            }
            if (ep.Name) { bits.push(ep.Name); }
            return bits.join(' · ') || null;
        }

        function airsLine(item) {
            var days = item.AirDays && item.AirDays.length ? item.AirDays.join(', ') : null;
            return [days, item.AirTime || null].filter(Boolean).join(' ') || null;
        }

        /* Pure: item JSON (plus whatever the two async extras have resolved to)
         * in, panel content out. Rows with no value are never emitted, so a
         * sparse item yields a short panel rather than a wall of dashes, and
         * an item with nothing to say yields none at all. */
        function detailFacts(item, opts) {
            var o = opts || {};
            var rows = [];
            var facts = { eyebrow: '', headline: null, sub: null, rows: rows };
            if (!item) { return facts; }
            var mode = o.videoMode ? videoModeLabel(o.videoMode) : null;

            function push(label, value, tone) {
                if (value === null || value === undefined || value === '') { return; }
                rows.push({ label: label, value: String(value), tone: tone || null });
            }

            var type = detailTypeFor(item);

            if (type === 'series') {
                facts.eyebrow = o.nextUp ? 'Next up' : 'Series';
                if (o.nextUp) {
                    facts.headline = nextUpLine(o.nextUp);
                    facts.sub = resumeLabel(o.nextUp);
                }
                push('Network', item.Studios && item.Studios[0] && item.Studios[0].Name);
                push('Status', item.Status);
                push('Airs', airsLine(item));
                push('Mode', mode, 'accent');
                return facts;
            }

            if (type === 'season') {
                facts.eyebrow = 'Season';
                var count = item.ChildCount != null ? item.ChildCount : item.RecursiveItemCount;
                var unplayed = (item.UserData || {}).UnplayedItemCount;
                if (count != null) { push('Episodes', String(count)); }
                if (count != null && unplayed != null) {
                    push('Watched', (count - unplayed) + ' of ' + count);
                }
                push('Mode', mode, 'accent');
                return facts;
            }

            facts.eyebrow = 'File';
            var streams = mediaStreams(item);
            push('Video', videoFact(streams));
            push('Audio', audioFact(streams));
            push('Subtitles', subtitleFact(streams));
            push('Mode', mode, 'accent');
            push('Size', formatBytes((item.MediaSources && item.MediaSources[0] || {}).Size));
            return facts;
        }

        var detailPanel = null;
        var detailId = null;
        var detailItem = null;
        var detailNextUp = null;

        function detailPage() {
            return doc.querySelector('.itemDetailPage:not(.hide)');
        }

        function detailPanelHost() {
            var page = detailPage();
            return page ? page.querySelector('.detailPageSecondaryContainer') : null;
        }

        function removeDetailPanel() {
            var el = detailPanel || doc.getElementById('af-detail-panel');
            if (el && el.parentNode) { el.parentNode.removeChild(el); }
            detailPanel = null;
        }

        /* First child of the right column, so the glass sits where the artboard
         * puts it (top right, 300px) and the cast/similar shelves stack under
         * it. Returns null while the page is still being built; the next
         * refresh retries, which is what makes it safe to call early. */
        function renderDetailPanel(item) {
            var host = detailPanelHost();
            if (!host) { return null; }
            var jmp = window.jmpInfo;
            var playback = jmp && jmp.settings && jmp.settings.playback;
            var facts = detailFacts(item, {
                nextUp: detailNextUp,
                videoMode: playback ? playback.videoMode : null
            });

            var panel = detailPanel || doc.getElementById('af-detail-panel');
            if (!panel) {
                panel = doc.createElement('aside');
                panel.id = 'af-detail-panel';
                panel.setAttribute('aria-hidden', 'true');
            }
            detailPanel = panel;
            if (panel.parentNode !== host || host.children[0] !== panel) {
                host.insertBefore(panel, host.firstChild);
            }

            panel.textContent = '';
            if (!facts.rows.length && !facts.headline) {
                panel.hidden = true;
                return panel;
            }
            panel.hidden = false;

            var frag = doc.createDocumentFragment();
            if (facts.eyebrow) {
                var eyebrow = div('af-dp-eyebrow');
                eyebrow.textContent = facts.eyebrow;
                frag.appendChild(eyebrow);
            }
            if (facts.headline) {
                var head = div('af-dp-headline');
                head.textContent = facts.headline;
                frag.appendChild(head);
                if (facts.sub) {
                    var sub = div('af-dp-sub');
                    sub.textContent = facts.sub;
                    frag.appendChild(sub);
                }
                if (facts.rows.length) { frag.appendChild(div('af-dp-rule')); }
            }
            facts.rows.forEach(function (r) {
                var row = div('af-dp-row');
                var k = doc.createElement('span');
                k.textContent = r.label;
                var v = doc.createElement('span');
                if (r.tone) { v.className = 'af-dp-' + r.tone; }
                v.textContent = r.value;
                row.appendChild(k);
                row.appendChild(v);
                frag.appendChild(row);
            });
            panel.appendChild(frag);
            return panel;
        }

        /* The remaining time on the Resume pill. An attribute, not a child:
         * .detailButton has no text element in 10.11.11 (the label is the
         * `title`), the sheet already draws that with ::before, and attribute
         * writes are invisible to the childList observer that drives refresh()
         * — appending a <span> here would feed the loop. */
        function markResumeButton(item) {
            var page = detailPage();
            var btn = page && page.querySelector('.mainDetailButtons .btnPlay');
            if (!btn) { return; }
            var label = btn.getAttribute('data-action') === 'resume'
                ? resumeLabel(item)
                : null;
            if (label) {
                if (btn.getAttribute('data-af-left') !== label) {
                    btn.setAttribute('data-af-left', label);
                }
            } else if (btn.getAttribute('data-af-left') != null) {
                btn.removeAttribute('data-af-left');
            }
        }

        function fetchNextUp(item) {
            if (!item || item.Type !== 'Series') { return; }
            var api = window.ApiClient;
            if (!api || !api.getNextUpEpisodes || !api.getCurrentUserId) { return; }
            var id = item.Id;
            Promise.resolve(api.getNextUpEpisodes({
                SeriesId: id,
                UserId: api.getCurrentUserId(),
                Limit: 1
            })).then(guard(function (result) {
                var next = result && result.Items && result.Items[0];
                if (!next || detailId !== id) { return; }
                detailNextUp = next;
                renderDetailPanel(item);
            })).catch(function (e) { log(e); });
        }

        function syncDetail() {
            var id = detailIdFromHash();
            if (!id) { return; }
            if (id === detailId) {
                // Same item, another refresh: jf-web rebuilds the page on some
                // navigations, so re-place the panel only when it is gone.
                if (detailItem) {
                    if (!detailPanel || !detailPanel.isConnected) {
                        renderDetailPanel(detailItem);
                    }
                    markResumeButton(detailItem);
                }
                return;
            }
            detailId = id;
            detailItem = null;
            detailNextUp = null;
            root().removeAttribute('data-af-detail-type');
            root().removeAttribute('data-af-backdrop-src');
            removeDetailPanel();
            /* The card that got us here belongs to a page on its way out; drop
             * it so a later return to Home cannot paint a stale item into the
             * spotlight. The art it is driving stays up until this item's own
             * backdrop replaces it, which is why the class goes but the
             * backdrop does not. */
            if (focusedCard) { focusedCard.classList.remove('af-focused'); }
            focusedCard = null;
            focusedItem = null;

            fetchItem(id).then(guard(function (item) {
                if (!item || detailId !== id) { return; }
                detailItem = item;
                root().setAttribute('data-af-detail-type', detailTypeFor(item));
                var src = backdropSourceFor(item);
                if (src) {
                    root().setAttribute('data-af-backdrop-src', src);
                } else {
                    root().removeAttribute('data-af-backdrop-src');
                }
                setBackdrop(backdropUrlFor(item));
                renderDetailPanel(item);
                markResumeButton(item);
                fetchNextUp(item);
            })).catch(function (e) { log(e); });
        }

        function leaveDetail() {
            detailId = null;
            detailItem = null;
            detailNextUp = null;
            root().removeAttribute('data-af-detail-type');
            root().removeAttribute('data-af-backdrop-src');
            removeDetailPanel();
            clearBackdrop();
        }

        /* ------------------------------------------------------------------ */
        /* 9. Visibility                                                       */
        /* ------------------------------------------------------------------ */

        var overlaysWanted = false;

        /* jellyfin-web rebuilds #homeTab on some navigations, which orphans the
         * panels. Rebuild rather than keep writing into detached nodes. */
        function ensureUi() {
            if (ui && (!ui.spotlight.isConnected
                || !ui.server.isConnected
                || !ui.hint.isConnected)) {
                ui = null;
            }
            buildUi();
        }

        // The spotlight is in flow now, so it hides only when it has nothing to
        // say or the route left Home - no scroll rule needed.
        function showOverlays(wanted) {
            if (wanted !== undefined) { overlaysWanted = wanted; }
            if (!ui) { return; }
            var visible = overlaysWanted && isHomeRoute();
            [ui.spotlight, ui.server, ui.hint].forEach(function (el) {
                if (visible) {
                    el.hidden = false;
                    // Flush layout so the opacity transition runs from 0. A rAF
                    // would be throttled to nothing in a background tab.
                    void el.offsetWidth;
                    el.classList.add('af-show');
                } else {
                    el.classList.remove('af-show');
                    el.hidden = true;
                }
            });
        }

        /* Tear the Home panels down. `keepSelection` is set only when the route
         * moved to a library grid, which keeps its own focused card and the art
         * backdrop that card is driving; everything the spotlight is made of
         * goes either way. With it falsy this is the original teardown. */
        function leaveHome(keepSelection) {
            overlaysWanted = false;
            if (!keepSelection) { focusedItem = null; }
            shownItem = null;
            shownCard = null;
            pendingItem = null;
            pendingCard = null;
            if (swapTimer) { clearTimeout(swapTimer); swapTimer = 0; }
            spotlightPainted = false;
            if (ui) { ui.body.classList.remove('af-sp-swap'); }
            if (!keepSelection) {
                if (focusedCard) { focusedCard.classList.remove('af-focused'); }
                focusedCard = null;
                clearBackdrop();
            }
            showOverlays(false);
        }

        /* jf-web 10.11.11: .card carries the item type in data-type. Mirror the
         * three that read well into data-af-kind for the CSS badge.
         *
         * The attribute has to land on .cardScalable as well as on .card: the
         * badge is drawn by `.cardScalable::before { content: attr(...) }`, and
         * attr() resolves against the pseudo-element's own originating element,
         * not against an ancestor. With it only on .card the rule matched and
         * every declaration applied, but content resolved to "" and the badge
         * measured 0x0 — which is why it never appeared. */
        var KIND_LABELS = { Movie: 'Movie', Series: 'Series', Episode: 'Episode' };

        function decorateCards() {
            var cards = doc.querySelectorAll('#homeTab .card[data-type]:not([data-af-scanned])');
            for (var i = 0; i < cards.length; i++) {
                var card = cards[i];
                card.setAttribute('data-af-scanned', '1');
                var label = KIND_LABELS[card.getAttribute('data-type')];
                if (!label) { continue; }
                card.setAttribute('data-af-kind', label);
                var tile = card.querySelector('.cardScalable');
                if (tile) { tile.setAttribute('data-af-kind', label); }
            }
        }

        function refresh() {
            ensureSpace();
            updateVideoMode();
            keepThemeLast();
            pinThemeColor();
            watchPages();
            var home = isHomeRoute();
            // Never more than one: Home carries .homePage AND .libraryPage in
            // 10.11.11, so each test only runs once the ones above it are out.
            var library = !home && isLibraryRoute();
            var detail = !home && !library && isDetailRoute();
            /* Leaving a library grid drops its selection outright. The card it
             * points at belongs to no #homeTab .verticalSection, so carrying it
             * into Home would paint a stale item into the spotlight. */
            if (!library && root().classList.contains('af-library')) { leaveHome(); }
            if (!detail && root().classList.contains('af-detail')) { leaveDetail(); }
            if (library) {
                root().classList.add('af-library');
            } else {
                root().classList.remove('af-library');
            }
            if (detail) {
                root().classList.add('af-detail');
            } else {
                root().classList.remove('af-detail');
            }
            if (home) {
                root().classList.add('af-home');
                ensureUi();
                renderServerPanel();
                decorateCards();
                if (focusedItem) {
                    placeSpotlight(sectionOf(focusedCard));
                    if (!spotlightPainted) {
                        writeSpotlight(focusedItem, focusedCard);
                        spotlightPainted = true;
                    }
                    showOverlays(true);
                } else {
                    placeSpotlight(null);
                    showOverlays(overlaysWanted);
                }
            } else {
                root().classList.remove('af-home');
                /* Both art routes keep the backdrop across a refresh. Detail
                 * pages refresh constantly while the page streams in, and a
                 * clearBackdrop() on each one would strobe the art the fetched
                 * item just put up. */
                leaveHome(library || detail);
            }
            if (detail) { syncDetail(); }
        }

        /* .mainAnimatedPages does not exist yet at DOMContentLoaded - jellyfin-web
         * creates it when the first view mounts - so the subtree observer that
         * catches cards streamed into the rails has to be attached lazily. */
        var pagesObserved = false;

        function watchPages() {
            if (pagesObserved) { return; }
            var pages = doc.querySelector('.mainAnimatedPages');
            if (!pages) { return; }
            pagesObserved = true;
            new MutationObserver(guard(function (records) {
                for (var i = 0; i < records.length; i++) {
                    var t = records[i].target;
                    // Both synthesised panels are in this subtree; their own
                    // repaints must not schedule a refresh, or refresh and
                    // repaint feed forever.
                    if (ui && ui.spotlight
                        && (t === ui.spotlight || ui.spotlight.contains(t))) {
                        continue;
                    }
                    if (detailPanel
                        && (t === detailPanel || detailPanel.contains(t))) {
                        continue;
                    }
                    queueRefresh();
                    return;
                }
            })).observe(pages, {
                childList: true,
                subtree: true
            });
        }

        var refreshQueued = false;
        function queueRefresh() {
            if (refreshQueued) { return; }
            refreshQueued = true;
            // setTimeout rather than rAF: rAF does not fire while the window is
            // hidden, and the theme must still settle for the next time it is not.
            setTimeout(guard(function () {
                refreshQueued = false;
                refresh();
            }), 0);
        }

        /* ------------------------------------------------------------------ */
        /* 10. Wiring                                                          */
        /* ------------------------------------------------------------------ */

        function start() {
            watchStylesheets();
            pinThemeColor();
            ensureSpace();
            updateVideoMode();

            // Body children change when .videoPlayerContainer appears/disappears
            // and when React swaps pages.
            if (doc.body) {
                new MutationObserver(guard(function () {
                    updateVideoMode();
                    ensureSpace();
                    queueRefresh();
                })).observe(doc.body, { childList: true });
            }

            doc.addEventListener('focusin', guard(onFocusIn), true);
            doc.addEventListener('mouseover', guard(onPointerOver), { capture: true, passive: true });
            doc.addEventListener('mousemove', guard(onPointerMove), { capture: true, passive: true });
            window.addEventListener('hashchange', guard(queueRefresh), { passive: true });
            window.addEventListener('popstate', guard(queueRefresh), { passive: true });
            doc.addEventListener('viewshow', guard(queueRefresh), true);

            // jellyfin-web's internal Events.trigger() bus is a plain callback list
            // on the object, not DOM events — same hook native-shim.js uses.
            doc._callbacks = doc._callbacks || {};
            doc._callbacks.HISTORY_UPDATE = doc._callbacks.HISTORY_UPDATE || [];
            doc._callbacks.HISTORY_UPDATE.push(guard(queueRefresh));
            // jf-web 10.11.11 swaps themes/<name>/theme.css on this signal.
            doc._callbacks.THEME_CHANGE = doc._callbacks.THEME_CHANGE || [];
            doc._callbacks.THEME_CHANGE.push(guard(keepThemeLast));

            refresh();
        }

        window.__afTheme = { refresh: guard(refresh) };

        // Unit tests run this file under node, where there is no window and
        // `module` exists; in the browser `module` is undefined and none of
        // this runs. `state()` is the read-only view of the module-private
        // variables the tests assert on.
        if (typeof module !== 'undefined' && module.exports) {
            module.exports = {
                isFolderLike: isFolderLike,
                guard: guard,
                tokenMs: tokenMs,
                keepThemeLast: keepThemeLast,
                watchStylesheets: watchStylesheets,
                pinThemeColor: pinThemeColor,
                ensureSpace: ensureSpace,
                updateVideoMode: updateVideoMode,
                setBackdrop: setBackdrop,
                clearBackdrop: clearBackdrop,
                isHomeRoute: isHomeRoute,
                isLibraryRoute: isLibraryRoute,
                isDetailRoute: isDetailRoute,
                detailIdFromHash: detailIdFromHash,
                videoModeLabel: videoModeLabel,
                buildUi: buildUi,
                renderServerPanel: renderServerPanel,
                cacheItem: cacheItem,
                minutes: minutes,
                formatRuntime: formatRuntime,
                videoStream: videoStream,
                resolutionLabel: resolutionLabel,
                chipsFor: chipsFor,
                titleFor: titleFor,
                backdropUrlFor: backdropUrlFor,
                backdropSourceFor: backdropSourceFor,
                fetchItem: fetchItem,
                setFocusedCard: setFocusedCard,
                homeSectionsContainer: homeSectionsContainer,
                sectionOf: sectionOf,
                defaultSection: defaultSection,
                placeSpotlight: placeSpotlight,
                writeSpotlight: writeSpotlight,
                renderSpotlight: renderSpotlight,
                primaryCardButton: primaryCardButton,
                cardLinkTarget: cardLinkTarget,
                clickSyntheticAction: clickSyntheticAction,
                navigateToDetails: navigateToDetails,
                onPlayClick: onPlayClick,
                onDetailsClick: onDetailsClick,
                cardFrom: cardFrom,
                onFocusIn: onFocusIn,
                onPointerMove: onPointerMove,
                onPointerOver: onPointerOver,
                ensureUi: ensureUi,
                showOverlays: showOverlays,
                leaveHome: leaveHome,
                decorateCards: decorateCards,
                detailTypeFor: detailTypeFor,
                mediaStreams: mediaStreams,
                formatBytes: formatBytes,
                resumeLabel: resumeLabel,
                detailFacts: detailFacts,
                renderDetailPanel: renderDetailPanel,
                removeDetailPanel: removeDetailPanel,
                markResumeButton: markResumeButton,
                syncDetail: syncDetail,
                leaveDetail: leaveDetail,
                refresh: refresh,
                watchPages: watchPages,
                queueRefresh: queueRefresh,
                start: start,
                state: function () {
                    return {
                        ui: ui,
                        space: space,
                        backdropLayers: backdropLayers,
                        backdropSlot: backdropSlot,
                        currentBackdropUrl: currentBackdropUrl,
                        focusedCard: focusedCard,
                        focusedItem: focusedItem,
                        shownCard: shownCard,
                        shownItem: shownItem,
                        spotlightPainted: spotlightPainted,
                        swapTimer: swapTimer,
                        hoverTimer: hoverTimer,
                        overlaysWanted: overlaysWanted,
                        pointerMovedSincePlace: pointerMovedSincePlace,
                        itemCache: itemCache,
                        itemCacheKeys: itemCacheKeys,
                        pagesObserved: pagesObserved,
                        refreshQueued: refreshQueued,
                        detailPanel: detailPanel,
                        detailId: detailId,
                        detailItem: detailItem,
                        detailNextUp: detailNextUp
                    };
                }
            };
        }

        if (doc.readyState === 'loading') {
            doc.addEventListener('DOMContentLoaded', guard(start), { once: true });
        } else {
            guard(start)();
        }
    } catch (err) {
        if (window.console && console.warn) {
            console.warn('[Astrofin theme] initialisation failed', err);
        }
    }
}());
