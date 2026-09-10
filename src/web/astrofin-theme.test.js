// Unit tests for the Astrofin theme runtime. Run with:
//
//     node --test src/web/astrofin-theme.test.js
//
// (or `just test-js`). astrofin-theme.js is injected at OnContextCreated and
// re-runs on every navigation that builds a fresh V8 context, so the two
// properties worth pinning are (a) it never throws into the bundle it shares
// with csd.js and select-menu.js, and (b) a second run against a window that
// already has it installed refreshes rather than installs again — no second
// set of MutationObservers, listeners or panels.
//
// Everything else here is the logic the panel is made of: the chip strip, the
// backdrop crossfade, the Home route test, the in-flow placement of the
// spotlight and the two buttons, which must act on the item the panel is
// *showing* rather than on whatever the pointer has since moved to.
//
// The fake DOM comes from src/web/test/player-fakes.js; the extra bits the
// theme needs (real textContent semantics, document fragments, bubbling
// clicks, a Home page) from src/web/test/theme-fakes.js.
const test = require('node:test');
const assert = require('node:assert');

const { loadModule } = require('./test/player-fakes.js');
const {
    makeThemeWindow, loadTheme, installThemeStyle, buildHome, makeCard,
    makeThemeApiClient, observersFor, childTexts
} = require('./test/theme-fakes.js');

// A window with the theme installed and Home already in the DOM.
function onHome(overrides) {
    const win = makeThemeWindow(Object.assign({ hash: '#/home.html' }, overrides));
    const home = buildHome(win);
    const theme = loadTheme(win);
    return { win, home, theme };
}

// ---------------------------------------------------------------------------
// Installation and idempotency
// ---------------------------------------------------------------------------

test('installing publishes window.__afTheme and paints without throwing', () => {
    const win = makeThemeWindow();
    const theme = loadTheme(win);
    assert.strictEqual(typeof win.__afTheme.refresh, 'function');
    assert.strictEqual(typeof theme.refresh, 'function');
    assert.deepStrictEqual(
        win.console.lines.filter((l) => l.level === 'warn').map((l) => l.args[0]),
        [],
        'the outer try must not have fired'
    );
});

test('a second run against the same window refreshes instead of installing again', () => {
    const { win } = onHome();
    const observersAfterFirst = win.observers.length;
    const docListenersAfterFirst = win.document.listeners.length;
    const winListenersAfterFirst = win.listeners.length;
    const spaceAfterFirst = win.document.getElementById('af-space');

    const second = loadModule('astrofin-theme.js', win);

    assert.strictEqual(second, win.__afTheme, 're-entry hands back the installation');
    assert.strictEqual(second.state, undefined, 'the install path did not run twice');
    assert.strictEqual(win.observers.length, observersAfterFirst, 'no second observers');
    assert.strictEqual(win.document.listeners.length, docListenersAfterFirst);
    assert.strictEqual(win.listeners.length, winListenersAfterFirst);
    assert.strictEqual(win.document.getElementById('af-space'), spaceAfterFirst);
    assert.strictEqual(win.document.querySelectorAll('#af-space').length, 1);
    assert.strictEqual(win.document.querySelectorAll('#af-spotlight').length, 1);
});

test('a third and fourth run still leave exactly one of every panel', () => {
    const { win } = onHome();
    loadModule('astrofin-theme.js', win);
    loadModule('astrofin-theme.js', win);
    for (const id of ['af-space', 'af-spotlight', 'af-server-panel', 'af-hint']) {
        assert.strictEqual(win.document.querySelectorAll('#' + id).length, 1, id);
    }
});

test('buildUi drops panels a previous execution left behind', () => {
    const { win, theme } = onHome();
    const first = win.document.getElementById('af-spotlight');
    // Simulate a fresh context over a document that still carries the old
    // panels: forget the handle and build again.
    theme.state().ui.spotlight.remove();
    win.document.body.appendChild(first);
    theme.leaveHome();
    const stale = win.document.querySelectorAll('#af-spotlight').length;
    assert.strictEqual(stale, 1);
});

test('the module survives a document that has no body yet', () => {
    const win = makeThemeWindow();
    win.document.documentElement.removeChild(win.document.body);
    win.document.body = null;
    const theme = loadTheme(win);
    assert.strictEqual(theme.state().space, null);
    assert.strictEqual(theme.state().ui, null);
});

// ---------------------------------------------------------------------------
// guard / tokenMs
// ---------------------------------------------------------------------------

test('guard swallows a throw and logs it to console.debug', () => {
    const win = makeThemeWindow();
    const theme = loadTheme(win);
    const boom = theme.guard(function () { throw new Error('kaboom'); });
    assert.strictEqual(boom(), undefined);
    const debug = win.console.lines.filter((l) => l.level === 'debug');
    assert.ok(debug.some((l) => String(l.args[1]).includes('kaboom')), 'logged');
});

test('guard passes arguments and the return value through', () => {
    const { theme } = onHome();
    assert.strictEqual(theme.guard((a, b) => a + b)(2, 3), 5);
});

test('tokenMs reads ms and s durations off the root element', () => {
    const { win, theme } = onHome();
    const root = win.document.documentElement;
    root.style.setProperty('--af-dur-tile', '250ms');
    assert.strictEqual(theme.tokenMs('--af-dur-tile', 180), 250);
    root.style.setProperty('--af-dur-tile', '0.4s');
    assert.strictEqual(theme.tokenMs('--af-dur-tile', 180), 400);
});

test('tokenMs falls back when the token is missing or unreadable', () => {
    const { theme } = onHome();
    assert.strictEqual(theme.tokenMs('--af-nope', 180), 180);
    assert.strictEqual(theme.tokenMs('--af-nope', 0), 0);
});

