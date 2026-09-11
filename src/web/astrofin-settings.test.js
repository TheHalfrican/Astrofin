// Unit tests for the Astrofin Settings page. Run with:
//
//     node --test src/web/astrofin-settings.test.js
//
// (or `just test-js`). astrofin-settings.js decorates the page
// client-settings.js builds, so these tests drive the real pair: the form is
// generated from a jmpInfo shaped like the one native-shim.js injects, the
// decorator runs off the `af-settings-show` event, and the assertions are made
// against the resulting DOM.
//
// The properties worth pinning are the ones a reskin can quietly break:
//
//   * every control reaches exactly one panel, and the *same node* does — the
//     change listeners client-settings.js attached are the only path to
//     window.api.settings.setValue, so a clone would silently stop saving;
//   * the segmented switch is a view of the <select>, never a second source of
//     truth: it writes selectedIndex and raises `change`, and follows the
//     select when something else changes it;
//   * the restart pill counts distinct RESTART keys and never counts videoMode;
//   * nothing is invented: the resolve card and the About rows are omitted
//     rather than filled with guesses.
const test = require('node:test');
const assert = require('node:assert');

const {
    makeUiWindow, loadModule, makeRecorder, buildAppShell, fireEvent
} = require('./test/ui-fakes.js');

// jmpInfo as native-shim.js builds it, trimmed to the descriptors that matter
// here but with the real keys, sections and option shapes.
function makeJmpInfo(overrides = {}) {
    return Object.assign({
        version: '0.5.0-dev',
        sections: [
            { key: 'playback', order: 0 },
            { key: 'audio', order: 1 },
            { key: 'transcode', order: 2 },
            { key: 'advanced', order: 3 }
        ],
        settings: {
            main: { userWebClient: 'https://jf.example.com/' },
            playback: { hwdec: 'auto', videoMode: 'auto', transcodeNotice: 'cpu' },
            audio: { audioPassthrough: '', audioExclusive: false, audioChannels: '' },
            transcode: { forceTranscoding: false },
            advanced: { hideScrollbar: true, deviceName: '', logLevel: '' }
        },
        settingsDescriptions: {
            playback: [
                { key: 'hwdec', displayName: 'Hardware Decoding', help: 'Decoder.',
                    options: ['auto', 'no'] },
                { key: 'videoMode', displayName: 'Video mode', help: 'Upscaling preset.',
                    options: [
                        { value: 'auto', title: 'Auto — pick per title from tags, genres and library' },
                        { value: 'live-action', title: 'Live-Action — FSRCNNX x2 + sharp scalers' },
                        { value: 'animation', title: 'Animation — Anime4K Mode A (HQ)' },
                        { value: 'off', title: 'Off — no shaders, mpv default scalers' }
                    ] },
                { key: 'transcodeNotice', displayName: 'Transcode warning', help: 'Toast.',
                    options: [{ value: 'off', title: 'Off' }, { value: 'cpu', title: 'CPU' }] }
            ],
            audio: [
                { key: 'audioPassthrough', displayName: 'Audio Passthrough',
                    inputType: 'textarea', help: 'Codecs.' },
                { key: 'audioExclusive', displayName: 'Exclusive Audio Output', help: 'Exclusive.' },
                { key: 'audioChannels', displayName: 'Audio Channel Layout',
                    options: [{ value: '', title: 'Auto' }, { value: '5.1', title: '5.1' }] }
            ],
            transcode: [
                { key: 'forceTranscoding', displayName: 'Force Transcoding', help: 'Debug aid.' }
            ],
            advanced: [
                { key: 'hideScrollbar', displayName: 'Hide Scrollbar', help: 'Hidden.' },
                { key: 'deviceName', displayName: 'Device Name', inputType: 'text',
                    maxLength: 64, help: 'Identifies this machine.' },
                { key: 'logLevel', displayName: 'Log Level',
                    options: [{ value: '', title: 'Default (Info)' }, { value: 'debug', title: 'Debug' }] }
            ]
        }
    }, overrides);
}

