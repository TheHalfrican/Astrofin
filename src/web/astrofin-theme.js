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
 *      the card popout that stands in for it on Home and on the library
 *      grids, plus #af-hint, which is Home-only;
 *   3b. mark the item detail pages with html.af-detail, which the sheet's
 *      colour-only section (o) is scoped to — those pages keep jellyfin-web's
 *      own layout and art band, so there is nothing to build there;
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

        /* Types the popout treats as containers rather than something to play:
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
        // which keeps this from ping-ponging with #af-popout and friends.
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
                /* The sheet hides the popout in video mode either way, but
                 * leaving it up in state means it reappears over the card the
                 * pointer left behind when playback ends. */
                hidePopout();
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
        // stays true forever once one has rendered. The sheet's section (o) is
        // scoped to `.itemDetailPage` and so also matches a hidden outgoing
        // page, which is harmless: a hidden page paints nothing.
        function isDetailRoute() {
            var hash = String(location.hash || '').replace(/^#!?/, '');
            return /^\/details([?/]|$)/.test(hash);
        }

        /* ------------------------------------------------------------------ */
        /* 5. Card popout, controller hint                                     */
        /* ------------------------------------------------------------------ */

        var ui = null;

        function buildUi() {
            if (ui || !doc.body) { return; }
            // Idempotent across a re-run in the same document: drop any panels a
            // previous execution left behind rather than shadowing them.
            ['af-popout', 'af-hint'].forEach(function (id) {
                var old = doc.getElementById(id);
                if (old && old.parentNode) { old.parentNode.removeChild(old); }
            });

            /* The popout is a body-level portal positioned over the card, not a
             * child of it. The drawer grows a couple of hundred pixels past the
             * bottom of the rail, and a fixed layer in <body> cannot be clipped
             * by anything jellyfin-web puts between the card and <html> — which
             * a child of the card would be the moment any one of those
             * ancestors stopped being `overflow: visible`. One node serves every
             * card on the page. */
            var popout = div('');
            popout.id = 'af-popout';
            popout.hidden = true;

            /* Clone target. writePopout() fills it with the card's own
             * .cardScalable, so the art, the kind badge and the watched-progress
             * bar are exactly what the card is already showing and no image URL
             * has to be reconstructed. */
            var art = div('af-po-art');
            var drawer = div('af-po-drawer');
            var actions = div('af-po-actions');
            var title = div('af-po-title');
            var badges = div('af-po-badges');
            var meta = div('af-po-meta');
            var genres = div('af-po-genres');

            // jf-web 10.11.11's focusManager builds its focusable set from
            //   INPUT/TEXTAREA/SELECT/BUTTON/A + ':not([tabindex="-1"]):not(:disabled)'
            //   plus '.focusable'
            // and autoFocus() additionally skips anything classed 'noautofocus'.
            // tabindex -1 is therefore what keeps the popout out of controller and
            // keyboard spatial navigation between the rails; noautofocus stops it
            // being chosen as a view's initial focus. Both buttons stay fully
            // usable with the mouse.
            function discButton(className, glyphClass, label) {
                var b = doc.createElement('button');
                b.type = 'button';
                b.className = className + ' noautofocus';
                b.tabIndex = -1;
                // Icon-only, so the name has to come from the label.
                b.setAttribute('aria-label', label);
                b.title = label;
                var g = div(glyphClass);
                b.appendChild(g);
                return b;
            }

            var play = discButton('af-po-btn af-po-btn-primary', 'af-po-glyph af-po-glyph-play', 'Play');
            var details = discButton('af-po-btn af-po-btn-ghost', 'af-po-glyph af-po-glyph-info', 'Details');

            actions.appendChild(play);
            actions.appendChild(details);
            drawer.appendChild(actions);
            drawer.appendChild(title);
            drawer.appendChild(badges);
            drawer.appendChild(meta);
            drawer.appendChild(genres);
            popout.appendChild(art);
            popout.appendChild(drawer);

            var hint = div('');
            hint.id = 'af-hint';
            hint.hidden = true;
            hint.setAttribute('aria-hidden', 'true');
            ['◀ ▶ MOVE', '✕ SELECT', '◯ BACK', 'OPTIONS ⋯'].forEach(function (t) {
                var s = doc.createElement('span');
                s.textContent = t;
                hint.appendChild(s);
            });

            doc.body.appendChild(popout);
            doc.body.appendChild(hint);

            ui = {
                popout: popout,
                art: art,
                drawer: drawer,
                actions: actions,
                title: title,
                badges: badges,
                meta: meta,
                genres: genres,
                play: play,
                details: details,
                hint: hint
            };

            play.addEventListener('click', guard(onPlayClick));
            details.addEventListener('click', guard(onDetailsClick));
            /* The popout covers the card it is standing in for, so without this
             * it would swallow the click that used to open the item. */
            art.addEventListener('click', guard(onDetailsClick));
            /* Leaving the popout for anything that is not a card closes it.
             * The card underneath is covered, so its own mouseout never fires
             * while the pointer is inside here. */
            popout.addEventListener('mouseleave', guard(function (e) {
                if (!cardFrom(e.relatedTarget)) { hidePopout(); }
            }));
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

        /* The drawer's three tiers. One chip strip was tried first and did not
         * survive contact: `.af-chip` is a 14 px uppercase pill with 9/14
         * padding, so at a poster's width three facts filled the strip and the
         * rest was clipped. Plain dot-separated text costs roughly a quarter of
         * the width per fact, which is what buys the extra detail back. Only
         * the certification and the picture format stay boxed — those read as
         * badges, and Netflix's hover card boxes exactly the same ones. */

        function badgesFor(item) {
            var out = [];
            if (!item || isFolderLike(item)) { return out; }
            if (item.OfficialRating) { out.push(String(item.OfficialRating)); }
            var vs = videoStream(item);
            var res = resolutionLabel(vs);
            if (res) { out.push(res); }
            var range = vs && (vs.VideoRangeType || vs.VideoRange);
            if (range && range !== 'SDR') { out.push(String(range).replace(/_/g, ' ')); }
            return out;
        }

        function metaFor(item) {
            var out = [];
            if (!item) { return out; }

            /* Containers get a count and nothing else: a library has no year,
             * runtime, rating or resume position worth showing. */
            if (isFolderLike(item)) {
                if (item.ChildCount) {
                    out.push(item.ChildCount + (item.ChildCount === 1 ? ' item' : ' items'));
                }
                return out;
            }

            if (item.Type === 'Episode') {
                if (item.ParentIndexNumber != null && item.IndexNumber != null) {
                    out.push('S' + item.ParentIndexNumber + ' E' + item.IndexNumber);
                }
                // The title line shows the series, so the episode's own name
                // has nowhere else to go.
                if (item.Name) { out.push(item.Name); }
            } else if (item.Type === 'Series' && item.ChildCount) {
                out.push(item.ChildCount + (item.ChildCount === 1 ? ' season' : ' seasons'));
            }

            if (item.ProductionYear) { out.push(String(item.ProductionYear)); }

            var rt = formatRuntime(item.RunTimeTicks);
            if (rt) { out.push(rt); }

            if (item.CommunityRating) {
                out.push('\u2605 ' + Number(item.CommunityRating).toFixed(1));
            }

            var ud = item.UserData || {};
            if (ud.PlaybackPositionTicks > 0 && item.RunTimeTicks > 0) {
                var left = minutes(item.RunTimeTicks - ud.PlaybackPositionTicks);
                if (left > 0) { out.push(left + 'm left'); }
            } else if (item.DateCreated) {
                var age = Date.now() - Date.parse(item.DateCreated);
                if (age >= 0 && age < 14 * 864e5) { out.push('New'); }
            }
            return out;
        }

        /* Three is what one line holds at a poster's width, and the server
         * returns them most-specific-first. An episode usually carries none —
         * the genres live on its series — so the line hides rather than
         * reserving space for nothing. */
        var MAX_GENRES = 3;

        function genresFor(item) {
            if (!item || isFolderLike(item) || !item.Genres || !item.Genres.length) {
                return [];
            }
            return item.Genres.slice(0, MAX_GENRES).map(String);
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
        /* The card the popout is currently *showing*. setFocusedCard runs
         * synchronously on hover/focus but the popout only repaints once
         * fetchItem() resolves, so the buttons must act on this, never on
         * focusedCard, or a slow server lets Play start the wrong item. */
        var shownCard = null;
        var shownItem = null;
        /* The card the popout is standing in for. Distinct from focusedCard:
         * it is set only once the popout is actually up, and it is what
         * carries .af-popped. */
        var poppedCard = null;

        function setFocusedCard(card) {
            if (card === focusedCard) { return; }
            if (focusedCard) { focusedCard.classList.remove('af-focused'); }
            focusedCard = card;
            if (!card) { return; }
            card.classList.add('af-focused');
            /* Resolve the route once, synchronously - the fetch below can
             * outlive a navigation. Home and the library grids behave
             * identically; every other page keeps plain jellyfin-web hover. */
            if (!isHomeRoute() && !isLibraryRoute()) { return; }

            var id = card.getAttribute('data-id');
            if (!id) { return; }
            var token = ++requestToken;
            fetchItem(id).then(guard(function (item) {
                if (token !== requestToken || !item) { return; }
                /* The pointer can leave the card while the fetch is out, and
                 * hidePopout() drops the selection without bumping the token,
                 * so the token alone does not cover it. */
                if (card !== focusedCard) { return; }
                focusedItem = item;
                renderPopout(item, card);
                setBackdrop(backdropUrlFor(item));
            })).catch(function (e) { log(e); });
        }

        /* ------------------------------------------------------------------ */
        /* 7b. Popout geometry                                                 */
        /*                                                                     */
        /* The popout is fixed to the viewport and positioned over the card it */
        /* stands in for. It grows from the art's own centre, then shifts to   */
        /* stay clear of the viewport edges and of the fixed header - which is */
        /* what makes a card at either end of a rail open inwards rather than  */
        /* half off-screen.                                                    */
        /* ------------------------------------------------------------------ */

        // Clearance from the viewport edges and from the header.
        var POPOUT_EDGE = 16;
        var POPOUT_SCALE_FALLBACK = 1.32;
        var POPOUT_MIN_WIDTH_FALLBACK = 200;
        // Below this the art has stopped being a poster and is just a strip.
        var POPOUT_MIN_ART = 96;

        function artOf(card) {
            return (card && card.querySelector && card.querySelector('.cardScalable')) || card;
        }

        /* How much bigger than the card the popout is. Read from a token so the
         * sheet owns it and a per-breakpoint override stays a CSS change. */
        function popoutScale() {
            return readToken('--af-popout-scale', 1, 3, POPOUT_SCALE_FALLBACK);
        }

        /* The drawer's floor, for cards small enough that a proportional popout
         * could not hold a readable line. It is set low on purpose: widening
         * past the card's proportion enlarges the art with it, so a floor that
         * bit on ordinary rail cards would cost height everywhere to buy width
         * in one place. */
        function popoutMinWidth() {
            return readToken('--af-popout-min-width', 0, 2000, POPOUT_MIN_WIDTH_FALLBACK);
        }

        /* Numeric token read with a sanity range. Anything outside it — a
         * missing token, a junk value, `parseFloat` returning NaN — falls back,
         * because every one of these is a geometry input and a bad one paints a
         * popout nobody can read. */
        function readToken(name, min, max, fallback) {
            var v = NaN;
            try {
                v = parseFloat(getComputedStyle(root()).getPropertyValue(name));
            } catch (e) { /* ignore */ }
            return (v > min && v < max) ? v : fallback;
        }

        /* The header the popout has to clear. Two candidates, because Jellyfin
         * 12 moved the header to a MUI AppBar and left the legacy .skinHeader
         * in a `display: none` wrapper: on a 12.x server the old selector alone
         * measures 0x0, `headerBottom()` returned 0, and the popout would tuck
         * under a bar that outranks it — MUI sits at z-index 1100 to our 900.
         *
         * Whichever candidate is actually laid out wins, so one function serves
         * both server generations. `height > 0` is what rejects the hidden one:
         * a node inside a `display: none` ancestor still reports its own
         * computed `display`, but every number on its rect is zero. */
        function headerBottom() {
            var els = doc.querySelectorAll('.skinHeader, header.MuiAppBar-root');
            var bottom = 0;
            for (var i = 0; i < els.length; i++) {
                if (!els[i].getBoundingClientRect) { continue; }
                var r = els[i].getBoundingClientRect();
                // Only a header pinned over the top of the viewport is in the
                // way; jellyfin-web lets some of them scroll away.
                if (r.height > 0 && r.top <= 0 && r.bottom > bottom) {
                    bottom = r.bottom;
                }
            }
            return bottom;
        }

        /* Two passes by necessity: the width and the art height follow from the
         * card, but the drawer's height is only knowable once it has been laid
         * out, and the vertical clamp needs it. Callers unhide the popout
         * first for that reason. Returns null when there is nothing to anchor
         * to, which is the caller's signal to give up rather than paint a
         * popout at 0,0. */
        function placePopout(card) {
            if (!ui || !card || !card.isConnected) { return null; }
            var rect = artOf(card).getBoundingClientRect();
            if (!rect.width || !rect.height) { return null; }

            var vw = window.innerWidth || root().clientWidth || 0;
            var vh = window.innerHeight || root().clientHeight || 0;
            var scale = popoutScale();

            /* Widen to the drawer's floor, then cap: an edge card's popout
             * still has to have somewhere to shift to. The cap wins, so on a
             * viewport narrower than the floor the popout simply fills it. */
            var w = Math.max(Math.round(rect.width * scale), Math.round(popoutMinWidth()));
            w = Math.min(w, Math.max(120, vw - POPOUT_EDGE * 2));

            var left = rect.left + rect.width / 2 - w / 2;
            left = Math.max(POPOUT_EDGE, Math.min(left, vw - w - POPOUT_EDGE));

            ui.popout.style.left = Math.round(left) + 'px';
            ui.popout.style.width = w + 'px';

            /* Everything below needs the drawer's laid-out height, and the
             * drawer's height depends on the width just written — so the width
             * goes on first and this read is what flushes it. */
            var drawerH = ui.drawer.offsetHeight || 0;
            var band = vh - POPOUT_EDGE * 2 - headerBottom();

            /* Art height follows the width, not the scale, so widening to the
             * floor enlarges the poster instead of stretching it. Then it is
             * capped to what is left of the band: the art is `cover`, so a
             * popout too tall for the viewport crops its poster rather than
             * hanging off the bottom of the screen. */
            var artH = Math.round(rect.height * (w / rect.width));
            artH = Math.max(POPOUT_MIN_ART, Math.min(artH, band - drawerH));
            ui.art.style.height = artH + 'px';

            var top = rect.top - (artH - rect.height) / 2;
            var min = headerBottom() + POPOUT_EDGE;
            var max = vh - POPOUT_EDGE - (artH + drawerH);
            // Still taller than the band: pin to the top of it.
            top = (max < min) ? min : Math.max(min, Math.min(top, max));
            ui.popout.style.top = Math.round(top) + 'px';

            return { left: left, top: top, width: w, artHeight: artH };
        }

        /* ------------------------------------------------------------------ */
        /* 7c. Rendering                                                       */
        /* ------------------------------------------------------------------ */

        /* A copy of the card's own tile, so the popout shows exactly the art,
         * kind badge and watched-progress the card is already showing and no
         * image URL has to be rebuilt.
         *
         * Two things come out of the clone:
         *   - <canvas>: cloneNode never copies a canvas's pixels, so a cloned
         *     blurhash placeholder paints as an empty box over the real image.
         *   - .cardOverlayContainer: the card's own hover buttons, which the
         *     drawer replaces.
         *
         * [data-action] is *disarmed* rather than dropped. jf-web 10.11.11
         * resolves a delegated card click from the closest [data-action] plus
         * the nearest [data-id] ancestor, and the clone has no .card[data-id]
         * above it, so a live one would resolve against whatever the popout
         * happens to be sitting over — but .cardImageContainer itself carries
         * data-action="link" and *is* the art, so removing the node would
         * remove the picture. The attribute comes off and the element stays;
         * the popout's own buttons act on shownCard. */
        function cloneArt(card) {
            var src = card && card.querySelector && card.querySelector('.cardScalable');
            if (!src) { return null; }
            var clone = src.cloneNode(true);
            clone.removeAttribute('id');
            var drop = clone.querySelectorAll('canvas, .cardOverlayContainer');
            for (var i = 0; i < drop.length; i++) {
                if (drop[i].parentNode) { drop[i].parentNode.removeChild(drop[i]); }
            }
            var armed = clone.querySelectorAll('[data-action]');
            for (var j = 0; j < armed.length; j++) {
                armed[j].removeAttribute('data-action');
            }
            clone.removeAttribute('data-action');
            // The sheet scales the real tile on hover; the clone is already the
            // size it wants to be.
            clone.style.transform = 'none';
            return clone;
        }

        /* Icon-only buttons, so the name has to come from the label. */
        function setDiscAction(btn, glyphClass, label) {
            if (!btn) { return; }
            btn.setAttribute('aria-label', label);
            btn.title = label;
            var g = btn.firstChild;
            if (g && g.className !== undefined) { g.className = 'af-po-glyph ' + glyphClass; }
        }

        /* A dot-separated line, written as text rather than as a node per fact:
         * the separator is punctuation here, not structure, and one text node
         * is what lets the line ellipsize as prose when it overruns. */
        function writeJoined(el, parts) {
            var text = parts.join(' \u00b7 ');
            el.textContent = text;
            el.hidden = !text;
        }

        function writeBadges(el, labels) {
            el.textContent = '';
            labels.forEach(function (label) {
                var b = div('af-po-badge');
                b.textContent = label;
                el.appendChild(b);
            });
            el.hidden = !labels.length;
        }

        function writePopout(item, card) {
            if (!ui) { return; }
            shownItem = item;
            shownCard = card;
            var folder = isFolderLike(item);

            var clone = cloneArt(card);
            ui.art.textContent = '';
            if (clone) { ui.art.appendChild(clone); }

            ui.title.textContent = titleFor(item);
            writeBadges(ui.badges, badgesFor(item));
            writeJoined(ui.meta, metaFor(item));
            writeJoined(ui.genres, genresFor(item));

            if (folder) {
                // A library, box set or season is browsed, not played.
                setDiscAction(ui.play, 'af-po-glyph-browse', 'Browse');
                ui.details.hidden = true;
            } else {
                var resumable = item.UserData && item.UserData.PlaybackPositionTicks > 0;
                setDiscAction(ui.play, 'af-po-glyph-play', resumable ? 'Resume' : 'Play');
                ui.details.hidden = false;
            }
        }

        var hideTimer = 0;

        function renderPopout(item, card) {
            ensureUi();
            if (!ui || !card || !card.isConnected) { return; }
            if (hideTimer) { clearTimeout(hideTimer); hideTimer = 0; }

            if (poppedCard && poppedCard !== card) {
                poppedCard.classList.remove('af-popped');
            }
            poppedCard = card;
            /* Cancels the card's own hover scale. The popout stands in for the
             * tile, and a tile still scaled to 1.06 underneath peeks out past
             * the popout's edges; it also means the rect read in placePopout()
             * is a plain layout box rather than a transformed one. */
            card.classList.add('af-popped');

            writePopout(item, card);

            // placePopout() reads the drawer's laid-out height, which a hidden
            // element does not have.
            ui.popout.hidden = false;
            if (!placePopout(card)) { hidePopout(); return; }
            // Flush layout so the transition runs from the collapsed state. A
            // rAF would be throttled to nothing in a background tab.
            void ui.popout.offsetWidth;
            ui.popout.classList.add('af-show');
            // The Home-only chrome comes up with the first card the pointer
            // lands on, not with the route — unchanged from before the popout.
            if (isHomeRoute()) { showOverlays(true); }
        }

        /* Closes the popout and drops the selection, so re-entering the same
         * card opens it again. The backdrop deliberately stays: it is the page
         * background on both routes, and clearing it on every pointer exit
         * would strobe the art across a rail. */
        function hidePopout() {
            if (poppedCard) { poppedCard.classList.remove('af-popped'); }
            poppedCard = null;
            shownCard = null;
            shownItem = null;
            if (focusedCard) { focusedCard.classList.remove('af-focused'); }
            focusedCard = null;
            if (!ui) { return; }
            ui.popout.classList.remove('af-show');
            if (hideTimer) { clearTimeout(hideTimer); }
            hideTimer = setTimeout(guard(function () {
                hideTimer = 0;
                // A hover during the fade-out re-shows it; don't undo that.
                if (ui && !ui.popout.classList.contains('af-show')) {
                    ui.popout.hidden = true;
                }
            }), tokenMs('--af-dur-tile', 180));
        }

        /* Scrolling moves the card out from under a viewport-fixed popout.
         * Following it rather than closing matters for the keyboard and
         * controller path, where jellyfin-web scrolls the newly focused card
         * into view and would otherwise close the popout it just opened. */
        var syncQueued = false;

        function syncPopout() {
            if (syncQueued || !ui || ui.popout.hidden || !poppedCard) { return; }
            syncQueued = true;
            // setTimeout, not rAF, for the same reason as queueRefresh: rAF
            // never fires while the window is hidden.
            setTimeout(guard(function () {
                syncQueued = false;
                if (!ui || ui.popout.hidden || !poppedCard) { return; }
                if (!poppedCard.isConnected) { hidePopout(); return; }
                var r = artOf(poppedCard).getBoundingClientRect();
                var vh = window.innerHeight || root().clientHeight || 0;
                // Scrolled out of the viewport: nothing left to anchor to.
                if (!r.width || r.bottom <= 0 || r.top >= vh) { hidePopout(); return; }
                placePopout(poppedCard);
            }), 0);
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

        /* True for anything inside the popout, which is not a card but must not
         * be treated as having left one either - the pointer crosses it on the
         * way to the buttons. */
        function inPopout(node) {
            return !!(ui && node && ui.popout.contains(node));
        }

        function onPointerOver(e) {
            var card = cardFrom(e.target);
            if (!card) {
                /* Left for something that is neither a card nor the popout.
                 * The popout covers its own card, so its mouseleave does not
                 * fire for a pointer that crossed onto a rail heading or the
                 * page background. */
                if (!inPopout(e.target)) {
                    if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = 0; }
                    if (poppedCard) { hidePopout(); }
                }
                return;
            }
            if (card === focusedCard) { return; }
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
            if (ui && (!ui.popout.isConnected || !ui.hint.isConnected)) {
                ui = null;
            }
            buildUi();
        }

        /* Home-only chrome: the controller hint, and nothing else since the
         * server card was dropped. The popout is not in here — it belongs to a
         * card rather than to a route, shows on both Home and the library
         * grids, and is driven by renderPopout/hidePopout. */
        function showOverlays(wanted) {
            if (wanted !== undefined) { overlaysWanted = wanted; }
            if (!ui) { return; }
            var el = ui.hint;
            if (overlaysWanted && isHomeRoute()) {
                el.hidden = false;
                // Flush layout so the opacity transition runs from 0. A rAF
                // would be throttled to nothing in a background tab.
                void el.offsetWidth;
                el.classList.add('af-show');
            } else {
                el.classList.remove('af-show');
                el.hidden = true;
            }
        }

        /* Tear the Home panels down. `keepSelection` is set only when the route
         * moved to a library grid, which keeps its own focused card and the art
         * backdrop that card is driving. The popout goes either way: it is
         * anchored to a viewport rect that the outgoing page owns. With
         * `keepSelection` falsy this is the original teardown. */
        function leaveHome(keepSelection) {
            overlaysWanted = false;
            if (!keepSelection) { focusedItem = null; }
            var card = focusedCard;
            hidePopout();
            if (keepSelection && card) {
                // hidePopout() drops the selection; the library grid keeps it.
                focusedCard = card;
                card.classList.add('af-focused');
            } else {
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
            /* Leaving a library grid drops its selection outright: the card it
             * points at is going away with the page, and the popout is anchored
             * to that card's viewport rect. */
            if (!library && root().classList.contains('af-library')) { leaveHome(); }
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
                decorateCards();
                showOverlays(overlaysWanted);
                /* Cards stream into the rails for seconds after the first
                 * hover and every batch reflows the one the popout is anchored
                 * to, so a refresh has to re-measure rather than leave it
                 * hanging over whatever moved underneath. */
                syncPopout();
            } else {
                root().classList.remove('af-home');
                /* The library grid keeps its selection and its art across a
                 * refresh. Everywhere else, the item detail pages included,
                 * both go: a detail page shows jellyfin-web's own art band,
                 * not ours. */
                leaveHome(library);
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
            /* Nothing Astrofin draws lives in this subtree — the popout is in
             * <body>, outside .mainAnimatedPages, and the .af-popped it writes
             * onto a card is an attribute mutation, which this observer does
             * not ask for — so any record here is jellyfin-web's. */
            new MutationObserver(guard(queueRefresh)).observe(pages, {
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
            /* Capture, because the rails scroll horizontally and Home scrolls
             * vertically — the popout is fixed to the viewport and has to
             * follow the card under either. */
            doc.addEventListener('scroll', guard(syncPopout), { capture: true, passive: true });
            window.addEventListener('resize', guard(syncPopout), { passive: true });
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
                buildUi: buildUi,
                cacheItem: cacheItem,
                minutes: minutes,
                formatRuntime: formatRuntime,
                videoStream: videoStream,
                resolutionLabel: resolutionLabel,
                badgesFor: badgesFor,
                metaFor: metaFor,
                genresFor: genresFor,
                titleFor: titleFor,
                backdropUrlFor: backdropUrlFor,
                fetchItem: fetchItem,
                setFocusedCard: setFocusedCard,
                artOf: artOf,
                popoutScale: popoutScale,
                popoutMinWidth: popoutMinWidth,
                readToken: readToken,
                headerBottom: headerBottom,
                placePopout: placePopout,
                cloneArt: cloneArt,
                writeJoined: writeJoined,
                writeBadges: writeBadges,
                setDiscAction: setDiscAction,
                writePopout: writePopout,
                renderPopout: renderPopout,
                hidePopout: hidePopout,
                syncPopout: syncPopout,
                inPopout: inPopout,
                primaryCardButton: primaryCardButton,
                cardLinkTarget: cardLinkTarget,
                clickSyntheticAction: clickSyntheticAction,
                navigateToDetails: navigateToDetails,
                onPlayClick: onPlayClick,
                onDetailsClick: onDetailsClick,
                cardFrom: cardFrom,
                onFocusIn: onFocusIn,
                onPointerOver: onPointerOver,
                ensureUi: ensureUi,
                showOverlays: showOverlays,
                leaveHome: leaveHome,
                decorateCards: decorateCards,
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
                        poppedCard: poppedCard,
                        hideTimer: hideTimer,
                        syncQueued: syncQueued,
                        hoverTimer: hoverTimer,
                        overlaysWanted: overlaysWanted,
                        itemCache: itemCache,
                        itemCacheKeys: itemCacheKeys,
                        pagesObserved: pagesObserved,
                        refreshQueued: refreshQueued
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
