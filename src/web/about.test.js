// Unit tests for the About panel. Run with:
//
//     node --test src/web/about.test.js
//
// (or `just test-js`). The panel is built once at script load from
// `window._aboutData`, which src/jfn_cef/src/resource.rs prepends to this file
// — so the interesting behaviour is what it makes of that blob: which rows
// appear for which fields, that the GPL-2.0 §2(a) upstream credit is rendered
// as prose rather than as a path, that a path row is a button wired to
// `aboutOpenPath`, and that every dismissal path removes the host exactly once
// and tells native the panel is gone.
const test = require('node:test');
const assert = require('node:assert');

const { makeUiWindow, loadModule, makeRecorder, fireEvent } = require('./test/ui-fakes.js');

const DATA = {
    app: '0.5.0-dev',
    cef: '151.3.24',
    basedOn: 'jellium-desktop by Andrew Rabert',
    configDir: 'C:\\Users\\test\\AppData\\Roaming\\astrofin',
    logFile: 'C:\\Users\\test\\AppData\\Local\\astrofin\\astrofin.log'
};

function load(data = DATA, { native = true } = {}) {
    const win = makeUiWindow();
    if (data) win._aboutData = data; // `null` = no blob prepended at all
    if (native) win.jmpNative = makeRecorder(['aboutOpenPath', 'aboutDismiss']);
    const api = loadModule('about.js', win);
    return { win, doc: win.document, api };
}

// [label, value] for every rendered row.
function rowPairs(api) {
    return api.rows.children.map((row) => [
        row.querySelector('.k').textContent,
        row.querySelector('.v').textContent
    ]);
}

test('the panel mounts into a closed shadow root over the whole viewport', () => {
    const ctx = load();
    const host = ctx.doc.getElementById('_jabout');
    assert.strictEqual(host, ctx.api.host);
    assert.strictEqual(host.parentNode, ctx.doc.body);
    // Closed: a server theme or Custom CSS must not be able to reach in.
    assert.strictEqual(host.shadowRoot, undefined);
    assert.match(host.style.cssText, /position:fixed/);
    assert.strictEqual(ctx.api.box.getAttribute('role'), 'dialog');
    assert.strictEqual(ctx.api.box.getAttribute('aria-modal'), 'true');
});

test('a full blob renders every row in order', () => {
    const ctx = load();
    assert.deepStrictEqual(rowPairs(ctx.api), [
        ['App version', DATA.app],
        ['CEF version', DATA.cef],
        ['Based on', DATA.basedOn],
        ['Config directory', DATA.configDir],
        ['Current log file', DATA.logFile]
    ]);
});

test('only the version rows survive a blob with nothing else in it', () => {
    const ctx = load({ app: '0.5.0-dev', cef: '151.3.24' });
    assert.deepStrictEqual(rowPairs(ctx.api).map((r) => r[0]), ['App version', 'CEF version']);
});

test('no blob at all still renders the panel with empty version rows', () => {
    // resource.rs always prepends the blob, but the panel must not depend on
    // any IPC having arrived before first paint.
    const ctx = load(null);
    assert.deepStrictEqual(rowPairs(ctx.api), [['App version', ''], ['CEF version', '']]);
});

test('the upstream credit is prose, not a path', () => {
    const ctx = load();
    const value = ctx.api.rows.children[2].querySelector('.v');
    // GPL-2.0 §2(a): the credit stays verbatim, and it must not look clickable.
    assert.strictEqual(value.textContent, DATA.basedOn);
    assert.ok(value.classList.contains('text'));
    assert.strictEqual(value.tagName, 'DIV');
});

test('path rows are buttons that ask native to open them', () => {
    const ctx = load();
    const paths = ctx.api.rows.querySelectorAll('button.path');
    assert.deepStrictEqual(paths.map((b) => b.textContent), [DATA.configDir, DATA.logFile]);
    fireEvent(paths[0], 'click');
    fireEvent(paths[1], 'click');
    assert.deepStrictEqual(ctx.win.jmpNative.callsTo('aboutOpenPath'),
        [[DATA.configDir], [DATA.logFile]]);
});