// A window with both modules installed, the app shell built and the page open.
function boot(opts = {}) {
    const win = makeUiWindow();
    win.navigator.userAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) '
        + 'AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36';
    win.jmpInfo = opts.jmpInfo || makeJmpInfo();
    win.api = { settings: makeRecorder(['setValue']) };
    win.jmpNative = makeRecorder(['openConfigDir', 'saveServerUrl']);
    if (opts.window) Object.assign(win, opts.window);

    const client = loadModule('client-settings.js', win);
    const af = loadModule('astrofin-settings.js', win);
    buildAppShell(win.document);
    if (opts.open !== false) client.showSettingsPage();
    const page = win.document.getElementById('clientSettingsPage');
    return { win, doc: win.document, client, af, page };
}

function fire(el, type) {
    el.dispatchEvent(new (el.ownerDocument.defaultView.CustomEvent)(type, { bubbles: true }));
}

function panelOf(page, id) {
    return page.querySelector('[data-af-panel="' + id + '"]');
}

// Which panel each setting ended up in, as a flat `key -> panel id` object.
function placement(page) {
    const map = {};
    page.querySelectorAll('[data-af-setting]').forEach((c) => {
        const panel = c.closest('[data-af-panel]');
        map[c.getAttribute('data-af-setting')] = panel ? panel.getAttribute('data-af-panel') : null;
    });
    return map;
}

function tabs(page) {
    return page.querySelectorAll('.af-set-tab');
}

function visiblePanels(page) {
    return page.querySelectorAll('.af-set-panel')
        .filter((p) => p.classList.contains('af-on'))
        .map((p) => p.getAttribute('data-af-panel'));
}

function segs(page) {
    return page.querySelectorAll('.af-mode-seg');
}

function text(el) {
    return el ? el.textContent : null;
}

// ---- installation ---------------------------------------------------------

test('a second load against the same window installs nothing twice', () => {
    const ctx = boot({ open: false });
    const again = loadModule('astrofin-settings.js', ctx.win);
    assert.strictEqual(again, ctx.af, 're-entry hands back the installation');
    assert.strictEqual(again.install(), false, 'install() is a no-op the second time');
    const count = (type) => ctx.doc.listeners.filter((l) => l.type === type).length;
    assert.strictEqual(count('af-settings-show'), 1);
    assert.strictEqual(count('af-settings-hide'), 1);
});

test('opening the page decorates it exactly once', () => {
    const ctx = boot();
    assert.strictEqual(ctx.page.querySelectorAll('.af-set-rail').length, 1);
    // A second show event for the same page must not build a second rail.
    ctx.af.decorate(ctx.page);
    assert.strictEqual(ctx.page.querySelectorAll('.af-set-rail').length, 1);
});

test('a page without the settings container is left alone', () => {
    const ctx = boot({ open: false });
    const bare = ctx.doc.createElement('div');
    assert.strictEqual(ctx.af.decorate(bare), null);
    assert.strictEqual(ctx.af.decorate(null), null);
});

// ---- the rail -------------------------------------------------------------

test('the rail has the six sections, in artboard order', () => {
    const ctx = boot();
    assert.deepStrictEqual(
        tabs(ctx.page).map((t) => t.getAttribute('data-af-tab')),
        ['server', 'playback', 'video', 'audio', 'advanced', 'about']);
    assert.deepStrictEqual(
        tabs(ctx.page).map((t) => text(t.querySelector('.af-set-tab-label'))),
        ['Server', 'Playback', 'Video mode', 'Audio', 'Advanced', 'About']);
});

test('video mode is the panel the page opens on', () => {
    const ctx = boot();
    assert.deepStrictEqual(visiblePanels(ctx.page), ['video']);
    const current = tabs(ctx.page).filter((t) => t.getAttribute('aria-current') === 'true');
    assert.deepStrictEqual(current.map((t) => t.getAttribute('data-af-tab')), ['video']);
});