// ---------------------------------------------------------------------------
// Stylesheet ordering
// ---------------------------------------------------------------------------

test('keepThemeLast moves the sheet to the end of body when a later sheet appears', () => {
    const win = makeThemeWindow();
    const style = installThemeStyle(win);
    const theme = loadTheme(win);
    assert.strictEqual(style.parentNode, win.document.body, 'moved out of head');

    const chunk = win.document.createElement('link');
    chunk.setAttribute('rel', 'stylesheet');
    win.document.body.appendChild(chunk);
    theme.keepThemeLast();
    const sheets = win.document.querySelectorAll('link[rel="stylesheet"], style');
    assert.strictEqual(sheets[sheets.length - 1], style);
});

test('keepThemeLast leaves the sheet alone when it is already last', () => {
    const win = makeThemeWindow();
    const style = installThemeStyle(win);
    const theme = loadTheme(win);
    const before = win.document.body.childNodes.indexOf(style);
    theme.keepThemeLast();
    assert.strictEqual(win.document.body.childNodes.indexOf(style), before);
});

test('keepThemeLast asks the Rust preamble to install the sheet when it is gone', () => {
    const win = makeThemeWindow();
    let installs = 0;
    // The fake's MutationObserver fires synchronously (the real one is a
    // microtask), so the append below re-enters keepThemeLast; hand back the
    // node already made rather than installing a second one.
    win.__afInstallTheme = () => {
        if (win.__afStyle) return win.__afStyle;
        installs += 1;
        const el = win.document.createElement('style');
        el.id = 'af-theme';
        win.__afStyle = el;
        win.document.head.appendChild(el);
        return el;
    };
    const theme = loadTheme(win);
    assert.ok(installs >= 1, 'called at least once during start()');
    win.document.getElementById('af-theme').remove();
    theme.keepThemeLast();
    assert.strictEqual(win.document.getElementById('af-theme').id, 'af-theme');
});

test('watchStylesheets observes head and body, and a later sheet re-sorts', () => {
    const win = makeThemeWindow();
    const style = installThemeStyle(win);
    loadTheme(win);
    assert.ok(observersFor(win, win.document.head).length >= 1, 'head observed');
    assert.ok(observersFor(win, win.document.body).length >= 1, 'body observed');

    const late = win.document.createElement('style');
    win.document.body.appendChild(late); // fires the body observer
    const sheets = win.document.querySelectorAll('link[rel="stylesheet"], style');
    assert.strictEqual(sheets[sheets.length - 1], style);
});

// ---------------------------------------------------------------------------
// theme-color
// ---------------------------------------------------------------------------

test('pinThemeColor creates the meta tag at the Astrofin base colour', () => {
    const win = makeThemeWindow();
    loadTheme(win);
    const meta = win.document.querySelector('meta[name="theme-color"]');
    assert.strictEqual(meta.getAttribute('content'), '#070A14');
    assert.strictEqual(meta.parentNode, win.document.head);
});

test('pinThemeColor rewrites a theme-color jellyfin-web has changed', () => {
    const win = makeThemeWindow();
    const meta = win.document.createElement('meta');
    meta.setAttribute('name', 'theme-color');
    meta.setAttribute('content', '#00a4dc');
    win.document.head.appendChild(meta);
    const theme = loadTheme(win);
    assert.strictEqual(meta.getAttribute('content'), '#070A14');

    // The observer put on the meta restores it after a later write.
    meta.setAttribute('content', '#ff0000');
    observersFor(win, meta).forEach((obs) => obs._fire([]));
    assert.strictEqual(meta.getAttribute('content'), '#070A14');
    theme.pinThemeColor();
    assert.strictEqual(observersFor(win, meta).length, 1, 'only ever one observer');
});

// ---------------------------------------------------------------------------
// #af-space and the backdrop
// ---------------------------------------------------------------------------

test('ensureSpace builds the starfield once and keeps it first in body', () => {
    const win = makeThemeWindow();
    const theme = loadTheme(win);
    const space = win.document.getElementById('af-space');
    assert.strictEqual(win.document.body.firstChild, space);
    assert.strictEqual(space.getAttribute('aria-hidden'), 'true');
    assert.strictEqual(space.querySelectorAll('.af-backdrop-layer').length, 2);
    assert.strictEqual(space.querySelectorAll('.af-stars').length, 2);
    theme.ensureSpace();
    assert.strictEqual(win.document.querySelectorAll('#af-space').length, 1);
});

test('ensureSpace adopts a space left in the document by a previous context', () => {
    const win = makeThemeWindow();
    loadTheme(win);
    const first = win.document.getElementById('af-space');
    const win2 = makeThemeWindow();
    // Rebuild the same markup in a second window and load into it.
    const clone = win2.document.createElement('div');
    clone.id = 'af-space';
    ['af-backdrop-layer', 'af-backdrop-layer'].forEach((cls) => {
        const layer = win2.document.createElement('div');
        layer.className = cls;
        clone.appendChild(layer);
    });
    win2.document.body.appendChild(clone);
    const theme2 = loadTheme(win2);
    assert.strictEqual(theme2.state().space, clone, 'adopted, not duplicated');
    assert.strictEqual(theme2.state().backdropLayers.length, 2);
    assert.notStrictEqual(clone, first);
});

test('updateVideoMode sets html.af-video while a video container exists', () => {
    const { win, theme } = onHome();
    const container = win.document.createElement('div');
    container.className = 'videoPlayerContainer';
    win.document.body.appendChild(container);
    theme.updateVideoMode();
    assert.ok(win.document.documentElement.classList.contains('af-video'));
    container.remove();
    theme.updateVideoMode();
    assert.ok(!win.document.documentElement.classList.contains('af-video'));
});

