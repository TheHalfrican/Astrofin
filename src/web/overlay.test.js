// Unit tests for the connect overlay. Run with:
//
//     node --test src/web/overlay.test.js
//
// (or `just test-js`). The overlay is the only screen the user sees before a
// server exists, and every path through it is asynchronous: a native probe, a
// spinner floor, a fade-out, and a cancel that can arrive during any of them.
// What is worth testing is that the three visual states are driven from one
// switch, that a successful probe saves the *normalised* url native returned
// (not what was typed), that the main browser is navigated exactly once, that
// a user cancel is never reported as a failure, and that the failure dialog is
// dismissable from a remote — Enter and Escape as well as the button.
//
// The DOM is the one in src/web/overlay.html; the language strings are the
// bare globals overlay.lang.js declares.
const test = require('node:test');
const assert = require('node:assert');

const {
    makeUiWindow, loadModule, makeRecorder, buildOverlayDom, installClock, fireEvent
} = require('./test/ui-fakes.js');

const NATIVE = [
    'getSavedServerUrl', 'saveServerUrl', 'navigateMain', 'dismissOverlay',
    'cancelServerConnectivity'
];

const STRINGS = {
    headerConnectionFailureText: 'Connection Failure',
    messageUnableToConnectToServerText: 'We are unable to connect to the selected server right now.',
    buttonGotItText: 'Got It',
    cancelButtonText: 'Cancel'
};

// A stand-in for connectivityHelper.js: one probe in flight, settled by hand.
function makeProbe() {
    const probe = (url) => {
        probe.calls.push(url);
        return new Promise((resolve, reject) => {
            probe.resolve = resolve;
            probe.reject = reject;
        });
    };
    probe.calls = [];
    probe.aborts = 0;
    probe.abort = () => {
        probe.aborts += 1;
        if (probe.reject) probe.reject(new Error('Connection cancelled'));
    };
    return probe;
}

function load({ native = true, probe = makeProbe() } = {}) {
    const win = makeUiWindow(STRINGS);
    const clock = installClock(win);
    const dom = buildOverlayDom(win.document);
    if (native) win.jmpNative = makeRecorder(NATIVE);
    win.jmpCheckServerConnectivity = probe;
    const api = loadModule('overlay.js', win);
    return { win, doc: win.document, api, dom, probe, clock, native: win.jmpNative };
}

// Let every pending microtask (and any `await` chain behind it) run.
async function flush(times = 4) {
    for (let i = 0; i < times; i++) await new Promise((r) => setImmediate(r));
}

// Deliver the saved-url reply native owes us, then let the auto-connect run.
async function boot(ctx, url) {
    ctx.win._onSavedServerUrl(url);
    await flush();
}

const stateOf = (ctx) => ctx.doc.body.getAttribute('data-state');
const dialog = (ctx) => ctx.doc.querySelector('.dialog');

// ---- boot -----------------------------------------------------------------

test('the overlay asks native for the saved url and waits in the boot state', () => {
    const ctx = load();
    assert.deepStrictEqual(ctx.native.callsTo('getSavedServerUrl'), [[]]);
    assert.strictEqual(stateOf(ctx), 'boot', 'orbit only until the reply lands');
    assert.deepStrictEqual(ctx.probe.calls, []);
});

test('no saved url leaves the form live, focused and with connect disabled', async () => {
    const ctx = load();
    await boot(ctx, null);
    assert.strictEqual(stateOf(ctx), 'idle');
    assert.strictEqual(ctx.doc.activeElement, ctx.dom.address);
    assert.strictEqual(ctx.dom.connect.disabled, true, 'nothing typed yet');
    assert.deepStrictEqual(ctx.probe.calls, []);
});

test('a saved url connects on its own without re-navigating the main browser', async () => {
    const ctx = load();
    await boot(ctx, 'https://saved.example');
    assert.strictEqual(ctx.dom.address.value, 'https://saved.example');
    assert.strictEqual(stateOf(ctx), 'connecting');
    assert.strictEqual(ctx.dom.address.disabled, true);
    assert.strictEqual(ctx.dom.status.textContent, 'https://saved.example');
    assert.deepStrictEqual(ctx.probe.calls, ['https://saved.example']);

    ctx.probe.resolve('https://saved.example');
    await flush();
    // main.cpp pre-loaded the saved url, so a second navigate would reload it.
    assert.deepStrictEqual(ctx.native.callsTo('navigateMain'), []);
    assert.strictEqual(ctx.api.state().mainLoaded, true);
});

