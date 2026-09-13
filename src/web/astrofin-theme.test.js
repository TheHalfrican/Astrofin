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
// Everything else here is the logic the popout is made of: the chip strip, the
// backdrop crossfade, the Home route test, the viewport geometry that decides
// where a body-level fixed panel lands over the card it stands in for, and the
// two buttons, which must act on the item the popout is *showing* rather than
// on whatever the pointer has since moved to.
//
// The fake DOM comes from src/web/test/player-fakes.js; the extra bits the
// theme needs (real textContent semantics, document fragments, bubbling
// clicks, cloneNode, a box model, a Home page) from src/web/test/theme-fakes.js.
const test = require('node:test');
const assert = require('node:assert');

const { loadModule } = require('./test/player-fakes.js');
const {
    makeThemeWindow, loadTheme, installThemeStyle, setRect, buildHome, makeCard,
    makeThemeApiClient, observersFor, childTexts
} = require('./test/theme-fakes.js');

// A window with the theme installed and Home already in the DOM.
function onHome(overrides) {
    const win = makeThemeWindow(Object.assign({ hash: '#/home.html' }, overrides));
    const home = buildHome(win);
    const theme = loadTheme(win);
    return { win, home, theme };
}

// The movies library as jf-web 10.11.11 renders it:
//   .mainAnimatedPages
//     > #moviesPage.page.libraryPage.backdropPage.pageWithAbsoluteTabs
//       > .pageTabContent#moviesTab
//         > .itemsContainer.vertical-wrap.padded-left.padded-right
// Built here rather than in theme-fakes.js because the grid is the only thing
// in the theme that reads it, and it is four nested divs.
function buildLibrary(win) {
    const doc = win.document;
    const pages = doc.createElement('div');
    pages.className = 'mainAnimatedPages';
    const page = doc.createElement('div');
    page.id = 'moviesPage';
    page.className = 'page libraryPage backdropPage pageWithAbsoluteTabs';
    const tab = doc.createElement('div');
    tab.id = 'moviesTab';
    tab.className = 'pageTabContent';
    const grid = doc.createElement('div');
    grid.className = 'itemsContainer vertical-wrap padded-left padded-right';
    tab.appendChild(grid);
    page.appendChild(tab);
    pages.appendChild(page);
    doc.body.appendChild(pages);
    return { pages, page, tab, grid };
}

// A window with the theme installed and the movies library in the DOM.
function onLibrary(overrides) {
    const win = makeThemeWindow(Object.assign(
        { hash: '#/movies.html?topParentId=lib1' }, overrides
    ));
    const library = buildLibrary(win);
    const theme = loadTheme(win);
    return { win, library, theme };
}

// The item detail page as jf-web 10.11.11 renders it, trimmed to the parts the
// theme reaches:
//   .mainAnimatedPages
//     > #itemDetailPage.page.libraryPage.itemDetailPage.selfBackdropPage
//       > .detailLogo
//       > .detailPageWrapperContainer
//         > .detailPagePrimaryContainer > .detailRibbon > .mainDetailButtons
//         > .detailPageSecondaryContainer > .detailPageContent
// The buttons are `button.button-flat.btnPlay.detailButton` with the label in
// `title` and no text element, which is why the sheet draws it with attr().
function buildDetail(win) {
    const doc = win.document;
    const pages = doc.createElement('div');
    pages.className = 'mainAnimatedPages';
    const page = doc.createElement('div');
    page.id = 'itemDetailPage';
    page.className = 'page libraryPage itemDetailPage selfBackdropPage';
    const logo = doc.createElement('div');
    logo.className = 'detailLogo hide';
    const wrapper = doc.createElement('div');
    wrapper.className = 'detailPageWrapperContainer';
    const primary = doc.createElement('div');
    primary.className = 'detailPagePrimaryContainer';
    const ribbon = doc.createElement('div');
    ribbon.className = 'detailRibbon padded-left padded-right';
    const buttons = doc.createElement('div');
    buttons.className = 'mainDetailButtons focuscontainer-x';
    const play = doc.createElement('button');
    play.className = 'button-flat btnPlay detailButton';
    play.setAttribute('data-action', 'resume');
    play.setAttribute('title', 'Resume');
    buttons.appendChild(play);
    ribbon.appendChild(buttons);
    primary.appendChild(ribbon);
    const secondary = doc.createElement('div');
    secondary.className = 'detailPageSecondaryContainer padded-bottom-page';
    const content = doc.createElement('div');
    content.className = 'detailPageContent';
    secondary.appendChild(content);
    wrapper.appendChild(primary);
    wrapper.appendChild(secondary);
    page.appendChild(logo);
    page.appendChild(wrapper);
    pages.appendChild(page);
    doc.body.appendChild(pages);
    return { pages, page, logo, wrapper, primary, ribbon, buttons, play, secondary, content };
}

// A window with the theme installed and a details route in the address bar.
// `api` is attached before the theme loads: start() calls refresh(), which is
// what kicks the item fetch off, and a client attached afterwards would arrive
// one route too late.
function onDetail(opts = {}) {
    const win = makeThemeWindow({
        hash: opts.hash === undefined ? '#/details?id=item-1&serverId=srv' : opts.hash
    });
    if (opts.api) win.ApiClient = opts.api;
    const detail = buildDetail(win);
    const theme = loadTheme(win);
    return { win, detail, theme };
}

// Let a fetchItem() chain and the render that follows it settle.
async function settle(times = 6) {
    for (let i = 0; i < times; i += 1) await Promise.resolve();
}

// A movie with everything the facts panel can read.
function fullMovie() {
    return {
        Id: 'item-1',
        Name: 'Blade Runner 2049',
        Type: 'Movie',
        RunTimeTicks: 98640000000,
        UserData: { PlaybackPositionTicks: 43200000000 },
        BackdropImageTags: ['bt'],
        MediaSources: [{
            Size: 41017541427,
            MediaStreams: [
                {
                    Type: 'Video', Codec: 'hevc', Width: 3840,
                    VideoRangeType: 'HDR10'
                },
                { Type: 'Audio', Codec: 'truehd', ChannelLayout: '7.1' },
                { Type: 'Subtitle', Language: 'eng' },
                { Type: 'Subtitle', Language: 'fra' },
                { Type: 'Subtitle', Language: 'jpn' },
                { Type: 'Subtitle', Language: 'deu' },
                { Type: 'Subtitle', Language: 'eng' }
            ]
        }]
    };
}

// A card whose tile has a measured box, which is the only kind the popout can
// anchor to: placePopout() gives up on anything reading 0x0. `box` is the
// tile's viewport rect; the default sits well inside the 1280x720 viewport
// theme-fakes.js gives every window.
function popCard(win, opts = {}, box = {}) {
    const card = makeCard(win, opts);
    setRect(
        card.querySelector('.cardScalable'),
        Object.assign({ left: 100, top: 200, width: 200, height: 300 }, box)
    );
    return card;
}

// A .skinHeader pinned over the top of the viewport, `height` tall — the one
// kind placePopout() has to clamp below.
function pinHeader(win, height) {
    const header = win.document.createElement('div');
    header.className = 'skinHeader';
    win.document.body.appendChild(header);
    setRect(header, { left: 0, top: 0, width: 1280, height });
    return header;
}

// The rows a rendered panel reads as, as [label, value] pairs.
function panelRows(panel) {
    return panel.children
        .filter((el) => el.classList.contains('af-dp-row'))
        .map((row) => row.children.map((span) => span.textContent));
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
    assert.strictEqual(win.document.querySelectorAll('#af-popout').length, 1);
});

test('a third and fourth run still leave exactly one of every panel', () => {
    const { win } = onHome();
    loadModule('astrofin-theme.js', win);
    loadModule('astrofin-theme.js', win);
    for (const id of ['af-space', 'af-popout', 'af-hint']) {
        assert.strictEqual(win.document.querySelectorAll('#' + id).length, 1, id);
    }
});

test('buildUi builds the popout as a hidden body-level portal', () => {
    const { win, theme } = onHome();
    const ui = theme.state().ui;
    assert.strictEqual(win.document.getElementById('af-popout'), ui.popout);
    assert.strictEqual(win.document.getElementById('af-spotlight'), null, 'the band is gone');
    // A child of the card would be clipped by the first rail ancestor that
    // stopped being overflow:visible, which is the whole reason for the portal.
    assert.strictEqual(ui.popout.parentNode, win.document.body);
    assert.strictEqual(ui.popout.hidden, true);
    assert.ok(ui.popout.contains(ui.art));
    assert.ok(ui.popout.contains(ui.drawer));
    assert.ok(ui.actions.contains(ui.play));
    assert.ok(ui.actions.contains(ui.details));
    // The drawer's four tiers, in reading order under the buttons. The order
    // is load-bearing: the title has to come before the facts that qualify it.
    assert.deepStrictEqual(
        ui.drawer.children,
        [ui.actions, ui.title, ui.badges, ui.meta, ui.genres]
    );
    assert.deepStrictEqual(
        ui.drawer.children.map((el) => el.className),
        ['af-po-actions', 'af-po-title', 'af-po-badges', 'af-po-meta', 'af-po-genres']
    );
    assert.strictEqual(ui.chips, undefined, 'the single chip strip is gone');
});

