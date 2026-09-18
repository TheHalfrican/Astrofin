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
 *   3. track the focused/hovered card on Home and the library grids and
 *      drive the backdrop from it, plus #af-hint, which is Home-only;
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

        /* ------------------------------------------------------------------ */
        /* 1. Stylesheet ordering                                              */
        /* ------------------------------------------------------------------ */

        // Cascade ordering. Two things load after us at equal specificity:
        //   - a <link> per lazily loaded webpack chunk, appended to <head>;
        //   - jf-web 10.11.11 puts themes/<name>/theme.css in a <div> inside
        //     <body>, which no position in <head> can ever beat.
        // So the sheet lives at the end of <body> and is moved back there whenever
        // another stylesheet node ends up after it. Only stylesheet nodes count,
        // which keeps this from ping-ponging with #af-space and #af-hint.
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
                // Otherwise the card the pointer left behind still wears its
                // ring when playback ends.
                setFocusedCard(null);
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
        /* 5. Controller hint                                                  */
        /* ------------------------------------------------------------------ */

        /* Home-only chrome. The card popout that used to share this section
         * (v0.6.0) was removed at the owner's request on 2026-09-18. */
        var ui = null;

        function buildUi() {
            if (ui || !doc.body) { return; }
            // Idempotent across a re-run in the same document: drop a hint a
            // previous execution left behind rather than shadowing it.
            var old = doc.getElementById('af-hint');
            if (old && old.parentNode) { old.parentNode.removeChild(old); }

            var hint = div('');
            hint.id = 'af-hint';
            hint.hidden = true;
            hint.setAttribute('aria-hidden', 'true');
            ['◀ ▶ MOVE', '✕ SELECT', '◯ BACK', 'OPTIONS ⋯'].forEach(function (t) {
                var s = doc.createElement('span');
                s.textContent = t;
                hint.appendChild(s);
            });

            doc.body.appendChild(hint);
            ui = { hint: hint };
        }

        /* ------------------------------------------------------------------ */
        /* 6. Item metadata                                                    */
        /* ------------------------------------------------------------------ */

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

        /* The card the art backdrop is following on Home and the library grids,
         * marked .af-focused so the sheet can hold its ring after the hover
         * debounce. Passing null drops the selection and leaves the art up:
         * the backdrop is the page background on both routes, and clearing it
         * on every pointer exit would strobe the art across a rail. */
        var focusedCard = null;

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
                 * dropping the selection does not bump the token, so the
                 * token alone does not cover it. */
                if (card !== focusedCard) { return; }
                setBackdrop(backdropUrlFor(item));
                // The Home-only chrome comes up with the first card the
                // pointer lands on, not with the route.
                if (isHomeRoute()) {
                    ensureUi();
                    showOverlays(true);
                }
            })).catch(function (e) { log(e); });
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

        function onPointerOver(e) {
            var card = cardFrom(e.target);
            if (!card) {
                // Left the cards for a rail heading or the page background.
                if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = 0; }
                setFocusedCard(null);
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
         * hint. Rebuild rather than keep writing into a detached node. */
        function ensureUi() {
            if (ui && !ui.hint.isConnected) {
                ui = null;
            }
            buildUi();
        }

        /* Home-only chrome: the controller hint, and nothing else since the
         * server card and the card popout were dropped. */
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

        /* Tear the Home chrome down. `keepSelection` is set only when the route
         * moved to a library grid, which keeps its own focused card and the art
         * backdrop that card is driving. With it falsy, both go. */
        function leaveHome(keepSelection) {
            overlaysWanted = false;
            if (!keepSelection) {
                setFocusedCard(null);
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
             * points at is going away with the page. */
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
            /* Nothing Astrofin draws lives in this subtree — the hint is in
             * <body>, outside .mainAnimatedPages, and the .af-focused and
             * data-af-kind it writes onto cards are attribute mutations, which
             * this observer does not ask for — so any record here is
             * jellyfin-web's. */
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
                guard: guard,
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
                backdropUrlFor: backdropUrlFor,
                fetchItem: fetchItem,
                setFocusedCard: setFocusedCard,
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
