// Unit tests for the A-B repeat loop. Run with:
//
//     node --test src/web/ab-loop.test.js
//
// (or `just test-js`). Much of what is worth testing here is pure — the
// cycle, the `]` guards, the readout and the band percentages — because the
// module deliberately holds no state of its own beyond what mpv pushes. The
// rest needs a window: the `[` return-to-A branch (which reads the player and
// seeks), the null/null push (which has to empty the state), and — with a
// document under it — the toasts, which now wait for mpv to report a point,
// and the remount when jellyfin-web swaps the player page out from under the
// controls.
const test = require('node:test');
const assert = require('node:assert');

// ---- harness --------------------------------------------------------------

// A playbackManager with the one asymmetry jellyfin-web really has:
// currentTime() is milliseconds, duration() is ticks.
function makePm(opts) {
    const o = opts || {};
    const calls = [];
    const player = { name: 'MPV Video Player' };
    return {
        calls,
        _currentPlayer: o.noPlayer ? null : player,
        getCurrentPlayer() {
            return this._currentPlayer;
        },
        currentTime(p) {
            if (!p) throw new Error('player cannot be null');
            return o.positionMs === undefined ? 90000 : o.positionMs;
        },
        duration(p) {
            if (!p) throw new Error('player cannot be null');
            return (o.durationSec === undefined ? 600 : o.durationSec) * 10000000;
        },
        seekMs(ms, p) {
            if (!p) throw new Error('player cannot be null');
            calls.push(['seekMs', ms]);
        },
        paused() {
            return !!o.paused;
        },
        unpause(p) {
            if (!p) throw new Error('player cannot be null');
            calls.push(['unpause']);
        }
    };
}

const Events = {
    on(obj, name, fn) {
        obj._callbacks = obj._callbacks || {};
        (obj._callbacks[name] = obj._callbacks[name] || []).push(fn);
    },
    off(obj, name, fn) {
        const list = obj._callbacks?.[name];
        if (!list) return;
        const i = list.indexOf(fn);
        if (i !== -1) list.splice(i, 1);
    },
    trigger(obj, name, args = []) {
        for (const fn of [...(obj._callbacks?.[name] || [])]) fn.apply(obj, [{ type: name }, ...args]);
    }
};

// No `document`: every ensure*() bails and render() returns early, which is
// exactly the shape of "the OSD is not mounted yet" and keeps these tests
// about behaviour rather than markup.
function load(pmOpts, extra) {
    delete require.cache[require.resolve('./ab-loop.js')];
    const native = { calls: [] };
    // The IPC is a step, never a pair of times: mpv stamps the point.
    native.playerAbLoop = (action) => native.calls.push(['abLoop', action]);
    native.playerSeek = (ms) => native.calls.push(['playerSeek', ms]);
    const win = Object.assign({ Events, jmpNative: native }, extra || {});
    global.window = win;
    global.console.debug = () => {};
    const api = require('./ab-loop.js');
    const pm = makePm(pmOpts);
    api.attach(pm);
    return { api, pm, native, win };
}

function push(win, a, b) {
    win._nativeAbLoop(a, b);
}

// ---- the cycle ------------------------------------------------------------

test('the button cycles set A, set B, clear', () => {
    const { api } = load();
    assert.equal(api.nextAction({ a: null, b: null }), 'setA');
    assert.equal(api.nextAction({ a: 10, b: null }), 'setB');
    assert.equal(api.nextAction({ a: 10, b: 20 }), 'clear');
    // A missing A means mpv is not looping whatever B says, so the cycle
    // restarts at A rather than pretending the loop is half-built.
    assert.equal(api.nextAction({ a: null, b: 20 }), 'setA');
});

test('a full cycle asks for set-a, then set-b, then clear', () => {
    const { api, native, win, pm } = load({ positionMs: 90000 });

    api.cycle(); // A: mpv stamps it
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'set-a']);
    push(win, 90, null); // mpv confirms

    pm.currentTime = () => 105500;
    api.cycle(); // B: mpv stamps it
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'set-b']);
    push(win, 90, 105.5);

    api.cycle(); // both set, so this clears
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'clear']);
    push(win, null, null);

    assert.equal(native.calls.length, 3);
    assert.equal(api.nextAction(api._state()), 'setA');
});

