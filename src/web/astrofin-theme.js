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

    if (window.__afTheme) {
        try { window.__afTheme.refresh(); } catch (e) { /* ignore */ }
        return;
    }

    var BG_BASE = '#070A14';
    var HOVER_DEBOUNCE_MS = 120;
    /* Spotlight stays up while the top of the page is in view. */
    var SCROLL_HIDE_RATIO = 0.35;

    var doc = document;
    var root = doc.documentElement;

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
        root.classList.toggle('af-video', playing);
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
            root.classList.add('af-backdrop');
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
        root.classList.remove('af-backdrop');
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
        // Kept out of jellyfin-web's focus manager: the container is not
        // tabbable and only the two real <button>s inside it are.
        spotlight.tabIndex = -1;

        var title = doc.createElement('h2');
        title.className = 'af-sp-title';
        var chips = div('af-sp-chips');
        var overview = doc.createElement('p');
        overview.className = 'af-sp-overview';
        var actions = div('af-sp-actions');

        var play = doc.createElement('button');
        play.type = 'button';
        play.className = 'af-btn af-btn-primary';
        var glyph = doc.createElement('span');
        glyph.className = 'af-btn-glyph';
        var playLabel = doc.createElement('span');
        playLabel.textContent = 'Play';
        play.appendChild(glyph);
        play.appendChild(playLabel);

        var details = doc.createElement('button');
        details.type = 'button';
        details.className = 'af-btn af-btn-secondary';
        details.textContent = 'Details';

        actions.appendChild(play);
        actions.appendChild(details);
        spotlight.appendChild(title);
        spotlight.appendChild(chips);
        spotlight.appendChild(overview);
        spotlight.appendChild(actions);

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

        doc.body.appendChild(spotlight);
        doc.body.appendChild(server);
        doc.body.appendChild(hint);

        ui = {
            spotlight: spotlight,
            title: title,
            chips: chips,
            overview: overview,
            playLabel: playLabel,
            play: play,
            details: details,
            server: server,
            hint: hint
        };

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
    var requestToken = 0;

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
            if (item) { itemCache[id] = item; }
            return item;
        });
    }

    /* ------------------------------------------------------------------ */
    /* 7. Focus tracking                                                   */
    /* ------------------------------------------------------------------ */

    var focusedCard = null;
    var focusedItem = null;

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
            renderSpotlight(item);
            setBackdrop(backdropUrlFor(item));
        })).catch(function (e) { log(e); });
    }

    function renderSpotlight(item) {
        buildUi();
        if (!ui) { return; }

        ui.title.textContent = titleFor(item);

        ui.chips.textContent = '';
        chipsFor(item).forEach(function (label) {
            var chip = div('af-chip');
            chip.textContent = label;
            ui.chips.appendChild(chip);
        });

        var overview = item.Overview || '';
        ui.overview.textContent = overview;
        ui.overview.style.display = overview ? '' : 'none';

        var resumable = item.UserData && item.UserData.PlaybackPositionTicks > 0;
        ui.playLabel.textContent = resumable ? 'Resume' : 'Play';

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
        if (!focusedCard) { return; }
        var btn = primaryCardButton(focusedCard);
        if (btn) { btn.click(); return; }
        navigateToDetails(focusedCard, focusedItem);
    }

    function onDetailsClick() {
        navigateToDetails(focusedCard, focusedItem);
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

    function onPointerOver(e) {
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

    function scrolledPastTop() {
        var el = doc.scrollingElement || doc.documentElement;
        var top = (el && el.scrollTop) || window.pageYOffset || 0;
        return top > (window.innerHeight || 720) * SCROLL_HIDE_RATIO;
    }

    function showOverlays(wanted) {
        if (wanted !== undefined) { overlaysWanted = wanted; }
        if (!ui) { return; }
        var visible = overlaysWanted && isHomeRoute() && !scrolledPastTop();
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
        if (isHomeRoute()) {
            root.classList.add('af-home');
            buildUi();
            renderServerPanel();
            decorateCards();
            if (focusedItem) { showOverlays(true); } else { showOverlays(overlaysWanted); }
        } else {
            root.classList.remove('af-home');
            leaveHome();
        }
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

        var pages = doc.querySelector('.mainAnimatedPages');
        if (pages) {
            new MutationObserver(guard(queueRefresh)).observe(pages, {
                childList: true,
                subtree: true
            });
        }

        doc.addEventListener('focusin', guard(onFocusIn), true);
        doc.addEventListener('mouseover', guard(onPointerOver), { capture: true, passive: true });
        window.addEventListener('hashchange', guard(queueRefresh), { passive: true });
        window.addEventListener('popstate', guard(queueRefresh), { passive: true });
        window.addEventListener('scroll', guard(function () { showOverlays(); }), {
            passive: true
        });
        window.addEventListener('resize', guard(function () { showOverlays(); }), {
            passive: true
        });
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
}());