test('buildUi drops panels a previous execution left behind', () => {
    const { win, theme } = onHome();
    const stale = theme.state().ui.popout;
    // A fresh V8 context over a document jellyfin-web never tore down: the
    // handle is gone but the nodes are still there. Orphaning one panel is
    // what makes ensureUi() forget the handle and build again.
    theme.state().ui.hint.remove();
    theme.ensureUi();
    assert.strictEqual(win.document.querySelectorAll('#af-popout').length, 1);
    assert.notStrictEqual(theme.state().ui.popout, stale, 'rebuilt, not shadowed');
    assert.strictEqual(stale.isConnected, false, 'the old one was dropped');
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

test('going into video mode clears the backdrop and closes the popout', () => {
    const { win, home, theme } = onHome();
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
    const card = popCard(win, { parent: home.sections[0] });
    theme.renderPopout({ Type: 'Movie', Name: 'Arrival' }, card);

    const container = win.document.createElement('div');
    container.className = 'videoPlayerContainer';
    win.document.body.appendChild(container);
    theme.updateVideoMode();
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
    // The sheet hides it either way; leaving it up in state means it comes
    // back over the card the pointer left behind when playback ends.
    assert.strictEqual(theme.state().poppedCard, null);
    assert.ok(!theme.state().ui.popout.classList.contains('af-show'));
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

test('isLibraryRoute claims the four grid views and nothing that merely starts like them', () => {
    const { win, theme } = onLibrary();
    const yes = [
        '#/movies.html?topParentId=lib1', '#/tv.html', '#/music.html',
        '#/list.html?parentId=7&serverId=s', '#!/movies', '#/tv/'
    ];
    for (const hash of yes) {
        win.location.hash = hash;
        assert.ok(theme.isLibraryRoute(), hash);
    }
    const no = ['#/home.html', '#/details?id=1', '#/movieslibrary', '#/tvguide', '#/search'];
    for (const hash of no) {
        win.location.hash = hash;
        assert.ok(!theme.isLibraryRoute(), hash);
    }
});

test('with no hash isLibraryRoute probes the DOM and never mistakes Home for a library', () => {
    const win = makeThemeWindow();
    const theme = loadTheme(win);
    assert.ok(!theme.isLibraryRoute(), 'nothing rendered yet');

    // jf-web 10.11.11 puts BOTH classes on Home, so a bare .libraryPage probe
    // would call Home a library.
    const home = win.document.createElement('div');
    home.className = 'page libraryPage homePage';
    win.document.body.appendChild(home);
    assert.ok(!theme.isLibraryRoute(), 'Home carries .libraryPage too');

    home.remove();
    buildLibrary(win);
    assert.ok(theme.isLibraryRoute());
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

// The drawer's three fact tiers. One .af-chip strip fitted about three facts
// at a poster's width and clipped the rest, so the boxed facts, the dense
// dot-joined line and the genres are now three separate functions with three
// separate budgets.

test('badgesFor boxes the certification and the picture format only', () => {
    const { theme } = onHome();
    assert.deepStrictEqual(
        theme.badgesFor({
            Type: 'Movie',
            OfficialRating: 'PG-13',
            ProductionYear: 2016,
            MediaStreams: [{ Type: 'Video', Width: 3840, VideoRangeType: 'HDR10' }]
        }),
        ['PG-13', '4K', 'HDR10'],
        'the year and everything else belongs to the meta line'
    );
    assert.deepStrictEqual(theme.badgesFor({ Type: 'Movie' }), []);
    assert.deepStrictEqual(theme.badgesFor(null), []);
});

test('badgesFor drops an SDR range and normalises the underscored ones', () => {
    const { theme } = onHome();
    assert.deepStrictEqual(
        theme.badgesFor({
            Type: 'Movie', MediaStreams: [{ Type: 'Video', Width: 1920, VideoRange: 'SDR' }]
        }),
        ['1080p'],
        'SDR is the norm, so it says nothing'
    );
    assert.deepStrictEqual(
        theme.badgesFor({
            Type: 'Movie',
            MediaStreams: [{ Type: 'Video', Width: 3840, VideoRangeType: 'DOLBY_VISION' }]
        }),
        ['4K', 'DOLBY VISION']
    );
});

test('badgesFor is empty for a folder, which has no file to describe', () => {
    const { theme } = onHome();
    for (const Type of ['BoxSet', 'Season', 'CollectionFolder', 'Playlist']) {
        assert.deepStrictEqual(
            theme.badgesFor({
                Type,
                OfficialRating: 'TV-14',
                MediaStreams: [{ Type: 'Video', Width: 3840, VideoRangeType: 'HDR10' }]
            }),
            [],
            Type
        );
    }
});

test('metaFor a folder is a child count and nothing else', () => {
    const { theme } = onHome();
    assert.deepStrictEqual(
        theme.metaFor({
            Type: 'BoxSet', ChildCount: 4, ProductionYear: 1999, CommunityRating: 7.2
        }),
        ['4 items'],
        'a container has no year, runtime or rating worth showing'
    );
    assert.deepStrictEqual(theme.metaFor({ Type: 'Season', ChildCount: 1 }), ['1 item']);
    assert.deepStrictEqual(theme.metaFor({ Type: 'CollectionFolder' }), []);
    assert.deepStrictEqual(theme.metaFor(null), []);
});

test('metaFor an episode leads with the S/E code and its own name', () => {
    const { theme } = onHome();
    // titleFor() puts the series on the title line, so the episode's own name
    // has nowhere else to go.
    assert.deepStrictEqual(
        theme.metaFor({
            Type: 'Episode',
            Name: 'Pilot',
            SeriesName: 'The Show',
            ParentIndexNumber: 1,
            IndexNumber: 4,
            ProductionYear: 2011,
            RunTimeTicks: 600000000 * 58,
            CommunityRating: 8.75
        }),
        ['S1 E4', 'Pilot', '2011', '58m', '★ 8.8']
    );
    // A special with no numbering keeps the name and loses the code.
    assert.deepStrictEqual(
        theme.metaFor({ Type: 'Episode', Name: 'Recap', ParentIndexNumber: 0 }),
        ['Recap']
    );
});

test('metaFor a series counts its seasons and a movie does not', () => {
    const { theme } = onHome();
    assert.deepStrictEqual(
        theme.metaFor({ Type: 'Series', Name: 'The Show', ChildCount: 5, ProductionYear: 2011 }),
        ['5 seasons', '2011']
    );
    assert.deepStrictEqual(
        theme.metaFor({ Type: 'Series', Name: 'The Show', ChildCount: 1 }),
        ['1 season']
    );
    assert.deepStrictEqual(
        theme.metaFor({ Type: 'Series', Name: 'The Show', ProductionYear: 2011 }),
        ['2011'],
        'no ChildCount, no count'
    );
    assert.deepStrictEqual(
        theme.metaFor({
            Type: 'Movie',
            Name: 'Arrival',
            ChildCount: 3,
            ProductionYear: 2016,
            RunTimeTicks: 600000000 * 116,
            CommunityRating: 7.9
        }),
        ['2016', '1h 56m', '★ 7.9'],
        'a movie never counts children'
    );
});

test('metaFor shows remaining time for a resumable item and New for a fresh one', () => {
    const { theme } = onHome();
    const resume = theme.metaFor({
        Type: 'Movie',
        RunTimeTicks: 600000000 * 100,
        UserData: { PlaybackPositionTicks: 600000000 * 70 },
        DateCreated: new Date().toISOString()
    });
    assert.ok(resume.includes('30m left'), resume.join(','));
    assert.ok(!resume.includes('New'), 'resume wins over New');

    assert.deepStrictEqual(
        theme.metaFor({ Type: 'Movie', DateCreated: new Date().toISOString() }),
        ['New']
    );
    assert.deepStrictEqual(
        theme.metaFor({
            Type: 'Movie',
            DateCreated: new Date(Date.now() - 30 * 864e5).toISOString()
        }),
        [],
        'a month old is not new'
    );
    assert.deepStrictEqual(
        theme.metaFor({
            Type: 'Movie',
            RunTimeTicks: 600000000 * 100,
            UserData: { PlaybackPositionTicks: 600000000 * 100 }
        }),
        ['1h 40m'],
        'watched to the end has no time left to report'
    );
});

test('genresFor takes the first three and nothing from a folder', () => {
    const { theme } = onHome();
    // The server returns them most-specific-first, and three is what one line
    // holds at a poster's width.
    assert.deepStrictEqual(
        theme.genresFor({
            Type: 'Movie',
            Genres: ['Science Fiction', 'Drama', 'Mystery', 'Thriller', 'Adventure']
        }),
        ['Science Fiction', 'Drama', 'Mystery']
    );
    assert.deepStrictEqual(theme.genresFor({ Type: 'Movie', Genres: ['Drama'] }), ['Drama']);
    assert.deepStrictEqual(theme.genresFor({ Type: 'Movie', Genres: [] }), []);
    assert.deepStrictEqual(theme.genresFor({ Type: 'Movie' }), [], 'an episode carries none');
    assert.deepStrictEqual(
        theme.genresFor({ Type: 'BoxSet', Genres: ['Drama'] }), [], 'not for a folder'
    );
    assert.deepStrictEqual(theme.genresFor(null), []);
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

test('focusing a card pops it out and drives the backdrop from the item', async () => {
    const { win, home, theme } = onHome();
    const item = {
        Id: 'one', Name: 'Arrival', Type: 'Movie', ProductionYear: 2016,
        BackdropImageTags: ['bt']
    };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = popCard(win, { id: 'one', parent: home.sections[0] });
    theme.setFocusedCard(card);
    await settle();
    const ui = theme.state().ui;
    assert.strictEqual(theme.state().poppedCard, card);
    assert.strictEqual(ui.popout.hidden, false);
    assert.ok(ui.popout.classList.contains('af-show'));
    assert.strictEqual(ui.title.textContent, 'Arrival');
    assert.strictEqual(ui.meta.textContent, '2016');
    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
});

test('a fetch that comes back for a card the pointer has left is dropped', async () => {
    const { win, home, theme } = onHome();
    const item = { Id: 'one', Name: 'Arrival', Type: 'Movie' };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = popCard(win, { id: 'one', parent: home.sections[0] });
    theme.setFocusedCard(card);
    // hidePopout() drops the selection without bumping the request token, so
    // the token alone would not catch this.
    theme.hidePopout();
    await settle();
    assert.strictEqual(theme.state().poppedCard, null, 'a slow server cannot re-open it');
    assert.ok(!card.classList.contains('af-popped'));
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

test('focusing a card on a library route pops it out there too', async () => {
    const { win, library, theme } = onLibrary();
    const item = {
        Id: 'one', Name: 'Arrival', Type: 'Movie', ProductionYear: 2016,
        BackdropImageTags: ['bt']
    };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = popCard(win, { id: 'one', parent: library.grid });
    theme.setFocusedCard(card);
    await settle();

    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
    win.images[0].onload();
    assert.ok(theme.state().backdropLayers[0].classList.contains('af-on'));
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
    // The popout belongs to a card, not to a route: the grids get it on the
    // same terms Home does, which the in-flow band it replaced never could.
    assert.strictEqual(theme.state().poppedCard, card);
    assert.ok(card.classList.contains('af-popped'));
    assert.ok(theme.state().ui.popout.classList.contains('af-show'));
    // The Home-only chrome stays down all the same.
    assert.strictEqual(theme.state().ui.hint.hidden, true);
});

// ---------------------------------------------------------------------------
// Popout geometry
// ---------------------------------------------------------------------------

test('artOf measures the card tile and falls back to the card itself', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    const tile = card.querySelector('.cardScalable');
    assert.strictEqual(theme.artOf(card), tile);
    tile.remove();
    assert.strictEqual(theme.artOf(card), card, 'a tile-less card still measures');
    assert.strictEqual(theme.artOf(null), null);
});

test('popoutScale reads the token and only believes a scale between 1 and 3', () => {
    const { win, theme } = onHome();
    const style = win.document.documentElement.style;
    assert.strictEqual(theme.popoutScale(), 1.32, 'no token: the built-in default');
    style.setProperty('--af-popout-scale', '1.5');
    assert.strictEqual(theme.popoutScale(), 1.5);
    for (const junk of ['', 'wide', '1', '3', '0.8', '4']) {
        style.setProperty('--af-popout-scale', junk);
        assert.strictEqual(theme.popoutScale(), 1.32, junk || '(empty)');
    }
});

test('readToken believes a number strictly inside the range and nothing else', () => {
    const { win, theme } = onHome();
    const style = win.document.documentElement.style;
    assert.strictEqual(theme.readToken('--af-probe', 0, 10, 7), 7, 'missing: the fallback');
    style.setProperty('--af-probe', '4.5');
    assert.strictEqual(theme.readToken('--af-probe', 0, 10, 7), 4.5);
    style.setProperty('--af-probe', '260px');
    assert.strictEqual(theme.readToken('--af-probe', 0, 1000, 7), 260, 'a unit is parsed off');
    // The range is exclusive at both ends, so a value sitting exactly on a
    // bound is read as junk rather than believed.
    for (const junk of ['', 'wide', 'NaN', '0', '10', '-3', '11']) {
        style.setProperty('--af-probe', junk);
        assert.strictEqual(theme.readToken('--af-probe', 0, 10, 7), 7, junk || '(empty)');
    }
});

test('popoutMinWidth reads the floor token and falls back to 200', () => {
    const { win, theme } = onHome();
    const style = win.document.documentElement.style;
    assert.strictEqual(theme.popoutMinWidth(), 200, 'no token: the built-in default');
    style.setProperty('--af-popout-min-width', '260px');
    assert.strictEqual(theme.popoutMinWidth(), 260);
    for (const junk of ['', 'narrow', '0', '2000', '-40']) {
        style.setProperty('--af-popout-min-width', junk);
        assert.strictEqual(theme.popoutMinWidth(), 200, junk || '(empty)');
    }
});

test('headerBottom measures a pinned header and ignores one that has scrolled off', () => {
    const { win, theme } = onHome();
    assert.strictEqual(theme.headerBottom(), 0, 'no header at all');
    const header = pinHeader(win, 88);
    assert.strictEqual(theme.headerBottom(), 88);
    setRect(header, { left: 0, top: 40, width: 1280, height: 88 });
    assert.strictEqual(theme.headerBottom(), 0, 'scrolled down the page, not in the way');
    setRect(header, { left: 0, top: -88, width: 1280, height: 88 });
    assert.strictEqual(theme.headerBottom(), 0, 'scrolled off the top');
});

test('headerBottom ignores a legacy header Jellyfin 12 has hidden and measures the MUI one', () => {
    const { win, theme } = onHome();
    // Measured live over CEF's debug port against a 12.0.0 server: .skinHeader is
    // still in the DOM but its wrapper is display:none, so every number on its
    // rect is zero, and the real header is a MuiAppBar at z-index 1100.
    const legacy = win.document.createElement('div');
    legacy.className = 'skinHeader';
    win.document.body.appendChild(legacy);
    setRect(legacy, { left: 0, top: 0, width: 0, height: 0 });
    assert.strictEqual(theme.headerBottom(), 0, 'a hidden legacy header is not in the way');

    const bar = win.document.createElement('header');
    bar.className = 'MuiPaper-root MuiAppBar-root MuiAppBar-colorDefault';
    win.document.body.appendChild(bar);
    setRect(bar, { left: 0, top: 0, width: 2048, height: 48 });
    assert.strictEqual(theme.headerBottom(), 48, 'the MUI header is the one in the way');
});

test('headerBottom takes the lower edge when a server shows both headers', () => {
    const { win, theme } = onHome();
    pinHeader(win, 88);
    const bar = win.document.createElement('header');
    bar.className = 'MuiAppBar-root';
    win.document.body.appendChild(bar);
    setRect(bar, { left: 0, top: 0, width: 2048, height: 48 });
    assert.strictEqual(theme.headerBottom(), 88, 'the popout has to clear both');
});

test('placePopout centres the popout on the card and sizes the art from it', () => {
    const { win, home, theme } = onHome();
    const card = popCard(win, { parent: home.sections[0] });
    // A 200x300 tile at x=100, grown 1.32x and centred on the tile's own x=200.
    const box = theme.placePopout(card);
    assert.deepStrictEqual(box, { left: 68, top: 152, width: 264, artHeight: 396 });
    const ui = theme.state().ui;
    assert.strictEqual(ui.popout.style.left, '68px');
    assert.strictEqual(ui.popout.style.top, '152px');
    assert.strictEqual(ui.popout.style.width, '264px');
    assert.strictEqual(ui.art.style.height, '396px');
});

test('placePopout clamps a card at either end of a rail inward', () => {
    const { win, home, theme } = onHome();
    const EDGE = 16;
    const first = popCard(win, { parent: home.sections[0] }, { left: 8 });
    const a = theme.placePopout(first);
    assert.strictEqual(a.left, EDGE, 'the first card opens inward, not off-screen');

    const last = popCard(win, { parent: home.sections[0] }, { left: 1060 });
    const b = theme.placePopout(last);
    assert.strictEqual(b.left + b.width, win.innerWidth - EDGE, 'and so does the last');
    assert.ok(b.left > a.left);
});

test('placePopout keeps the poster aspect when the viewport caps the width', () => {
    const { win, home, theme } = onHome({ innerWidth: 300 });
    // 400 wide is already more than the capped 300 - 2*16 the popout may have.
    const card = popCard(win, { parent: home.sections[0] }, { width: 400, height: 600 });
    const box = theme.placePopout(card);
    assert.strictEqual(box.width, 300 - 32, 'capped, so it has somewhere to shift to');
    assert.strictEqual(box.artHeight, 402, 'height follows the clamped width, not the scale');
    assert.strictEqual(box.artHeight / box.width, 600 / 400);
});

test('placePopout widens a small card to the floor and keeps the art proportional', () => {
    const { win, home, theme } = onHome();
    // A square-ish music tile: 120 x 1.32 is 158, too narrow for a readable
    // drawer line, so the floor takes over.
    const small = popCard(win, { parent: home.sections[0] }, { width: 120, height: 180 });
    const box = theme.placePopout(small);
    assert.strictEqual(box.width, 200, 'the floor beat the proportional width');
    // Widening enlarges the poster rather than stretching it: the art height
    // follows the *clamped* width, so the card's aspect survives the floor.
    assert.strictEqual(box.artHeight, 300);
    assert.strictEqual(box.artHeight / box.width, 180 / 120);
    assert.strictEqual(theme.state().ui.art.style.height, '300px');

    // An ordinary rail card is already wider than the floor, which is the
    // whole point of setting it low — a floor that bit here would cost height
    // on every card to buy width on one.
    const normal = popCard(win, { parent: home.sections[1] });
    assert.strictEqual(theme.placePopout(normal).width, 264);

    // And the floor is the token's, not a constant baked into the geometry.
    win.document.documentElement.style.setProperty('--af-popout-min-width', '300px');
    const raised = theme.placePopout(small);
    assert.strictEqual(raised.width, 300);
    assert.strictEqual(raised.artHeight / raised.width, 180 / 120);
});

test('placePopout caps the art to what is left of the band under a tall drawer', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    const EDGE = 16;
    // Proportionally the art wants 396px, and the drawer already eats more of
    // the band than that leaves. The art is `cover`, so it crops rather than
    // hanging the popout off the bottom of the screen.
    setRect(ui.drawer, { width: 264, height: 400 });
    const band = win.innerHeight - EDGE * 2;
    const capped = theme.placePopout(card);
    assert.strictEqual(capped.artHeight, band - 400);
    assert.ok(capped.artHeight < 396, 'reduced, not left proportional');
    assert.strictEqual(capped.artHeight + 400, band, 'exactly fills the band, never more');
    assert.strictEqual(ui.art.style.height, capped.artHeight + 'px');

    // A pinned header comes out of the band too, so the same drawer crops the
    // art further under one.
    pinHeader(win, 88);
    const underHeader = theme.placePopout(card);
    assert.strictEqual(underHeader.artHeight, band - 88 - 400);
    assert.ok(underHeader.artHeight < capped.artHeight);
});

test('placePopout never crops the art below the floor, however tall the drawer', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    // 688 of band against a 660px drawer leaves 28px — below that the art has
    // stopped being a poster and is just a strip, so it holds at 96.
    setRect(ui.drawer, { width: 264, height: 660 });
    assert.strictEqual(theme.placePopout(card).artHeight, 96);
    // A drawer taller than the whole band cannot push it any lower.
    setRect(ui.drawer, { width: 264, height: 900 });
    assert.strictEqual(theme.placePopout(card).artHeight, 96);
    assert.strictEqual(ui.art.style.height, '96px');
});

test('placePopout clamps the popout below a pinned header and above the fold', () => {
    const { win, home, theme } = onHome();
    pinHeader(win, 88);
    const high = popCard(win, { parent: home.sections[0] }, { top: 50 });
    assert.strictEqual(theme.placePopout(high).top, 88 + 16, 'clear of the header');
    assert.strictEqual(theme.state().ui.popout.style.top, '104px');

    // The drawer grows past the bottom of the art, so it is part of what has
    // to fit: a tall one lifts the whole popout off the bottom edge.
    setRect(theme.state().ui.drawer, { width: 264, height: 200 });
    const low = popCard(win, { parent: home.sections[0] }, { top: 380 });
    assert.strictEqual(theme.placePopout(low).top, 720 - 16 - (396 + 200));
});

test('placePopout paints nothing for a card it cannot measure', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    assert.strictEqual(theme.placePopout(popCard(win, {})), null, 'never in the document');
    // In the rail, but laid out as 0x0 - a rail that has not rendered yet.
    assert.strictEqual(theme.placePopout(makeCard(win, { parent: home.sections[0] })), null);
    assert.strictEqual(theme.placePopout(null), null);
    assert.strictEqual(ui.popout.style.left, undefined, 'no popout was painted at 0,0');
});

// ---------------------------------------------------------------------------
// Rendering the popout
// ---------------------------------------------------------------------------

test('cloneArt copies the tile, drops the canvas and disarms the delegation', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { action: 'play', link: true, parent: home.sections[0] });
    const tile = card.querySelector('.cardScalable');
    tile.id = 'cardScalable-1';
    tile.appendChild(win.document.createElement('canvas'));
    const overlay = win.document.createElement('div');
    overlay.className = 'cardOverlayContainer';
    tile.appendChild(overlay);
    const indicators = win.document.createElement('div');
    indicators.className = 'cardIndicators';
    tile.appendChild(indicators);

    const clone = theme.cloneArt(card);
    assert.notStrictEqual(clone, tile, 'a copy, not the card tile itself');
    assert.ok(clone.classList.contains('cardScalable'));
    assert.strictEqual(clone.id, '', 'a duplicate id would break getElementById');
    assert.strictEqual(clone.querySelector('canvas'), null, 'a cloned blurhash paints empty');
    assert.strictEqual(clone.querySelector('.cardOverlayContainer'), null);
    assert.ok(clone.querySelector('.cardIndicators'), 'everything else survives');
    // The card itself is left exactly as it was.
    assert.ok(tile.querySelector('canvas'));
    assert.ok(card.querySelector('.cardOverlayButton[data-action="play"]'));
});

test('cloneArt disarms data-action without deleting the art it sits on', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { link: true, parent: home.sections[0] });
    const tile = card.querySelector('.cardScalable');
    tile.setAttribute('data-action', 'link');
    // jf-web 10.11.11 hangs data-action="link" on .cardImageContainer, which
    // *is* the picture: removing the node would remove the art. The attribute
    // comes off instead, or jellyfin-web's delegated handler would resolve a
    // click in the clone against whatever [data-id] the popout sits over.
    const clone = theme.cloneArt(card);
    assert.ok(clone.querySelector('.cardImageContainer'), 'the art survived');
    assert.strictEqual(clone.querySelectorAll('[data-action]').length, 0, 'disarmed');
    assert.strictEqual(clone.getAttribute('data-action'), null, 'the root too');
});

test('cloneArt has nothing to copy when the card has no tile', () => {
    const { win, theme } = onHome();
    const bare = win.document.createElement('div');
    bare.className = 'card';
    assert.strictEqual(theme.cloneArt(bare), null);
    assert.strictEqual(theme.cloneArt(null), null);
});

test('setDiscAction renames the disc button and swaps its glyph', () => {
    const { theme } = onHome();
    const play = theme.state().ui.play;
    theme.setDiscAction(play, 'af-po-glyph-browse', 'Browse');
    // Icon-only, so the accessible name has to come from the label.
    assert.strictEqual(play.getAttribute('aria-label'), 'Browse');
    assert.strictEqual(play.title, 'Browse');
    assert.strictEqual(play.firstChild.className, 'af-po-glyph af-po-glyph-browse');

    theme.setDiscAction(play, 'af-po-glyph-play', 'Resume');
    assert.strictEqual(play.getAttribute('aria-label'), 'Resume');
    assert.strictEqual(play.firstChild.className, 'af-po-glyph af-po-glyph-play');
    theme.setDiscAction(null, 'af-po-glyph-play', 'Play'); // never throws
});

test('writeJoined writes one dot-separated text node and hides an empty line', () => {
    const { win, theme } = onHome();
    const el = win.document.createElement('div');
    theme.writeJoined(el, ['S1 E4', 'Pilot', '2011']);
    assert.strictEqual(el.textContent, 'S1 E4 · Pilot · 2011');
    // One text node, not a node per fact: the separator is punctuation here,
    // and a single node is what lets the line ellipsize as prose.
    assert.strictEqual(el.childNodes.length, 1);
    assert.strictEqual(el.children.length, 0);
    assert.strictEqual(el.hidden, false);

    theme.writeJoined(el, ['2016']);
    assert.strictEqual(el.textContent, '2016', 'one fact carries no separator');

    theme.writeJoined(el, []);
    assert.strictEqual(el.textContent, '');
    assert.strictEqual(el.hidden, true, 'an empty tier holds no gap in the drawer');
});

test('writeBadges writes a box per label and replaces the previous set', () => {
    const { win, theme } = onHome();
    const el = win.document.createElement('div');
    theme.writeBadges(el, ['PG-13', '4K', 'HDR10']);
    assert.deepStrictEqual(childTexts(el), ['PG-13', '4K', 'HDR10']);
    assert.deepStrictEqual(
        el.children.map((b) => b.className),
        ['af-po-badge', 'af-po-badge', 'af-po-badge']
    );
    assert.strictEqual(el.hidden, false);

    theme.writeBadges(el, ['TV-MA']);
    assert.deepStrictEqual(childTexts(el), ['TV-MA'], 'replaced, not appended to');

    theme.writeBadges(el, []);
    assert.strictEqual(el.children.length, 0);
    assert.strictEqual(el.hidden, true);
});

test('writePopout puts the card tile in the art slot and replaces the drawer', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.writePopout({ Type: 'Movie', Name: 'The Matrix', ProductionYear: 1999 }, card);
    assert.strictEqual(ui.art.children.length, 1);
    assert.ok(ui.art.children[0].classList.contains('cardScalable'));
    assert.strictEqual(ui.title.textContent, 'The Matrix');
    assert.strictEqual(ui.meta.textContent, '1999');
    assert.strictEqual(theme.state().shownCard, card);

    const other = popCard(win, { parent: home.sections[1] });
    theme.writePopout({ Type: 'Movie', Name: 'Amelie', ProductionYear: 2001 }, other);
    assert.strictEqual(ui.art.children.length, 1, 'the first clone went with it');
    assert.strictEqual(ui.title.textContent, 'Amelie', 'replaced, not appended to');
    assert.strictEqual(ui.meta.textContent, '2001');
    assert.strictEqual(theme.state().shownCard, other);
});

test('writePopout fills the four drawer tiers from the item and hides the empty ones', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.writePopout({
        Type: 'Episode',
        Name: 'Good News About Hell',
        SeriesName: 'Severance',
        ParentIndexNumber: 1,
        IndexNumber: 2,
        ProductionYear: 2022,
        RunTimeTicks: 600000000 * 47,
        OfficialRating: 'TV-MA',
        CommunityRating: 8.1,
        Genres: ['Drama', 'Mystery', 'Sci-Fi & Fantasy', 'Thriller'],
        MediaStreams: [{ Type: 'Video', Width: 3840, VideoRangeType: 'HDR10' }]
    }, card);
    assert.strictEqual(ui.title.textContent, 'Severance', 'the series, not the episode');
    assert.deepStrictEqual(childTexts(ui.badges), ['TV-MA', '4K', 'HDR10']);
    assert.strictEqual(
        ui.meta.textContent,
        'S1 E2 · Good News About Hell · 2022 · 47m · ★ 8.1'
    );
    assert.strictEqual(ui.genres.textContent, 'Drama · Mystery · Sci-Fi & Fantasy');
    for (const el of [ui.badges, ui.meta, ui.genres]) assert.strictEqual(el.hidden, false);

    // A sparse item leaves three of the four tiers with nothing to say, and an
    // empty tier is hidden rather than left holding a gap in the drawer.
    theme.writePopout({ Type: 'Movie', Name: 'Untitled' }, card);
    assert.strictEqual(ui.title.textContent, 'Untitled');
    assert.strictEqual(ui.badges.hidden, true);
    assert.strictEqual(ui.meta.hidden, true);
    assert.strictEqual(ui.genres.hidden, true);
    assert.strictEqual(ui.badges.children.length, 0, 'the old badges went with it');
});