test('clicking a rail row shows that panel and only that panel', () => {
    const ctx = boot();
    fireEvent(tabs(ctx.page)[0], 'click');
    assert.deepStrictEqual(visiblePanels(ctx.page), ['server']);
    assert.strictEqual(panelOf(ctx.page, 'video').hidden, true);
    assert.strictEqual(panelOf(ctx.page, 'server').hidden, false);
});

test('the rail rows are buttons, so they cannot submit the settings form', () => {
    const ctx = boot();
    assert.ok(tabs(ctx.page).every((t) => t.type === 'button'));
    assert.ok(segs(ctx.page).every((s) => s.type === 'button'));
});

test('up and down move between rail rows', () => {
    const ctx = boot();
    const rows = tabs(ctx.page);
    fireEvent(rows[2], 'keydown', { key: 'ArrowDown' });
    assert.deepStrictEqual(visiblePanels(ctx.page), ['audio']);
    fireEvent(rows[3], 'keydown', { key: 'ArrowUp' });
    assert.deepStrictEqual(visiblePanels(ctx.page), ['video']);
    // Anything else is left to jellyfin-web.
    fireEvent(rows[2], 'keydown', { key: 'Enter' });
    assert.deepStrictEqual(visiblePanels(ctx.page), ['video']);
});

test('only the selected row is in the tab order', () => {
    const ctx = boot();
    assert.deepStrictEqual(
        tabs(ctx.page).map((t) => t.getAttribute('tabindex')),
        ['-1', '-1', '0', '-1', '-1', '-1']);
});

test('selectPanel ignores a panel that does not exist', () => {
    const ctx = boot();
    assert.strictEqual(ctx.af.selectPanel('nope', false), null);
    assert.deepStrictEqual(visiblePanels(ctx.page), ['video']);
});

// ---- the key -> panel map -------------------------------------------------

test('every setting lands in the panel its key maps to', () => {
    const ctx = boot();
    assert.deepStrictEqual(placement(ctx.page), {
        hwdec: 'playback',
        videoMode: 'video',
        transcodeNotice: 'playback',
        audioPassthrough: 'audio',
        audioExclusive: 'audio',
        audioChannels: 'audio',
        forceTranscoding: 'playback',
        hideScrollbar: 'advanced',
        deviceName: 'server',
        logLevel: 'advanced'
    });
});

test('every control from the stock form is present exactly once', () => {
    const ctx = boot();
    assert.strictEqual(ctx.page.querySelectorAll('[data-af-setting]').length, 10);
    assert.strictEqual(ctx.page.querySelectorAll('select').length, 5);
    assert.strictEqual(ctx.page.querySelectorAll('[data-af-action]').length, 2);
});

test('an unmapped key falls back to its section', () => {
    const jmpInfo = makeJmpInfo();
    jmpInfo.settings.transcode.newThing = false;
    jmpInfo.settingsDescriptions.transcode.push({ key: 'newThing', displayName: 'New thing' });
    jmpInfo.sections.push({ key: 'weird', order: 9 });
    jmpInfo.settings.weird = { mystery: false };
    jmpInfo.settingsDescriptions.weird = [{ key: 'mystery', displayName: 'Mystery' }];
    const ctx = boot({ jmpInfo });
    const map = placement(ctx.page);
    assert.strictEqual(map.newThing, 'playback', 'transcode belongs to the playback panel');
    assert.strictEqual(map.mystery, 'advanced', 'an unknown section falls back to Advanced');
});

test('panelFor maps keys first, then sections, then Advanced', () => {
    const ctx = boot({ open: false });
    assert.strictEqual(ctx.af.panelFor('deviceName', 'advanced'), 'server');
    assert.strictEqual(ctx.af.panelFor('videoMode', 'playback'), 'video');
    assert.strictEqual(ctx.af.panelFor('somethingNew', 'transcode'), 'playback');
    assert.strictEqual(ctx.af.panelFor('somethingNew', 'mpv'), 'advanced');
    assert.strictEqual(ctx.af.panelFor('somethingNew', 'audio'), 'audio');
    assert.strictEqual(ctx.af.panelFor('', ''), 'advanced');
});