// ---- the button and the form ----------------------------------------------

test('the connect button follows whether the address has content', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = '   ';
    fireEvent(ctx.dom.address, 'input');
    assert.strictEqual(ctx.dom.connect.disabled, true, 'whitespace is not an address');
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.address, 'input');
    assert.strictEqual(ctx.dom.connect.disabled, false);
    ctx.dom.address.value = '';
    fireEvent(ctx.dom.address, 'input');
    assert.strictEqual(ctx.dom.connect.disabled, true);
});

test('the button state is left alone while a probe is running', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.address, 'input');
    ctx.api.startConnecting();
    await flush();
    ctx.dom.connect.disabled = false;
    ctx.api.updateButtonState();
    assert.strictEqual(ctx.dom.connect.disabled, false, 'the cancel affordance owns the UI');
});

test('clicking connect starts a probe; a disabled button does nothing', async () => {
    const ctx = load();
    await boot(ctx, null);
    fireEvent(ctx.dom.connect, 'click', { target: ctx.dom.connect });
    assert.deepStrictEqual(ctx.probe.calls, [], 'disabled: no probe');

    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.address, 'input');
    fireEvent(ctx.dom.connect, 'click', { target: ctx.dom.connect });
    await flush();
    assert.deepStrictEqual(ctx.probe.calls, ['jellyfin.example']);
});

test('submitting the form connects, and never twice at once', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    const event = fireEvent(ctx.dom.form, 'submit');
    await flush();
    assert.strictEqual(event.defaultPrevented, true, 'no page navigation');
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    assert.deepStrictEqual(ctx.probe.calls, ['jellyfin.example']);
});

test('Enter connects from anywhere on the page when the form is live', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    const event = fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    await flush();
    assert.strictEqual(event.defaultPrevented, true);
    assert.deepStrictEqual(ctx.probe.calls, ['jellyfin.example']);
});

test('Enter does nothing with an empty or disabled address', async () => {
    const ctx = load();
    await boot(ctx, null);
    fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    ctx.dom.address.value = '  ';
    fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    ctx.dom.address.value = 'jellyfin.example';
    ctx.dom.address.disabled = true;
    fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    await flush();
    assert.deepStrictEqual(ctx.probe.calls, []);
});

test('the cancel button takes its label from the language file', () => {
    const ctx = load();
    assert.strictEqual(ctx.dom.cancel.innerText, 'Cancel');
});

// ---- a successful connection ----------------------------------------------

test('a successful probe saves the url native normalised, not what was typed', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();

    assert.deepStrictEqual(ctx.native.lastCall('saveServerUrl'), ['http://jellyfin.example:8096/']);
    assert.deepStrictEqual(ctx.native.lastCall('navigateMain'), ['http://jellyfin.example:8096/']);
    assert.strictEqual(ctx.api.state().savedServerUrl, 'http://jellyfin.example:8096/');
});

test('the overlay holds the spinner for a second before it fades out', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();

    // Navigation goes out immediately; the fade waits out the spinner floor so
    // the main browser has time to paint.
    assert.deepStrictEqual(ctx.native.callsTo('dismissOverlay'), []);
    ctx.win.timers.advance(999);
    await flush();
    assert.deepStrictEqual(ctx.native.callsTo('dismissOverlay'), []);
    ctx.win.timers.advance(1);
    await flush();
    assert.deepStrictEqual(ctx.native.callsTo('dismissOverlay'), [[]]);
    assert.ok(ctx.doc.body.classList.contains('fade-out'));
});

test('the overlay closes only when its own fade-out animation ends', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();
    ctx.win.timers.advance(1000);
    await flush();

    fireEvent(ctx.doc.body, 'animationend', { animationName: 'starfieldDrift' });
    assert.strictEqual(ctx.win.closed, 0, 'someone else\'s animation is not our cue');
    fireEvent(ctx.doc.body, 'animationend', { animationName: 'fadeOut' });
    assert.strictEqual(ctx.win.closed, 1);
    // The handler unhooks itself, so a repeat cannot close twice.
    fireEvent(ctx.doc.body, 'animationend', { animationName: 'fadeOut' });
    assert.strictEqual(ctx.win.closed, 1);
});

