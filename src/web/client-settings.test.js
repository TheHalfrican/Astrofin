// Unit tests for the Client Settings page. Run with:
//
//     node --test src/web/client-settings.test.js
//
// (or `just test-js`). The module has no state of its own: it renders a
// jellyfin-web legacy page from `window.jmpInfo` and writes every change
// straight back through `window.api.settings.setValue`. What is worth testing
// is therefore the wiring — that each descriptor shape produces the control it
// claims, that a change reaches both the local mirror and the IPC, that a
// `null` option value survives the round trip through the DOM (it cannot go
// through `select.value`, which is always a string), and that the page tears
// itself down on navigation and puts jellyfin-web's own pages back.
const test = require('node:test');
const assert = require('node:assert');

const {
    makeUiWindow, loadModule, makeRecorder, buildAppShell
} = require('./test/ui-fakes.js');

// A jmpInfo with one descriptor of every supported shape.
function makeJmpInfo(overrides = {}) {
    return Object.assign({
        sections: [{ key: 'video', order: 2 }, { key: 'main', order: 1 }],
        settings: {
            main: { userWebClient: 'http://server.example', fullscreen: true, title: 'Astrofin' },
            video: { hwdec: 'auto', codecs: ['hevc', 'h264'] }
        },
        settingsDescriptions: {
            main: [
                { key: 'fullscreen', displayName: 'Start fullscreen', help: 'On boot.' },
                { key: 'title', displayName: 'Window title', inputType: 'text',
                    placeholder: 'Astrofin', maxLength: 40 }
            ],
            video: [
                { key: 'hwdec', displayName: 'Hardware decoding',
                    options: ['auto', 'no', { value: null, title: 'System default' }] },
                { key: 'codecs', displayName: 'Codecs', inputType: 'codecList',
                    codecListSource: 'mpvCodecs', help: 'Order matters.' }
            ]
        },
        mpvCodecs: ['hevc', 'h264', 'av1']
    }, overrides);
}

function load(overrides = {}) {
    const win = makeUiWindow();
    win.jmpInfo = overrides.jmpInfo || makeJmpInfo();
    win.api = { settings: makeRecorder(['setValue']) };
    win.jmpNative = makeRecorder(['openConfigDir', 'saveServerUrl']);
    const api = loadModule('client-settings.js', win);
    return { win, doc: win.document, api, jmpInfo: win.jmpInfo };
}

// Render just the form, without the page shell around it.
function renderForm(ctx) {
    const form = ctx.doc.createElement('form');
    ctx.doc.body.appendChild(form);
    ctx.api.buildSettingsForm(form);
    return form;
}

function fire(el, type) {
    el.dispatchEvent(new (el.ownerDocument.defaultView.CustomEvent)(type, { bubbles: true }));
}

// ---- page lifecycle -------------------------------------------------------

test('the page is not opened when jellyfin-web has no animated pages', () => {
    const ctx = load();
    // No `.mainAnimatedPages`: nothing to insert into, and nothing must break.
    assert.doesNotThrow(() => ctx.api.showSettingsPage());
    assert.strictEqual(ctx.doc.getElementById('clientSettingsPage'), null);
    assert.strictEqual(ctx.win.history.calls.length, 0);
});

test('opening inserts a legacy page and hides what was showing', () => {
    const ctx = load();
    const shell = buildAppShell(ctx.doc, { visiblePages: 2 });
    ctx.api.showSettingsPage();

    const page = ctx.doc.getElementById('clientSettingsPage');
    assert.ok(page, 'the settings page is inserted');
    assert.strictEqual(page.parentNode, shell.pages);
    assert.strictEqual(page.getAttribute('data-role'), 'page');
    assert.strictEqual(page.getAttribute('data-backbutton'), 'true');
    assert.ok(page.classList.contains('mainAnimatedPage'));
    assert.ok(page.classList.contains('userPreferencesPage'));
    // Both legacy pages and the React container are hidden, not removed.
    assert.ok(shell.legacy.every((p) => p.classList.contains('hide')));
    assert.ok(shell.react.classList.contains('hide'));
    assert.ok(shell.legacy.every((p) => p.isConnected));
});