test('a moved control is the same node, and still writes through the bridge', () => {
    const ctx = boot();
    const select = ctx.page.querySelector('[data-af-setting="hwdec"] select');
    assert.ok(select, 'the hwdec select survived the move');
    select.selectedIndex = 1;
    fire(select, 'change');
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'), ['playback', 'hwdec', 'no']);
});

test('the label, description and control end up in one row', () => {
    const ctx = boot();
    const row = ctx.page.querySelector('[data-af-row="hwdec"]');
    assert.strictEqual(text(row.querySelector('.af-set-row-label')), 'Hardware Decoding');
    assert.strictEqual(text(row.querySelector('.fieldDescription')), 'Decoder.');
    assert.ok(row.querySelector('.af-set-row-control [data-af-setting="hwdec"]'));
});

test('each row carries the LIVE or RESTART tag its container declared', () => {
    const ctx = boot();
    const restart = ctx.page.querySelector('[data-af-row="hwdec"]');
    assert.strictEqual(restart.getAttribute('data-af-applies'), 'restart');
    assert.strictEqual(text(restart.querySelector('.af-set-tag')), 'RESTART');
    // Video mode is not a row: it is the panel, and its tag is on the heading.
    assert.strictEqual(
        text(panelOf(ctx.page, 'video').querySelector('.af-set-head .af-set-tag-live')),
        'LIVE · APPLIES MID-PLAYBACK');
});

test('the emptied stock sections are hidden and the banner becomes the rail note', () => {
    const ctx = boot();
    const groups = ctx.page.querySelectorAll('.verticalSection');
    assert.ok(groups.length >= 4);
    assert.ok(groups.every((g) => g.hidden === true), 'every stock group was emptied');
    const note = ctx.page.querySelector('[data-af-notice]');
    assert.strictEqual(note.parentNode.className, 'af-set-rail');
    assert.match(text(note), /^Live settings apply at once\./);
});

// ---- the segmented switch -------------------------------------------------

test('the switch has one segment per option, with short labels', () => {
    const ctx = boot();
    assert.deepStrictEqual(segs(ctx.page).map((s) => s.getAttribute('data-af-mode')),
        ['auto', 'live-action', 'animation', 'off']);
    assert.deepStrictEqual(segs(ctx.page).map((s) => text(s.querySelector('.af-mode-seg-label'))),
        ['Auto', 'Live-Action', 'Animation', 'Off']);
    // The descriptor's own wording stays reachable as the tooltip.
    assert.match(segs(ctx.page)[1].getAttribute('title'), /FSRCNNX/);
});

test('the switch marks the stored mode and leaves the select authoritative', () => {
    const ctx = boot();
    assert.deepStrictEqual(segs(ctx.page).map((s) => s.getAttribute('aria-checked')),
        ['true', 'false', 'false', 'false']);
    const select = ctx.page.querySelector('[data-af-setting="videoMode"] select');
    assert.ok(select.isConnected, 'the select stays in the DOM');
    assert.strictEqual(select.closest('[data-af-panel]').getAttribute('data-af-panel'), 'video');
});

test('picking a segment writes the mode through window.api.settings', () => {
    const ctx = boot();
    fireEvent(segs(ctx.page)[2], 'click');
    assert.deepStrictEqual(ctx.win.api.settings.lastCall('setValue'),
        ['playback', 'videoMode', 'animation']);
    assert.strictEqual(ctx.win.jmpInfo.settings.playback.videoMode, 'animation');
    assert.deepStrictEqual(segs(ctx.page).map((s) => s.classList.contains('af-on')),
        [false, false, true, false]);
});

test('picking the mode that is already set writes nothing', () => {
    const ctx = boot();
    fireEvent(segs(ctx.page)[0], 'click');
    assert.deepStrictEqual(ctx.win.api.settings.callsTo('setValue'), []);
});

