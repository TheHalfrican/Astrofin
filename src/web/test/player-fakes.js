'use strict';
// Minimal browser fakes for the phase-3 frontend tests (docs/test-plan.md §4).
//
// `src/web/*.js` are injected scripts, not modules: they are IIFEs that read
// bare `document` / `window` and hang their surface off `window`. `loadModule`
// evaluates one of them inside `with (window)`, so a bare identifier resolves
// against the fake window exactly as it would in a browser, and returns
// whatever the file put on `module.exports`.
//
// The DOM here covers only what the modules under test actually touch. It is a
// stand-in for a real DOM, not an implementation of one.

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const WEB_DIR = path.join(__dirname, '..');

// ---------------------------------------------------------------------------
// Selector engine
// ---------------------------------------------------------------------------

// Supports what the injected scripts use: comma lists, descendant combinators,
// and compound `tag#id.class[attr="v"]:not(<compound>)` steps.
const TOKEN_RE = /(\[[^\]]*\]|:not\([^)]*\)|[#.]?[A-Za-z0-9_-]+|\*)/g;

function matchesCompound(el, compound) {
    const tokens = String(compound).match(TOKEN_RE) || [];
    for (const token of tokens) {
        if (token === '*') continue;
        if (token.startsWith(':not(')) {
            if (matchesCompound(el, token.slice(5, -1))) return false;
        } else if (token.startsWith('#')) {
            if (el.id !== token.slice(1)) return false;
        } else if (token.startsWith('.')) {
            if (!el.classList.contains(token.slice(1))) return false;
        } else if (token.startsWith('[')) {
            const body = token.slice(1, -1);
            const eq = body.indexOf('=');
            if (eq === -1) {
                if (el.getAttribute(body) == null) return false;
            } else {
                const name = body.slice(0, eq);
                const want = body.slice(eq + 1).replace(/^["']|["']$/g, '');
                if (el.getAttribute(name) !== want) return false;
            }
        } else if (el.tagName !== token.toUpperCase()) {
            return false;
        }
    }
    return true;
}

function matchesSelector(el, selector) {
    return String(selector).split(',').some((part) => {
        const steps = part.trim().split(/\s+/).filter(Boolean);
        if (!steps.length) return false;
        if (!matchesCompound(el, steps[steps.length - 1])) return false;
        let node = el.parentNode;
        for (let i = steps.length - 2; i >= 0; i--) {
            while (node && !matchesCompound(node, steps[i])) node = node.parentNode;
            if (!node) return false;
            node = node.parentNode;
        }
        return true;
    });
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

function makeStyle() {
    const style = {
        setProperty(name, value) { style[name] = value; },
        getPropertyValue(name) { return style[name] == null ? '' : String(style[name]); },
        removeProperty(name) { delete style[name]; }
    };
    Object.defineProperty(style, 'cssText', {
        get() { return style._cssText || ''; },
        set(v) { style._cssText = v; },
        enumerable: false,
        configurable: true
    });
    return style;
}

// Make `el.<name>` read and write the attribute, the way the DOM reflects
// `<meta name>` / `<meta content>`.
function reflectAttribute(el, name) {
    Object.defineProperty(el, name, {
        get() { return el.getAttribute(name) || ''; },
        set(v) { el.setAttribute(name, v); },
        enumerable: true,
        configurable: true
    });
}

let nextNodeId = 1;

class FakeElement {
    constructor(doc, tagName) {
        this.ownerDocument = doc;
        this.tagName = String(tagName).toUpperCase();
        this.nodeType = 1;
        this._nodeId = nextNodeId++;
        this.childNodes = [];
        this.parentNode = null;
        this._attrs = new Map();
        this._classes = new Set();
        this.style = makeStyle();
        this.listeners = [];
        this.textContent = '';
        this.offsetWidth = 0;
        this.hidden = false;

        const el = this;
        this.classList = {
            add(...names) { names.forEach((n) => n && el._classes.add(n)); },
            remove(...names) { names.forEach((n) => el._classes.delete(n)); },
            contains(name) { return el._classes.has(name); },
            toggle(name, force) {
                const on = force === undefined ? !el._classes.has(name) : !!force;
                if (on) el._classes.add(name); else el._classes.delete(name);
                return on;
            },
            get length() { return el._classes.size; }
        };
    }

    get nodeName() { return this.tagName; }

    get className() { return Array.from(this._classes).join(' '); }
    set className(v) {
        this._classes = new Set(String(v).split(/\s+/).filter(Boolean));
    }

    get id() { return this._attrs.get('id') || ''; }
    set id(v) { this._attrs.set('id', String(v)); }

    get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
    get firstChild() { return this.childNodes[0] || null; }
    get isConnected() {
        let node = this;
        while (node.parentNode) node = node.parentNode;
        return node === this.ownerDocument || node.nodeType === 9;
    }

    get nextElementSibling() {
        if (!this.parentNode) return null;
        const sibs = this.parentNode.children;
        return sibs[sibs.indexOf(this) + 1] || null;
    }

    setAttribute(name, value) { this._attrs.set(name, String(value)); }
    getAttribute(name) {
        if (this._attrs.has(name)) return this._attrs.get(name);
        return null;
    }
    hasAttribute(name) { return this._attrs.has(name); }
    removeAttribute(name) { this._attrs.delete(name); }

    appendChild(child) {
        if (child.parentNode) child.parentNode.removeChild(child);
        child.parentNode = this;
        this.childNodes.push(child);
        this.ownerDocument._mutate(this, [child], []);
        return child;
    }

    insertBefore(child, ref) {
        if (child.parentNode) child.parentNode.removeChild(child);
        child.parentNode = this;
        const at = ref ? this.childNodes.indexOf(ref) : this.childNodes.length;
        this.childNodes.splice(at === -1 ? this.childNodes.length : at, 0, child);
        this.ownerDocument._mutate(this, [child], []);
        return child;
    }

    removeChild(child) {
        const at = this.childNodes.indexOf(child);
        if (at !== -1) this.childNodes.splice(at, 1);
        child.parentNode = null;
        this.ownerDocument._mutate(this, [], [child]);
        return child;
    }

    replaceChildren(...nodes) {
        this.childNodes.slice().forEach((c) => this.removeChild(c));
        nodes.forEach((n) => this.appendChild(n));
    }

    remove() { if (this.parentNode) this.parentNode.removeChild(this); }

    contains(node) {
        for (let n = node; n; n = n.parentNode) if (n === this) return true;
        return false;
    }

    // Bit 4 (DOCUMENT_POSITION_FOLLOWING) when `other` comes after this node.
    compareDocumentPosition(other) {
        const order = [];
        (function walk(node) {
            order.push(node);
            node.childNodes.forEach(walk);
        })(this.ownerDocument.documentElement);
        const a = order.indexOf(this);
        const b = order.indexOf(other);
        if (a === -1 || b === -1) return 0;
        return b > a ? 4 : 2;
    }

    matches(selector) { return matchesSelector(this, selector); }
    closest(selector) {
        for (let n = this; n; n = n.parentNode) {
            if (n.nodeType === 1 && matchesSelector(n, selector)) return n;
        }
        return null;
    }

    _descendants(out = []) {
        for (const child of this.childNodes) {
            if (child.nodeType !== 1) continue;
            out.push(child);
            child._descendants(out);
        }
        return out;
    }

    querySelectorAll(selector) {
        return this._descendants().filter((el) => matchesSelector(el, selector));
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }

    addEventListener(type, fn, opts) { this.listeners.push({ type, fn, opts }); }
    removeEventListener(type, fn) {
        const at = this.listeners.findIndex((l) => l.type === type && l.fn === fn);
        if (at !== -1) this.listeners.splice(at, 1);
    }
    dispatchEvent(event) {
        event.target = event.target || this;
        for (let node = this; node; node = node.parentNode) {
            const list = (node.listeners || []).filter((l) => l.type === event.type);
            for (const l of list) {
                event.currentTarget = node;
                l.fn.call(node, event);
                if (l.opts && l.opts.once) node.removeEventListener(l.type, l.fn);
            }
            if (!event.bubbles) break;
        }
        return true;
    }
}

class FakeSelect extends FakeElement {
    get selectedIndex() {
        const opts = this.children.filter((c) => c.tagName === 'OPTION');
        const at = opts.findIndex((o) => o.selected);
        return at;
    }
    set selectedIndex(v) {
        this.children.filter((c) => c.tagName === 'OPTION')
            .forEach((o, i) => { o.selected = i === Number(v); });
    }
    get value() {
        const opt = this.children.filter((c) => c.tagName === 'OPTION')[this.selectedIndex];
        return opt ? opt.value : '';
    }
}

class FakeDocument extends FakeElement {
    constructor() {
        super(null, '#document');
        this.ownerDocument = this;
        this.nodeType = 9;
        this.readyState = 'complete';
        this._mutations = [];

        this.documentElement = this.createElement('html');
        this.documentElement.parentNode = this;
        this.childNodes.push(this.documentElement);
        this.head = this.createElement('head');
        this.body = this.createElement('body');
        this.documentElement.appendChild(this.head);
        this.documentElement.appendChild(this.body);
    }

    createElement(tagName) {
        const tag = String(tagName).toLowerCase();
        const el = tag === 'select' ? new FakeSelect(this, tag) : new FakeElement(this, tag);
        if (tag === 'option') { el.selected = false; el.value = ''; }
        if (tag === 'input' || tag === 'textarea') { el.value = ''; el.checked = false; }
        if (tag === 'button') el.disabled = false;
        // <meta name content> reflect to properties in a real DOM, and
        // native-shim.js reads both (`node.name`, `meta.content`).
        if (tag === 'meta') { reflectAttribute(el, 'name'); reflectAttribute(el, 'content'); }
        return el;
    }
    createTextNode(text) {
        const node = new FakeElement(this, '#text');
        node.nodeType = 3;
        node.textContent = String(text);
        return node;
    }

    getElementById(id) {
        return this.documentElement._descendants().find((el) => el.id === id) || null;
    }
    querySelectorAll(selector) {
        return this.documentElement._descendants()
            .concat([this.documentElement])
            .filter((el) => matchesSelector(el, selector));
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }

    _mutate(target, addedNodes = [], removedNodes = []) {
        this._mutations.push(target);
        for (const obs of (this._observers || [])) {
            if (obs.target === target || (obs.opts.subtree && obs.target.contains(target))) {
                obs.observer._fire([{ type: 'childList', target, addedNodes, removedNodes }]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

function makeWindow(overrides = {}) {
    const doc = new FakeDocument();
    const timers = makeTimers();
    const observers = [];
    doc._observers = observers;

    class FakeMutationObserver {
        constructor(callback) {
            this.callback = callback;
            this.records = [];
            FakeMutationObserver.instances.push(this);
        }
        observe(target, opts) {
            this.target = target;
            this.opts = opts || {};
            observers.push({ observer: this, target, opts: this.opts });
        }
        disconnect() { this.disconnected = true; }
        takeRecords() { return []; }
        _fire(records) { if (!this.disconnected) this.callback(records, this); }
    }
    FakeMutationObserver.instances = [];

    class FakeCustomEvent {
        constructor(type, init = {}) {
            this.type = type;
            this.detail = init.detail;
            this.bubbles = !!init.bubbles;
            this.cancelable = !!init.cancelable;
            this.defaultPrevented = false;
        }
        preventDefault() { this.defaultPrevented = true; }
        stopPropagation() {}
    }

    const images = [];
    class FakeImage {
        constructor() {
            this.onload = null;
            this.onerror = null;
            images.push(this);
        }
        set src(v) { this._src = v; }
        get src() { return this._src; }
    }

    const win = {
        document: doc,
        console: makeConsole(),
        navigator: {
            language: 'en-US',
            languages: ['en-US'],
            userAgent: 'astrofin-test',
            // native-shim.js branches on this for the macOS-only settings.
            platform: 'Win32'
        },
        location: makeLocation(),
        history: makeHistory(),
        Node: { DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_PRECEDING: 2 },
        MutationObserver: FakeMutationObserver,
        CustomEvent: FakeCustomEvent,
        Event: FakeCustomEvent,
        Image: FakeImage,
        Promise,
        JSON,
        Math,
        Date,
        Number,
        String,
        Array,
        Object,
        Boolean,
        Set,
        Map,
        Error,
        isNaN,
        parseFloat,
        parseInt,
        encodeURIComponent,
        decodeURIComponent,
        getComputedStyle(el) {
            return {
                getPropertyValue(name) {
                    return el && el.style ? el.style.getPropertyValue(name) : '';
                }
            };
        },
        requestAnimationFrame(fn) { return timers.setTimeout(fn, 16); },
        cancelAnimationFrame(id) { timers.clearTimeout(id); },
        setTimeout: timers.setTimeout,
        clearTimeout: timers.clearTimeout,
        setInterval: timers.setInterval,
        clearInterval: timers.clearInterval,
        // window.open — recorded, never a real navigation.
        opened: [],
        open(url, target) { win.opened.push([url, target]); return null; },
        listeners: [],
        addEventListener(type, fn, opts) { win.listeners.push({ type, fn, opts }); },
        removeEventListener(type, fn) {
            const at = win.listeners.findIndex((l) => l.type === type && l.fn === fn);
            if (at !== -1) win.listeners.splice(at, 1);
        },
        dispatchEvent(event) {
            win.listeners.filter((l) => l.type === event.type).forEach((l) => l.fn(event));
            return true;
        }
    };
    win.window = win;
    win.self = win;
    win.globalThis = win;
    win.timers = timers;
    win.observers = FakeMutationObserver.instances;
    win.images = images;
    doc.defaultView = win;
    doc.location = win.location;
    Object.assign(win, overrides);
    return win;
}

function makeConsole() {
    const lines = [];
    const rec = (level) => (...args) => lines.push({ level, args });
    return {
        lines,
        log: rec('log'),
        info: rec('info'),
        warn: rec('warn'),
        error: rec('error'),
        debug: rec('debug')
    };
}

function makeLocation() {
    const loc = { href: 'http://localhost/web/index.html', hash: '', reloads: 0 };
    loc.reload = () => { loc.reloads += 1; };
    loc.assign = (url) => { loc.href = url; };
    return loc;
}

function makeHistory() {
    const hist = { calls: [], length: 1 };
    hist.pushState = (state, title, url) => { hist.calls.push({ state, title, url }); };
    hist.replaceState = (state, title, url) => { hist.calls.push({ state, title, url, replace: true }); };
    hist.back = () => { hist.calls.push({ back: true }); };
    return hist;
}

// Manual clock: nothing fires until `runAll`/`advance` is called, so tests never
// depend on real time.
function makeTimers() {
    let seq = 1;
    const pending = new Map();
    const api = {
        now: 0,
        setTimeout(fn, delay = 0, ...args) {
            const id = seq++;
            pending.set(id, { fn, at: api.now + Number(delay || 0), args, repeat: null });
            return id;
        },
        clearTimeout(id) { pending.delete(id); },
        setInterval(fn, delay = 0, ...args) {
            const id = seq++;
            pending.set(id, { fn, at: api.now + Number(delay || 0), args, repeat: Number(delay || 0) });
            return id;
        },
        clearInterval(id) { pending.delete(id); },
        get pendingCount() { return pending.size; },
        // Fire everything due at or before `now + ms`, in due order.
        advance(ms = 0) {
            const until = api.now + Number(ms);
            let guardCount = 0;
            for (;;) {
                const due = Array.from(pending.entries())
                    .filter(([, t]) => t.at <= until)
                    .sort((a, b) => a[1].at - b[1].at);
                if (!due.length) break;
                if (++guardCount > 1000) throw new Error('timer storm');
                const [id, timer] = due[0];
                api.now = Math.max(api.now, timer.at);
                if (timer.repeat === null) pending.delete(id);
                else timer.at += timer.repeat || 1;
                timer.fn(...timer.args);
            }
            api.now = until;
        },
        // One pass over whatever is queued right now (no repeats re-fired).
        runAll() {
            const due = Array.from(pending.entries()).sort((a, b) => a[1].at - b[1].at);
            due.forEach(([id, timer]) => {
                if (timer.repeat === null) pending.delete(id);
                timer.fn(...timer.args);
            });
        }
    };
    return api;
}

// ---------------------------------------------------------------------------
// Module loader
// ---------------------------------------------------------------------------

const sourceCache = new Map();

function sourceOf(file) {
    const full = path.isAbsolute(file) ? file : path.join(WEB_DIR, file);
    if (!sourceCache.has(full)) sourceCache.set(full, fs.readFileSync(full, 'utf8'));
    return { full, code: sourceCache.get(full) };
}

// Evaluate an injected script against `win`. `with (window)` reproduces the
// browser's scope chain: a bare `document`, `MpvPlayerBase` or `location` in the
// file resolves to the property of the fake window.
//
// `opts.replace` substitutes `__PLACEHOLDER__` tokens the same way the
// renderer does (`src/jfn_cef/src/app.rs` `replace_first`): first occurrence
// only, the value spliced in as raw JS source.
function loadModule(file, win, opts = {}) {
    const { full, code } = sourceOf(file);
    let source = code;
    for (const [ph, value] of Object.entries(opts.replace || {})) {
        const at = source.indexOf(ph);
        if (at !== -1) source = source.slice(0, at) + value + source.slice(at + ph.length);
    }
    const wrapper = `(function (window, module, exports, require) {\n`
        + `with (window) {\n${source}\n}\n})`;
    const factory = vm.runInThisContext(wrapper, { filename: full });
    const module = { exports: {} };
    factory(win, module, module.exports, undefined);
    return module.exports;
}

// The six placeholders `render_injected_scripts` fills in before native-shim.js
// reaches the renderer. `settings` is the `cli_json` blob (a JSON *string* the
// shim `JSON.parse`s), the rest are spliced in as JS source.
function nativeShimPlaceholders(opts = {}) {
    return {
        __SERVER_URL__: JSON.stringify(opts.serverUrl ?? 'https://jf.example.com/'),
        __SETTINGS_JSON__: JSON.stringify(JSON.stringify(opts.settings ?? {})),
        __APP_VERSION__: opts.version ?? '0.5.0-dev',
        __WINDOW_DECORATION_OPTIONS__: JSON.stringify(opts.decorationOptions ?? []),
        __DEVICE_PROFILE_JSON__: JSON.stringify(opts.deviceProfile ?? { Name: 'Astrofin' }),
        __WINDOW_DECORATIONS__: opts.windowDecorations === undefined
            ? 'null'
            : JSON.stringify(opts.windowDecorations)
    };
}

// Install native-shim.js on a fake window, placeholders and all.
function loadNativeShim(win, opts = {}) {
    return loadModule('native-shim.js', win, { replace: nativeShimPlaceholders(opts) });
}

// ---------------------------------------------------------------------------
// jellyfin-web / native fakes
// ---------------------------------------------------------------------------

// Records every call as [name, ...args] plus per-name arrays.
function makeRecorder(names) {
    const rec = { calls: [] };
    names.forEach((name) => {
        rec[name] = (...args) => {
            rec.calls.push([name, ...args]);
            return undefined;
        };
    });
    rec.callsTo = (name) => rec.calls.filter((c) => c[0] === name).map((c) => c.slice(1));
    rec.lastCall = (name) => {
        const hits = rec.callsTo(name);
        return hits.length ? hits[hits.length - 1] : null;
    };
    rec.names = () => rec.calls.map((c) => c[0]);
    return rec;
}

function makeSignal() {
    const sig = { handlers: [] };
    sig.connect = (fn) => { sig.handlers.push(fn); };
    sig.disconnect = (fn) => {
        const at = sig.handlers.indexOf(fn);
        if (at !== -1) sig.handlers.splice(at, 1);
    };
    sig.emit = (...args) => sig.handlers.slice().forEach((fn) => fn(...args));
    return sig;
}

const PLAYER_SIGNALS = [
    'playing', 'paused', 'finished', 'stopped', 'canceled', 'error', 'buffering',
    'seeking', 'positionUpdate', 'updateDuration', 'stateChanged'
];

const PLAYER_METHODS = [
    'stop', 'pause', 'play', 'seekTo', 'setVolume', 'setMuted', 'setPlaybackRate',
    'getPosition', 'setAspectMode', 'setSubtitleStream', 'setAudioStream',
    'addSubtitleStream', 'setSubtitleDelay', 'setVideoRectangle'
];

// `window.api.player` as native-shim.js exposes it. `load` mirrors the shim's
// own translation (src/web/native-shim.js `load()`) into the nine-slot
// `jmpNative.playerLoad` call that src/jfn_cef/src/business_web.rs
// `parse_player_load` reads, so tests can assert both surfaces.
function makeApiPlayer(win) {
    const player = makeRecorder(PLAYER_METHODS);
    PLAYER_SIGNALS.forEach((name) => { player[name] = makeSignal(); });
    player.position = 0;
    player.getPosition = (cb) => {
        player.calls.push(['getPosition']);
        if (cb) cb(player.position);
    };
    player.load = (url, options, streamdata, videoStream, audioStream,
        subtitleStream, externalAudioUrl, externalSubUrl, callback) => {
        player.calls.push(['load', url, options, streamdata, videoStream, audioStream,
            subtitleStream, externalAudioUrl, externalSubUrl]);
        player.lastLoad = {
            url, options, streamdata, videoStream, audioStream, subtitleStream,
            externalAudioUrl, externalSubUrl, callback
        };
        const native = win.jmpNative;
        if (native && native.playerLoad) {
            const metadataJson = streamdata && streamdata.metadata
                ? JSON.stringify(streamdata.metadata) : '{}';
            native.playerLoad(url, options.startMilliseconds, videoStream, audioStream,
                subtitleStream, metadataJson, externalAudioUrl || '', externalSubUrl || '',
                !!options.isInfiniteStream);
        }
        if (callback) {
            player.loadCallback = callback;
            // mpv answers `playing` asynchronously; set this when a test only
            // cares about what happens after the load resolves.
            if (player.autoResolveLoad) callback();
        }
    };
    // Off by default: `setCurrentSrc`'s promise stays pending until the test
    // calls `player.loadCallback()`, which is what mpv makes it do.
    player.autoResolveLoad = false;
    return player;
}

// Every name `NativeFunction::from_name` (src/jfn_cef/src/injection.rs) binds
// on `window.jmpNative`, so a module calling one of them in a test fails the
// same way it would in the browser — on the argument, never on a typo.
const NATIVE_METHODS = [
    'playerLoad', 'playerStop', 'playerPause', 'playerPlay', 'playerSeek',
    'playerSetVolume', 'playerSetMuted', 'playerSetSpeed', 'playerSetSubtitle',
    'playerAddSubtitle', 'playerSetAudio', 'playerAddAudio', 'playerSetAudioDelay',
    'playerSetSubtitleDelay', 'playerSetAspectMode', 'playerAbLoop',
    'playerOsdActive', 'playerStatsActive', 'notifyRateChange', 'notifyMetadata',
    'notifyPosition', 'notifySeek', 'notifyPlaybackState', 'notifyArtwork',
    'notifyQueueChange', 'toggleFullscreen', 'themeColor', 'setOsdVisible',
    'setPlaybackVideoMode', 'openConfigDir', 'saveServerUrl', 'getSavedServerUrl',
    'setSettingValue', 'appExit', 'aboutOpenPath', 'aboutDismiss', 'navigateMain',
    'dismissOverlay', 'checkServerConnectivity', 'cancelServerConnectivity',
    'windowMinimize', 'windowToggleMaximize', 'windowClose', 'windowStartMove',
    'windowStartResize', 'csdReady'
];

function makeJmpNative() { return makeRecorder(NATIVE_METHODS); }

// jellyfin-web's Events module: `trigger(obj, name, args)` plus a recorder.
function makeEvents() {
    const events = { triggered: [], handlers: new Map() };
    events.trigger = (obj, name, args) => {
        events.triggered.push({ obj, name, args });
        (events.handlers.get(name) || []).forEach((fn) => fn(obj, ...(args || [])));
    };
    events.on = (obj, name, fn) => {
        if (!events.handlers.has(name)) events.handlers.set(name, []);
        events.handlers.get(name).push(fn);
    };
    events.off = (obj, name, fn) => {
        const list = events.handlers.get(name) || [];
        const at = list.indexOf(fn);
        if (at !== -1) list.splice(at, 1);
    };
    events.names = () => events.triggered.map((t) => t.name);
    return events;
}

function makeAppSettings(initial = {}) {
    const store = Object.assign({}, initial);
    return {
        store,
        get: (key) => store[key],
        set: (key, value) => { store[key] = value; }
    };
}

function makeAppHost(profile) {
    return { getDeviceProfile: () => Promise.resolve(profile || { Name: 'fake' }) };
}

function makePlaybackManager() {
    const pm = makeRecorder(['setAudioStreamIndex', 'setSubtitleStreamIndex', 'stop', 'play']);
    return pm;
}

function makeApiClient(overrides = {}) {
    const client = {
        userId: 'user-1',
        items: new Map(),
        ancestors: new Map(),
        getCurrentUserId() { return client.userId; },
        getItem(userId, id) {
            client.lastGetItem = [userId, id];
            return client.items.has(id)
                ? Promise.resolve(client.items.get(id))
                : Promise.reject(new Error('no such item ' + id));
        },
        getAncestorItems(id, userId) {
            client.lastAncestors = [id, userId];
            return client.ancestors.has(id)
                ? Promise.resolve(client.ancestors.get(id))
                : Promise.reject(new Error('no ancestors for ' + id));
        }
    };
    return Object.assign(client, overrides);
}

// `loading` / `appRouter` / `globalize` / `dashboard` as the video plugin wants.
function makePlayerArgs(win, extra = {}) {
    return Object.assign({
        events: makeEvents(),
        appHost: makeAppHost(),
        appSettings: makeAppSettings({ volume: 1 }),
        loading: makeRecorder(['show', 'hide']),
        appRouter: makeRecorder(['showVideoOsd']),
        globalize: { translate: (key) => key },
        dashboard: null,
        playbackManager: makePlaybackManager()
    }, extra);
}

module.exports = {
    WEB_DIR,
    loadModule,
    loadNativeShim,
    nativeShimPlaceholders,
    makeWindow,
    makeTimers,
    makeConsole,
    makeRecorder,
    makeSignal,
    makeApiPlayer,
    makeJmpNative,
    makeEvents,
    makeAppSettings,
    makeAppHost,
    makePlaybackManager,
    makeApiClient,
    makePlayerArgs,
    matchesSelector,
    FakeElement,
    FakeDocument
};