test('opening pushes history so the back button leaves the page', () => {
    const ctx = load();
    buildAppShell(ctx.doc);
    ctx.api.showSettingsPage();
    assert.deepStrictEqual(ctx.win.history.calls, [
        { state: { clientSettings: true }, title: '', url: undefined }
    ]);
});

test('opening dispatches the four page events libraryMenu listens on', () => {
    const ctx = load();
    buildAppShell(ctx.doc);
    const seen = [];
    ['viewbeforeshow', 'pagebeforeshow', 'viewshow', 'pageshow'].forEach((type) => {
        ctx.doc.body.addEventListener(type, (e) => seen.push([type, e.detail.isRestored]));
    });
    ctx.api.showSettingsPage();
    assert.deepStrictEqual(seen, [
        ['viewbeforeshow', false],
        ['pagebeforeshow', false],
        ['viewshow', false],
        ['pageshow', false]
    ]);
});

test('dispatchPageEvents marks a restored page as restored', () => {
    const ctx = load();
    const target = ctx.doc.createElement('div');
    ctx.doc.body.appendChild(target);
    const seen = [];
    target.addEventListener('pageshow', (e) => seen.push(e.detail));
    ctx.api.dispatchPageEvents(target, true);
    assert.strictEqual(seen.length, 1);
    assert.strictEqual(seen[0].isRestored, true);
    assert.deepStrictEqual(seen[0].properties, []);
    assert.deepStrictEqual(seen[0].params, {});
});

test('navigating away removes the page and restores what it hid', () => {
    const ctx = load();
    const shell = buildAppShell(ctx.doc);
    ctx.api.showSettingsPage();

    const restored = [];
    shell.reactPage.addEventListener('pageshow', (e) => restored.push(e.detail.isRestored));

    const teardown = ctx.doc._callbacks.HISTORY_UPDATE[0];
    assert.strictEqual(typeof teardown, 'function');
    teardown();

    assert.strictEqual(ctx.doc.getElementById('clientSettingsPage'), null);
    assert.strictEqual(shell.react.classList.contains('hide'), false);
    assert.ok(shell.legacy.every((p) => !p.classList.contains('hide')));
    // The React page was only CSS-hidden, so its header has to be re-announced.
    assert.deepStrictEqual(restored, [true]);
    // ...and the teardown deregisters itself, so a second navigation is a no-op.
    assert.deepStrictEqual(ctx.doc._callbacks.HISTORY_UPDATE, []);
});

// ---- form building --------------------------------------------------------

test('the form leads with the restart notice and one section per group', () => {
    const ctx = load();
    const form = renderForm(ctx);
    assert.strictEqual(form.children[0].className, 'infoBanner');
    assert.match(form.children[0].textContent, /restarting the application/);
    // `order` decides the sequence, not the object order of settingsDescriptions.
    assert.deepStrictEqual(
        form.querySelectorAll('h2.sectionTitle').map((h) => h.textContent),
        ['Main', 'Video', 'MPV config', 'Server']
    );
});

test('a section with no descriptors is skipped', () => {
    const jmpInfo = makeJmpInfo();
    jmpInfo.sections.push({ key: 'empty', order: 0 });
    jmpInfo.settings.empty = {};
    jmpInfo.settingsDescriptions.empty = [];
    const ctx = load({ jmpInfo });
    const form = renderForm(ctx);
    assert.ok(!form.querySelectorAll('h2.sectionTitle')
        .some((h) => h.textContent === 'Empty'));
});

test('a checkbox descriptor reflects and writes back its value', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const box = form.querySelector('.checkboxContainer input');
    assert.strictEqual(box.checked, true, 'seeded from jmpInfo.settings');
    assert.strictEqual(box.getAttribute('is'), 'emby-checkbox');
    assert.strictEqual(form.querySelector('.checkboxLabel').textContent, 'Start fullscreen');

    box.checked = false;
    fire(box, 'change');
    assert.strictEqual(ctx.jmpInfo.settings.main.fullscreen, false);
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['main', 'fullscreen', false]);
});

test('a checkbox with help text gets the description class and the text', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const container = form.querySelector('.checkboxContainer');
    assert.ok(container.classList.contains('checkboxContainer-withDescription'));
    assert.strictEqual(
        container.querySelector('.fieldDescription').textContent, 'On boot.');
});