test('writePopout labels a resumable item Resume and a fresh one Play', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.writePopout({ Type: 'Movie', Name: 'A' }, card);
    assert.strictEqual(ui.play.getAttribute('aria-label'), 'Play');
    assert.strictEqual(ui.details.hidden, false);

    theme.writePopout(
        { Type: 'Movie', Name: 'B', UserData: { PlaybackPositionTicks: 5 } }, card
    );
    assert.strictEqual(ui.play.getAttribute('aria-label'), 'Resume');
    assert.strictEqual(ui.play.firstChild.className, 'af-po-glyph af-po-glyph-play');
});

test('writePopout turns a folder into a Browse-only panel', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.writePopout({ Type: 'BoxSet', Name: 'Trilogy', ChildCount: 3 }, card);
    assert.strictEqual(ui.play.getAttribute('aria-label'), 'Browse');
    assert.strictEqual(ui.play.firstChild.className, 'af-po-glyph af-po-glyph-browse');
    assert.strictEqual(ui.details.hidden, true, 'a box set is browsed, not opened');
    assert.strictEqual(ui.title.textContent, 'Trilogy');
    assert.strictEqual(ui.meta.textContent, '3 items');
    assert.strictEqual(ui.badges.hidden, true, 'a container has no file to describe');
    assert.strictEqual(ui.genres.hidden, true);
});