test('set B is asked for once the playhead has moved on', () => {
    const { api, native, win, pm } = load({ positionMs: 90000 });
    api.setA();
    push(win, 90, null);
    pm.currentTime = () => 105500;
    api.setB();
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'set-b']);
});

test('clearing asks for a clear, and does nothing when nothing is set', () => {
    const { api, native, win } = load();
    api.clear();
    assert.equal(native.calls.length, 0, 'no points, nothing to clear');
    push(win, 90, 105.5);
    api.clear();
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'clear']);
});

// ---- the `]` guards -------------------------------------------------------

test('B is refused without an A', () => {
    const { api, native } = load();
    assert.equal(api.checkB({ a: null, b: null }, 12), 'no-a');
    api.setB();
    assert.equal(native.calls.length, 0);
});

test('B is refused at or before A', () => {
    const { api, native, win, pm } = load({ positionMs: 90000 });
    assert.equal(api.checkB({ a: 10, b: null }, 10), 'before-a');
    assert.equal(api.checkB({ a: 10, b: null }, 4), 'before-a');
    push(win, 90, null);
    pm.currentTime = () => 80000;
    api.setB();
    assert.equal(native.calls.length, 0, 'a B behind A is never sent');
});

test('B is refused less than half a second after A', () => {
    const { api, native, win, pm } = load();
    assert.equal(api.checkB({ a: 10, b: null }, 10.4), 'too-short');
    assert.equal(api.checkB({ a: 10, b: null }, 10.5), null);
    assert.equal(api.checkB({ a: 10, b: null }, 11), null);
    push(win, 90, null);
    pm.currentTime = () => 90400;
    api.setB();
    assert.equal(native.calls.length, 0);
    pm.currentTime = () => 90500;
    api.setB();
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'set-b']);
});

test('B is refused when there is no player to ask for a position', () => {
    const { api } = load({ noPlayer: true });
    assert.equal(api.checkB({ a: 10, b: null }, null), 'no-position');
});

test('every rejection has a sentence of its own', () => {
    const { api } = load();
    const seen = new Set();
    for (const r of ['no-a', 'before-a', 'too-short', 'no-position']) {
        const text = api.rejectText(r);
        assert.ok(text && text.length > 0);
        assert.ok(!seen.has(text), 'reason ' + r + ' reuses another message');
        seen.add(text);
    }
});

// ---- the `[` return-to-A branch -------------------------------------------

test('[ sets A when there is none', () => {
    const { api, native, pm } = load({ positionMs: 42000 });
    api.jumpToA();
    assert.deepEqual(native.calls.at(-1), ['abLoop', 'set-a']);
    assert.deepEqual(pm.calls, [], 'nothing is seeked when A is being set');
});

test('[ seeks back to A once it is set, and does not touch the points', () => {
    const { api, native, pm, win } = load({ positionMs: 120000 });
    push(win, 90, 105.5);
    native.calls.length = 0;
    api.jumpToA();
    assert.deepEqual(pm.calls, [['seekMs', 90000]]);
    assert.equal(native.calls.length, 0, 'returning to A never rewrites the loop');
});

test('[ resumes playback when it was paused', () => {
    const { api, pm, win } = load({ positionMs: 120000, paused: true });
    push(win, 90, 105.5);
    api.jumpToA();
    assert.deepEqual(pm.calls, [['seekMs', 90000], ['unpause']]);
});

test('[ leaves a playing item playing', () => {
    const { api, pm, win } = load({ positionMs: 120000, paused: false });
    push(win, 90, 105.5);
    api.jumpToA();
    assert.deepEqual(pm.calls, [['seekMs', 90000]]);
});

// ---- the readout ----------------------------------------------------------

test('the readout is empty with no points', () => {
    const { api } = load();
    assert.equal(api.readoutText({ a: null, b: null }), '');
    assert.equal(api.readoutText(null), '');
});

test('the readout names A alone, then the span', () => {
    const { api } = load();
    assert.equal(api.readoutText({ a: 83, b: null }), 'A 1:23');
    assert.equal(api.readoutText({ a: 83, b: 105 }), 'A↔B 1:23–1:45');
});

