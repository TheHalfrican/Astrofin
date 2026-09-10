'use strict';
// Extra browser fakes for the phase-3 UI modules (client-settings, overlay,
// connectivityHelper, csd, select-menu, about). `player-fakes.js` covers the
// DOM the player shims touch; these modules additionally need shadow roots,
// `dataset`, `getBoundingClientRect`, focus tracking, SVG creation and a
// window with `innerWidth`/`innerHeight` and a `close()` recorder.
//
// Nothing here tries to be a DOM implementation: every addition exists because
// one of the six modules under test calls it. The element-class patches are
// additive (a method is only installed when it is missing), and `node --test`
// runs each test file in its own process, so they cannot leak into another
// agent's test file.

const fakes = require('./player-fakes.js');

const { FakeElement, makeWindow } = fakes;

// ---------------------------------------------------------------------------
// Element-class patches
// ---------------------------------------------------------------------------

const EMPTY_RECT = { x: 0, y: 0, left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 };

function dashed(name) {
    return String(name).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
}

function patchElements() {
    const proto = FakeElement.prototype;

    // `el.dataset.foo` <-> the `data-foo` attribute, as in the real DOM
    // (overlay.js writes `document.body.dataset.state`, select-menu.js round-
    // trips an option index through `el.dataset.idx`).
    if (!Object.getOwnPropertyDescriptor(proto, 'dataset')) {
        Object.defineProperty(proto, 'dataset', {
            configurable: true,
            get() {
                if (!this._datasetProxy) {
                    const el = this;
                    this._datasetProxy = new Proxy({}, {
                        get(_t, key) {
                            if (typeof key !== 'string') return undefined;
                            const v = el.getAttribute('data-' + dashed(key));
                            return v === null ? undefined : v;
                        },
                        set(_t, key, value) {
                            el.setAttribute('data-' + dashed(key), value);
                            return true;
                        },
                        has(_t, key) { return el.hasAttribute('data-' + dashed(key)); },
                        deleteProperty(_t, key) {
                            el.removeAttribute('data-' + dashed(key));
                            return true;
                        },
                        ownKeys() { return []; }
                    });
                }
                return this._datasetProxy;
            }
        });
    }

    // A shadow root is just another element here: it holds children and
    // answers querySelector, which is all the shadow-using modules need. An
    // `open` root is also published as `el.shadowRoot`; a `closed` one is
    // reachable from a test only through `el._shadow`, never from the module.
    if (!proto.attachShadow) {
        proto.attachShadow = function attachShadow(init) {
            const root = new FakeElement(this.ownerDocument, '#shadow-root');
            root.nodeType = 11;
            root.host = this;
            root.mode = (init && init.mode) || 'open';
            this._shadow = root;
            if (root.mode === 'open') this.shadowRoot = root;
            return root;
        };
    }

    // Layout is whatever the test says it is (`el._rect`); zero by default.
    if (!proto.getBoundingClientRect) {
        proto.getBoundingClientRect = function getBoundingClientRect() {
            return Object.assign({}, EMPTY_RECT, this._rect || {});
        };
    }

    if (!proto.append) {
        proto.append = function append(...nodes) {
            for (const node of nodes) {
                this.appendChild(typeof node === 'string'
                    ? this.ownerDocument.createTextNode(node)
                    : node);
            }
        };
    }

    if (!proto.focus) {
        proto.focus = function focus() {
            const doc = this.ownerDocument;
            if (doc) doc.activeElement = this;
            this.focusCount = (this.focusCount || 0) + 1;
        };
        proto.blur = function blur() {
            const doc = this.ownerDocument;
            if (doc && doc.activeElement === this) doc.activeElement = null;
        };
    }

    if (!proto.scrollIntoView) {
        proto.scrollIntoView = function scrollIntoView(opts) {
            this.scrolledIntoView = (this.scrolledIntoView || 0) + 1;
            this.lastScrollOpts = opts;
        };
    }
}

patchElements();

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

// `makeWindow` plus the extras the UI modules read: viewport size, a `close()`
// recorder, `document.activeElement`, `createElementNS`, and select/option
// defaults so `isDropdown()` in select-menu.js can make a decision.
function makeUiWindow(overrides = {}) {
    const win = makeWindow();
    const doc = win.document;

    win.innerWidth = 1280;
    win.innerHeight = 720;
    win.closed = 0;
    win.close = () => { win.closed += 1; };

    doc.activeElement = null;
    doc.fullscreenElement = null;

    const create = doc.createElement.bind(doc);
    doc.createElement = (tagName) => {
        const el = create(tagName);
        const tag = String(tagName).toLowerCase();
        if (tag === 'select') {
            // The real defaults; `isDropdown` compares against all three.
            el.multiple = false;
            el.size = 0;
            el.disabled = false;
        } else if (tag === 'option') {
            el.disabled = false;
            el.index = -1;
        } else if (tag === 'optgroup') {
            el.disabled = false;
            el.label = '';
        }
        return el;
    };
    doc.createElementNS = (ns, tagName) => {
        const el = doc.createElement(tagName);
        el.namespaceURI = ns;
        return el;
    };

    Object.assign(win, overrides);
    return win;
}