test('a missing navigateMain is a failure, not a silent dead end', async () => {
    const ctx = load();
    await boot(ctx, null);
    delete ctx.win.jmpNative.navigateMain;
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();
    assert.ok(dialog(ctx), 'the user is told the connection failed');
    assert.strictEqual(stateOf(ctx), 'idle');
});

// ---- failure --------------------------------------------------------------

test('a failed probe returns to the form and raises the dialog', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.reject(new Error('Connection failed'));
    await flush();

    assert.strictEqual(stateOf(ctx), 'idle');
    assert.strictEqual(ctx.dom.address.disabled, false);
    assert.strictEqual(ctx.api.state().isConnecting, false);
    const box = dialog(ctx);
    assert.ok(box);
    assert.strictEqual(box.getAttribute('role'), 'alertdialog');
    assert.strictEqual(box.getAttribute('aria-modal'), 'true');
    assert.strictEqual(box.querySelector('h1').innerText, STRINGS.headerConnectionFailureText);
    assert.strictEqual(box.querySelector('.dialog-message').innerText,
        STRINGS.messageUnableToConnectToServerText);
    assert.strictEqual(box.querySelector('button').innerText, STRINGS.buttonGotItText);
    assert.strictEqual(ctx.doc.activeElement, box.querySelector('button'));
});

test('the failure dialog is dismissable by button, Enter and Escape', async () => {
    for (const dismiss of [
        (ctx) => fireEvent(dialog(ctx).querySelector('button'), 'click'),
        (ctx) => fireEvent(ctx.doc, 'keydown', { key: 'Enter' }),
        (ctx) => fireEvent(ctx.doc, 'keydown', { key: 'Escape' })
    ]) {
        const ctx = load();
        await boot(ctx, null);
        ctx.api.showConnectionFailedDialog();
        dismiss(ctx);
        assert.strictEqual(dialog(ctx), null);
        assert.strictEqual(ctx.doc.activeElement, ctx.dom.address, 'focus returns to the form');
    }
});

test('the dialog owns Enter while it is up', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    ctx.api.showConnectionFailedDialog();
    fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    await flush();
    // The first Enter dismissed the dialog; it must not also start a probe.
    assert.deepStrictEqual(ctx.probe.calls, []);
    assert.strictEqual(dialog(ctx), null);
});

test('a dismissed dialog leaves no key handler behind', async () => {
    const ctx = load();
    await boot(ctx, null);
    const before = ctx.doc.listeners.length;
    ctx.api.showConnectionFailedDialog();
    fireEvent(ctx.doc, 'keydown', { key: 'Escape' });
    assert.strictEqual(ctx.doc.listeners.length, before);
});

// ---- cancelling -----------------------------------------------------------

test('Escape while connecting cancels without reporting a failure', async () => {
    const ctx = load();
    await boot(ctx, 'https://saved.example');
    fireEvent(ctx.doc, 'keydown', { key: 'Escape' });
    await flush();

    assert.strictEqual(ctx.probe.aborts, 1);
    assert.strictEqual(stateOf(ctx), 'idle');
    assert.strictEqual(dialog(ctx), null, 'a cancel is not a failure');
    assert.strictEqual(ctx.doc.activeElement, ctx.dom.address);
    assert.strictEqual(ctx.dom.address.disabled, false);
});

test('the cancel button cancels the same way', async () => {
    const ctx = load();
    await boot(ctx, 'https://saved.example');
    const event = fireEvent(ctx.dom.cancel, 'click');
    await flush();
    assert.strictEqual(event.defaultPrevented, true);
    assert.strictEqual(ctx.probe.aborts, 1);
    assert.strictEqual(stateOf(ctx), 'idle');
    assert.strictEqual(dialog(ctx), null);
});

test('cancelling forgets that main was loaded so the retry navigates again', async () => {
    const ctx = load();
    await boot(ctx, 'https://saved.example');
    assert.strictEqual(ctx.api.state().mainLoaded, true);
    ctx.api.cancelConnection();
    await flush();
    assert.strictEqual(ctx.api.state().mainLoaded, false);

    ctx.dom.address.value = 'https://saved.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('https://saved.example/');
    await flush();
    assert.deepStrictEqual(ctx.native.lastCall('navigateMain'), ['https://saved.example/']);
});