test('a text descriptor carries placeholder and maxLength and writes on change', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const input = form.querySelector('.inputContainer input');
    assert.strictEqual(input.value, 'Astrofin');
    assert.strictEqual(input.placeholder, 'Astrofin');
    assert.strictEqual(input.maxLength, 40);

    input.value = 'Astrofin (dev)';
    fire(input, 'change');
    assert.strictEqual(ctx.jmpInfo.settings.main.title, 'Astrofin (dev)');
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['main', 'title', 'Astrofin (dev)']);
});

test('a textarea descriptor renders a textarea, not an input', () => {
    const jmpInfo = makeJmpInfo();
    jmpInfo.settingsDescriptions.main = [
        { key: 'extraArgs', displayName: 'Extra mpv arguments', inputType: 'textarea' }
    ];
    const ctx = load({ jmpInfo });
    const form = renderForm(ctx);
    const area = form.querySelector('textarea');
    assert.ok(area, 'a textarea is used');
    assert.strictEqual(area.rows, 2);
    assert.strictEqual(area.value, '', 'a missing value renders as empty, not "undefined"');
    area.value = '--vo=gpu-next';
    fire(area, 'change');
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['main', 'extraArgs', '--vo=gpu-next']);
});

test('an options descriptor renders a select with the current value chosen', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const select = form.querySelector('.selectContainer select');
    assert.strictEqual(select.getAttribute('is'), 'emby-select');
    assert.strictEqual(select.getAttribute('label'), 'Hardware decoding');
    assert.deepStrictEqual(select.children.map((o) => o.textContent),
        ['auto', 'no', 'System default']);
    assert.strictEqual(select.selectedIndex, 0, 'hwdec: auto is selected');
});

test('choosing an option writes the option value, not the select text', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const select = form.querySelector('.selectContainer select');
    select.selectedIndex = 1;
    fire(select, 'change');
    assert.strictEqual(ctx.jmpInfo.settings.video.hwdec, 'no');
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['video', 'hwdec', 'no']);
});

test('a null option value survives the round trip through the DOM', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const select = form.querySelector('.selectContainer select');
    // The "System default" option carries value null; the DOM would stringify
    // it, so the handler indexes back into the descriptor instead.
    select.selectedIndex = 2;
    fire(select, 'change');
    assert.strictEqual(ctx.jmpInfo.settings.video.hwdec, null);
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['video', 'hwdec', null]);
});

test('a null option is preselected when the setting is unset', () => {
    const jmpInfo = makeJmpInfo();
    jmpInfo.settings.video.hwdec = undefined;
    const ctx = load({ jmpInfo });
    const form = renderForm(ctx);
    const select = form.querySelector('.selectContainer select');
    assert.strictEqual(select.selectedIndex, 2);
});

test('the mpv config and reset buttons appear only with a saved server', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const buttons = form.querySelectorAll('button.emby-button');
    assert.deepStrictEqual(buttons.map((b) => b.textContent),
        ['Open mpv config directory', 'Reset Saved Server']);

    const jmpInfo = makeJmpInfo();
    jmpInfo.settings.main.userWebClient = '';
    const bare = load({ jmpInfo });
    assert.deepStrictEqual(
        renderForm(bare).querySelectorAll('button.emby-button').map((b) => b.textContent),
        []);
});

test('the mpv config button asks native to open the config directory', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const btn = form.querySelectorAll('button.emby-button')[0];
    fire(btn, 'click');
    assert.deepStrictEqual(ctx.win.jmpNative.callsTo('openConfigDir'), [[]]);
});

test('the mpv config button is inert when native is missing', () => {
    const ctx = load();
    const form = renderForm(ctx);
    delete ctx.win.jmpNative;
    assert.doesNotThrow(() => fire(form.querySelectorAll('button.emby-button')[0], 'click'));
});

test('resetting the server clears it locally, natively, and reloads', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const btn = form.querySelectorAll('button.emby-button')[1];
    fire(btn, 'click');
    assert.strictEqual(ctx.jmpInfo.settings.main.userWebClient, '');
    assert.deepStrictEqual(ctx.win.jmpNative.lastCall('saveServerUrl'), ['']);
    assert.strictEqual(ctx.win.location.reloads, 1);
});