test('a path row click is inert when native is missing', () => {
    const ctx = load(DATA, { native: false });
    const button = ctx.api.rows.querySelector('button.path');
    assert.doesNotThrow(() => fireEvent(button, 'click'));
});

test('addRow renders a plain value row when it is not a path', () => {
    const ctx = load();
    const value = ctx.api.addRow('Extra', 'plain value', false);
    assert.strictEqual(value.tagName, 'DIV');
    assert.strictEqual(value.className, 'v');
    assert.strictEqual(ctx.api.rows.children.at(-1).querySelector('.k').textContent, 'Extra');
});

test('addRow with an empty path falls back to a plain row', () => {
    const ctx = load();
    const value = ctx.api.addRow('Current log file', '', true);
    assert.strictEqual(value.tagName, 'DIV', 'nothing to open, so nothing clickable');
    assert.strictEqual(value.textContent, '');
});

test('addRow renders a missing value as empty, never as "undefined"', () => {
    const ctx = load();
    assert.strictEqual(ctx.api.addRow('Nothing', undefined, false).textContent, '');
});

test('the close button dismisses the panel and tells native', () => {
    const ctx = load();
    const close = ctx.api.shadow.querySelector('button.x');
    assert.strictEqual(close.getAttribute('aria-label'), 'Close');
    fireEvent(close, 'click');
    assert.strictEqual(ctx.doc.getElementById('_jabout'), null);
    assert.deepStrictEqual(ctx.win.jmpNative.callsTo('aboutDismiss'), [[]]);
    assert.strictEqual(ctx.api.isDismissed(), true);
});

test('Escape dismisses the panel; other keys do not', () => {
    const ctx = load();
    fireEvent(ctx.win, 'keydown', { key: 'a' });
    assert.strictEqual(ctx.api.isDismissed(), false);
    const event = fireEvent(ctx.win, 'keydown', { key: 'Escape' });
    assert.strictEqual(event.defaultPrevented, true);
    assert.strictEqual(ctx.doc.getElementById('_jabout'), null);
});

test('clicking the backdrop dismisses the panel', () => {
    const ctx = load();
    fireEvent(ctx.api.shadow.querySelector('.bg'), 'mousedown', { button: 0 });
    assert.strictEqual(ctx.api.isDismissed(), true);
    assert.deepStrictEqual(ctx.win.jmpNative.callsTo('aboutDismiss'), [[]]);
});

test('clicking inside the panel does not dismiss it', () => {
    const ctx = load();
    const event = fireEvent(ctx.api.rows, 'mousedown', { button: 0 });
    assert.strictEqual(event.propagationStopped, true, 'the box swallows the click');
    assert.strictEqual(ctx.api.isDismissed(), false);
    assert.ok(ctx.doc.getElementById('_jabout'));
});

test('dismissing twice notifies native only once', () => {
    const ctx = load();
    ctx.api.dismiss();
    ctx.api.dismiss();
    fireEvent(ctx.win, 'keydown', { key: 'Escape' });
    assert.strictEqual(ctx.win.jmpNative.callsTo('aboutDismiss').length, 1);
});

test('dismissal unhooks the key listener it installed', () => {
    const ctx = load();
    const before = ctx.win.listeners.length;
    assert.ok(before > 0);
    ctx.api.dismiss();
    assert.strictEqual(ctx.win.listeners.length, before - 1);
});

test('dismissal works when native is missing', () => {
    const ctx = load(DATA, { native: false });
    assert.doesNotThrow(() => ctx.api.dismiss());
    assert.strictEqual(ctx.doc.getElementById('_jabout'), null);
});

test('onKeyDown ignores keys that are not Escape', () => {
    const ctx = load();
    const event = fireEvent(ctx.win, 'keydown', { key: 'Enter' });
    assert.strictEqual(event.defaultPrevented, false);
    assert.strictEqual(ctx.api.isDismissed(), false);
});
