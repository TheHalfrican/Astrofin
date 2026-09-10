// Unit tests for the client-side window decorations. Run with:
//
//     node --test src/web/csd.test.js
//
// (or `just test-js`). csd.js is a small state machine — enabled, fullscreen,
// videoActive, osdVisible — feeding one `update()` that decides two things:
// whether the titlebar is visible and whether the page content is inset below
// it. The rules are easy to get subtly wrong (the inset must stay constant
// while decorated so the layout does not jump when playback starts; only the
// bar follows the OSD; fullscreen drops both), so each combination gets a
// test. The rest is wiring: every button and resize grip maps to one jmpNative
// call, and each of those must be a no-op when native is not bound yet.
const test = require('node:test');
const assert = require('node:assert');

const {
    makeUiWindow, loadModule, makeRecorder, makeApiPlayer, installClock, fireEvent
} = require('./test/ui-fakes.js');

const NATIVE = [
    'csdReady', 'windowStartMove', 'windowStartResize', 'windowMinimize',
    'windowToggleMaximize', 'windowClose'
];

function load({ native = true, fullscreenHook = null } = {}) {
    const win = makeUiWindow();
    const clock = installClock(win);
    if (native) win.jmpNative = makeRecorder(NATIVE);
    if (fullscreenHook) win._nativeFullscreenChanged = fullscreenHook;
    const player = makeApiPlayer(win);
    win.api = { player };
    const api = loadModule('csd.js', win);
    return { win, doc: win.document, api, clock, player, native: win.jmpNative };
}

const visible = (ctx) => ctx.api.host() && ctx.api.host().getAttribute('data-visible');
const inset = (ctx) => ctx.doc.documentElement.classList.contains('jmp-csd-inset');
const shadow = (ctx) => ctx.api.host()._shadow;

// Turn the titlebar on and hand back the pieces the tests click.
function enable(ctx) {
    ctx.api.csd.setEnabled(true);
    const root = shadow(ctx);
    return {
        root,
        drag: root.querySelector('.drag'),
        min: root.querySelector('button.min'),
        max: root.querySelector('button.max'),
        close: root.querySelector('button.close')
    };
}

function showOsd(ctx, on) {
    ctx.doc._callbacks.SHOW_VIDEO_OSD.forEach((fn) => fn({ type: 'SHOW_VIDEO_OSD' }, on));
}

// ---- lifecycle ------------------------------------------------------------

test('the script asks native whether CSD applies and otherwise stays dormant', () => {
    const ctx = load();
    assert.deepStrictEqual(ctx.native.callsTo('csdReady'), [[]]);
    assert.strictEqual(ctx.api.host(), null, 'no titlebar until native says so');
    assert.strictEqual(ctx.api.insetStyle(), null);
    assert.strictEqual(inset(ctx), false);
});

test('loading without jmpNative bound yet does not throw', () => {
    assert.doesNotThrow(() => load({ native: false }));
});

test('enabling builds the titlebar, the inset sheet and shows both', () => {
    const ctx = load();
    enable(ctx);
    const host = ctx.api.host();
    assert.strictEqual(host.tagName, 'JMP-TITLEBAR');
    assert.strictEqual(host.parentNode, ctx.doc.documentElement);
    assert.ok(host.shadowRoot, 'an open root, so a test/theme can inspect it');
    assert.strictEqual(visible(ctx), '1');
    assert.strictEqual(inset(ctx), true);
    assert.strictEqual(ctx.api.insetStyle().parentNode, ctx.doc.head);
    assert.match(ctx.api.insetStyle().textContent, /--jmp-csd-height/);
});

test('enabling twice reuses the one host and the one stylesheet', () => {
    const ctx = load();
    enable(ctx);
    const host = ctx.api.host();
    const style = ctx.api.insetStyle();
    ctx.api.csd.setEnabled(true);
    assert.strictEqual(ctx.api.host(), host);
    assert.strictEqual(ctx.api.insetStyle(), style);
    assert.strictEqual(ctx.doc.querySelectorAll('jmp-titlebar').length, 1);
});

test('disabling hides the bar and the inset but keeps the host mounted', () => {
    const ctx = load();
    enable(ctx);
    ctx.api.csd.setEnabled(false);
    assert.strictEqual(visible(ctx), '0');
    assert.strictEqual(inset(ctx), false);
    assert.ok(ctx.api.host().isConnected, 'kept, so re-enabling is instant');
});

test('a falsy argument to setEnabled is treated as off', () => {
    const ctx = load();
    enable(ctx);
    ctx.api.csd.setEnabled(undefined);
    assert.strictEqual(ctx.api.state.enabled, false);
    assert.strictEqual(visible(ctx), '0');
});

