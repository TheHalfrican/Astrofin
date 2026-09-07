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
 *   3. on Home only: track the focused/hovered card and drive the backdrop
 *      plus the #af-spotlight panel, #af-server-panel and #af-hint;
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
                if (jmp.settings.transcode) {
                    rows.push(['Mode', jmp.settings.transcode.forceTranscoding ? 'Transcode' : 'Direct']);
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
            if (!isHomeRoute()) { return; }

            var id = card.getAttribute('data-id');
            if (!id) { return; }
            var token = ++requestToken;
            fetchItem(id).then(guard(function (item) {
                if (token !== requestToken || !item) { return; }
                focusedItem = item;
                renderSpotlight(item, card);
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
            // Home rails only; other pages keep plain jellyfin-web behaviour.
            return card.closest('#homeTab, .homePage') ? card : null;
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
        /* 8. Visibility                                                       */
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

        function leaveHome() {
            overlaysWanted = false;
            focusedItem = null;
            shownItem = null;
            shownCard = null;
            pendingItem = null;
            pendingCard = null;
            if (swapTimer) { clearTimeout(swapTimer); swapTimer = 0; }
            spotlightPainted = false;
            if (ui) { ui.body.classList.remove('af-sp-swap'); }
            if (focusedCard) { focusedCard.classList.remove('af-focused'); }
            focusedCard = null;
            clearBackdrop();
            showOverlays(false);
        }

        /* jf-web 10.11.11: .card carries the item type in data-type. Mirror the
         * three that read well into data-af-kind for the CSS badge. */
        var KIND_LABELS = { Movie: 'Movie', Series: 'Series', Episode: 'Episode' };

        function decorateCards() {
            var cards = doc.querySelectorAll('#homeTab .card[data-type]:not([data-af-scanned])');
            for (var i = 0; i < cards.length; i++) {
                var card = cards[i];
                card.setAttribute('data-af-scanned', '1');
                var label = KIND_LABELS[card.getAttribute('data-type')];
                if (label) { card.setAttribute('data-af-kind', label); }
            }
        }

        function refresh() {
            ensureSpace();
            updateVideoMode();
            keepThemeLast();
            pinThemeColor();
            watchPages();
            if (isHomeRoute()) {
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
                leaveHome();
            }
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
                    // The panel is in this subtree; its own repaints must not
                    // schedule a refresh, or refresh and repaint feed forever.
                    if (ui && ui.spotlight
                        && (t === ui.spotlight || ui.spotlight.contains(t))) {
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
        /* 9. Wiring                                                           */
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