// ---- the codec list widget ------------------------------------------------

function codecRows(widget) {
    return widget.children.map((row) => ({
        codec: row.children[1].textContent,
        checked: row.children[0].checked,
        upDisabled: row.children[2].disabled,
        downDisabled: row.children[3].disabled
    }));
}

function makeWidget(ctx, opts) {
    const changes = [];
    const widget = ctx.api.renderCodecList(Object.assign({
        enabled: ['hevc', 'h264'],
        all: ['hevc', 'h264', 'av1'],
        onChange: (list) => changes.push(list)
    }, opts));
    ctx.doc.body.appendChild(widget);
    return { widget, changes };
}

test('the codec list shows enabled codecs first, in preference order', () => {
    const ctx = load();
    const { widget } = makeWidget(ctx, { enabled: ['h264', 'hevc'] });
    assert.deepStrictEqual(codecRows(widget).map((r) => [r.codec, r.checked]),
        [['h264', true], ['hevc', true], ['av1', false]]);
});

test('an enabled codec mpv does not support is dropped from the list', () => {
    const ctx = load();
    // `enabled` comes from settings.json and can name a codec this mpv build
    // has no decoder for; it must not be rendered as an option.
    const { widget } = makeWidget(ctx, { enabled: ['hevc', 'vp9'], all: ['hevc', 'h264'] });
    assert.deepStrictEqual(codecRows(widget).map((r) => r.codec), ['hevc', 'h264']);
});

test('the codec list widget emits nothing until something is toggled', () => {
    const ctx = load();
    const { changes } = makeWidget(ctx);
    assert.deepStrictEqual(changes, []);
});

test('enabling a codec appends it to the enabled block and re-emits', () => {
    const ctx = load();
    const { widget, changes } = makeWidget(ctx);
    const av1 = widget.children[2].children[0];
    av1.checked = true;
    fire(av1, 'change');
    assert.deepStrictEqual(changes, [['hevc', 'h264', 'av1']]);
    assert.deepStrictEqual(codecRows(widget).map((r) => [r.codec, r.checked]),
        [['hevc', true], ['h264', true], ['av1', true]]);
});

test('disabling a codec moves it below the enabled block and re-emits', () => {
    const ctx = load();
    const { widget, changes } = makeWidget(ctx);
    const hevc = widget.children[0].children[0];
    hevc.checked = false;
    fire(hevc, 'change');
    assert.deepStrictEqual(changes, [['h264']]);
    assert.deepStrictEqual(codecRows(widget).map((r) => [r.codec, r.checked]),
        [['h264', true], ['hevc', false], ['av1', false]]);
});

test('the arrows reorder within the enabled block and re-emit', () => {
    const ctx = load();
    const { widget, changes } = makeWidget(ctx);
    fire(widget.children[1].children[2], 'click'); // h264 up
    assert.deepStrictEqual(changes, [['h264', 'hevc']]);
    fire(widget.children[0].children[3], 'click'); // h264 back down
    assert.deepStrictEqual(changes.at(-1), ['hevc', 'h264']);
});

test('the arrows are disabled where a move would leave the enabled block', () => {
    const ctx = load();
    const { widget } = makeWidget(ctx);
    assert.deepStrictEqual(codecRows(widget).map((r) => [r.upDisabled, r.downDisabled]), [
        [true, false],  // hevc: first, so no up
        [false, true],  // h264: last enabled, so no down
        [true, true]    // av1: disabled codec, neither
    ]);
});

test('a codec list built from the descriptor writes the ordered list back', () => {
    const ctx = load();
    const form = renderForm(ctx);
    const widget = form.querySelector('.codecList');
    assert.ok(widget, 'the codecList inputType renders the widget');
    assert.strictEqual(form.querySelectorAll('.inputLabel')
        .some((l) => l.textContent === 'Codecs'), true);
    const av1 = widget.children[2].children[0];
    av1.checked = true;
    fire(av1, 'change');
    assert.deepStrictEqual(ctx.jmpInfo.settings.video.codecs, ['hevc', 'h264', 'av1']);
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['video', 'codecs', ['hevc', 'h264', 'av1']]);
});