test('renderPopout shows the popout over the card and moves af-popped with it', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const first = popCard(win, { parent: home.sections[0] });
    theme.renderPopout({ Type: 'Movie', Name: 'Arrival' }, first);
    assert.ok(first.classList.contains('af-popped'), 'cancels the tile hover scale');
    assert.strictEqual(theme.state().poppedCard, first);
    assert.strictEqual(ui.popout.hidden, false);
    assert.ok(ui.popout.classList.contains('af-show'));
    assert.strictEqual(ui.title.textContent, 'Arrival');
    // The title is visible in the drawer now, so an aria-label on the
    // container would override the content it duplicates.
    assert.strictEqual(ui.popout.getAttribute('aria-label'), null);
    assert.strictEqual(ui.popout.style.left, '68px', 'placed, not left at 0,0');

    const second = popCard(win, { parent: home.sections[1] }, { left: 400 });
    theme.renderPopout({ Type: 'Episode', SeriesName: 'Severance', Name: 'Ep 1' }, second);
    assert.ok(!first.classList.contains('af-popped'), 'only ever one popped card');
    assert.ok(second.classList.contains('af-popped'));
    assert.strictEqual(theme.state().poppedCard, second);
    assert.strictEqual(ui.title.textContent, 'Severance');
    assert.strictEqual(ui.popout.style.left, '368px', 're-anchored');
});