test('the readout grows an hours field only when it needs one', () => {
    const { api } = load();
    assert.equal(api.fmtTime(0), '0:00');
    assert.equal(api.fmtTime(9), '0:09');
    assert.equal(api.fmtTime(83), '1:23');
    assert.equal(api.fmtTime(3599), '59:59');
    assert.equal(api.fmtTime(3600), '1:00:00');
    assert.equal(api.fmtTime(3723), '1:02:03');
    // Sub-second and negative times floor rather than print oddities.
    assert.equal(api.fmtTime(83.9), '1:23');
    assert.equal(api.fmtTime(-5), '0:00');
});

test('the readout puts the points in order even if mpv reports B before A', () => {
    const { api } = load();
    assert.equal(api.readoutText({ a: 105, b: 83 }), 'A↔B 1:23–1:45');
});

// ---- the overlay percentages ----------------------------------------------

test('the band spans A to B as a percentage of the duration', () => {
    const { api } = load();
    const geo = api.bandGeometry({ a: 60, b: 180 }, 600);
    assert.deepEqual(geo, { aPct: 10, bPct: 30, leftPct: 10, widthPct: 20 });
});

test('with A alone the band has no width and no B pin', () => {
    const { api } = load();
    assert.deepEqual(api.bandGeometry({ a: 150, b: null }, 600), {
        aPct: 25,
        bPct: null,
        leftPct: 25,
        widthPct: 0
    });
});

test('the band is clamped to the track and survives reversed points', () => {
    const { api } = load();
    assert.deepEqual(api.bandGeometry({ a: -30, b: 900 }, 600), {
        aPct: 0,
        bPct: 100,
        leftPct: 0,
        widthPct: 100
    });
    assert.deepEqual(api.bandGeometry({ a: 180, b: 60 }, 600), {
        aPct: 30,
        bPct: 10,
        leftPct: 10,
        widthPct: 20
    });
});

test('there is nothing to draw without an A or without a duration', () => {
    const { api } = load();
    assert.equal(api.bandGeometry({ a: null, b: null }, 600), null);
    assert.equal(api.bandGeometry({ a: null, b: 120 }, 600), null);
    assert.equal(api.bandGeometry({ a: 60, b: 180 }, 0), null);
    assert.equal(api.bandGeometry({ a: 60, b: 180 }, null), null);
    assert.equal(api.bandGeometry({ a: 60, b: 180 }, NaN), null);
});

// ---- the native push ------------------------------------------------------

test('the state is whatever the push carried, and only that', () => {
    const { api, win } = load({ positionMs: 90000 });
    api.setA();
    assert.deepEqual(api._state(), { a: null, b: null }, 'asking is not setting');
    push(win, 90, null);
    assert.deepEqual(api._state(), { a: 90, b: null });
    push(win, 90, 105.5);
    assert.deepEqual(api._state(), { a: 90, b: 105.5 });
});

test('a null/null push clears everything', () => {
    const { api, win } = load();
    push(win, 90, 105.5);
    assert.equal(api.nextAction(api._state()), 'clear');
    push(win, null, null);
    assert.deepEqual(api._state(), { a: null, b: null });
    assert.equal(api.readoutText(api._state()), '');
    assert.equal(api.bandGeometry(api._state(), 600), null);
    assert.equal(api.nextAction(api._state()), 'setA');
});

test('a push of something that is not a time is treated as unset', () => {
    const { api } = load();
    assert.deepEqual(api.normalizePush('no', undefined), { a: null, b: null });
    assert.deepEqual(api.normalizePush(NaN, Infinity), { a: null, b: null });
    assert.deepEqual(api.normalizePush(0, 12.5), { a: 0, b: 12.5 });
});

test('playbackstop empties the state without a push', () => {
    const { api, pm, win } = load();
    push(win, 90, 105.5);
    Events.trigger(pm, 'playbackstop');
    assert.deepEqual(api._state(), { a: null, b: null });
});

// ---- a document ------------------------------------------------------------
//
// Enough DOM for the three mounts, the toasts and a page swap: descendant
// class selectors (the only kind the module uses), insertBefore/nextSibling,
// isConnected and contains. The same shape as playback-source.test.js's fake
// DOM, kept local so neither file's harness constrains the other.

function parseSelector(sel) {
    return sel
        .trim()
        .split(/\s+/)
        .map((part) => part.split('.').filter(Boolean));
}