test('going into video mode clears the backdrop', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));

    const container = win.document.createElement('div');
    container.className = 'videoPlayerContainer';
    win.document.body.appendChild(container);
    theme.updateVideoMode();
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
});

test('setBackdrop only paints once the image has loaded, and crossfades slots', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/one.jpg');
    const layers = theme.state().backdropLayers;
    assert.ok(!layers[0].classList.contains('af-on'), 'nothing shown before load');
    win.images[0].onload();
    assert.ok(layers[0].classList.contains('af-on'));
    assert.strictEqual(theme.state().backdropSlot, 1);

    theme.setBackdrop('https://server/two.jpg');
    win.images[1].onload();
    assert.ok(layers[1].classList.contains('af-on'));
    assert.ok(!layers[0].classList.contains('af-on'), 'the previous layer faded out');
});

test('setBackdrop ignores a load that a newer request has overtaken', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/slow.jpg');
    theme.setBackdrop('https://server/fast.jpg');
    win.images[1].onload();
    win.images[0].onload(); // the stale one lands second
    const layers = theme.state().backdropLayers;
    assert.match(layers[0].style.backgroundImage, /fast\.jpg/);
    assert.strictEqual(theme.state().currentBackdropUrl, 'https://server/fast.jpg');
});

test('setBackdrop escapes quotes in the url', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/a".jpg');
    win.images[0].onload();
    assert.strictEqual(
        theme.state().backdropLayers[0].style.backgroundImage,
        'url("https://server/a%22.jpg")'
    );
});

test('a backdrop that fails to load clears instead of sticking', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/404.jpg');
    win.images[0].onerror();
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
});

test('setBackdrop(null) and clearBackdrop take the class off the root', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();
    theme.setBackdrop(null);
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
    theme.clearBackdrop();
    assert.ok(!theme.state().backdropLayers.some((l) => l.classList.contains('af-on')));
});

test('setBackdrop does not re-request the url already showing', () => {
    const { win, theme } = onHome();
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();
    theme.setBackdrop('https://server/a.jpg');
    assert.strictEqual(win.images.length, 1);
});

// ---------------------------------------------------------------------------
// Route detection
// ---------------------------------------------------------------------------

test('isHomeRoute reads the hash whenever there is one', () => {
    const { win, theme } = onHome();
    for (const hash of ['#/home.html', '#!/home.html', '#/home', '#/home?tab=1', '#/home/']) {
        win.location.hash = hash;
        assert.ok(theme.isHomeRoute(), hash);
    }
    for (const hash of ['#/details?id=1', '#/homevideos', '#/search']) {
        win.location.hash = hash;
        assert.ok(!theme.isHomeRoute(), hash);
    }
});

test('with no hash isHomeRoute falls back to the DOM probe', () => {
    const win = makeThemeWindow();
    const theme = loadTheme(win);
    assert.ok(!theme.isHomeRoute(), 'nothing rendered yet');
    buildHome(win);
    assert.ok(theme.isHomeRoute());
});

// ---------------------------------------------------------------------------
// Item metadata
// ---------------------------------------------------------------------------

test('isFolderLike is true for the container types and nothing else', () => {
    const { theme } = onHome();
    for (const type of ['CollectionFolder', 'UserView', 'Folder', 'BoxSet', 'Season', 'Playlist']) {
        assert.ok(theme.isFolderLike({ Type: type }), type);
    }
    for (const type of ['Movie', 'Episode', 'Series', 'Audio']) {
        assert.ok(!theme.isFolderLike({ Type: type }), type);
    }
    assert.strictEqual(theme.isFolderLike(null), false);
    assert.strictEqual(theme.isFolderLike({}), false);
});

test('videoModeLabel maps the wire values including the pre-rename spellings', () => {
    const { theme } = onHome();
    assert.strictEqual(theme.videoModeLabel('auto'), 'Auto');
    assert.strictEqual(theme.videoModeLabel('LIVE-ACTION'), 'Live-Action');
    assert.strictEqual(theme.videoModeLabel('movies'), 'Live-Action');
    assert.strictEqual(theme.videoModeLabel('anime'), 'Animation');
    assert.strictEqual(theme.videoModeLabel('off'), 'Off');
    assert.strictEqual(theme.videoModeLabel('something-else'), 'something-else');
    assert.strictEqual(theme.videoModeLabel(null), '');
});

test('minutes rounds ticks and tolerates rubbish', () => {
    const { theme } = onHome();
    assert.strictEqual(theme.minutes(600000000), 1);
    assert.strictEqual(theme.minutes(900000000), 2, 'rounds, not truncates');
    assert.strictEqual(theme.minutes(null), 0);
    assert.strictEqual(theme.minutes('nonsense'), 0);
});

test('formatRuntime writes hours and minutes, and nothing under a minute', () => {
    const { theme } = onHome();
    assert.strictEqual(theme.formatRuntime(600000000 * 95), '1h 35m');
    assert.strictEqual(theme.formatRuntime(600000000 * 42), '42m');
    assert.strictEqual(theme.formatRuntime(600000000 * 120), '2h 0m');
    assert.strictEqual(theme.formatRuntime(0), null);
    assert.strictEqual(theme.formatRuntime(undefined), null);
});

test('videoStream picks the first video stream and skips audio', () => {
    const { theme } = onHome();
    const video = { Type: 'Video', Width: 1920 };
    assert.strictEqual(theme.videoStream({ MediaStreams: [{ Type: 'Audio' }, video] }), video);
    assert.strictEqual(theme.videoStream({ MediaStreams: [] }), null);
    assert.strictEqual(theme.videoStream(null), null);
});