test('renderPopout gives up rather than paint a popout over nothing', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] }); // never laid out
    theme.renderPopout({ Type: 'Movie', Name: 'A' }, card);
    assert.strictEqual(theme.state().poppedCard, null);
    assert.ok(!card.classList.contains('af-popped'));
    assert.ok(!theme.state().ui.popout.classList.contains('af-show'));
});

test('hidePopout drops the selection at once and hides the element on the timer', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();
    theme.setFocusedCard(card);
    theme.renderPopout({ Type: 'Movie', Name: 'Arrival' }, card);

    theme.hidePopout();
    assert.ok(!card.classList.contains('af-popped'));
    assert.ok(!card.classList.contains('af-focused'));
    const state = theme.state();
    assert.strictEqual(state.poppedCard, null);
    assert.strictEqual(state.shownCard, null);
    assert.strictEqual(state.shownItem, null);
    assert.strictEqual(state.focusedCard, null, 're-entering the card opens it again');
    assert.ok(!ui.popout.classList.contains('af-show'));
    assert.strictEqual(ui.popout.hidden, false, 'still there for the fade-out');
    win.timers.advance(180);
    assert.strictEqual(ui.popout.hidden, true);
    // The art is the page background on both routes; dropping it on every
    // pointer exit would strobe it across a rail.
    assert.strictEqual(theme.state().currentBackdropUrl, 'https://server/a.jpg');
});

