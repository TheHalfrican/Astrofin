// Unit tests for the connect screen's connectivity probe. Run with:
//
//     node --test src/web/connectivityHelper.test.js
//
// (or `just test-js`). The module is one promise bridged across two worlds:
// `jmpCheckServerConnectivity(url)` asks native to probe and parks the
// resolve/reject pair, and `_onServerConnectivityResult(url, ok, resolved)` —
// called from Rust — settles it. What is worth testing is the parking: the
// url guard that keeps a stale reply from settling a newer probe, the abort
// path (both while a probe is outstanding and while nothing is), and the
// startup race where the script is injected before `jmpNative` exists.
const test = require('node:test');
const assert = require('node:assert');

const { makeUiWindow, loadModule } = require('./test/ui-fakes.js');

// Drain the microtask queue and fire any timer that is due, `steps` times.
// The module's wait loop is `await setTimeout(100)`, so one step per attempt.
async function settle(win, steps = 3) {
    for (let i = 0; i < steps; i++) {
        await new Promise((r) => setImmediate(r));
        win.timers.advance(100);
    }
    await new Promise((r) => setImmediate(r));
}

// `native: false` loads the module with no `jmpNative` at all (the injection
// race); otherwise the recorder is in place before the script runs.
function load({ native = true } = {}) {
    const win = makeUiWindow();
    if (native) win.jmpNative = makeNative();
    const check = loadModule('connectivityHelper.js', win);
    return { win, check, result: win._onServerConnectivityResult };
}

function makeNative() {
    const calls = [];
    return {
        calls,
        checkServerConnectivity(url) { calls.push(['check', url]); },
        cancelServerConnectivity() { calls.push(['cancel']); }
    };
}

test('a successful native result resolves with the url native normalised', async () => {
    const { win, check, result } = load();
    const pending = check('jellyfin.example');
    await settle(win);
    assert.deepStrictEqual(win.jmpNative.calls, [['check', 'jellyfin.example']]);

    // Native answers with the resolved URL, which is what the caller saves.
    result('jellyfin.example', true, 'https://jellyfin.example:8920');
    assert.strictEqual(await pending, 'https://jellyfin.example:8920');
});

test('a failed native result rejects with a connection error', async () => {
    const { win, check, result } = load();
    const pending = check('http://nope.invalid');
    await settle(win);
    result('http://nope.invalid', false, '');
    await assert.rejects(pending, /Connection failed/);
});

test('a result for a url we are no longer probing is ignored', async () => {
    const { win, check, result } = load();
    let settled = false;
    const pending = check('http://a.example');
    pending.then(() => { settled = true; }, () => { settled = true; });
    await settle(win);

    // A reply for the previous server must not settle the current probe.
    result('http://old.example', true, 'http://old.example');
    await settle(win);
    assert.strictEqual(settled, false);

    result('http://a.example', true, 'http://a.example');
    assert.strictEqual(await pending, 'http://a.example');
});

test('a second result for the same url after settling is ignored', async () => {
    const { win, check, result } = load();
    const pending = check('http://a.example');
    await settle(win);
    result('http://a.example', true, 'http://a.example');
    await pending;
    // The pending state was cleared, so the duplicate finds nothing to do.
    assert.doesNotThrow(() => result('http://a.example', false, ''));
});

test('a result while nothing is pending currently throws when the url is null', () => {
    // Characterisation, not endorsement: `pendingUrl` starts as null, so a
    // null-url result from native passes the `pendingUrl === url` guard and
    // calls a null `pendingResolve`. Native only ever sends the url it was
    // given, so this is unreachable today — see the phase-3 report.
    const { result } = load();
    assert.throws(() => result(null, true, 'http://x.example'), TypeError);
});

test('abort cancels the native probe and rejects the outstanding promise', async () => {
    const { win, check } = load();
    const pending = check('http://a.example');
    await settle(win);
    check.abort();
    await assert.rejects(pending, /Connection cancelled/);
    assert.deepStrictEqual(win.jmpNative.calls.at(-1), ['cancel']);
});

test('abort with nothing outstanding still tells native to cancel', () => {
    const { win, check } = load();
    assert.doesNotThrow(() => check.abort());
    assert.deepStrictEqual(win.jmpNative.calls, [['cancel']]);
});

test('a result arriving after abort no longer settles anything', async () => {
    const { win, check, result } = load();
    const pending = check('http://a.example');
    await settle(win);
    check.abort();
    await assert.rejects(pending, /Connection cancelled/);
    // Native's in-flight reply races the cancel; it must be a no-op.
    assert.doesNotThrow(() => result('http://a.example', true, 'http://a.example'));
});

test('abort works when native has no cancel entry point', async () => {
    const { win, check } = load();
    const pending = check('http://a.example');
    await settle(win);
    delete win.jmpNative.cancelServerConnectivity;
    check.abort();
    await assert.rejects(pending, /Connection cancelled/);
});

test('a probe started before jmpNative exists waits for it', async () => {
    const { win, check } = load({ native: false });
    const pending = check('http://a.example');
    // Three failed polls, then native arrives.
    await settle(win, 3);
    win.jmpNative = makeNative();
    await settle(win, 2);
    assert.deepStrictEqual(win.jmpNative.calls, [['check', 'http://a.example']]);
    win._onServerConnectivityResult('http://a.example', true, 'http://a.example/');
    assert.strictEqual(await pending, 'http://a.example/');
});

test('a probe gives up once native has not appeared in 50 polls', async () => {
    const { win, check } = load({ native: false });
    // The assertion is attached before the clock runs: the rejection lands
    // inside `settle`, and an unhandled one fails the whole file.
    const rejected = assert.rejects(check('http://a.example'),
        /Native connectivity check not available/);
    await settle(win, 55);
    await rejected;
});

test('the module installs both halves of the bridge on window', () => {
    const { win, check } = load();
    assert.strictEqual(typeof win.jmpCheckServerConnectivity, 'function');
    assert.strictEqual(win.jmpCheckServerConnectivity, check);
    assert.strictEqual(typeof check.abort, 'function');
    assert.strictEqual(typeof win._onServerConnectivityResult, 'function');
});