test('resolutionLabel buckets by width', () => {
    const { theme } = onHome();
    assert.strictEqual(theme.resolutionLabel({ Width: 3840 }), '4K');
    assert.strictEqual(theme.resolutionLabel({ Width: 3600 }), '4K');
    assert.strictEqual(theme.resolutionLabel({ Width: 1920 }), '1080p');
    assert.strictEqual(theme.resolutionLabel({ Width: 1280 }), '720p');
    assert.strictEqual(theme.resolutionLabel({ Width: 720 }), 'SD');
    assert.strictEqual(theme.resolutionLabel({ Width: 0 }), null);
    assert.strictEqual(theme.resolutionLabel(null), null);
});

test('chipsFor a folder is a child count and nothing else', () => {
    const { theme } = onHome();
    assert.deepStrictEqual(
        theme.chipsFor({ Type: 'BoxSet', ChildCount: 4, ProductionYear: 1999 }),
        ['4 items']
    );
    assert.deepStrictEqual(theme.chipsFor({ Type: 'Season', ChildCount: 1 }), ['1 item']);
    assert.deepStrictEqual(theme.chipsFor({ Type: 'CollectionFolder' }), []);
});

test('chipsFor an episode leads with the episode name and the S/E code', () => {
    const { theme } = onHome();
    const chips = theme.chipsFor({
        Type: 'Episode',
        Name: 'Pilot',
        ParentIndexNumber: 1,
        IndexNumber: 2,
        ProductionYear: 2011,
        RunTimeTicks: 600000000 * 58,
        MediaStreams: [{ Type: 'Video', Width: 1920, VideoRangeType: 'HDR10' }],
        CommunityRating: 8.75
    });
    assert.deepStrictEqual(chips, ['Pilot', 'S1 · E2', '2011', '58m', '1080p', 'HDR10', '★ 8.8']);
});

test('chipsFor shows remaining time for a resumable item and New for a fresh one', () => {
    const { theme } = onHome();
    const resume = theme.chipsFor({
        Type: 'Movie',
        RunTimeTicks: 600000000 * 100,
        UserData: { PlaybackPositionTicks: 600000000 * 70 },
        DateCreated: new Date().toISOString()
    });
    assert.ok(resume.includes('left 30 m'), resume.join(','));
    assert.ok(!resume.includes('New'), 'resume wins over New');

    const fresh = theme.chipsFor({ Type: 'Movie', DateCreated: new Date().toISOString() });
    assert.deepStrictEqual(fresh, ['New']);

    const old = theme.chipsFor({
        Type: 'Movie',
        DateCreated: new Date(Date.now() - 30 * 864e5).toISOString()
    });
    assert.deepStrictEqual(old, []);
});

test('chipsFor drops an SDR range and normalises the underscored ones', () => {
    const { theme } = onHome();
    const sdr = theme.chipsFor({
        Type: 'Movie', MediaStreams: [{ Type: 'Video', Width: 1920, VideoRange: 'SDR' }]
    });
    assert.deepStrictEqual(sdr, ['1080p']);
    const dv = theme.chipsFor({
        Type: 'Movie',
        MediaStreams: [{ Type: 'Video', Width: 3840, VideoRangeType: 'DOLBY_VISION' }]
    });
    assert.deepStrictEqual(dv, ['4K', 'DOLBY VISION']);
});

test('titleFor prefers the series name for an episode', () => {
    const { theme } = onHome();
    assert.strictEqual(
        theme.titleFor({ Type: 'Episode', Name: 'Pilot', SeriesName: 'The Show' }),
        'The Show'
    );
    assert.strictEqual(theme.titleFor({ Type: 'Movie', Name: 'Arrival' }), 'Arrival');
    assert.strictEqual(theme.titleFor({}), '');
});

test('backdropUrlFor prefers the item backdrop, then the parent, then the poster', () => {
    const { win, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const own = theme.backdropUrlFor({ Id: 'a', BackdropImageTags: ['t1'] });
    assert.match(own, /Items\/a\/Images\/Backdrop\?maxWidth=1280&tag=t1/);

    const parent = theme.backdropUrlFor({
        Id: 'b', ParentBackdropItemId: 'p', ParentBackdropImageTags: ['t2']
    });
    assert.match(parent, /Items\/p\/Images\/Backdrop.*tag=t2/);

    const primary = theme.backdropUrlFor({ Id: 'c', ImageTags: { Primary: 't3' } });
    assert.match(primary, /Items\/c\/Images\/Primary.*tag=t3/);

    assert.strictEqual(theme.backdropUrlFor({ Id: 'd' }), null);
});

test('backdropUrlFor caps the requested width at 1920 and needs an ApiClient', () => {
    const { win, theme } = onHome();
    assert.strictEqual(theme.backdropUrlFor({ Id: 'a', BackdropImageTags: ['t'] }), null);
    win.ApiClient = makeThemeApiClient();
    win.innerWidth = 3840;
    win.devicePixelRatio = 2;
    assert.match(theme.backdropUrlFor({ Id: 'a', BackdropImageTags: ['t'] }), /maxWidth=1920/);
});

test('fetchItem caches, and the cache evicts the oldest past its cap', async () => {
    const { win, theme } = onHome();
    const items = new Map([['x', { Id: 'x', Name: 'X' }]]);
    win.ApiClient = makeThemeApiClient({ items });
    const first = await theme.fetchItem('x');
    assert.strictEqual(first.Name, 'X');
    await theme.fetchItem('x');
    assert.strictEqual(
        win.ApiClient.calls.filter((c) => c[0] === 'getItem').length, 1, 'served from cache'
    );

    for (let i = 0; i < 70; i++) theme.cacheItem('id-' + i, { Id: 'id-' + i });
    assert.strictEqual(theme.state().itemCacheKeys.length, 64);
    assert.strictEqual(theme.state().itemCache['id-0'], undefined, 'oldest evicted');
    assert.ok(theme.state().itemCache['id-69'], 'newest kept');
});

test('fetchItem rejects when there is no ApiClient', async () => {
    const { theme } = onHome();
    await assert.rejects(() => theme.fetchItem('nope'), /no ApiClient/);
});

// ---------------------------------------------------------------------------
// Focus tracking and placement
// ---------------------------------------------------------------------------

test('setFocusedCard moves the af-focused class and ignores a repeat', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const one = makeCard(win, { id: 'one', parent: home.sections[0] });
    const two = makeCard(win, { id: 'two', parent: home.sections[1] });
    theme.setFocusedCard(one);
    assert.ok(one.classList.contains('af-focused'));
    theme.setFocusedCard(two);
    assert.ok(!one.classList.contains('af-focused'));
    assert.ok(two.classList.contains('af-focused'));
    theme.setFocusedCard(two);
    assert.strictEqual(theme.state().focusedCard, two);
    theme.setFocusedCard(null);
    assert.strictEqual(theme.state().focusedCard, null);
    assert.ok(!two.classList.contains('af-focused'));
});

