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
// Everything else here is what the runtime still does: the backdrop crossfade,
// the route tests, the focus tracking that drives the art from the card under
// the pointer (on Home and the library grids, and only once its fetch comes
// back for the card that is still selected), the Home-only controller hint,
// and the detail gate that marks those pages and otherwise leaves them alone.
// The v0.6.0 card popout was removed on 2026-09-18; one test below pins that
// nothing builds it again.
//
// The fake DOM comes from src/web/test/player-fakes.js; the Home page, the
// cards and the ApiClient the theme reads come from src/web/test/theme-fakes.js.
const test = require('node:test');
const assert = require('node:assert');

const { loadModule } = require('./test/player-fakes.js');
const {
    makeThemeWindow, loadTheme, installThemeStyle, buildHome, makeCard,
    makeThemeApiClient, observersFor
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

// The item detail page as jf-web 10.11.11 renders it, trimmed to the parts an
// earlier theme used to rearrange:
//   .mainAnimatedPages
//     > #itemDetailPage.page.libraryPage.itemDetailPage.selfBackdropPage
//       > .detailLogo
//       > .detailPageWrapperContainer
//         > .detailPagePrimaryContainer > .detailRibbon > .mainDetailButtons
//         > .detailPageSecondaryContainer > .detailPageContent
// The detail pages keep jellyfin-web's stock layout and only take Astrofin's
// colours from the sheet, so the tests use this scaffold to prove the runtime
// leaves every node of it where jellyfin-web put it.
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
    // jf-web renders .detailImageContainer twice: .hide-mobile beside the
    // ribbon and .hide-desktop inside it. Both stay exactly where they are.
    const poster = doc.createElement('div');
    poster.className = 'detailImageContainer hide-mobile';
    const posterCard = doc.createElement('div');
    posterCard.className = 'card portraitCard';
    poster.appendChild(posterCard);
    primary.appendChild(poster);
    const posterMobile = doc.createElement('div');
    posterMobile.className = 'detailImageContainer hide-desktop hide-tv';
    ribbon.appendChild(posterMobile);

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
    return { pages, page, logo, wrapper, primary, ribbon, buttons, play, secondary,
        content, poster, posterMobile };
}

// A window with the theme installed and a details route in the address bar.
// `api` is attached before the theme loads, because start() calls refresh():
// a client attached afterwards could not show that the first refresh on a
// details route fetches nothing.
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

// A movie with art of its own, so a test can tell "did not fetch" and "did not
// paint" apart from "had nothing to paint".
function movieWithArt(id) {
    return {
        Id: id,
        Name: 'Blade Runner 2049',
        Type: 'Movie',
        RunTimeTicks: 98640000000,
        UserData: { PlaybackPositionTicks: 43200000000 },
        BackdropImageTags: ['bt']
    };
}

// A window on Home whose ApiClient knows `items`, plus one card in the first
// rail for the first of them. What every focus test starts from.
function homeWithCard(items = [movieWithArt('one')]) {
    const ctx = onHome();
    ctx.api = makeThemeApiClient({ items: new Map(items.map((it) => [it.Id, it])) });
    ctx.win.ApiClient = ctx.api;
    ctx.card = makeCard(ctx.win, { id: items[0].Id, parent: ctx.home.sections[0] });
    return ctx;
}

// Identity check for fake DOM nodes. assert.strictEqual would read better, but
// when it fails it inspects the nodes it was handed, and a fake node's
// parentNode/ownerDocument graph takes minutes to print: a regression would
// hang the suite instead of failing it.
function sameNode(actual, expected, message) {
    assert.ok(actual === expected, message || 'not the expected node');
}

