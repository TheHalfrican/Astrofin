// Unit tests for the A-B repeat loop. Run with:
//
//     node --test src/web/ab-loop.test.js
//
// (or `just test-js`). Almost everything worth testing here is pure — the
// cycle, the `]` guards, the readout and the band percentages — because the
// module deliberately holds no state of its own beyond what mpv pushes. The
// two exceptions need a window: the `[` return-to-A branch (which reads the
// player and seeks) and the null/null push (which has to empty the state).
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
function load(pmOpts) {
    delete require.cache[require.resolve('./ab-loop.js')];
    const native = { calls: [] };
    native.playerSetAbLoop = (a, b) => native.calls.push(['setAbLoop', a, b]);
    native.playerSeek = (ms) => native.calls.push(['playerSeek', ms]);
    const win = { Events, jmpNative: native };
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

test('a full cycle sends A, then A+B, then a clear', () => {
    const { api, native, win, pm } = load({ positionMs: 90000 });

    api.cycle(); // A at 90 s
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', 90000, -1]);
    push(win, 90, null); // mpv confirms

    pm.currentTime = () => 105500;
    api.cycle(); // B at 105.5 s
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', 90000, 105500]);
    push(win, 90, 105.5);

    api.cycle(); // both set, so this clears
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', -1, -1]);
    push(win, null, null);

    assert.equal(native.calls.length, 3);
    assert.equal(api.nextAction(api._state()), 'setA');
});

test('set B is sent with both ends once the playhead has moved on', () => {
    const { api, native, win, pm } = load({ positionMs: 90000 });
    api.setA();
    push(win, 90, null);
    pm.currentTime = () => 105500;
    api.setB();
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', 90000, 105500]);
});

test('clearing sends both ends unset, and does nothing when nothing is set', () => {
    const { api, native, win } = load();
    api.clear();
    assert.equal(native.calls.length, 0, 'no points, nothing to clear');
    push(win, 90, 105.5);
    api.clear();
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', -1, -1]);
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
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', 90000, 90500]);
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
    assert.deepEqual(native.calls.at(-1), ['setAbLoop', 42000, -1]);
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