test('focusing a card paints the spotlight and the backdrop from the item', async () => {
    const { win, home, theme } = onHome();
    const item = {
        Id: 'one', Name: 'Arrival', Type: 'Movie', ProductionYear: 2016,
        Overview: 'Linguist meets heptapods.', BackdropImageTags: ['bt']
    };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = makeCard(win, { id: 'one', parent: home.sections[0] });
    theme.setFocusedCard(card);
    await Promise.resolve();
    await Promise.resolve();
    const ui = theme.state().ui;
    assert.strictEqual(ui.title.textContent, 'Arrival');
    assert.deepStrictEqual(childTexts(ui.chips), ['2016']);
    assert.strictEqual(ui.overview.textContent, 'Linguist meets heptapods.');
    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
});

test('a card with no data-id, or off Home, never reaches the panel', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const bare = makeCard(win, { id: null, parent: home.sections[0] });
    theme.setFocusedCard(bare);
    assert.strictEqual(win.ApiClient.calls.length, 0);

    win.location.hash = '#/details?id=1';
    const card = makeCard(win, { id: 'one', parent: home.sections[0] });
    theme.setFocusedCard(card);
    assert.strictEqual(win.ApiClient.calls.length, 0, 'off Home the fetch is skipped');
    assert.ok(card.classList.contains('af-focused'), 'the class still tracks focus');
});

test('homeSectionsContainer prefers the rails container and falls back to the tab', () => {
    const { win, home, theme } = onHome();
    assert.strictEqual(theme.homeSectionsContainer(), home.container);
    home.container.remove();
    assert.strictEqual(theme.homeSectionsContainer(), home.homeTab);
});

test('sectionOf finds the rail a card sits in', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[1] });
    assert.strictEqual(theme.sectionOf(card), home.sections[1]);
    assert.strictEqual(theme.sectionOf(null), null);
    assert.strictEqual(theme.sectionOf(win.document.body), null);
});

test('defaultSection is the first rail that actually shows cards', () => {
    const { win, home, theme } = onHome();
    home.sections[0].classList.add('hide');
    makeCard(win, { parent: home.sections[1] });
    assert.strictEqual(theme.defaultSection(), home.sections[1]);
});

test('defaultSection falls back to the first rail when none has cards', () => {
    const { home, theme } = onHome();
    assert.strictEqual(theme.defaultSection(), home.sections[0]);
});

test('placeSpotlight inserts the panel after the focused rail', () => {
    const { win, home, theme } = onHome();
    theme.placeSpotlight(home.sections[1]);
    const spotlight = theme.state().ui.spotlight;
    assert.strictEqual(home.sections[1].nextElementSibling, spotlight);
    assert.strictEqual(theme.state().pointerMovedSincePlace, false, 'hover is armed off');

    // Placing it where it already is must not move anything.
    const before = home.container.childNodes.slice();
    theme.placeSpotlight(home.sections[1]);
    assert.deepStrictEqual(home.container.childNodes, before);
});

test('placeSpotlight parks the panel in body when there are no rails', () => {
    const win = makeThemeWindow({ hash: '#/home.html' });
    const theme = loadTheme(win);
    theme.ensureUi();
    theme.placeSpotlight(null);
    assert.strictEqual(theme.state().ui.spotlight.parentNode, win.document.body);
});

// ---------------------------------------------------------------------------
// Rendering the panel
// ---------------------------------------------------------------------------

test('writeSpotlight labels a resumable item Resume and a fresh one Play', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'Movie', Name: 'A' }, card);
    const ui = theme.state().ui;
    assert.strictEqual(ui.playLabel.textContent, 'Play');
    assert.strictEqual(ui.glyph.hidden, false);
    assert.strictEqual(ui.details.hidden, false);

    theme.writeSpotlight(
        { Type: 'Movie', Name: 'B', UserData: { PlaybackPositionTicks: 5 } }, card
    );
    assert.strictEqual(ui.playLabel.textContent, 'Resume');
});