test('update before anything is built is a no-op', () => {
    const ctx = load();
    assert.doesNotThrow(() => ctx.api.update());
    assert.strictEqual(inset(ctx), false);
});

// ---- the titlebar controls ------------------------------------------------

test('the three window buttons carry labels and call native', () => {
    const ctx = load();
    const ui = enable(ctx);
    assert.deepStrictEqual([ui.min, ui.max, ui.close].map((b) => b.getAttribute('aria-label')),
        ['Minimize', 'Maximize', 'Close']);
    assert.deepStrictEqual([ui.min, ui.max, ui.close].map((b) => b.title),
        ['Minimize', 'Maximize', 'Close']);
    fireEvent(ui.min, 'click');
    fireEvent(ui.max, 'click');
    fireEvent(ui.close, 'click');
    assert.deepStrictEqual(ctx.native.names().slice(1),
        ['windowMinimize', 'windowToggleMaximize', 'windowClose']);
});

test('pressing the drag region starts an interactive move', () => {
    const ctx = load();
    const ui = enable(ctx);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    assert.deepStrictEqual(ctx.native.callsTo('windowStartMove'), [[]]);
});

test('a second press inside 400ms maximizes instead of moving again', () => {
    const ctx = load();
    const ui = enable(ctx);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    ctx.clock.advance(120);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    assert.deepStrictEqual(ctx.native.callsTo('windowStartMove'), [[]]);
    assert.deepStrictEqual(ctx.native.callsTo('windowToggleMaximize'), [[]]);
});

test('a press after the double-click window starts another move', () => {
    const ctx = load();
    const ui = enable(ctx);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    ctx.clock.advance(500);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    assert.strictEqual(ctx.native.callsTo('windowStartMove').length, 2);
    assert.deepStrictEqual(ctx.native.callsTo('windowToggleMaximize'), []);
});

test('the third press after a maximize starts a move, not another maximize', () => {
    const ctx = load();
    const ui = enable(ctx);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    ctx.clock.advance(100);
    fireEvent(ui.drag, 'mousedown', { button: 0 }); // maximize; resets the timer
    ctx.clock.advance(100);
    fireEvent(ui.drag, 'mousedown', { button: 0 });
    assert.strictEqual(ctx.native.callsTo('windowStartMove').length, 2);
    assert.strictEqual(ctx.native.callsTo('windowToggleMaximize').length, 1);
});

test('a right-click on the drag region is ignored', () => {
    const ctx = load();
    const ui = enable(ctx);
    fireEvent(ui.drag, 'mousedown', { button: 2 });
    assert.deepStrictEqual(ctx.native.callsTo('windowStartMove'), []);
});

test('each resize grip sends its xdg_toplevel edge bitmask', () => {
    const ctx = load();
    const ui = enable(ctx);
    const expected = [
        ['rz-t', 1], ['rz-b', 2], ['rz-l', 4], ['rz-r', 8],
        ['rz-tl', 5], ['rz-tr', 9], ['rz-bl', 6], ['rz-br', 10]
    ];
    for (const [cls, edge] of expected) {
        const zone = ui.root.querySelector('.rz.' + cls);
        assert.ok(zone, cls + ' exists');
        const event = fireEvent(zone, 'mousedown', { button: 0 });
        assert.strictEqual(event.defaultPrevented, true);
        assert.deepStrictEqual(ctx.native.lastCall('windowStartResize'), [edge]);
    }
    assert.strictEqual(ctx.native.callsTo('windowStartResize').length, 8);
});

test('a non-primary button on a resize grip is ignored', () => {
    const ctx = load();
    const ui = enable(ctx);
    fireEvent(ui.root.querySelector('.rz-br'), 'mousedown', { button: 1 });
    assert.deepStrictEqual(ctx.native.callsTo('windowStartResize'), []);
});

test('the controls are inert when native is not bound', () => {
    const ctx = load({ native: false });
    const ui = enable(ctx);
    assert.doesNotThrow(() => {
        fireEvent(ui.close, 'click');
        fireEvent(ui.drag, 'mousedown', { button: 0 });
        fireEvent(ui.root.querySelector('.rz-t'), 'mousedown', { button: 0 });
    });
});

test('a native binding missing one entry point skips just that call', () => {
    const ctx = load();
    const ui = enable(ctx);
    delete ctx.win.jmpNative.windowClose;
    assert.doesNotThrow(() => fireEvent(ui.close, 'click'));
    fireEvent(ui.min, 'click');
    assert.deepStrictEqual(ctx.native.callsTo('windowMinimize'), [[]]);
});

// ---- visibility rules -----------------------------------------------------

test('fullscreen drops both the bar and the inset, and leaving restores them', () => {
    const ctx = load();
    enable(ctx);
    ctx.win._nativeFullscreenChanged(true);
    assert.strictEqual(visible(ctx), '0');
    assert.strictEqual(inset(ctx), false);
    ctx.win._nativeFullscreenChanged(false);
    assert.strictEqual(visible(ctx), '1');
    assert.strictEqual(inset(ctx), true);
});