class El {
    constructor(tag) {
        this.tagName = String(tag).toUpperCase();
        this.children = [];
        this.parentNode = null;
        this.hidden = false;
        this._attrs = {};
        this._class = '';
        this._text = '';
        this._style = {};
        this._listeners = {};
        this.style = {
            setProperty: (k, v) => {
                this._style[k] = String(v);
            },
            getPropertyValue: (k) => this._style[k] || ''
        };
    }

    get className() {
        return this._class;
    }
    set className(v) {
        this._class = String(v);
    }

    get classList() {
        const self = this;
        const list = () => self._class.split(/\s+/).filter(Boolean);
        return {
            contains: (c) => list().includes(c),
            add(...cs) {
                const cur = list();
                for (const c of cs) if (!cur.includes(c)) cur.push(c);
                self._class = cur.join(' ');
            },
            remove(...cs) {
                self._class = list()
                    .filter((c) => !cs.includes(c))
                    .join(' ');
            }
        };
    }

    get textContent() {
        if (this.children.length) return this.children.map((c) => c.textContent).join('');
        return this._text;
    }
    set textContent(v) {
        this.children.splice(0).forEach((c) => {
            c.parentNode = null;
        });
        this._text = String(v);
    }

    setAttribute(k, v) {
        this._attrs[k] = String(v);
    }
    getAttribute(k) {
        return Object.prototype.hasOwnProperty.call(this._attrs, k) ? this._attrs[k] : null;
    }
    addEventListener(name, fn) {
        (this._listeners[name] = this._listeners[name] || []).push(fn);
    }

    appendChild(child) {
        if (child.parentNode) child.parentNode.removeChild(child);
        child.parentNode = this;
        this.children.push(child);
        return child;
    }
    insertBefore(child, ref) {
        if (child.parentNode) child.parentNode.removeChild(child);
        const i = ref ? this.children.indexOf(ref) : -1;
        child.parentNode = this;
        if (i === -1) this.children.push(child);
        else this.children.splice(i, 0, child);
        return child;
    }
    removeChild(child) {
        const i = this.children.indexOf(child);
        if (i !== -1) this.children.splice(i, 1);
        child.parentNode = null;
        return child;
    }

    get nextSibling() {
        if (!this.parentNode) return null;
        const i = this.parentNode.children.indexOf(this);
        return this.parentNode.children[i + 1] || null;
    }

    // The whole point of the swap tests: a detached node keeps its parent
    // chain, so "is it still in the document" is a walk to the root.
    get isConnected() {
        let n = this;
        while (n.parentNode) n = n.parentNode;
        return n.tagName === '#DOCUMENT';
    }

    contains(other) {
        let n = other;
        while (n) {
            if (n === this) return true;
            n = n.parentNode;
        }
        return false;
    }

    *descendants() {
        for (const child of this.children) {
            yield child;
            yield* child.descendants();
        }
    }

    matches(classes) {
        return classes.every((c) => this.classList.contains(c));
    }

    querySelector(sel) {
        return this._find(parseSelector(sel));
    }

    querySelectorAll(sel) {
        const parts = parseSelector(sel);
        const out = [];
        for (const node of this.descendants()) {
            if (node.matches(parts[parts.length - 1]) && node._chainMatches(parts)) out.push(node);
        }
        return out;
    }

    _chainMatches(parts) {
        let i = parts.length - 2;
        let node = this.parentNode;
        while (i >= 0 && node) {
            if (node.matches(parts[i])) i -= 1;
            node = node.parentNode;
        }
        return i < 0;
    }

    _find(parts) {
        for (const node of this.descendants()) {
            if (node.matches(parts[0])) {
                if (parts.length === 1) return node;
                const deeper = node._find(parts.slice(1));
                if (deeper) return deeper;
            }
        }
        return null;
    }
}

class Document extends El {
    constructor() {
        super('#document');
        this.body = new El('body');
        this.appendChild(this.body);
    }
    createElement(tag) {
        return new El(tag);
    }
}