// Nothing the removed card popout used to build or stamp is in the document.
function assertNoPopout(win) {
    const doc = win.document;
    assert.strictEqual(doc.querySelectorAll('#af-popout').length, 0, 'no #af-popout');
    assert.strictEqual(doc.querySelectorAll('.af-popped').length, 0, 'no card is .af-popped');
    const drawerBits = doc.querySelectorAll('*').filter((el) =>
        String(el.className).split(/\s+/).some((c) => c.startsWith('af-po-')));
    assert.strictEqual(drawerBits.length, 0, 'no .af-po-* node');
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
    sameNode(win.document.getElementById('af-space'), spaceAfterFirst);
    assert.strictEqual(win.document.querySelectorAll('#af-space').length, 1);
    assert.strictEqual(win.document.querySelectorAll('#af-hint').length, 1);
    assertNoPopout(win);
});

test('a third and fourth run still leave exactly one of every panel', () => {
    const { win } = onHome();
    loadModule('astrofin-theme.js', win);
    loadModule('astrofin-theme.js', win);
    for (const id of ['af-space', 'af-hint']) {
        assert.strictEqual(win.document.querySelectorAll('#' + id).length, 1, id);
    }
    assertNoPopout(win);
});

test('buildUi builds the controller hint alone, hidden, at the end of body', () => {
    const { win, theme } = onHome();
    const ui = theme.state().ui;
    // The hint is the whole of the Home chrome: the server card and then the
    // card popout were both removed at the owner's request.
    assert.deepStrictEqual(Object.keys(ui), ['hint']);
    sameNode(win.document.getElementById('af-hint'), ui.hint);
    sameNode(ui.hint.parentNode, win.document.body);
    assert.strictEqual(ui.hint.hidden, true, 'nothing shows until a card is selected');
    assert.strictEqual(ui.hint.getAttribute('aria-hidden'), 'true');
    assert.deepStrictEqual(
        ui.hint.children.map((el) => el.textContent),
        ['◀ ▶ MOVE', '✕ SELECT', '◯ BACK', 'OPTIONS ⋯']
    );
    sameNode(win.document.getElementById('af-spotlight'), null, 'the band is gone');
    assertNoPopout(win);
});

test('buildUi drops a hint a previous execution left behind', () => {
    // A fresh V8 context over a document jellyfin-web never tore down: the
    // old handle is gone with the old context, but its node is still there.
    const win = makeThemeWindow({ hash: '#/home.html' });
    buildHome(win);
    const stale = win.document.createElement('div');
    stale.id = 'af-hint';
    win.document.body.appendChild(stale);

    const theme = loadTheme(win);
    assert.strictEqual(win.document.querySelectorAll('#af-hint').length, 1);
    assert.ok(theme.state().ui.hint !== stale, 'rebuilt, not shadowed');
    assert.strictEqual(stale.isConnected, false, 'the old one was dropped');
});

test('the module survives a document that has no body yet', () => {
    const win = makeThemeWindow();
    win.document.documentElement.removeChild(win.document.body);
    win.document.body = null;
    const theme = loadTheme(win);
    sameNode(theme.state().space, null);
    sameNode(theme.state().ui, null);
});

// ---------------------------------------------------------------------------
// guard
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

// ---------------------------------------------------------------------------
// Stylesheet ordering
// ---------------------------------------------------------------------------