test('writeSpotlight turns a folder into a Browse-only panel', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'BoxSet', Name: 'Trilogy', Overview: 'ignored' }, card);
    const ui = theme.state().ui;
    assert.strictEqual(ui.playLabel.textContent, 'Browse');
    assert.strictEqual(ui.glyph.hidden, true);
    assert.strictEqual(ui.details.hidden, true);
    assert.strictEqual(ui.overview.textContent, '');
    assert.strictEqual(ui.overview.hidden, true);
});

test('writeSpotlight replaces the chip strip rather than appending to it', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'Movie', ProductionYear: 1999 }, card);
    theme.writeSpotlight({ Type: 'Movie', ProductionYear: 2001 }, card);
    assert.deepStrictEqual(childTexts(theme.state().ui.chips), ['2001']);
});

test('renderSpotlight paints the first item at once and fades the next one in', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.renderSpotlight({ Type: 'Movie', Name: 'First' }, card);
    const ui = theme.state().ui;
    assert.strictEqual(ui.title.textContent, 'First');
    assert.ok(!ui.body.classList.contains('af-sp-swap'));

    theme.renderSpotlight({ Type: 'Movie', Name: 'Second' }, card);
    assert.strictEqual(ui.title.textContent, 'First', 'still the old item during the fade');
    assert.ok(ui.body.classList.contains('af-sp-swap'));
    win.timers.advance(200);
    assert.strictEqual(ui.title.textContent, 'Second');
    assert.ok(!ui.body.classList.contains('af-sp-swap'));
    assert.strictEqual(theme.state().swapTimer, 0);
});

test('a hover during the fade writes the newest item, not the one that started it', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.renderSpotlight({ Type: 'Movie', Name: 'First' }, card);
    theme.renderSpotlight({ Type: 'Movie', Name: 'Second' }, card);
    theme.renderSpotlight({ Type: 'Movie', Name: 'Third' }, card);
    win.timers.advance(200);
    assert.strictEqual(theme.state().ui.title.textContent, 'Third');
    assert.strictEqual(win.timers.pendingCount, 0, 'one timer, not one per hover');
});

test('renderServerPanel prints the server name and the mode rows it can source', () => {
    const { win, theme } = onHome();
    win.ApiClient = makeThemeApiClient({ serverName: 'Nebula' });
    win.jmpInfo = { settings: { playback: { videoMode: 'anime', hwdec: 'd3d11va' } } };
    theme.renderServerPanel();
    const panel = theme.state().ui.server;
    assert.deepStrictEqual(childTexts(panel).slice(0, 2), ['Server', 'Nebula']);
    assert.match(panel.textContent, /ModeAnimation/);
    assert.match(panel.textContent, /Decoded3d11va/);

    // Rendering again replaces the rows instead of stacking them.
    theme.renderServerPanel();
    assert.strictEqual(panel.textContent.match(/Nebula/g).length, 1);
});

test('renderServerPanel falls back to serverInfo and hides with no name at all', () => {
    const { win, theme } = onHome();
    win.ApiClient = makeThemeApiClient({
        serverName() { throw new Error('not connected'); },
        serverInfo: { Name: 'From info' }
    });
    theme.renderServerPanel();
    assert.match(theme.state().ui.server.textContent, /From info/);

    win.ApiClient = makeThemeApiClient({});
    theme.renderServerPanel();
    assert.strictEqual(theme.state().ui.server.hidden, true);
});

// ---------------------------------------------------------------------------
// The two buttons
// ---------------------------------------------------------------------------

test('primaryCardButton finds resume, play and the primary fab', () => {
    const { win, home, theme } = onHome();
    const resume = makeCard(win, { action: 'resume', parent: home.sections[0] });
    assert.strictEqual(
        theme.primaryCardButton(resume), resume.querySelector('[data-action="resume"]')
    );
    const play = makeCard(win, { action: 'play', parent: home.sections[0] });
    assert.ok(theme.primaryCardButton(play));
    const plain = makeCard(win, { parent: home.sections[0] });
    assert.strictEqual(theme.primaryCardButton(plain), null);
});

test('cardLinkTarget finds the delegated link node', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { link: true, parent: home.sections[0] });
    assert.strictEqual(theme.cardLinkTarget(card), card.querySelector('.cardImageContainer'));
    assert.strictEqual(theme.cardLinkTarget(makeCard(win, { parent: home.sections[0] })), null);
});

test('clickSyntheticAction clicks an itemAction inside the card and removes it', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    const seen = [];
    home.container.addEventListener('click', (e) => {
        seen.push([e.target.getAttribute('data-action'), e.target.parentNode === card]);
    });
    theme.clickSyntheticAction(card, 'resume');
    assert.deepStrictEqual(seen, [['resume', true]], 'still attached while it dispatches');
    assert.strictEqual(card.querySelector('.itemAction'), null, 'removed synchronously');
});

test('Play clicks the card button for a normal item and the link for a folder', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { action: 'play', link: true, parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'Movie', Id: 'a' }, card);
    theme.onPlayClick();
    assert.deepStrictEqual(card.clicks, [['overlay', 'play']]);

    const folder = makeCard(win, { action: 'play', link: true, parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'CollectionFolder', Id: 'lib' }, folder);
    theme.onPlayClick();
    assert.deepStrictEqual(folder.clicks, [['link']], 'a library is browsed, not played');
});

test('Play falls back to a synthetic resume/play action when the card has no button', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    const seen = [];
    home.container.addEventListener('click', (e) => seen.push(e.target.getAttribute('data-action')));
    theme.writeSpotlight({ Type: 'Movie', UserData: { PlaybackPositionTicks: 10 } }, card);
    theme.onPlayClick();
    theme.writeSpotlight({ Type: 'Movie' }, card);
    theme.onPlayClick();
    assert.deepStrictEqual(seen, ['resume', 'play']);
});