test('the switch follows a change made on the select itself', () => {
    const ctx = boot();
    const select = ctx.page.querySelector('[data-af-setting="videoMode"] select');
    select.selectedIndex = 3;
    fire(select, 'change');
    assert.deepStrictEqual(segs(ctx.page).map((s) => s.classList.contains('af-on')),
        [false, false, false, true]);
});

test('the arrow keys move the selection along the switch', () => {
    const ctx = boot();
    fireEvent(segs(ctx.page)[0], 'keydown', { key: 'ArrowRight' });
    assert.strictEqual(ctx.win.jmpInfo.settings.playback.videoMode, 'live-action');
    fireEvent(segs(ctx.page)[1], 'keydown', { key: 'ArrowLeft' });
    assert.strictEqual(ctx.win.jmpInfo.settings.playback.videoMode, 'auto');
    // Past the ends is a no-op, not a wrap.
    fireEvent(segs(ctx.page)[0], 'keydown', { key: 'ArrowLeft' });
    assert.strictEqual(ctx.win.jmpInfo.settings.playback.videoMode, 'auto');
});

test('the mode description follows the selection', () => {
    const ctx = boot();
    const title = ctx.page.querySelector('.af-mode-detail-title');
    const body = ctx.page.querySelector('.af-mode-detail-body');
    assert.strictEqual(text(title), 'Auto');
    assert.match(text(body), /^pick per title/);
    fireEvent(segs(ctx.page)[3], 'click');
    assert.strictEqual(text(title), 'Off');
    assert.match(text(body), /^no shaders/);
});

test('shortModeLabel and modeDescription split the descriptor wording', () => {
    const ctx = boot({ open: false });
    assert.strictEqual(ctx.af.shortModeLabel('live-action', 'Live-Action — sharp'), 'Live-Action');
    assert.strictEqual(ctx.af.shortModeLabel('mystery', 'Mystery — later'), 'Mystery');
    assert.strictEqual(ctx.af.shortModeLabel('', ''), '');
    assert.strictEqual(ctx.af.modeDescription('Auto — pick per title'), 'pick per title');
    assert.strictEqual(ctx.af.modeDescription('No dash here'), '');
    assert.strictEqual(ctx.af.modeLabel('anime'), 'Animation');
});

// ---- the restart pill -----------------------------------------------------

test('the pill is hidden until a RESTART setting changes', () => {
    const ctx = boot();
    const pill = ctx.page.querySelector('.af-set-pill');
    assert.strictEqual(pill.hidden, true);
    fire(ctx.page.querySelector('[data-af-setting="hwdec"] select'), 'change');
    assert.strictEqual(pill.hidden, false);
    assert.strictEqual(text(pill.querySelector('.af-set-pill-label')),
        'Restart to apply 1 change');
});

test('the pill counts distinct keys, not events', () => {
    const ctx = boot();
    const pill = ctx.page.querySelector('.af-set-pill');
    const hwdec = ctx.page.querySelector('[data-af-setting="hwdec"] select');
    fire(hwdec, 'change');
    fire(hwdec, 'change');
    assert.strictEqual(text(pill.querySelector('.af-set-pill-label')),
        'Restart to apply 1 change');
    fire(ctx.page.querySelector('[data-af-setting="logLevel"] select'), 'change');
    assert.strictEqual(text(pill.querySelector('.af-set-pill-label')),
        'Restart to apply 2 changes');
});

test('a live setting never raises the pill', () => {
    const ctx = boot();
    fireEvent(segs(ctx.page)[1], 'click');
    assert.strictEqual(ctx.win.jmpInfo.settings.playback.videoMode, 'live-action');
    assert.strictEqual(ctx.page.querySelector('.af-set-pill').hidden, true);
});

