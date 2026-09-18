'use strict';
// Extra browser fakes for the Astrofin theme runtime tests.
//
// src/web/test/player-fakes.js owns the fake DOM, and since the card popout
// was removed (2026-09-18) the theme walks into none of its gaps: the box
// model, cloneNode(), a dispatching click() and real textContent semantics the
// popout needed are gone with it. What is left here are the jellyfin-web-shaped
// scaffolds the theme reads — a window with a viewport, a Home page with rails
// and cards, an ApiClient with items and images.
//
// Nothing here touches the real DOM, the network or a profile dir.

const { loadModule, makeWindow } = require('./player-fakes.js');

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

// A window the theme can install into. `hash` seeds location.hash so a test can
// start on Home ('#/home.html') or anywhere else.
function makeThemeWindow(overrides = {}) {
    const opts = Object.assign({}, overrides);
    const hash = opts.hash;
    delete opts.hash;
    // A viewport, because backdropUrlFor() sizes the art it asks for from the
    // window's width and pixel ratio.
    const win = makeWindow(Object.assign({
        innerWidth: 1280,
        innerHeight: 720,
        devicePixelRatio: 1
    }, opts));
    if (hash !== undefined) win.location.hash = hash;
    return win;
}

// Load astrofin-theme.js into `win`. The file installs itself at once because
// the fake document reports readyState 'complete'.
function loadTheme(win) {
    return loadModule('astrofin-theme.js', win);
}

// The <style id="af-theme"> the Rust preamble installs, appended wherever the
// caller wants it (default: <head>, which is what the preamble does).
function installThemeStyle(win, host) {
    const style = win.document.createElement('style');
    style.id = 'af-theme';
    (host || win.document.head).appendChild(style);
    return style;
}

// ---------------------------------------------------------------------------
// jellyfin-web scaffolds (selectors marked `jf-web 10.11.11` in the theme)
// ---------------------------------------------------------------------------

// .mainAnimatedPages > #homeTab > .homeSectionsContainer, plus `sections`
// rails. Returns the pieces the tests need to reach.
function buildHome(win, sectionCount = 2) {
    const doc = win.document;
    const pages = doc.createElement('div');
    pages.className = 'mainAnimatedPages';
    const homeTab = doc.createElement('div');
    homeTab.id = 'homeTab';
    const container = doc.createElement('div');
    container.className = 'homeSectionsContainer';
    homeTab.appendChild(container);
    pages.appendChild(homeTab);
    doc.body.appendChild(pages);

    const sections = [];
    for (let i = 0; i < sectionCount; i++) {
        const section = doc.createElement('div');
        section.className = 'verticalSection';
        container.appendChild(section);
        sections.push(section);
    }
    return { pages, homeTab, container, sections };
}

// A .card as the home rails and the library grids render it: the item id in
// data-id (`id: null` leaves it off), the type in data-type, the tile in
// .cardScalable, appended to `opts.parent` when one is given.
function makeCard(win, opts = {}) {
    const doc = win.document;
    const card = doc.createElement('div');
    card.className = 'card';
    if (opts.id !== null) card.setAttribute('data-id', opts.id || 'item-1');
    if (opts.type) card.setAttribute('data-type', opts.type);

    const scalable = doc.createElement('div');
    scalable.className = 'cardScalable';
    card.appendChild(scalable);

    if (opts.parent) opts.parent.appendChild(card);
    return card;
}

// window.ApiClient as the theme uses it: item lookups and image URLs. Every
// call is recorded.
function makeThemeApiClient(overrides = {}) {
    const calls = [];
    const items = overrides.items || new Map();
    return {
        calls,
        items,
        getCurrentUserId() { return overrides.userId || 'user-1'; },
        getItem(userId, id) {
            calls.push(['getItem', userId, id]);
            return items.has(id)
                ? Promise.resolve(items.get(id))
                : Promise.reject(new Error('no such item ' + id));
        },
        getScaledImageUrl(id, o) {
            calls.push(['getScaledImageUrl', id, o]);
            return 'https://server/Items/' + id + '/Images/' + o.type
                + '?maxWidth=' + o.maxWidth + '&tag=' + o.tag;
        }
    };
}

// The MutationObserver instances player-fakes records, filtered to one target.
function observersFor(win, target) {
    return win.observers.filter((obs) => obs.target === target && !obs.disconnected);
}

module.exports = {
    makeThemeWindow,
    loadTheme,
    installThemeStyle,
    buildHome,
    makeCard,
    makeThemeApiClient,
    observersFor
};