test('Play acts on the item the panel is showing, not on a later hover', () => {
    const { win, home, theme } = onHome();
    const shown = makeCard(win, { id: 'shown', action: 'play', parent: home.sections[0] });
    const hovered = makeCard(win, { id: 'hovered', action: 'play', parent: home.sections[1] });
    win.ApiClient = makeThemeApiClient();
    theme.writeSpotlight({ Type: 'Movie', Id: 'shown' }, shown);
    theme.setFocusedCard(hovered); // the fetch for it has not resolved
    theme.onPlayClick();
    assert.deepStrictEqual(shown.clicks, [['overlay', 'play']]);
    assert.deepStrictEqual(hovered.clicks, []);
});

test('Play does nothing once the card has left the document', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { action: 'play', parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'Movie' }, card);
    card.remove();
    theme.onPlayClick();
    assert.deepStrictEqual(card.clicks, []);
});

test('navigateToDetails builds the hash from the item and the card server id', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { id: 'a b', serverId: 'srv 1', parent: home.sections[0] });
    theme.navigateToDetails(card, { Id: 'a b', ServerId: 'ignored' });
    assert.strictEqual(win.location.hash, '#/details?id=a%20b&serverId=srv%201');
});

test('navigateToDetails falls back to the item, then to the ApiClient, then omits the server', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { id: 'x', parent: home.sections[0] });
    theme.navigateToDetails(card, { Id: 'x', ServerId: 'from-item' });
    assert.match(win.location.hash, /serverId=from-item$/);

    win.ApiClient = makeThemeApiClient({ serverId: 'from-client' });
    theme.navigateToDetails(card, { Id: 'x' });
    assert.match(win.location.hash, /serverId=from-client$/);

    win.ApiClient = null;
    theme.navigateToDetails(card, { Id: 'x' });
    assert.strictEqual(win.location.hash, '#/details?id=x');

    win.location.hash = '#/home.html';
    theme.navigateToDetails(null, null);
    assert.strictEqual(win.location.hash, '#/home.html', 'no id, no navigation');
});

test('Details navigates to the shown item', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { id: 'shown', parent: home.sections[0] });
    theme.writeSpotlight({ Type: 'Movie', Id: 'shown' }, card);
    theme.onDetailsClick();
    assert.strictEqual(win.location.hash, '#/details?id=shown');
});

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

test('cardFrom only claims cards inside the home rails', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    assert.strictEqual(theme.cardFrom(card.querySelector('.cardScalable')), card);

    const stray = makeCard(win, { parent: win.document.body });
    assert.strictEqual(theme.cardFrom(stray), null);
    assert.strictEqual(theme.cardFrom(null), null);
    assert.strictEqual(theme.cardFrom(win.document.body), null);
});

test('focusin selects the card immediately and cancels a pending hover', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const hovered = makeCard(win, { id: 'hovered', parent: home.sections[0] });
    const focused = makeCard(win, { id: 'focused', parent: home.sections[1] });
    theme.onPointerMove(); // the panel has been placed; arm hover again
    theme.onPointerOver({ target: hovered });
    assert.ok(theme.state().hoverTimer, 'hover is pending');
    theme.onFocusIn({ target: focused });
    assert.strictEqual(theme.state().focusedCard, focused);
    win.timers.advance(500);
    assert.strictEqual(theme.state().focusedCard, focused, 'the hover was cancelled');
});

test('hover is debounced and ignored until the pointer has moved after a placement', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.placeSpotlight(home.sections[0]); // arms the guard
    theme.onPointerOver({ target: card });
    assert.strictEqual(theme.state().hoverTimer, 0, 'reflow-induced hover ignored');

    theme.onPointerMove();
    assert.strictEqual(theme.state().pointerMovedSincePlace, true);
    theme.onPointerOver({ target: card });
    assert.ok(theme.state().hoverTimer, 'now it debounces');
    assert.strictEqual(theme.state().focusedCard, null, 'not yet');
    win.timers.advance(120);
    assert.strictEqual(theme.state().focusedCard, card);
});

test('hovering the card that is already focused does nothing', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.onPointerMove();
    theme.setFocusedCard(card);
    theme.onPointerOver({ target: card });
    assert.strictEqual(theme.state().hoverTimer, 0);
});

// ---------------------------------------------------------------------------
// Visibility, refresh and the page observer
// ---------------------------------------------------------------------------

test('ensureUi rebuilds the panels when jellyfin-web has orphaned them', () => {
    const { theme } = onHome();
    const first = theme.state().ui;
    theme.ensureUi();
    assert.strictEqual(theme.state().ui, first, 'still connected: kept');
    first.spotlight.remove();
    theme.ensureUi();
    assert.notStrictEqual(theme.state().ui, first, 'orphaned: rebuilt');
});

test('showOverlays shows the three panels on Home and hides them elsewhere', () => {
    const { win, theme } = onHome();
    theme.showOverlays(true);
    const ui = theme.state().ui;
    [ui.spotlight, ui.server, ui.hint].forEach((el) => {
        assert.strictEqual(el.hidden, false);
        assert.ok(el.classList.contains('af-show'));
    });
    win.location.hash = '#/details?id=1';
    theme.showOverlays();
    [ui.spotlight, ui.server, ui.hint].forEach((el) => {
        assert.strictEqual(el.hidden, true);
        assert.ok(!el.classList.contains('af-show'));
    });
});