// One jellyfin-web player page: the video container the key handler looks
// for, and the OSD bottom bar with the three hosts the module mounts into.
function mountPage(doc) {
    const page = doc.createElement('div');
    page.className = 'page libraryPage';
    const video = doc.createElement('div');
    video.className = 'videoPlayerContainer';
    const osd = doc.createElement('div');
    osd.className = 'videoOsdBottom videoOsdBottom-maincontrols';
    const controls = doc.createElement('div');
    controls.className = 'osdControls';
    const row = doc.createElement('div');
    row.className = 'flex flex-direction-row';
    const start = doc.createElement('div');
    start.className = 'startTimeText';
    const slider = doc.createElement('div');
    slider.className = 'sliderContainer flex-grow';
    const end = doc.createElement('div');
    end.className = 'endTimeText';
    row.appendChild(start);
    row.appendChild(slider);
    row.appendChild(end);
    const buttons = doc.createElement('div');
    buttons.className = 'buttons focuscontainer-x';
    const subs = doc.createElement('button');
    subs.className = 'btnSubtitles';
    buttons.appendChild(subs);
    controls.appendChild(row);
    controls.appendChild(buttons);
    osd.appendChild(controls);
    page.appendChild(video);
    page.appendChild(osd);
    doc.body.appendChild(page);
    return { page, osd, slider, buttons, end };
}

function loadDom(pmOpts) {
    const doc = new Document();
    const timers = [];
    const intervals = [];
    const observers = [];
    const win = {
        document: doc,
        setTimeout(fn, ms) {
            timers.push({ fn, ms });
            return timers.length;
        },
        clearTimeout() {},
        setInterval(fn, ms) {
            intervals.push({ fn, ms });
            return intervals.length;
        },
        clearInterval(id) {
            intervals.splice(id - 1, 1, null);
        }
    };
    win.MutationObserver = function (cb) {
        const rec = { cb, observed: null, live: true };
        observers.push(rec);
        this.observe = (target, opts) => {
            rec.observed = { target, opts };
        };
        this.disconnect = () => {
            rec.live = false;
        };
    };
    const out = load(pmOpts, win);
    return Object.assign(out, { doc, timers, intervals, observers });
}

function toastTexts(doc) {
    const c = doc.querySelector('.toastContainer');
    return c ? c.children.map((t) => t.textContent) : [];
}

// ---- the toasts now come from mpv's points --------------------------------

test('setting A announces nothing until mpv says where A landed', () => {
    const { api, doc, win } = loadDom({ positionMs: 90000 });
    mountPage(doc);
    api.setA();
    assert.deepEqual(toastTexts(doc), [], 'the key press knows no times');
    // mpv stamped 92.5 s, not the 90 s the OSD had sampled.
    push(win, 92.5, null);
    assert.deepEqual(toastTexts(doc), ['Loop start (A) at 1:32']);
});

test('setting B announces the span mpv actually looped', () => {
    const { api, doc, win, pm } = loadDom({ positionMs: 90000 });
    mountPage(doc);
    api.setA();
    push(win, 92.5, null);
    pm.currentTime = () => 105500;
    api.setB();
    assert.deepEqual(toastTexts(doc).slice(-1), ['Loop start (A) at 1:32'], 'nothing new yet');
    push(win, 92.5, 106.25);
    assert.deepEqual(toastTexts(doc).slice(-1), ['Looping 1:32–1:46']);
});

test('a push nobody asked for says nothing', () => {
    const { doc, win } = loadDom();
    mountPage(doc);
    // The pair mpv reports at startup, then the clear a new item triggers —
    // exactly the sequence in the live log, and none of it is news.
    push(win, 3.041, null);
    push(win, 3.041, 8.041);
    push(win, null, 8.041);
    push(win, null, null);
    assert.deepEqual(toastTexts(doc), []);
});

test('a request mpv never answers does not announce the next push', () => {
    const { api, doc, win } = loadDom({ positionMs: 90000 });
    mountPage(doc);
    api.setA();
    // mpv refused it (a stale UI): the pair does not move, and the next
    // thing to arrive is an unrelated clear.
    push(win, null, null);
    push(win, null, null);
    assert.deepEqual(toastTexts(doc), []);
});

test('a clear and a refusal still speak for themselves at once', () => {
    const { api, doc, win } = loadDom();
    mountPage(doc);
    api.setB();
    assert.deepEqual(toastTexts(doc), ['Set the loop start (A) first']);
    push(win, 90, 105.5);
    api.clear();
    assert.deepEqual(toastTexts(doc).slice(-1), ['A-B loop cleared']);
});