test('syncPopout follows the card when the page scrolls under it', () => {
    const { win, home, theme } = onHome();
    win.timers.runAll();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.renderPopout({ Type: 'Movie', Name: 'Arrival' }, card);
    assert.strictEqual(ui.popout.style.top, '152px');

    setRect(card.querySelector('.cardScalable'),
        { left: 100, top: 120, width: 200, height: 300 });
    theme.syncPopout();
    assert.strictEqual(theme.state().syncQueued, true, 'coalesced, not measured inline');
    theme.syncPopout();
    win.timers.runAll();
    assert.strictEqual(ui.popout.style.top, '72px', 'followed the card');
    assert.strictEqual(theme.state().syncQueued, false);
    assert.strictEqual(theme.state().poppedCard, card);
});

test('syncPopout closes the popout when the card goes out from under it', () => {
    const { win, home, theme } = onHome();
    win.timers.runAll();
    const scrolled = popCard(win, { parent: home.sections[0] });
    theme.renderPopout({ Type: 'Movie', Name: 'A' }, scrolled);
    setRect(scrolled.querySelector('.cardScalable'),
        { left: 100, top: -400, width: 200, height: 300 });
    theme.syncPopout();
    win.timers.runAll();
    assert.strictEqual(theme.state().poppedCard, null, 'scrolled out of the viewport');

    const gone = popCard(win, { parent: home.sections[0] });
    theme.renderPopout({ Type: 'Movie', Name: 'B' }, gone);
    gone.remove();
    theme.syncPopout();
    win.timers.runAll();
    assert.strictEqual(theme.state().poppedCard, null, 'nothing left to anchor to');
});

test('inPopout claims the popout subtree and nothing else', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    assert.strictEqual(theme.inPopout(ui.popout), true);
    assert.strictEqual(theme.inPopout(ui.play), true, 'the pointer crosses it on the way');
    assert.strictEqual(theme.inPopout(makeCard(win, { parent: home.sections[0] })), false);
    assert.strictEqual(theme.inPopout(null), false);
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
    theme.writePopout({ Type: 'Movie', Id: 'a' }, card);
    theme.onPlayClick();
    assert.deepStrictEqual(card.clicks, [['overlay', 'play']]);

    const folder = makeCard(win, { action: 'play', link: true, parent: home.sections[0] });
    theme.writePopout({ Type: 'CollectionFolder', Id: 'lib' }, folder);
    theme.onPlayClick();
    assert.deepStrictEqual(folder.clicks, [['link']], 'a library is browsed, not played');
});

test('Play falls back to a synthetic resume/play action when the card has no button', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    const seen = [];
    home.container.addEventListener('click', (e) => seen.push(e.target.getAttribute('data-action')));
    theme.writePopout({ Type: 'Movie', UserData: { PlaybackPositionTicks: 10 } }, card);
    theme.onPlayClick();
    theme.writePopout({ Type: 'Movie' }, card);
    theme.onPlayClick();
    assert.deepStrictEqual(seen, ['resume', 'play']);
});

test('Play acts on the item the panel is showing, not on a later hover', () => {
    const { win, home, theme } = onHome();
    const shown = makeCard(win, { id: 'shown', action: 'play', parent: home.sections[0] });
    const hovered = makeCard(win, { id: 'hovered', action: 'play', parent: home.sections[1] });
    win.ApiClient = makeThemeApiClient();
    theme.writePopout({ Type: 'Movie', Id: 'shown' }, shown);
    theme.setFocusedCard(hovered); // the fetch for it has not resolved
    theme.onPlayClick();
    assert.deepStrictEqual(shown.clicks, [['overlay', 'play']]);
    assert.deepStrictEqual(hovered.clicks, []);
});

test('Play does nothing once the card has left the document', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { action: 'play', parent: home.sections[0] });
    theme.writePopout({ Type: 'Movie' }, card);
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
    theme.writePopout({ Type: 'Movie', Id: 'shown' }, card);
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

test('cardFrom also claims cards inside a library grid', () => {
    const { win, library, theme } = onLibrary();
    const card = makeCard(win, { parent: library.grid });
    assert.strictEqual(theme.cardFrom(card.querySelector('.cardScalable')), card);

    // Same page, but outside the grid: jellyfin-web keeps that card.
    const stray = makeCard(win, { parent: library.tab });
    assert.strictEqual(theme.cardFrom(stray), null);
});

test('focusin selects the card immediately and cancels a pending hover', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const hovered = makeCard(win, { id: 'hovered', parent: home.sections[0] });
    const focused = makeCard(win, { id: 'focused', parent: home.sections[1] });
    theme.onPointerOver({ target: hovered });
    assert.ok(theme.state().hoverTimer, 'hover is pending');
    theme.onFocusIn({ target: focused });
    assert.strictEqual(theme.state().focusedCard, focused);
    win.timers.advance(500);
    assert.strictEqual(theme.state().focusedCard, focused, 'the hover was cancelled');
});

test('hover is debounced and re-anchors to whichever card the pointer reaches', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const one = popCard(win, { id: 'one', parent: home.sections[0] });
    const two = popCard(win, { id: 'two', parent: home.sections[1] }, { left: 400 });
    // The popout no longer sits in the flow, so there is no reflow-induced
    // mouseover to guard against and no pointer-moved gate to arm.
    theme.onPointerOver({ target: one });
    assert.ok(theme.state().hoverTimer, 'debounced, not immediate');
    assert.strictEqual(theme.state().focusedCard, null, 'not yet');
    win.timers.advance(120);
    assert.strictEqual(theme.state().focusedCard, one);

    theme.onPointerOver({ target: two });
    win.timers.advance(120);
    assert.strictEqual(theme.state().focusedCard, two, 're-anchored to the new card');
});

test('hovering the card that is already focused does nothing', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    theme.onPointerOver({ target: card });
    assert.strictEqual(theme.state().hoverTimer, 0);
});

test('a pointer leaving for anything but a card or the popout closes it', () => {
    const { win, home, theme } = onHome();
    const ui = theme.state().ui;
    const card = popCard(win, { parent: home.sections[0] });
    theme.renderPopout({ Type: 'Movie', Name: 'Arrival' }, card);

    // Inside the popout: the pointer is on its way to the disc buttons.
    theme.onPointerOver({ target: ui.play });
    assert.strictEqual(theme.state().poppedCard, card, 'crossing it is not leaving');

    // A rail heading. The popout covers its own card, so the card's own
    // mouseout never fires and nothing else would say the pointer had gone.
    const heading = win.document.createElement('h2');
    home.sections[0].appendChild(heading);
    theme.onPointerOver({ target: heading });
    assert.strictEqual(theme.state().poppedCard, null);
    assert.ok(!ui.popout.classList.contains('af-show'));
});

// ---------------------------------------------------------------------------
// Visibility, refresh and the page observer
// ---------------------------------------------------------------------------

test('ensureUi rebuilds the panels when jellyfin-web has orphaned them', () => {
    const { theme } = onHome();
    const first = theme.state().ui;
    theme.ensureUi();
    assert.strictEqual(theme.state().ui, first, 'still connected: kept');
    first.popout.remove();
    theme.ensureUi();
    assert.notStrictEqual(theme.state().ui, first, 'orphaned: rebuilt');
});

test('showOverlays shows the controller hint on Home and hides it elsewhere', () => {
    const { win, theme } = onHome();
    theme.showOverlays(true);
    const ui = theme.state().ui;
    assert.strictEqual(ui.hint.hidden, false);
    assert.ok(ui.hint.classList.contains('af-show'));
    // The server card was dropped at the owner's request, so the hint is the
    // whole of the Home-only chrome now.
    assert.strictEqual(ui.server, undefined);
    // The popout is not Home chrome either: it belongs to a card, shows on the
    // library grids too, and is driven by renderPopout/hidePopout alone.
    assert.strictEqual(ui.popout.hidden, true);

    win.location.hash = '#/details?id=1';
    theme.showOverlays();
    assert.strictEqual(ui.hint.hidden, true);
    assert.ok(!ui.hint.classList.contains('af-show'));
});

test('leaveHome tears the popout down and drops the selection and the art', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = popCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    theme.renderPopout({ Type: 'Movie', Name: 'A' }, card);
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();

    theme.leaveHome();
    const state = theme.state();
    assert.strictEqual(state.focusedCard, null);
    assert.strictEqual(state.poppedCard, null);
    assert.strictEqual(state.shownItem, null);
    assert.strictEqual(state.currentBackdropUrl, null);
    assert.strictEqual(state.overlaysWanted, false);
    assert.ok(!card.classList.contains('af-focused'));
    assert.ok(!card.classList.contains('af-popped'));
    assert.ok(!state.ui.popout.classList.contains('af-show'));
    win.timers.advance(500);
    assert.strictEqual(theme.state().ui.popout.hidden, true);
});

