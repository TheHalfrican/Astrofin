'use strict';
// Extra browser fakes for the Astrofin theme runtime tests.
//
// src/web/test/player-fakes.js owns the fake DOM; this file only fills the two
// gaps astrofin-theme.js walks into, then adds the jellyfin-web-shaped scaffolds
// the theme reads (a Home page with rails and cards, an ApiClient with images).
//
// The two gaps, both patched on the prototypes exported by player-fakes.js:
//
//   1. `textContent` is a plain data property there, so `el.textContent = ''`
//      does not detach the children. The theme clears the chip strip and the
//      server panel exactly that way, so without real semantics a second render
//      would append to the first and the tests would be measuring the fake.
//   2. `createDocumentFragment()` does not exist, and renderServerPanel()
//      builds its rows in one. A fragment appends its children, not itself.
//
// Nothing here touches the real DOM, the network or a profile dir; the
// prototype patches are process-local and `node --test` gives each test file
// its own process.

const {
    loadModule, makeWindow, FakeElement, FakeDocument
} = require('./player-fakes.js');

// ---------------------------------------------------------------------------
// DOM gaps
// ---------------------------------------------------------------------------

const FRAGMENT_NODE = 11;
const TEXT_NODE = 3;

const rawAppendChild = FakeElement.prototype.appendChild;

FakeElement.prototype.appendChild = function appendChild(child) {
    if (child && child.nodeType === FRAGMENT_NODE) {
        child.childNodes.slice().forEach((node) => rawAppendChild.call(this, node));
        return child;
    }
    return rawAppendChild.call(this, child);
};

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

// placeSpotlight() inserts the panel at `section.nextSibling` and then asks
// whether it already sits right after that rail. Without these two the insert
// lands at the end of the container and the check never holds, so the panel
// moves on every refresh — which the fake's synchronous observers turn into an
// endless refresh loop.
Object.defineProperty(FakeElement.prototype, 'nextSibling', {
    configurable: true,
    get() {
        if (!this.parentNode) return null;
        const sibs = this.parentNode.childNodes;
        return sibs[sibs.indexOf(this) + 1] || null;
    }
});

Object.defineProperty(FakeElement.prototype, 'previousElementSibling', {
    configurable: true,
    get() {
        if (!this.parentNode) return null;
        const sibs = this.parentNode.children;
        return sibs[sibs.indexOf(this) - 1] || null;
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

FakeDocument.prototype.createDocumentFragment = function createDocumentFragment() {
    const frag = new FakeElement(this, '#fragment');
    frag.nodeType = FRAGMENT_NODE;
    return frag;
};

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

// A window the theme can install into. `hash` seeds location.hash so a test can
// start on Home ('#/home.html') or anywhere else.
function makeThemeWindow(overrides = {}) {
    const opts = Object.assign({}, overrides);
    const hash = opts.hash;
    delete opts.hash;
    const win = makeWindow(Object.assign({
        innerWidth: 1280,
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
        // make serverName() throw the way a disconnected ApiClient does.
        serverName() { return value(overrides.serverName); },
        serverInfo() { return value(overrides.serverInfo); },
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
    buildHome,
    makeCard,
    makeThemeApiClient,
    observersFor,
    childTexts
};