test('the pill starts empty again on the next page', () => {
    const ctx = boot();
    fire(ctx.page.querySelector('[data-af-setting="hwdec"] select'), 'change');
    assert.strictEqual(ctx.page.querySelector('.af-set-pill').hidden, false);

    ctx.doc._callbacks.HISTORY_UPDATE[0]();          // back button
    assert.strictEqual(ctx.af.state().view, null, 'the hide event dropped the view');
    assert.strictEqual(ctx.af.updatePill(), 0, 'with no view there is nothing to count');

    ctx.client.showSettingsPage();
    const page = ctx.doc.getElementById('clientSettingsPage');
    assert.strictEqual(page.querySelector('.af-set-pill').hidden, true);
});

// ---- the server panel -----------------------------------------------------

test('the server panel leads with the saved address and its scheme', () => {
    const ctx = boot();
    const row = ctx.page.querySelector('[data-af-row="serverAddress"]');
    assert.strictEqual(row.parentNode.children[0], row, 'it is the first row in the panel');
    assert.strictEqual(text(row.querySelector('.af-set-address-url')), 'https://jf.example.com/');
    assert.strictEqual(text(row.querySelector('.af-set-address-kind')), 'HTTPS');
});

test('the address row is omitted when no server is saved', () => {
    const jmpInfo = makeJmpInfo();
    jmpInfo.settings.main.userWebClient = '';
    const ctx = boot({ jmpInfo });
    assert.strictEqual(ctx.page.querySelector('[data-af-row="serverAddress"]'), null);
    // ...and so are the two buttons client-settings.js gates on the same value.
    assert.strictEqual(ctx.page.querySelector('[data-af-action]'), null);
});

test('connectionKind reports the scheme and nothing more', () => {
    const ctx = boot({ open: false });
    assert.strictEqual(ctx.af.connectionKind('http://192.168.50.76:8096'), 'http');
    assert.strictEqual(ctx.af.connectionKind('HTTPS://jf.example.com'), 'https');
    assert.strictEqual(ctx.af.connectionKind('jf.example.com'), '');
    assert.strictEqual(ctx.af.connectionKind(null), '');
});

test('the sign-out row keeps the real button and its handler', () => {
    const ctx = boot();
    const row = ctx.page.querySelector('[data-af-row="reset-server"]');
    assert.strictEqual(row.closest('[data-af-panel]').getAttribute('data-af-panel'), 'server');
    assert.match(text(row.querySelector('.fieldDescription')), /Forgets the saved address/);
    const btn = row.querySelector('[data-af-action="reset-server"]');
    assert.strictEqual(text(btn), 'Sign out of this server');
    fireEvent(btn, 'click');
    assert.deepStrictEqual(ctx.win.jmpNative.lastCall('saveServerUrl'), ['']);
});

test('the mpv config button moves to Advanced with its handler intact', () => {
    const ctx = boot();
    const row = ctx.page.querySelector('[data-af-row="open-config-dir"]');
    assert.strictEqual(row.closest('[data-af-panel]').getAttribute('data-af-panel'), 'advanced');
    fireEvent(row.querySelector('[data-af-action="open-config-dir"]'), 'click');
    assert.deepStrictEqual(ctx.win.jmpNative.callsTo('openConfigDir'), [[]]);
});

// ---- the about panel ------------------------------------------------------

test('the about panel prints the version, platform and Chromium build', () => {
    const ctx = boot();
    const rows = panelOf(ctx.page, 'about').querySelectorAll('.af-about-row');
    assert.deepStrictEqual(rows.map((r) => text(r.children[0])),
        ['Version', 'Platform', 'Chromium (CEF)']);
    assert.deepStrictEqual(rows.map((r) => text(r.children[1])),
        ['0.5.0-dev', 'Win32', '151.0.0.0']);
    assert.strictEqual(text(panelOf(ctx.page, 'about').querySelector('.af-about-version')),
        '0.5.0-dev');
    assert.match(text(panelOf(ctx.page, 'about').querySelector('.af-about-blurb')), /GPL-2\.0/);
});