test('leaveHome for a library grid keeps the selection and the art', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = popCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    theme.renderPopout({ Type: 'Movie', Name: 'A' }, card);
    theme.setBackdrop('https://server/a.jpg');
    win.images[0].onload();

    theme.leaveHome(true);
    const state = theme.state();
    assert.strictEqual(state.focusedCard, card, 'the grid goes on hovering it');
    assert.ok(card.classList.contains('af-focused'));
    assert.strictEqual(state.currentBackdropUrl, 'https://server/a.jpg', 'the art stays up');
    // The popout goes either way: it is anchored to a viewport rect the
    // outgoing page owns.
    assert.strictEqual(state.poppedCard, null);
    assert.ok(!card.classList.contains('af-popped'));
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

test('refresh on Home marks the root and builds the panels', () => {
    const { win, theme } = onHome();
    theme.refresh();
    assert.ok(win.document.documentElement.classList.contains('af-home'));
    assert.ok(theme.state().ui, 'panels exist');
    assert.ok(theme.state().ui.popout.isConnected);
    assert.ok(theme.state().ui.hint.isConnected);
    // No server card is built at all any more, on any route.
    assert.strictEqual(win.document.querySelectorAll('#af-server-panel').length, 0);
});

test('refresh off Home clears af-home and tears the selection down', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = popCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    theme.renderPopout({ Type: 'Movie', Name: 'A' }, card);
    win.location.hash = '#/details?id=1';
    theme.refresh();
    assert.ok(!win.document.documentElement.classList.contains('af-home'));
    assert.strictEqual(theme.state().focusedCard, null);
    assert.strictEqual(theme.state().poppedCard, null);
    assert.ok(!theme.state().ui.popout.classList.contains('af-show'));
    win.timers.advance(500);
    assert.strictEqual(theme.state().ui.popout.hidden, true);
});

test('refresh on a library route sets af-library alone and keeps the selection', () => {
    const { win, library, theme } = onLibrary();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { id: 'one', parent: library.grid });
    theme.setFocusedCard(card);
    theme.refresh();
    const html = win.document.documentElement;
    assert.ok(html.classList.contains('af-library'));
    assert.ok(!html.classList.contains('af-home'), 'never both');
    // Cards stream into the grid long after the first hover, and each batch
    // queues a refresh; the art must not blink off under the cursor.
    assert.strictEqual(theme.state().focusedCard, card);
    assert.ok(card.classList.contains('af-focused'));
});

