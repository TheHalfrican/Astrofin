'use strict';
// Extra browser fakes for the Astrofin theme runtime tests.
//
// src/web/test/player-fakes.js owns the fake DOM; this file only fills the
// three gaps astrofin-theme.js walks into, then adds the jellyfin-web-shaped
// scaffolds the theme reads (a Home page with rails and cards, an ApiClient
// with images).
//
// The gaps, all patched on the prototypes exported by player-fakes.js:
//
//   1. `textContent` is a plain data property there, so `el.textContent = ''`
//      does not detach the children. The theme clears the popout's art slot
//      and its badge row exactly that way, so without real semantics a second
//      render would append to the first and the tests would be measuring the
//      fake.
//   2. there is no box model at all — nothing has a position or a size and
//      `getBoundingClientRect()` does not exist — but placePopout() is made of
//      nothing else. See `setRect` below: every box is one the test wrote.
//   3. `cloneNode()` does not exist, and cloneArt() deep-clones the card tile.
//
// Nothing here touches the real DOM, the network or a profile dir; the
// prototype patches are process-local and `node --test` gives each test file
// its own process.

const {
    loadModule, makeWindow, FakeElement
} = require('./player-fakes.js');

// ---------------------------------------------------------------------------
// DOM gaps
// ---------------------------------------------------------------------------

const TEXT_NODE = 3;

Object.defineProperty(FakeElement.prototype, 'textContent', {
    configurable: true,
    get() {
        if (this.nodeType === TEXT_NODE) return this._text || '';
        return this.childNodes.map((node) => node.textContent).join('');
    },
    set(value) {
        const text = value == null ? '' : String(value);
        if (this.nodeType === TEXT_NODE) {
            this._text = text;
            return;
        }
        while (this.childNodes.length) this.removeChild(this.childNodes[0]);
        if (text && this.ownerDocument && this.ownerDocument.createTextNode) {
            this.appendChild(this.ownerDocument.createTextNode(text));
        }
    }
});

// The theme drives jellyfin-web's delegated handlers by clicking real nodes
// (the card's own overlay button, or a throwaway .itemAction it appends), so
// `click()` has to dispatch a bubbling event rather than be a stub.
FakeElement.prototype.click = function click() {
    const event = {
        type: 'click',
        bubbles: true,
        target: this,
        defaultPrevented: false,
        preventDefault() { event.defaultPrevented = true; },
        stopPropagation() {}
    };
    this.dispatchEvent(event);
    return event;
};

// cloneArt() deep-clones the card's own .cardScalable into the popout. A real
// clone copies attributes, classes and inline style but never listeners, and
// belongs to the same document — which is what lets the theme drop the canvas
// and disarm the delegation in the copy without touching the card.
FakeElement.prototype.cloneNode = function cloneNode(deep) {
    const doc = this.ownerDocument;
    if (this.nodeType === TEXT_NODE) return doc.createTextNode(this.textContent);
    const copy = doc.createElement(this.tagName);
    this._attrs.forEach((value, name) => copy.setAttribute(name, value));
    copy.className = this.className;
    copy.hidden = this.hidden;
    Object.keys(this.style).forEach((name) => {
        if (typeof this.style[name] !== 'function') copy.style[name] = this.style[name];
    });
    if (deep) this.childNodes.forEach((child) => copy.appendChild(child.cloneNode(true)));
    return copy;
};

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

// The popout is placed entirely from measured boxes — the card's rect, the
// drawer's laid-out height, the header's bottom edge — so the fake has to
// have a box model. It is deliberately not a layout engine: a box is whatever
// the test wrote with setRect(), and an element nobody measured reads as 0x0
// at the origin. That is what a detached or display:none node reads as in a
// real browser, and it is the case placePopout() refuses to paint over.
const ZERO_RECT = Object.freeze({
    x: 0, y: 0, left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0
});

FakeElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
    return this._rect || ZERO_RECT;
};

// Give `el` a laid-out box, in viewport coordinates. offsetWidth/offsetHeight
// follow it, so the drawer whose height placePopout() reads is measured the
// same way as a card's art.
function setRect(el, box) {
    const left = Number(box.left || 0);
    const top = Number(box.top || 0);
    const width = Number(box.width || 0);
    const height = Number(box.height || 0);
    el._rect = {
        x: left,
        y: top,
        left,
        top,
        width,
        height,
        right: left + width,
        bottom: top + height
    };
    el.offsetWidth = width;
    el.offsetHeight = height;
    return el;
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

// A window the theme can install into. `hash` seeds location.hash so a test can
// start on Home ('#/home.html') or anywhere else.
function makeThemeWindow(overrides = {}) {
    const opts = Object.assign({}, overrides);
    const hash = opts.hash;
    delete opts.hash;
    // A viewport, because placePopout() clamps against both axes of it.
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

// A .card as the home rails render it. `opts.action` adds the primary overlay
// button, `opts.link` the delegated navigation target; both record clicks on
// the returned `clicks` array.
function makeCard(win, opts = {}) {
    const doc = win.document;
    const clicks = [];
    const card = doc.createElement('div');
    card.className = 'card';
    if (opts.id !== null) card.setAttribute('data-id', opts.id || 'item-1');
    if (opts.type) card.setAttribute('data-type', opts.type);
    if (opts.serverId) card.setAttribute('data-serverid', opts.serverId);

    const scalable = doc.createElement('div');
    scalable.className = 'cardScalable';
    card.appendChild(scalable);

    if (opts.action) {
        const btn = doc.createElement('button');
        btn.className = 'cardOverlayButton';
        btn.setAttribute('data-action', opts.action);
        btn.addEventListener('click', () => clicks.push(['overlay', opts.action]));
        scalable.appendChild(btn);
    }
    if (opts.link) {
        const link = doc.createElement('div');
        link.className = 'cardImageContainer';
        link.setAttribute('data-action', 'link');
        link.addEventListener('click', () => clicks.push(['link']));
        scalable.appendChild(link);
    }
    if (opts.parent) opts.parent.appendChild(card);
    card.clicks = clicks;
    return card;
}

// window.ApiClient as the theme uses it: item lookups, image URLs and the
// server identity the panel prints. Every call is recorded.
function makeThemeApiClient(overrides = {}) {
    const value = (v) => (typeof v === 'function' ? v() : v);
    const calls = [];
    const items = overrides.items || new Map();
    const client = {
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
        },
        // A function override stands in for the accessor itself, so a test can
        // make serverId() throw the way a disconnected ApiClient does.
        serverId() { return value(overrides.serverId); }
    };
    return Object.assign(client, overrides.extra || {});
}

// The MutationObserver instances player-fakes records, filtered to one target.
function observersFor(win, target) {
    return win.observers.filter((obs) => obs.target === target && !obs.disconnected);
}

// Text of every child element, in order — what a rendered chip strip or panel
// reads as.
function childTexts(el) {
    return el.children.map((child) => child.textContent);
}

module.exports = {
    makeThemeWindow,
    loadTheme,
    installThemeStyle,
    setRect,
    buildHome,
    makeCard,
    makeThemeApiClient,
    observersFor,
    childTexts
};