test('cancelling with nothing in flight does nothing at all', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.api.cancelConnection();
    assert.strictEqual(ctx.probe.aborts, 0);
    assert.strictEqual(stateOf(ctx), 'idle');
});

test('Escape when not connecting does not cancel anything', async () => {
    const ctx = load();
    await boot(ctx, null);
    fireEvent(ctx.doc, 'keydown', { key: 'Escape' });
    assert.strictEqual(ctx.probe.aborts, 0);
});

test('a cancel during the spinner floor stops the fade-out', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();
    // The probe succeeded and we are waiting out the spinner floor.
    ctx.api.cancelConnection();
    await flush();
    ctx.win.timers.advance(2000);
    await flush();
    assert.deepStrictEqual(ctx.native.callsTo('dismissOverlay'), []);
    assert.strictEqual(ctx.doc.body.classList.contains('fade-out'), false);
    assert.strictEqual(stateOf(ctx), 'idle');
    assert.strictEqual(dialog(ctx), null);
});

test('a probe that lands after a cancel is dropped', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.api.cancelConnection();
    // Native's answer was already on its way when the user pressed cancel.
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();
    assert.deepStrictEqual(ctx.native.callsTo('saveServerUrl'), [], 'nothing is saved');
    assert.deepStrictEqual(ctx.native.callsTo('navigateMain'), []);
});

// ---- the pieces on their own ----------------------------------------------

test('cancellableDelay resolves when its timer fires', async () => {
    const ctx = load();
    let done = false;
    ctx.api.cancellableDelay(500, 'test').then(() => { done = true; });
    ctx.win.timers.advance(499);
    await flush(1);
    assert.strictEqual(done, false);
    ctx.win.timers.advance(1);
    await flush(1);
    assert.strictEqual(done, true);
});

test('cancelling the spinner floor clears its timer instead of letting it fire', async () => {
    const ctx = load();
    await boot(ctx, null);
    ctx.dom.address.value = 'jellyfin.example';
    fireEvent(ctx.dom.form, 'submit');
    await flush();
    ctx.probe.resolve('http://jellyfin.example:8096/');
    await flush();
    assert.strictEqual(ctx.win.timers.pendingCount, 1, 'the pre-fade wait is armed');

    ctx.api.cancelConnection();
    await flush();
    assert.strictEqual(ctx.win.timers.pendingCount, 0, 'cleared, not left to fire later');
    assert.strictEqual(stateOf(ctx), 'idle');
});

test('setState drives the one attribute overlay.css keys off', () => {
    const ctx = load();
    ctx.api.setState('connecting');
    assert.strictEqual(stateOf(ctx), 'connecting');
    ctx.api.setState('idle');
    assert.strictEqual(stateOf(ctx), 'idle');
});

test('tryConnect abandons a probe that resolves after connecting was cleared', async () => {
    const ctx = load();
    await boot(ctx, null);
    // Called directly, so `isConnecting` is false the whole time: the result
    // must be dropped rather than saved.
    const pending = ctx.api.tryConnect('jellyfin.example');
    ctx.probe.resolve('http://jellyfin.example:8096/');
    assert.strictEqual(await pending, false);
    assert.deepStrictEqual(ctx.native.callsTo('saveServerUrl'), []);
});

test('a probe error that is not a cancellation is reported as a failure', async () => {
    const ctx = load();
    await boot(ctx, null);
    const pending = ctx.api.tryConnect('jellyfin.example');
    ctx.probe.reject(new Error('Connection failed'));
    assert.strictEqual(await pending, false);
    assert.ok(ctx.win.console.lines.some((l) => l.level === 'error'));
});

test('a cancellation is logged as one, not as an error', async () => {
    const ctx = load();
    await boot(ctx, null);
    const pending = ctx.api.tryConnect('jellyfin.example');
    ctx.probe.reject(new Error('Connection cancelled'));
    assert.strictEqual(await pending, false);
    assert.strictEqual(ctx.win.console.lines.some((l) => l.level === 'error'), false);
});