test('leaveHome drops the selection, the backdrop and the pending swap', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    theme.renderSpotlight({ Type: 'Movie', Name: 'A' }, card);
    theme.renderSpotlight({ Type: 'Movie', Name: 'B' }, card);
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();

    theme.leaveHome();
    const state = theme.state();
    assert.strictEqual(state.focusedCard, null);
    assert.strictEqual(state.shownItem, null);
    assert.strictEqual(state.swapTimer, 0);
    assert.strictEqual(state.spotlightPainted, false);
    assert.strictEqual(state.currentBackdropUrl, null);
    assert.strictEqual(state.overlaysWanted, false);
    assert.ok(!card.classList.contains('af-focused'));
    win.timers.advance(500);
    assert.strictEqual(
        theme.state().ui.title.textContent, 'A', 'the cleared swap never wrote B'
    );
});

test('decorateCards mirrors the item type onto the card and its tile, once', () => {
    const { win, home, theme } = onHome();
    const movie = makeCard(win, { type: 'Movie', parent: home.sections[0] });
    const other = makeCard(win, { type: 'MusicAlbum', parent: home.sections[0] });
    theme.decorateCards();
    assert.strictEqual(movie.getAttribute('data-af-kind'), 'Movie');
    assert.strictEqual(movie.querySelector('.cardScalable').getAttribute('data-af-kind'), 'Movie');
    assert.strictEqual(other.getAttribute('data-af-kind'), null);
    assert.strictEqual(other.getAttribute('data-af-scanned'), '1', 'scanned, just not labelled');

    movie.removeAttribute('data-af-kind');
    theme.decorateCards();
    assert.strictEqual(movie.getAttribute('data-af-kind'), null, 'a scanned card is skipped');
});

test('refresh on Home marks the root and paints the panels', () => {
    const { win, theme } = onHome();
    win.ApiClient = makeThemeApiClient({ serverName: 'Nebula' });
    theme.refresh();
    assert.ok(win.document.documentElement.classList.contains('af-home'));
    assert.ok(theme.state().ui, 'panels exist');
    assert.match(theme.state().ui.server.textContent, /Nebula/);
});

test('refresh off Home clears af-home and tears the selection down', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    win.location.hash = '#/details?id=1';
    theme.refresh();
    assert.ok(!win.document.documentElement.classList.contains('af-home'));
    assert.strictEqual(theme.state().focusedCard, null);
    assert.strictEqual(theme.state().ui.spotlight.hidden, true);
});

test('refresh repaints the panel for the card that is still focused', () => {
    const { win, home, theme } = onHome();
    const item = { Id: 'one', Type: 'Movie', Name: 'Arrival' };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = makeCard(win, { id: 'one', parent: home.sections[0] });
    return theme.fetchItem('one').then(() => {
        theme.setFocusedCard(card);
        return Promise.resolve().then(() => {
            theme.state().ui.spotlight.remove(); // jellyfin-web rebuilt #homeTab
            theme.refresh();
            assert.strictEqual(theme.state().ui.title.textContent, 'Arrival');
            assert.strictEqual(theme.state().ui.spotlight.hidden, false);
        });
    });
});

test('watchPages attaches once and a rail mutation queues one refresh', () => {
    const { win, home, theme } = onHome();
    assert.strictEqual(theme.state().pagesObserved, true);
    const before = observersFor(win, home.pages).length;
    theme.watchPages();
    assert.strictEqual(observersFor(win, home.pages).length, before, 'not attached twice');

    makeCard(win, { parent: home.sections[0] }); // childList mutation in the subtree
    assert.strictEqual(theme.state().refreshQueued, true);
    win.timers.runAll();
    assert.strictEqual(theme.state().refreshQueued, false);
});

test('the panel repainting itself does not queue a refresh', () => {
    const { win, home, theme } = onHome();
    theme.placeSpotlight(home.sections[0]);
    win.timers.runAll();
    assert.strictEqual(theme.state().refreshQueued, false);
    theme.state().ui.chips.appendChild(win.document.createElement('div'));
    assert.strictEqual(theme.state().refreshQueued, false, 'own subtree is ignored');
});

test('queueRefresh coalesces a burst into a single refresh', () => {
    const { win, theme } = onHome();
    theme.queueRefresh();
    theme.queueRefresh();
    theme.queueRefresh();
    assert.strictEqual(win.timers.pendingCount, 1);
    win.timers.runAll();
    assert.strictEqual(theme.state().refreshQueued, false);
});

test('start defers to DOMContentLoaded while the document is still loading', () => {
    const win = makeThemeWindow();
    win.document.readyState = 'loading';
    const theme = loadTheme(win);
    assert.strictEqual(win.document.getElementById('af-space'), null, 'nothing built yet');
    const ready = win.document.listeners.filter((l) => l.type === 'DOMContentLoaded');
    assert.strictEqual(ready.length, 1);
    ready[0].fn({ type: 'DOMContentLoaded' });
    assert.ok(win.document.getElementById('af-space'), 'built on DOMContentLoaded');
    assert.strictEqual(typeof theme.start, 'function');
});

test('hashchange, popstate, viewshow and the jellyfin-web buses all queue a refresh', () => {
    const { win, theme } = onHome();
    const cases = [
        () => win.dispatchEvent({ type: 'hashchange' }),
        () => win.dispatchEvent({ type: 'popstate' }),
        () => win.document.dispatchEvent({ type: 'viewshow' }),
        () => win.document._callbacks.HISTORY_UPDATE.forEach((fn) => fn()),
        () => win.document._callbacks.THEME_CHANGE.forEach((fn) => fn())
    ];
    for (const fire of cases) {
        win.timers.runAll();
        fire();
        win.timers.runAll();
    }
    assert.strictEqual(theme.state().refreshQueued, false, 'every bus settled');
    assert.ok(win.document._callbacks.THEME_CHANGE.length >= 1);
});