test('the fullscreen hook chains onto whatever native-shim installed first', () => {
    const seen = [];
    const ctx = load({ fullscreenHook: (fs) => seen.push(fs) });
    enable(ctx);
    ctx.win._nativeFullscreenChanged(true);
    assert.deepStrictEqual(seen, [true], 'the original hook still runs');
    assert.strictEqual(ctx.api.state.fullscreen, true);
});

test('the DOM fullscreenchange event is honoured too', () => {
    const ctx = load();
    enable(ctx);
    ctx.doc.fullscreenElement = ctx.doc.body;
    fireEvent(ctx.doc, 'fullscreenchange');
    assert.strictEqual(visible(ctx), '0');
    ctx.doc.fullscreenElement = null;
    fireEvent(ctx.doc, 'fullscreenchange');
    assert.strictEqual(visible(ctx), '1');
});

test('video hides the bar but keeps the inset so the layout does not jump', () => {
    const ctx = load();
    enable(ctx);
    ctx.player.playing.emit();
    assert.strictEqual(visible(ctx), '0');
    assert.strictEqual(inset(ctx), true);
});

test('the bar follows the OSD while video is playing', () => {
    const ctx = load();
    enable(ctx);
    ctx.player.playing.emit();
    showOsd(ctx, true);
    assert.strictEqual(visible(ctx), '1');
    showOsd(ctx, false);
    assert.strictEqual(visible(ctx), '0');
});

test('stopping or finishing playback brings the bar back', () => {
    for (const signal of ['stopped', 'finished']) {
        const ctx = load();
        enable(ctx);
        ctx.player.playing.emit();
        showOsd(ctx, true);
        ctx.player[signal].emit();
        assert.strictEqual(ctx.api.state.videoActive, false, signal);
        assert.strictEqual(ctx.api.state.osdVisible, false, signal + ' clears the OSD flag');
        assert.strictEqual(visible(ctx), '1');
    }
});

test('an OSD event before any playing signal still counts as video', () => {
    const ctx = load();
    enable(ctx);
    // jellyfin-web can raise the OSD from the resume path before the player
    // signal lands; the bar must not be left visible over the video.
    showOsd(ctx, false);
    assert.strictEqual(ctx.api.state.videoActive, true);
    assert.strictEqual(visible(ctx), '0');
});

test('fullscreen wins over a visible OSD', () => {
    const ctx = load();
    enable(ctx);
    ctx.player.playing.emit();
    showOsd(ctx, true);
    ctx.win._nativeFullscreenChanged(true);
    assert.strictEqual(visible(ctx), '0');
});

test('state changes while disabled are remembered but change nothing', () => {
    const ctx = load();
    enable(ctx);
    ctx.api.csd.setEnabled(false);
    ctx.player.playing.emit();
    showOsd(ctx, true);
    assert.strictEqual(visible(ctx), '0');
    assert.strictEqual(inset(ctx), false);
    // Re-enabling picks the state up: video with the OSD showing.
    ctx.api.csd.setEnabled(true);
    assert.strictEqual(visible(ctx), '1');
});

test('the OSD hook is appended to jellyfin-web callbacks, never over them', () => {
    const ctx = load();
    // wireSignals ran at load, after which jellyfin-web's own callback list
    // must still hold the entries it had.
    assert.strictEqual(ctx.doc._callbacks.SHOW_VIDEO_OSD.length, 1);
});

// ---- small builders -------------------------------------------------------

test('svgIcon builds a namespaced icon with the given geometry', () => {
    const ctx = load();
    const icon = ctx.api.svgIcon([
        { tag: 'line', attrs: { x1: 1, y1: 6, x2: 10, y2: 6 } }
    ]);
    assert.strictEqual(icon.tagName, 'SVG');
    assert.strictEqual(icon.namespaceURI, 'http://www.w3.org/2000/svg');
    assert.strictEqual(icon.getAttribute('viewBox'), '0 0 11 11');
    assert.strictEqual(icon.children.length, 1);
    assert.strictEqual(icon.children[0].getAttribute('x2'), '10');
});

test('makeButton labels a button for both pointer and screen reader', () => {
    const ctx = load();
    const icon = ctx.api.svgIcon([]);
    const button = ctx.api.makeButton('max', 'Maximize', icon);
    assert.strictEqual(button.className, 'max');
    assert.strictEqual(button.getAttribute('part'), 'button');
    assert.strictEqual(button.title, 'Maximize');
    assert.strictEqual(button.getAttribute('aria-label'), 'Maximize');
    assert.strictEqual(button.children[0], icon);
});