test('keepThemeLast moves the sheet to the end of body when a later sheet appears', () => {
    const win = makeThemeWindow();
    const style = installThemeStyle(win);
    const theme = loadTheme(win);
    sameNode(style.parentNode, win.document.body, 'moved out of head');

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
    sameNode(meta.parentNode, win.document.head);
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
    assert.ok(clone !== first);
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

test('going into video mode clears the backdrop and drops the selection', async () => {
    const { win, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
    assert.ok(card.classList.contains('af-focused'));

    const container = win.document.createElement('div');
    container.className = 'videoPlayerContainer';
    win.document.body.appendChild(container);
    theme.updateVideoMode();
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
    // The sheet hides the ring either way; leaving the selection in state
    // means it comes back on the card the pointer left behind when playback
    // ends.
    sameNode(theme.state().focusedCard, null);
    assert.ok(!card.classList.contains('af-focused'));
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
// Focus tracking
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
    sameNode(theme.state().focusedCard, two);
    theme.setFocusedCard(null);
    sameNode(theme.state().focusedCard, null);
    assert.ok(!two.classList.contains('af-focused'));
});

test('focusing a card on Home drives the backdrop from its item and brings the hint up', async () => {
    const { win, card, theme } = homeWithCard();
    const hint = theme.state().ui.hint;
    assert.strictEqual(hint.hidden, true, 'the route alone does not show it');

    theme.setFocusedCard(card);
    assert.ok(card.classList.contains('af-focused'), 'marked at once, before the fetch');
    await settle();
    assert.match(theme.state().currentBackdropUrl, /Items\/one\/Images\/Backdrop/);
    win.images[0].onload();
    assert.ok(theme.state().backdropLayers[0].classList.contains('af-on'));
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
    // The Home chrome comes up with the first card, not with the route.
    assert.strictEqual(theme.state().overlaysWanted, true);
    assert.strictEqual(hint.hidden, false);
    assert.ok(hint.classList.contains('af-show'));
    assertNoPopout(win);
});

test('dropping the selection takes the ring off and leaves the art up', async () => {
    const { win, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();

    theme.setFocusedCard(null);
    assert.ok(!card.classList.contains('af-focused'));
    // The backdrop is the page background: clearing it on every pointer exit
    // would strobe the art across a rail.
    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
});

test('a fetch that comes back for a card the pointer has left is dropped', async () => {
    const { win, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    // Dropping the selection does not bump the request token, so the token
    // alone would not catch this; the card check in the callback does.
    theme.setFocusedCard(null);
    await settle();
    assert.strictEqual(theme.state().currentBackdropUrl, null, 'a slow server paints nothing');
    assert.strictEqual(win.images.length, 0);
    assert.strictEqual(theme.state().overlaysWanted, false);
    assert.strictEqual(theme.state().ui.hint.hidden, true);
});

test('a fetch for an earlier card never paints over the card selected after it', async () => {
    const { win, home, theme, card } = homeWithCard([movieWithArt('one'), movieWithArt('two')]);
    const two = makeCard(win, { id: 'two', parent: home.sections[1] });
    theme.setFocusedCard(card);
    theme.setFocusedCard(two);
    await settle();
    assert.match(theme.state().currentBackdropUrl, /Items\/two\//);
    assert.strictEqual(win.images.length, 1, 'the superseded fetch requested nothing');
});

test('a card with no data-id, or off Home and the grids, fetches nothing', () => {
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

test('focusing a card on a library route drives the backdrop there too, without the hint', async () => {
    const { win, library, theme } = onLibrary();
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', movieWithArt('one')]]) });
    const card = makeCard(win, { id: 'one', parent: library.grid });
    theme.setFocusedCard(card);
    await settle();

    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
    win.images[0].onload();
    assert.ok(theme.state().backdropLayers[0].classList.contains('af-on'));
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
    assert.ok(card.classList.contains('af-focused'));
    // The hint is Home-only chrome: on a grid it is never asked for.
    assert.strictEqual(theme.state().overlaysWanted, false);
    const ui = theme.state().ui;
    assert.ok(!ui || ui.hint.hidden, 'no hint on a library grid');
    assertNoPopout(win);
});

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

test('cardFrom only claims cards inside the home rails', () => {
    const { win, home, theme } = onHome();
    const card = makeCard(win, { parent: home.sections[0] });
    sameNode(theme.cardFrom(card.querySelector('.cardScalable')), card);

    const stray = makeCard(win, { parent: win.document.body });
    sameNode(theme.cardFrom(stray), null);
    sameNode(theme.cardFrom(null), null);
    sameNode(theme.cardFrom(win.document.body), null);
});

test('cardFrom also claims cards inside a library grid', () => {
    const { win, library, theme } = onLibrary();
    const card = makeCard(win, { parent: library.grid });
    sameNode(theme.cardFrom(card.querySelector('.cardScalable')), card);

    // Same page, but outside the grid: jellyfin-web keeps that card.
    const stray = makeCard(win, { parent: library.tab });
    sameNode(theme.cardFrom(stray), null);
});

test('focusin selects the card immediately and cancels a pending hover', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const hovered = makeCard(win, { id: 'hovered', parent: home.sections[0] });
    const focused = makeCard(win, { id: 'focused', parent: home.sections[1] });
    theme.onPointerOver({ target: hovered });
    assert.ok(theme.state().hoverTimer, 'hover is pending');
    theme.onFocusIn({ target: focused });
    sameNode(theme.state().focusedCard, focused);
    win.timers.advance(500);
    sameNode(theme.state().focusedCard, focused, 'the hover was cancelled');
});

test('hover is debounced and re-anchors to whichever card the pointer reaches', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const one = makeCard(win, { id: 'one', parent: home.sections[0] });
    const two = makeCard(win, { id: 'two', parent: home.sections[1] });
    theme.onPointerOver({ target: one });
    assert.ok(theme.state().hoverTimer, 'debounced, not immediate');
    sameNode(theme.state().focusedCard, null, 'not yet');
    win.timers.advance(120);
    sameNode(theme.state().focusedCard, one);

    theme.onPointerOver({ target: two });
    win.timers.advance(120);
    sameNode(theme.state().focusedCard, two, 're-anchored to the new card');
});

test('hovering the card that is already focused does nothing', () => {
    const { win, home, theme } = onHome();
    win.ApiClient = makeThemeApiClient();
    const card = makeCard(win, { parent: home.sections[0] });
    theme.setFocusedCard(card);
    theme.onPointerOver({ target: card });
    assert.strictEqual(theme.state().hoverTimer, 0);
});

test('a pointer leaving the cards drops the selection but keeps the art', async () => {
    const { win, home, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();

    // A rail heading, or the page background: anything that is not a card.
    const heading = win.document.createElement('h2');
    home.sections[0].appendChild(heading);
    theme.onPointerOver({ target: heading });
    sameNode(theme.state().focusedCard, null);
    assert.ok(!card.classList.contains('af-focused'));
    // The art is the page background and stays up until the next card, so
    // sweeping across a rail does not strobe it.
    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
});

test('a pointer leaving the cards cancels a pending hover and drops an art-less selection', () => {
    const { win, home, theme } = onHome();
    // No item behind either card: the fetch fails and nothing is ever painted,
    // which is the case where only the ring says a card is selected.
    win.ApiClient = makeThemeApiClient();
    const one = makeCard(win, { id: 'one', parent: home.sections[0] });
    const two = makeCard(win, { id: 'two', parent: home.sections[1] });
    theme.setFocusedCard(one);
    theme.onPointerOver({ target: two });
    assert.ok(theme.state().hoverTimer, 'hover is pending');

    theme.onPointerOver({ target: win.document.body });
    assert.strictEqual(theme.state().hoverTimer, 0, 'the pending hover is cancelled');
    sameNode(theme.state().focusedCard, null);
    assert.ok(!one.classList.contains('af-focused'));
    win.timers.advance(500);
    sameNode(theme.state().focusedCard, null, 'the cancelled hover never lands');
    assert.ok(!two.classList.contains('af-focused'));
});

// ---------------------------------------------------------------------------
// Visibility, refresh and the page observer
// ---------------------------------------------------------------------------

test('ensureUi rebuilds the hint when jellyfin-web has orphaned it', () => {
    const { win, theme } = onHome();
    const first = theme.state().ui;
    theme.ensureUi();
    sameNode(theme.state().ui, first, 'still connected: kept');
    first.hint.remove();
    theme.ensureUi();
    assert.ok(theme.state().ui !== first, 'orphaned: rebuilt');
    assert.ok(theme.state().ui.hint.isConnected);
    assert.strictEqual(win.document.querySelectorAll('#af-hint').length, 1);
});

test('showOverlays shows the controller hint on Home and hides it elsewhere', () => {
    const { win, theme } = onHome();
    theme.showOverlays(true);
    const ui = theme.state().ui;
    assert.strictEqual(ui.hint.hidden, false);
    assert.ok(ui.hint.classList.contains('af-show'));

    win.location.hash = '#/details?id=1';
    theme.showOverlays();
    assert.strictEqual(ui.hint.hidden, true);
    assert.ok(!ui.hint.classList.contains('af-show'));
});

test('leaveHome drops the selection, the art and the hint', async () => {
    const { win, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    assert.strictEqual(theme.state().ui.hint.hidden, false);

    theme.leaveHome();
    const state = theme.state();
    sameNode(state.focusedCard, null);
    assert.strictEqual(state.currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
    assert.strictEqual(state.overlaysWanted, false);
    assert.ok(!card.classList.contains('af-focused'));
    assert.strictEqual(state.ui.hint.hidden, true);
    assert.ok(!state.ui.hint.classList.contains('af-show'));
});

test('leaveHome for a library grid keeps the selection and the art', async () => {
    const { win, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    const art = theme.state().currentBackdropUrl;

    theme.leaveHome(true);
    const state = theme.state();
    sameNode(state.focusedCard, card, 'the grid goes on hovering it');
    assert.ok(card.classList.contains('af-focused'));
    assert.strictEqual(state.currentBackdropUrl, art, 'the art stays up');
    assert.ok(win.document.documentElement.classList.contains('af-backdrop'));
    // The hint is Home chrome and goes either way.
    assert.strictEqual(state.overlaysWanted, false);
    assert.strictEqual(state.ui.hint.hidden, true);
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

test('refresh on Home marks the root and builds the hint', () => {
    const { win, theme } = onHome();
    theme.refresh();
    assert.ok(win.document.documentElement.classList.contains('af-home'));
    assert.ok(theme.state().ui, 'the hint exists');
    assert.ok(theme.state().ui.hint.isConnected);
    // Neither the server card nor the card popout is built any more.
    assert.strictEqual(win.document.querySelectorAll('#af-server-panel').length, 0);
    assertNoPopout(win);
});

test('refresh on Home keeps the selected card and its art while the rails stream in', async () => {
    const { win, home, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    const art = theme.state().currentBackdropUrl;

    // Every batch of cards jellyfin-web streams into the rails queues one.
    makeCard(win, { id: 'late', parent: home.sections[1] });
    win.timers.runAll();
    sameNode(theme.state().focusedCard, card);
    assert.ok(card.classList.contains('af-focused'));
    assert.strictEqual(theme.state().currentBackdropUrl, art);
    assert.strictEqual(theme.state().ui.hint.hidden, false);
});

test('refresh off Home clears af-home and tears the selection, art and hint down', async () => {
    const { win, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    win.location.hash = '#/search';
    theme.refresh();
    const html = win.document.documentElement;
    assert.ok(!html.classList.contains('af-home'));
    sameNode(theme.state().focusedCard, null);
    assert.ok(!card.classList.contains('af-focused'));
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!html.classList.contains('af-backdrop'));
    assert.strictEqual(theme.state().ui.hint.hidden, true);
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
    sameNode(theme.state().focusedCard, card);
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

test('refresh leaving a library grid for Home drops the grid selection', async () => {
    const { win, library, theme } = onLibrary();
    win.ApiClient = makeThemeApiClient({ items: new Map([['one', movieWithArt('one')]]) });
    const card = makeCard(win, { id: 'one', parent: library.grid });
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    theme.refresh();
    sameNode(theme.state().focusedCard, card, 'the grid selection is live');
    assert.match(theme.state().currentBackdropUrl, /Images\/Backdrop/);

    // The card is going away with the outgoing page.
    buildHome(win);
    win.location.hash = '#/home.html';
    theme.refresh();
    sameNode(theme.state().focusedCard, null);
    assert.ok(!card.classList.contains('af-focused'));
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!win.document.documentElement.classList.contains('af-backdrop'));
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

test('selecting a card and showing the hint do not queue a refresh', async () => {
    const { win, card, theme } = homeWithCard();
    win.timers.runAll();
    assert.strictEqual(theme.state().refreshQueued, false);
    // .af-focused is an attribute write, which the rail observer does not ask
    // for, and the hint lives in <body>, outside .mainAnimatedPages. Nothing
    // Astrofin draws lives in that subtree, so the observer needs no filter.
    theme.setFocusedCard(card);
    await settle();
    theme.showOverlays(true);
    theme.decorateCards();
    assert.strictEqual(theme.state().ui.hint.hidden, false);
    assert.strictEqual(theme.state().refreshQueued, false);
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
    sameNode(win.document.getElementById('af-space'), null, 'nothing built yet');
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

test('leaving a details route for Home or an unthemed page drops af-detail too', () => {
    const { win, theme } = onDetail();
    const html = win.document.documentElement;
    theme.refresh();
    assert.ok(html.classList.contains('af-detail'));

    buildHome(win);
    win.location.hash = '#/home.html';
    theme.refresh();
    assert.ok(!html.classList.contains('af-detail'));
    assert.ok(html.classList.contains('af-home'));

    // A season page is a details route of its own; coming back to one from
    // Home puts the class straight back.
    win.location.hash = '#/details?id=season-2&serverId=srv';
    theme.refresh();
    assert.ok(html.classList.contains('af-detail'));
    assert.ok(!html.classList.contains('af-home'), 'never both');

    win.location.hash = '#/search';
    theme.refresh();
    assert.ok(!html.classList.contains('af-detail'));
    assert.ok(!html.classList.contains('af-home'));
    assert.ok(!html.classList.contains('af-library'));
});

test('a details route fetches nothing, builds nothing and leaves the stock page alone', async () => {
    const api = makeThemeApiClient({ items: new Map([['item-1', movieWithArt('item-1')]]) });
    const { win, detail, theme } = onDetail({ api });
    theme.refresh();
    await settle();
    win.timers.runAll();
    await settle();

    const html = win.document.documentElement;
    assert.ok(html.classList.contains('af-detail'));
    // The item JSON used to be fetched once per route for the facts panel, the
    // type attribute and the art. The page keeps jellyfin-web's own now, so the
    // item is never asked for, even though the client could have answered.
    assert.deepStrictEqual(api.calls.filter((c) => c[0] === 'getItem'), []);
    assert.strictEqual(win.images.length, 0, 'no backdrop was requested');
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!html.classList.contains('af-backdrop'));
    sameNode(win.document.getElementById('af-detail-panel'), null);
    assert.strictEqual(html.getAttribute('data-af-detail-type'), null);
    assert.strictEqual(html.getAttribute('data-af-backdrop-src'), null);
    assertNoPopout(win);

    // jellyfin-web's own layout, node for node: neither poster copy moved,
    // nothing inserted into the right column, the Resume pill unannotated.
    sameNode(detail.poster.parentNode, detail.primary);
    sameNode(detail.posterMobile.parentNode, detail.ribbon);
    assert.strictEqual(detail.secondary.children.length, 1);
    sameNode(detail.secondary.children[0], detail.content);
    assert.strictEqual(detail.play.getAttribute('data-af-left'), null);
});

test('entering a details route from Home clears the art and the selection', async () => {
    const { win, api, card, theme } = homeWithCard();
    theme.setFocusedCard(card);
    await settle();
    win.images[0].onload();
    const html = win.document.documentElement;
    assert.ok(html.classList.contains('af-backdrop'), 'the card is driving the art');
    assert.ok(card.classList.contains('af-focused'));

    buildDetail(win);
    win.location.hash = '#/details?id=item-9&serverId=srv';
    theme.refresh();
    await settle();
    assert.ok(html.classList.contains('af-detail'));
    // A details page shows jellyfin-web's own art band, so the backdrop the
    // card put up goes with the card. It used to stay until the fetched
    // item's own art replaced it.
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!html.classList.contains('af-backdrop'));
    sameNode(theme.state().focusedCard, null);
    assert.ok(!card.classList.contains('af-focused'));
    assert.strictEqual(theme.state().ui.hint.hidden, true, 'the hint is Home-only');
    assert.deepStrictEqual(
        api.calls.filter((c) => c[0] === 'getItem').map((c) => c[2]),
        ['one'], 'the card fetch only, never the details item');
});

test('a refresh on a details route keeps no art or selection alive', () => {
    const { win, theme } = onDetail();
    const html = win.document.documentElement;
    // Details pages used to keep the backdrop across a refresh: the item's own
    // art was on it, and a page streaming in refreshes constantly. That art is
    // gone, so a refresh here now tears down like any other unthemed route.
    theme.setBackdrop('https://server/stale.jpg');
    win.images[0].onload();
    assert.ok(html.classList.contains('af-backdrop'));
    const card = makeCard(win, { id: 'stale' });
    theme.setFocusedCard(card);
    assert.ok(card.classList.contains('af-focused'));

    theme.refresh();
    assert.ok(html.classList.contains('af-detail'));
    assert.strictEqual(theme.state().currentBackdropUrl, null);
    assert.ok(!html.classList.contains('af-backdrop'));
    sameNode(theme.state().focusedCard, null);
    assert.ok(!card.classList.contains('af-focused'));
});

test('a details page streaming in queues one refresh, and the refresh writes nothing back', () => {
    const { win, detail, theme } = onDetail();
    assert.strictEqual(theme.state().pagesObserved, true);
    assert.strictEqual(observersFor(win, detail.pages).length, 1);
    win.timers.runAll();
    assert.strictEqual(theme.state().refreshQueued, false);

    // The .mainAnimatedPages observer filters nothing any more: the facts
    // panel it used to skip is gone, so every record under it is
    // jellyfin-web's own.
    detail.content.appendChild(win.document.createElement('div'));
    assert.strictEqual(theme.state().refreshQueued, true);
    win.timers.runAll();
    // Had refresh() written into the page, the observer would have queued the
    // next one before this line, and the two would feed each other forever.
    assert.strictEqual(theme.state().refreshQueued, false);
    assert.strictEqual(detail.secondary.children.length, 1);
});

// ---------------------------------------------------------------------------
// The removed card popout
// ---------------------------------------------------------------------------

test('no #af-popout is ever created, on any route or any way a card is selected', async () => {
    const { win, home, card, theme } = homeWithCard([movieWithArt('one'), movieWithArt('two')]);
    assertNoPopout(win);

    // Hover, debounced, with the fetch coming back and the art going up.
    theme.onPointerOver({ target: card.querySelector('.cardScalable') });
    win.timers.advance(500);
    await settle();
    win.images[0].onload();
    sameNode(theme.state().focusedCard, card);
    assertNoPopout(win);

    // Keyboard/controller focus on another card.
    const two = makeCard(win, { id: 'two', parent: home.sections[1] });
    theme.onFocusIn({ target: two });
    await settle();
    win.timers.runAll();
    assertNoPopout(win);

    // A library grid, and a card selected on it.
    const library = buildLibrary(win);
    win.location.hash = '#/movies.html?topParentId=lib1';
    theme.refresh();
    const gridCard = makeCard(win, { id: 'one', parent: library.grid });
    theme.setFocusedCard(gridCard);
    await settle();
    win.timers.runAll();
    assertNoPopout(win);

    // A details page, then playback, then a fresh context re-running the file.
    buildDetail(win);
    win.location.hash = '#/details?id=one&serverId=srv';
    theme.refresh();
    const container = win.document.createElement('div');
    container.className = 'videoPlayerContainer';
    win.document.body.appendChild(container);
    theme.updateVideoMode();
    win.timers.runAll();
    loadModule('astrofin-theme.js', win);
    assertNoPopout(win);
});