// A frozen clock for the modules that read `Date.now()` (csd's double-click
// window, overlay's spinner floor). `win.Date` is what `with (window)`
// resolves, so replacing it is enough.
function installClock(win, start = 1000000) {
    const clock = { now: start };
    win.Date = { now: () => clock.now };
    clock.advance = (ms) => { clock.now += ms; };
    return clock;
}

// ---------------------------------------------------------------------------
// Element builders
// ---------------------------------------------------------------------------

// A <select> with working `selectedIndex`, `option.index` and `option.text`
// across optgroups — select-menu.js keys its rows off `opt.index`, which the
// bare fake element does not model. `spec` entries are
// `{ text, value, disabled, selected }` or `{ label, options: [...] }`.
function makeSelect(doc, spec = [], attrs = {}) {
    const select = doc.createElement('select');
    const options = [];

    const addOption = (o, parent) => {
        const opt = doc.createElement('option');
        opt.textContent = o.text;
        opt.text = o.text;
        opt.value = o.value === undefined ? o.text : o.value;
        opt.disabled = !!o.disabled;
        opt.index = options.length;
        opt.selected = !!o.selected;
        options.push(opt);
        parent.appendChild(opt);
    };

    for (const entry of spec) {
        if (entry && entry.options) {
            const group = doc.createElement('optgroup');
            group.label = entry.label || '';
            group.disabled = !!entry.disabled;
            select.appendChild(group);
            entry.options.forEach((o) => addOption(o, group));
        } else {
            addOption(entry, select);
        }
    }

    select.options = options;
    Object.defineProperty(select, 'selectedIndex', {
        configurable: true,
        get() { return options.findIndex((o) => o.selected); },
        set(v) { options.forEach((o, i) => { o.selected = i === Number(v); }); }
    });
    Object.assign(select, attrs);
    return select;
}

// Dispatch `type` on `el` with a synthetic event object: enough of the real
// Event surface for the handlers under test (key, button, target, and the two
// cancellation methods, with the calls recorded). Unlike the plain fake
// `dispatchEvent`, this walk honours `stopPropagation()` — about.js relies on
// it to keep a click inside the panel from reaching the backdrop.
function fireEvent(el, type, props = {}) {
    const event = Object.assign({
        type,
        target: el,
        bubbles: true,
        defaultPrevented: false,
        propagationStopped: false
    }, props);
    event.preventDefault = () => { event.defaultPrevented = true; };
    event.stopPropagation = () => { event.propagationStopped = true; };

    if (typeof el.nodeType !== 'number') {
        el.dispatchEvent(event); // the window, which has no tree to walk
        return event;
    }
    for (let node = el; node; node = node.parentNode) {
        for (const listener of (node.listeners || []).filter((l) => l.type === type)) {
            event.currentTarget = node;
            listener.fn.call(node, event);
            if (listener.opts && listener.opts.once) node.removeEventListener(type, listener.fn);
        }
        if (!event.bubbles || event.propagationStopped) break;
    }
    return event;
}

// The connect screen, built to match src/web/overlay.html: the ids overlay.js
// looks up (`address`, `connect-button`, `connect-form`, `connect-status`,
// `cancel-button`) inside the same card/form nesting.
function buildOverlayDom(doc) {
    const main = doc.createElement('main');
    main.id = 'container';
    const card = doc.createElement('section');
    card.className = 'af-card';
    main.appendChild(card);

    const form = doc.createElement('form');
    form.id = 'connect-form';
    const address = doc.createElement('input');
    address.id = 'address';
    address.type = 'text';
    address.value = '';
    address.disabled = false;
    const connect = doc.createElement('button');
    connect.id = 'connect-button';
    connect.type = 'submit';
    connect.disabled = false;
    form.appendChild(address);
    form.appendChild(connect);
    card.appendChild(form);

    const status = doc.createElement('p');
    status.id = 'connect-status';
    const cancel = doc.createElement('button');
    cancel.id = 'cancel-button';
    cancel.type = 'button';
    card.appendChild(status);
    card.appendChild(cancel);

    doc.body.appendChild(main);
    doc.body.setAttribute('data-state', 'boot');
    return { main, card, form, address, connect, status, cancel };
}

// jellyfin-web's page shell as client-settings.js expects to find it:
// `.mainAnimatedPages` holding the visible legacy pages, with the React
// container as its next element sibling.
function buildAppShell(doc, { visiblePages = 1 } = {}) {
    const pages = doc.createElement('div');
    pages.className = 'mainAnimatedPages';
    const legacy = [];
    for (let i = 0; i < visiblePages; i++) {
        const page = doc.createElement('div');
        page.className = 'mainAnimatedPage page';
        pages.appendChild(page);
        legacy.push(page);
    }
    const react = doc.createElement('div');
    react.className = 'skinBody';
    const reactPage = doc.createElement('div');
    reactPage.setAttribute('data-role', 'page');
    react.appendChild(reactPage);
    doc.body.appendChild(pages);
    doc.body.appendChild(react);
    return { pages, legacy, react, reactPage };
}

module.exports = Object.assign({}, fakes, {
    makeUiWindow,
    installClock,
    makeSelect,
    fireEvent,
    buildOverlayDom,
    buildAppShell,
    dashed
});