test('refresh moving between Home and a library never leaves both classes set', () => {
    const { win, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const html = win.document.documentElement;
    theme.refresh();
    assert.ok(html.classList.contains('af-home'));
    assert.ok(!html.classList.contains('af-library'));

    buildLibrary(win);
    win.location.hash = '#/movies.html?topParentId=lib1';
    theme.refresh();
    assert.ok(html.classList.contains('af-library'));
    assert.ok(!html.classList.contains('af-home'));

    win.location.hash = '#/home.html';
    theme.refresh();
    assert.ok(html.classList.contains('af-home'));
    assert.ok(!html.classList.contains('af-library'));
});

test('refresh leaving a library grid for Home drops the grid selection', () => {
    const { win, library, theme } = onLibrary();
    const item = { Id: 'one', Type: 'Movie', Name: 'Arrival', BackdropImageTags: ['bt'] };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = makeCard(win, { id: 'one', parent: library.grid });
    return theme.fetchItem('one').then(() => {
        theme.setFocusedCard(card);
        return Promise.resolve().then(() => {
            theme.refresh();
            assert.ok(theme.state().focusedItem, 'the grid selection is live');

            // The card is going away with the outgoing page, and the popout is
            // anchored to a viewport rect that page owns.
            buildHome(win);
            win.location.hash = '#/home.html';
            theme.refresh();
            assert.strictEqual(theme.state().focusedCard, null);
            assert.strictEqual(theme.state().focusedItem, null);
            assert.ok(!card.classList.contains('af-focused'));
            assert.strictEqual(theme.state().currentBackdropUrl, null);
        });
    });
});

test('refresh re-measures the popout for the card that is still focused', () => {
    const { win, home, theme } = onHome();
    const item = { Id: 'one', Type: 'Movie', Name: 'Arrival' };
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', item]]) });
    const card = popCard(win, { id: 'one', parent: home.sections[0] });
    return theme.fetchItem('one').then(() => {
        theme.setFocusedCard(card);
        return Promise.resolve().then(() => {
            const ui = theme.state().ui;
            assert.strictEqual(ui.popout.style.left, '68px');
            // Cards stream into the rails for seconds after the first hover,
            // and every batch reflows the one the popout is standing over.
            setRect(card.querySelector('.cardScalable'),
                { left: 500, top: 200, width: 200, height: 300 });
            theme.refresh();
            win.timers.runAll();
            assert.strictEqual(ui.popout.style.left, '468px', 're-measured, not left hanging');
            assert.strictEqual(theme.state().poppedCard, card);
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

test('the popout repainting itself does not queue a refresh', () => {
    const { win, home, theme } = onHome();
    const card = popCard(win, { parent: home.sections[0] });
    win.timers.runAll();
    assert.strictEqual(theme.state().refreshQueued, false);
    // The popout lives in <body>, outside .mainAnimatedPages, so the observer
    // that catches cards streaming into the rails never sees it and needs no
    // filter of its own — unlike the detail panel, which does.
    theme.renderPopout({ Type: 'Movie', Name: 'Arrival' }, card);
    assert.strictEqual(theme.state().refreshQueued, false);
    theme.state().ui.badges.appendChild(win.document.createElement('div'));
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

// ---------------------------------------------------------------------------
// Item detail
// ---------------------------------------------------------------------------

test('isDetailRoute claims the details view and nothing that merely starts like it', () => {
    const { win, theme } = onDetail();
    const yes = [
        '#/details?id=1&serverId=s', '#!/details?id=1', '#/details',
        '#/details/', '#/details?id=1&context=tvshows'
    ];
    for (const hash of yes) {
        win.location.hash = hash;
        assert.ok(theme.isDetailRoute(), hash);
    }
    const no = ['#/home.html', '#/movies.html', '#/detailsomething?id=1', '#/search', ''];
    for (const hash of no) {
        win.location.hash = hash;
        assert.ok(!theme.isDetailRoute(), hash);
    }
});

test('detailIdFromHash reads the id wherever the query puts it', () => {
    const { win, theme } = onDetail();
    win.location.hash = '#/details?id=abc123&serverId=s';
    assert.strictEqual(theme.detailIdFromHash(), 'abc123');
    // jf-web orders these differently depending on where the click came from.
    win.location.hash = '#/details?serverId=s&id=def456&context=tvshows';
    assert.strictEqual(theme.detailIdFromHash(), 'def456');
    win.location.hash = '#/details';
    assert.strictEqual(theme.detailIdFromHash(), null);
});

test('detailTypeFor names the four kinds the one template renders', () => {
    const { theme } = onDetail();
    assert.strictEqual(theme.detailTypeFor({ Type: 'Movie' }), 'movie');
    assert.strictEqual(theme.detailTypeFor({ Type: 'Series' }), 'series');
    assert.strictEqual(theme.detailTypeFor({ Type: 'Season' }), 'season');
    assert.strictEqual(theme.detailTypeFor({ Type: 'Episode' }), 'episode');
    // A details route can also be a box set, a person or a music album; the
    // eyebrow has no label for those and the attribute must not invent one.
    assert.strictEqual(theme.detailTypeFor({ Type: 'BoxSet' }), 'other');
    assert.strictEqual(theme.detailTypeFor({}), 'other');
    assert.strictEqual(theme.detailTypeFor(null), 'other');
});

test('refresh on a details route sets af-detail alone and drops it on the way out', () => {
    const { win, theme } = onDetail();
    const html = win.document.documentElement;
    theme.refresh();
    assert.ok(html.classList.contains('af-detail'));
    assert.ok(!html.classList.contains('af-home'), 'never both');
    assert.ok(!html.classList.contains('af-library'), 'never both');

    win.location.hash = '#/movies.html?topParentId=lib1';
    theme.refresh();
    assert.ok(!html.classList.contains('af-detail'));
    assert.ok(html.classList.contains('af-library'));
});

test('detailFacts reads a full movie file', () => {
    const { theme } = onDetail();
    const facts = theme.detailFacts(fullMovie(), { videoMode: 'live-action' });
    assert.strictEqual(facts.eyebrow, 'File');
    assert.strictEqual(facts.headline, null);
    assert.deepStrictEqual(facts.rows.map((r) => [r.label, r.value]), [
        ['Video', 'HEVC 4K · HDR10'],
        ['Audio', 'TRUEHD 7.1'],
        // Four distinct languages, one of them twice: three plus a count.
        ['Subtitles', 'ENG · FRA · JPN +1'],
        ['Mode', 'Live-Action'],
        ['Size', '38.2 GB']
    ]);
    assert.strictEqual(facts.rows[3].tone, 'accent', 'the mode is app state, not file metadata');
});

test('detailFacts never throws on a movie with no media sources', () => {
    const { theme } = onDetail();
    const facts = theme.detailFacts({ Id: 'm', Name: 'Unscanned', Type: 'Movie' });
    assert.strictEqual(facts.eyebrow, 'File');
    // Every row would have been empty, so the panel renders as nothing at all
    // rather than as a stack of dashes.
    assert.deepStrictEqual(facts.rows, []);
});

test('detailFacts reads a series, with and without a next-up episode', () => {
    const { theme } = onDetail();
    const series = {
        Id: 's1',
        Name: "Frieren: Beyond Journey's End",
        Type: 'Series',
        Status: 'Continuing',
        Studios: [{ Name: 'Nippon TV' }, { Name: 'Madhouse' }],
        AirDays: ['Friday'],
        AirTime: '11:00 PM'
    };
    const bare = theme.detailFacts(series, { videoMode: 'anime' });
    assert.strictEqual(bare.eyebrow, 'Series');
    assert.strictEqual(bare.headline, null);
    assert.deepStrictEqual(bare.rows.map((r) => [r.label, r.value]), [
        ['Network', 'Nippon TV'],
        ['Status', 'Continuing'],
        ['Airs', 'Friday 11:00 PM'],
        ['Mode', 'Animation']
    ]);

    const withNext = theme.detailFacts(series, {
        nextUp: {
            Name: 'Aura the Guillotine', ParentIndexNumber: 1, IndexNumber: 9,
            RunTimeTicks: 14400000000, UserData: { PlaybackPositionTicks: 7800000000 }
        }
    });
    assert.strictEqual(withNext.eyebrow, 'Next up');
    assert.strictEqual(withNext.headline, 'S1 E9 · Aura the Guillotine');
    assert.strictEqual(withNext.sub, '11m left');
});

test('detailFacts counts a season', () => {
    const { theme } = onDetail();
    const facts = theme.detailFacts({
        Id: 'se1', Name: 'Season 1', Type: 'Season',
        ChildCount: 28, UserData: { UnplayedItemCount: 20 }
    }, {});
    assert.strictEqual(facts.eyebrow, 'Season');
    assert.deepStrictEqual(facts.rows.map((r) => [r.label, r.value]), [
        ['Episodes', '28'],
        ['Watched', '8 of 28']
    ]);

    // A season the server has not counted yields no rows rather than "NaN".
    const empty = theme.detailFacts({ Id: 'se2', Type: 'Season' }, {});
    assert.deepStrictEqual(empty.rows, []);
});

test('the facts panel is inserted once at the head of the right column and updated in place', () => {
    const { win, detail, theme } = onDetail();
    const first = theme.renderDetailPanel(fullMovie());
    assert.strictEqual(first.id, 'af-detail-panel');
    assert.strictEqual(detail.secondary.children[0], first, 'first child of the right column');
    assert.strictEqual(win.document.querySelectorAll('#af-detail-panel').length, 1);
    assert.deepStrictEqual(panelRows(first)[0], ['Video', 'HEVC 4K · HDR10']);

    const second = theme.renderDetailPanel({
        Id: 'item-2', Name: 'Dune', Type: 'Movie',
        MediaSources: [{ MediaStreams: [{ Type: 'Video', Codec: 'av1', Width: 1920 }] }]
    });
    assert.strictEqual(second, first, 'the same node, rewritten');
    assert.strictEqual(win.document.querySelectorAll('#af-detail-panel').length, 1);
    assert.deepStrictEqual(panelRows(second), [['Video', 'AV1 1080p']]);
    assert.strictEqual(second.hidden, false);

    // Nothing to say: the panel stays in place but paints nothing.
    theme.renderDetailPanel({ Id: 'item-3', Type: 'Movie' });
    assert.strictEqual(first.hidden, true);
    assert.deepStrictEqual(panelRows(first), []);
});

test('resumeLabel only speaks when there is a resume position to report', () => {
    const { theme } = onDetail();
    assert.strictEqual(theme.resumeLabel(fullMovie()), '1h 32m left');
    assert.strictEqual(theme.resumeLabel({ RunTimeTicks: 98640000000 }), null);
    // Watched to the end: jellyfin keeps the position, the pill must not.
    assert.strictEqual(theme.resumeLabel({
        RunTimeTicks: 100, UserData: { PlaybackPositionTicks: 100 }
    }), null);
});

test('markResumeButton writes the remaining time as an attribute and clears it', () => {
    const { detail, theme } = onDetail();
    theme.markResumeButton(fullMovie());
    assert.strictEqual(detail.play.getAttribute('data-af-left'), '1h 32m left');

    // A start-from-scratch play button says nothing, and the stale value goes.
    detail.play.setAttribute('data-action', 'play');
    theme.markResumeButton(fullMovie());
    assert.strictEqual(detail.play.getAttribute('data-af-left'), null);
});

test('entering a details route stamps the type, paints the panel and drives the art', async () => {
    const item = fullMovie();
    const api = makeThemeApiClient({ items: new Map([['item-1', item]]) });
    const { win, detail, theme } = onDetail({ api });
    await settle();

    const html = win.document.documentElement;
    assert.strictEqual(html.getAttribute('data-af-detail-type'), 'movie');
    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
    win.images[0].onload();
    assert.ok(html.classList.contains('af-backdrop'));
    assert.strictEqual(detail.secondary.children[0].id, 'af-detail-panel');
    assert.strictEqual(detail.play.getAttribute('data-af-left'), '1h 32m left');

    // Leaving takes all of it back down: a stale type would keep the eyebrow
    // saying "Movie" over the next page's title.
    win.location.hash = '#/home.html';
    theme.leaveDetail();
    assert.strictEqual(html.getAttribute('data-af-detail-type'), null);
    assert.strictEqual(win.document.querySelector('#af-detail-panel'), null);
    assert.strictEqual(theme.state().currentBackdropUrl, null);
});

test('a season page is a details route of its own, so the gate re-fetches', async () => {
    const series = { Id: 'item-1', Name: 'Frieren', Type: 'Series', Status: 'Continuing' };
    const season = { Id: 'season-2', Name: 'Season 1', Type: 'Season', ChildCount: 28 };
    const api = makeThemeApiClient({
        items: new Map([['item-1', series], ['season-2', season]])
    });
    const { win, theme } = onDetail({ api });
    await settle();
    assert.strictEqual(win.document.documentElement.getAttribute('data-af-detail-type'), 'series');

    win.location.hash = '#/details?id=season-2&serverId=srv';
    theme.refresh();
    await settle();
    assert.strictEqual(win.document.documentElement.getAttribute('data-af-detail-type'), 'season');
    assert.strictEqual(theme.state().detailId, 'season-2');
});

test('backdropSourceFor names the branch of the fallback chain the item lands on', () => {
    const { theme } = onDetail();
    assert.strictEqual(theme.backdropSourceFor({ BackdropImageTags: ['t'] }), 'backdrop');
    assert.strictEqual(theme.backdropSourceFor({
        ParentBackdropItemId: 'p', ParentBackdropImageTags: ['t']
    }), 'parent');
    // A poster cropped to a 16:9 canvas; the sheet blurs this one back towards
    // a wash rather than showing somebody's chin at 1708px wide.
    assert.strictEqual(theme.backdropSourceFor({ ImageTags: { Primary: 't' } }), 'primary');
    // An empty tag array is not art, and neither is a parent id on its own.
    assert.strictEqual(theme.backdropSourceFor({ BackdropImageTags: [] }), null);
    assert.strictEqual(theme.backdropSourceFor({ ParentBackdropItemId: 'p' }), null);
    assert.strictEqual(theme.backdropSourceFor({}), null);
    assert.strictEqual(theme.backdropSourceFor(null), null);
});

test('entering a details route stamps where the art came from, and leaving clears it', async () => {
    const item = fullMovie();
    const api = makeThemeApiClient({ items: new Map([['item-1', item]]) });
    const { win, theme } = onDetail({ api });
    await settle();
    const html = win.document.documentElement;
    assert.strictEqual(html.getAttribute('data-af-backdrop-src'), 'backdrop');
    theme.leaveDetail();
    assert.strictEqual(html.getAttribute('data-af-backdrop-src'), null);
});

test('the facts panel repainting itself does not queue a refresh', () => {
    const { win, theme } = onDetail();
    theme.renderDetailPanel(fullMovie());
    win.timers.runAll();
    theme.renderDetailPanel(fullMovie());
    assert.strictEqual(theme.state().refreshQueued, false);
});