test('an A-B toast is lifted clear of the OSD bar, and only while it is up', () => {
    const { api, doc, win } = loadDom({ positionMs: 90000 });
    const page = mountPage(doc);
    page.osd.getBoundingClientRect = () => ({ height: 96 });
    api.setA();
    push(win, 90, null);
    const lifted = doc.querySelector('.toastContainer').children.at(-1);
    assert.ok(lifted.classList.contains('af-abloop-toast'));
    assert.ok(lifted.classList.contains('af-abloop-toast--lifted'));
    assert.equal(lifted.style.getPropertyValue('--af-abloop-toast-lift'), '96px');

    // A bar with no height is nothing to clear, so the toast stays put.
    page.osd.getBoundingClientRect = () => ({ height: 0 });
    api.clear();
    const flat = doc.querySelector('.toastContainer').children.at(-1);
    assert.equal(flat.classList.contains('af-abloop-toast--lifted'), false);
});

// ---- surviving jellyfin-web's player page swap ----------------------------

test('the controls mount into the OSD that is on screen', () => {
    const { api, doc, pm } = loadDom({ positionMs: 90000 });
    const first = mountPage(doc);
    Events.trigger(pm, 'playbackstart');
    assert.ok(first.osd.querySelector('.af-abloop-btn'));
    assert.ok(first.osd.querySelector('.af-abloop-readout'));
    assert.ok(first.osd.querySelector('.af-abloop-overlay'));
    assert.equal(api._uiIsMounted(), true);
});

test('a page swap remounts the controls into the incoming page', () => {
    const { api, doc, pm } = loadDom({ positionMs: 90000 });
    const first = mountPage(doc);
    Events.trigger(pm, 'playbackstart');
    assert.equal(api._uiIsMounted(), true);

    // jellyfin-web appends the incoming page and only then detaches the
    // outgoing one — which is how the first mount ended up in a page that
    // was already on its way out (probe7 in the live report).
    const second = mountPage(doc);
    assert.equal(api._uiIsMounted(), false, 'the new OSD has none of the three');
    doc.body.removeChild(first.page);
    assert.equal(first.osd.querySelector('.af-abloop-btn').isConnected, false);

    api._checkMounts();

    assert.ok(second.osd.querySelector('.af-abloop-btn'), 'button remounted');
    assert.ok(second.osd.querySelector('.af-abloop-readout'), 'readout remounted');
    assert.ok(second.osd.querySelector('.af-abloop-overlay'), 'overlay remounted');
    assert.equal(api._uiIsMounted(), true);
    assert.equal(doc.querySelectorAll('.af-abloop-btn').length, 1, 'exactly one button');
    assert.equal(doc.querySelectorAll('.af-abloop-overlay').length, 1, 'exactly one overlay');
});

test('the check is a no-op while the controls are where they belong', () => {
    const { api, doc, pm } = loadDom({ positionMs: 90000 });
    const first = mountPage(doc);
    Events.trigger(pm, 'playbackstart');
    const btn = first.osd.querySelector('.af-abloop-btn');
    api._checkMounts();
    api._checkMounts();
    assert.equal(first.osd.querySelector('.af-abloop-btn'), btn, 'the same button');
    assert.equal(doc.querySelectorAll('.af-abloop-btn').length, 1);
});

test('the swap watch runs only while a video player is up', () => {
    const { doc, pm, observers, intervals } = loadDom({ positionMs: 90000 });
    mountPage(doc);
    assert.equal(observers.length, 0, 'nothing is watched before playback');

    Events.trigger(pm, 'playbackstart');
    assert.equal(observers.length, 1);
    assert.equal(observers[0].live, true);
    assert.equal(observers[0].observed.target, doc.body);
    assert.deepEqual(observers[0].observed.opts, { childList: true, subtree: true });
    assert.equal(intervals.filter(Boolean).length, 1, 'plus the safety sweep');

    Events.trigger(pm, 'playbackstop');
    assert.equal(observers[0].live, false, 'disconnected on stop');
    assert.equal(intervals.filter(Boolean).length, 0);
});

test('playbackstop takes the controls down with it', () => {
    const { doc, pm } = loadDom({ positionMs: 90000 });
    const first = mountPage(doc);
    Events.trigger(pm, 'playbackstart');
    assert.equal(doc.querySelectorAll('.af-abloop-btn').length, 1);
    Events.trigger(pm, 'playbackstop');
    assert.equal(doc.querySelectorAll('.af-abloop-btn').length, 0);
    assert.equal(first.osd.querySelectorAll('.af-abloop-overlay').length, 0);
});