test('an about row with nothing behind it is left out', () => {
    const ctx = boot({ open: false });
    assert.deepStrictEqual(ctx.af.aboutRows(null, null), []);
    assert.deepStrictEqual(ctx.af.aboutRows({ version: '1.0' }, { platform: 'MacIntel' }),
        [['Version', '1.0'], ['Platform', 'MacIntel']]);
});

test('chromiumVersion reads the UA and gives up cleanly', () => {
    const ctx = boot({ open: false });
    assert.strictEqual(ctx.af.chromiumVersion('… Chrome/151.3.24 Safari/537.36'), '151.3.24');
    assert.strictEqual(ctx.af.chromiumVersion('astrofin-test'), '');
    assert.strictEqual(ctx.af.chromiumVersion(undefined), '');
});

// ---- the now-playing resolve card -----------------------------------------

// The card exists only when a title is loaded *and* a resolution was recorded
// by mpv-video-player.js. Nothing here fabricates one.
function playing(win, resolved) {
    const layer = win.document.createElement('div');
    layer.className = 'videoPlayerContainer';
    win.document.body.appendChild(layer);
    if (resolved) win.__afVideoModeResolved = resolved;
}

test('nothing is playing, so there is no resolve card', () => {
    const ctx = boot();
    assert.strictEqual(ctx.af.resolveInfo(), null);
    assert.strictEqual(ctx.page.querySelector('.af-resolve'), null);
});

test('a recorded resolution is shown while a title is loaded', () => {
    const ctx = boot({ open: false });
    playing(ctx.win, { mode: 'animation', reason: 'genre: Animation', title: 'Perfect Blue' });
    ctx.client.showSettingsPage();
    const page = ctx.doc.getElementById('clientSettingsPage');
    const card = page.querySelector('.af-resolve');
    assert.ok(card, 'the card is rendered');
    assert.strictEqual(text(card.querySelector('.af-resolve-title')), 'Perfect Blue');
    assert.strictEqual(text(card.querySelector('.af-resolve-mode')), 'Animation');
    assert.strictEqual(text(card.querySelector('.af-resolve-reason')), 'genre: Animation');
    // Genre won, so the tag step is spent and the two below it never ran.
    assert.deepStrictEqual(
        card.querySelectorAll('.af-resolve-step').map((s) => [
            s.classList.contains('af-past'), s.classList.contains('af-on')]),
        [[true, false], [false, true], [false, false], [false, false]]);
});

test('the card is omitted when the mode is not Auto', () => {
    const jmpInfo = makeJmpInfo();
    jmpInfo.settings.playback.videoMode = 'animation';
    const ctx = boot({ jmpInfo, open: false });
    playing(ctx.win, { mode: 'animation', reason: 'genre: Animation', title: 'Perfect Blue' });
    assert.strictEqual(ctx.af.resolveInfo(), null);
});

test('the card is omitted when nothing was recorded', () => {
    const ctx = boot({ open: false });
    playing(ctx.win, null);
    assert.strictEqual(ctx.af.resolveInfo(), null);
    ctx.win.__afVideoModeResolved = { mode: 'live-action' };   // no reason
    assert.strictEqual(ctx.af.resolveInfo(), null);
});

test('chainIndexFor names the rule that won, and nothing when it cannot', () => {
    const ctx = boot({ open: false });
    assert.strictEqual(ctx.af.chainIndexFor('tag: astrofin:anime'), 0);
    assert.strictEqual(ctx.af.chainIndexFor('series tag: astrofin:live'), 0);
    assert.strictEqual(ctx.af.chainIndexFor('genre: Animation'), 1);
    assert.strictEqual(ctx.af.chainIndexFor('series genre: Anime'), 1);
    assert.strictEqual(ctx.af.chainIndexFor('library: Movies'), 2);
    assert.strictEqual(ctx.af.chainIndexFor('library name: Anime'), 2);
    assert.strictEqual(ctx.af.chainIndexFor('default'), 3);
    assert.strictEqual(ctx.af.chainIndexFor('fallback after error: boom'), -1);
    assert.strictEqual(ctx.af.chainIndexFor(''), -1);
});
