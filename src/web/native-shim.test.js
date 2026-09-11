// Unit tests for the native shim. Run with:
//
//     node --test src/web/native-shim.test.js
//
// (or `just test-js`). This file is the boundary jellyfin-web sees: it puts
// `window.api`, `window.jmpInfo` and `window.NativeShell` where a Jellyfin
// Media Player build would, and it is the only place the `jmpNative` bridge
// is called with the argument shapes `src/jfn_cef/src/business_web.rs` parses.
// So what is worth testing is the translation in both directions:
//
//   * the settings blob the renderer splices in (`__SETTINGS_JSON__` and
//     friends) turned into `jmpInfo`, defaults and all;
//   * every `api.player` call turned into the exact native call and argument
//     units — milliseconds to seconds for the delays, the nine-slot
//     `playerLoad`, thousandths for the rate;
//   * the `_native*` callbacks the Rust side invokes turned back into
//     Qt-style signals;
//   * the document listeners: fullscreen, Escape, the absence of a mousedown
//     fullscreen toggle (the platform delivers dblclick now, adf6a92), and the
//     DOMContentLoaded stylesheet and theme-colour sync.
//
// The placeholders are substituted exactly as `render_injected_scripts` does
// (see `nativeShimPlaceholders` in test/player-fakes.js), so a change to the
// injection contract shows up here. Nothing touches a profile dir or a server.
const test = require('node:test');
const assert = require('node:assert');
const { loadNativeShim, makeWindow, makeJmpNative, makeEvents } = require('./test/player-fakes.js');

// A fake window with the shim installed on it. `opts` are the placeholder
// values (`settings`, `serverUrl`, `version`, `deviceProfile`,
// `decorationOptions`, `windowDecorations`); `opts.platform` fakes
// navigator.platform and `opts.noNative` leaves the bridge missing.
function setup(opts = {}) {
    const win = makeWindow();
    if (opts.platform) win.navigator.platform = opts.platform;
    if (!opts.noNative) win.jmpNative = makeJmpNative();
    const shim = loadNativeShim(win, opts);
    return { win, shim, native: win.jmpNative, doc: win.document, player: win.api.player };
}

// The instance native-shim.js reaches for when fullscreen changes.
function installPlayerInstance(win) {
    const events = makeEvents();
    win._mpvVideoPlayerInstance = { events };
    return events;
}

function fireDomContentLoaded(ctx) {
    ctx.doc.dispatchEvent({ type: 'DOMContentLoaded' });
}

function injectedCss(ctx) {
    const style = ctx.doc.head.children.find((el) => el.tagName === 'STYLE');
    return style ? style.textContent : null;
}

// ---- jmpInfo: the settings blob -------------------------------------------

test('the injected settings blob becomes jmpInfo', () => {
    const { win } = setup({
        version: '0.5.0',
        serverUrl: 'https://jf.example.com/',
        settings: { deviceName: 'Living Room', deviceNameDefault: 'DESKTOP-1' }
    });
    assert.equal(win.jmpInfo.version, '0.5.0');
    assert.equal(win.jmpInfo.deviceName, 'Living Room');
    assert.equal(win.jmpInfo.mode, 'desktop');
    assert.equal(win.jmpInfo.userAgent, win.navigator.userAgent);
    assert.equal(win.jmpInfo.settings.main.userWebClient, 'https://jf.example.com/');
    assert.deepEqual(win.jmpInfo.sections.map((s) => s.key),
        ['playback', 'audio', 'transcode', 'advanced']);
});

test('an unnamed device falls back to the machine name the native side sent', () => {
    const { win } = setup({ settings: { deviceNameDefault: 'DESKTOP-1' } });
    assert.equal(win.jmpInfo.deviceName, 'DESKTOP-1');
    assert.equal(win.jmpInfo.settings.advanced.deviceName, '', 'the field itself stays empty');
});

test('hardware decoding shows the per-OS default when nothing is saved', () => {
    // settings.json omits hwdec when it matches the Rust-side default, which
    // is videotoolbox on macOS and no elsewhere.
    assert.equal(setup({ settings: { hwdecDefault: 'videotoolbox' } })
        .win.jmpInfo.settings.playback.hwdec, 'videotoolbox');
    assert.equal(setup({ settings: { hwdec: 'auto-safe', hwdecDefault: 'no' } })
        .win.jmpInfo.settings.playback.hwdec, 'auto-safe');
    assert.equal(setup({ settings: {} }).win.jmpInfo.settings.playback.hwdec, 'no');
});

test('the saved playback, audio and transcode settings are carried over', () => {
    const { win } = setup({
        settings: {
            videoMode: 'animation',
            transcodeNotice: 'any',
            audioPassthrough: 'ac3,eac3',
            audioExclusive: true,
            audioChannels: '5.1',
            forceTranscoding: true,
            logLevel: 'debug'
        }
    });
    assert.equal(win.jmpInfo.settings.playback.videoMode, 'animation');
    assert.equal(win.jmpInfo.settings.playback.transcodeNotice, 'any');
    assert.equal(win.jmpInfo.settings.audio.audioPassthrough, 'ac3,eac3');
    assert.equal(win.jmpInfo.settings.audio.audioExclusive, true);
    assert.equal(win.jmpInfo.settings.audio.audioChannels, '5.1');
    assert.equal(win.jmpInfo.settings.transcode.forceTranscoding, true);
    assert.equal(win.jmpInfo.settings.advanced.logLevel, 'debug');
});

test('the settings with no saved value fall back to their defaults', () => {
    const { win } = setup({ settings: {} });
    assert.equal(win.jmpInfo.settings.playback.videoMode, 'auto');
    assert.equal(win.jmpInfo.settings.playback.transcodeNotice, 'cpu');
    assert.equal(win.jmpInfo.settings.transcode.forceTranscoding, false);
    assert.equal(win.jmpInfo.settings.audio.audioExclusive, false);
    // These two are on unless the user turned them off, so an absent value
    // must not read as false.
    assert.equal(win.jmpInfo.settings.advanced.hideScrollbar, true);
    assert.equal(win.jmpInfo.settings.advanced.transparentTitlebar, true);
});

test('turning the scrollbar and titlebar settings off is respected', () => {
    const { win } = setup({ settings: { hideScrollbar: false, transparentTitlebar: false } });
    assert.equal(win.jmpInfo.settings.advanced.hideScrollbar, false);
    assert.equal(win.jmpInfo.settings.advanced.transparentTitlebar, false);
});

test('the hand-edited library video modes are passed through untouched', () => {
    const map = { 'lib-1': 'animation' };
    assert.deepEqual(setup({ settings: { videoModeLibraries: map } }).win.jmpInfo.videoModeLibraries, map);
    assert.deepEqual(setup({ settings: {} }).win.jmpInfo.videoModeLibraries, {});
});

test('the hardware decoding options come from mpv, not from a hard-coded list', () => {
    const options = [{ value: 'no', title: 'None' }, { value: 'd3d11va', title: 'D3D11VA' }];
    const { win } = setup({ settings: { hwdecOptions: options } });
    const hwdec = win.jmpInfo.settingsDescriptions.playback.find((d) => d.key === 'hwdec');
    assert.deepEqual(hwdec.options, options);
});

test('the video mode setting offers auto, both presets and off', () => {
    const { win } = setup();
    const videoMode = win.jmpInfo.settingsDescriptions.playback.find((d) => d.key === 'videoMode');
    assert.deepEqual(videoMode.options.map((o) => o.value), ['auto', 'live-action', 'animation', 'off']);
});

test('the transparent titlebar setting is macOS only', () => {
    const mac = setup({ platform: 'MacIntel' }).win;
    assert.equal(mac.jmpInfo.settingsDescriptions.advanced[0].key, 'transparentTitlebar');
    const win32 = setup({ platform: 'Win32' }).win;
    assert.ok(!win32.jmpInfo.settingsDescriptions.advanced.some((d) => d.key === 'transparentTitlebar'));
});

test('window decorations are offered only where there is a choice to make', () => {
    const none = setup({ decorationOptions: [] }).win;
    assert.ok(!none.jmpInfo.settingsDescriptions.advanced.some((d) => d.key === 'windowDecorations'));
    const one = setup({ decorationOptions: ['csd'] }).win;
    assert.ok(!one.jmpInfo.settingsDescriptions.advanced.some((d) => d.key === 'windowDecorations'));

    const both = setup({ decorationOptions: ['csd', 'server', 'serverThemed'], windowDecorations: 'csd' }).win;
    const setting = both.jmpInfo.settingsDescriptions.advanced.find((d) => d.key === 'windowDecorations');
    assert.deepEqual(setting.options, [
        { value: null, title: 'Auto' },
        { value: 'csd', title: 'In-app (client-side)' },
        { value: 'server', title: 'System (server-side)' },
        { value: 'serverThemed', title: 'System, themed (KDE)' }
    ]);
    assert.equal(both.jmpInfo.settings.advanced.windowDecorations, 'csd');
    assert.equal(none.jmpInfo.settings.advanced.windowDecorations, null, 'auto when unset');
});

// ---- signals --------------------------------------------------------------

test('a signal calls every listener with the arguments it was fired with', () => {
    const { shim } = setup();
    const seen = [];
    const signal = shim.createSignal('test');
    signal.connect((...args) => seen.push(['a', ...args]));
    signal.connect((...args) => seen.push(['b', ...args]));
    signal(1, 'two');
    assert.deepEqual(seen, [['a', 1, 'two'], ['b', 1, 'two']]);
});

test('a disconnected listener stops hearing, and an unknown one is harmless', () => {
    const { shim } = setup();
    const seen = [];
    const signal = shim.createSignal('test');
    const listener = () => seen.push('heard');
    signal.connect(listener);
    signal.disconnect(listener);
    signal.disconnect(() => {}); // never connected
    signal();
    assert.deepEqual(seen, []);
});

test('a listener that throws does not stop the ones after it', () => {
    const { shim, win } = setup();
    const seen = [];
    const signal = shim.createSignal('test');
    signal.connect(() => { throw new Error('listener exploded'); });
    signal.connect(() => seen.push('still called'));
    signal();
    assert.deepEqual(seen, ['still called']);
    assert.ok(win.console.lines.some((l) => l.level === 'error'));
});

// ---- api.player -> jmpNative ----------------------------------------------

test('loading a file fills the nine slots the native side parses', () => {
    const { player, native } = setup();
    player.load(
        'http://server/stream.mkv',
        { startMilliseconds: 5000, isInfiniteStream: true },
        { type: 'video', metadata: { Id: 'i1' } },
        1, 2, 0,
        '/audio/1.mka', '/subs/1.ass'
    );
    assert.deepEqual(native.lastCall('playerLoad'), [
        'http://server/stream.mkv', 5000, 1, 2, 0,
        JSON.stringify({ Id: 'i1' }), '/audio/1.mka', '/subs/1.ass', true
    ]);
});

test('loading without metadata or external files sends empty strings, not undefined', () => {
    const { player, native } = setup();
    player.load('http://server/stream.mkv', { startMilliseconds: 0 }, {}, 1, 1, 0, null, undefined);
    const call = native.lastCall('playerLoad');
    assert.equal(call[5], '{}');
    assert.equal(call[6], '');
    assert.equal(call[7], '');
    assert.equal(call[8], false, 'a normal file is not an infinite stream');
});

test('the load callback waits for mpv to report the file is playing', () => {
    const { player } = setup();
    let calls = 0;
    player.load('http://server/a.mkv', { startMilliseconds: 0 }, {}, 1, 1, 0, '', '', () => { calls += 1; });
    assert.equal(calls, 0, 'nothing has opened yet');
    player.playing();
    assert.equal(calls, 1);
    player.playing();
    player.error();
    assert.equal(calls, 1, 'the handlers unsubscribed themselves');
});

test('a file that fails to open still calls back, so playback is not left hanging', () => {
    const { player } = setup();
    let calls = 0;
    player.load('http://server/a.mkv', { startMilliseconds: 0 }, {}, 1, 1, 0, '', '', () => { calls += 1; });
    player.error('no decoder');
    assert.equal(calls, 1);
    player.playing();
    assert.equal(calls, 1, 'both handlers were removed');
});

test('stop, pause and play reach mpv and are remembered locally', () => {
    const { player, native, shim } = setup();
    player.stop();
    assert.deepEqual(native.names().slice(-1), ['playerStop']);
    player.pause();
    assert.deepEqual(native.names().slice(-1), ['playerPause']);
    assert.equal(shim.playerState.paused, true);
    player.play();
    assert.deepEqual(native.names().slice(-1), ['playerPlay']);
    assert.equal(shim.playerState.paused, false);
});

test('seeking sends the position in milliseconds', () => {
    const { player, native } = setup();
    player.seekTo(90_500);
    assert.deepEqual(native.lastCall('playerSeek'), [90_500]);
});

test('the volume and mute state reach mpv and are remembered locally', () => {
    const { player, native, shim } = setup();
    player.setVolume(40);
    assert.deepEqual(native.lastCall('playerSetVolume'), [40]);
    assert.equal(shim.playerState.volume, 40);
    player.setMuted(true);
    assert.deepEqual(native.lastCall('playerSetMuted'), [true]);
    assert.equal(shim.playerState.muted, true);
});

test('the playback rate is passed straight through as the integer ipc value', () => {
    const { player, native } = setup();
    player.setPlaybackRate(1500);
    assert.deepEqual(native.lastCall('playerSetSpeed'), [1500]);
});

test('track selection and external tracks each have their own native call', () => {
    const { player, native } = setup();
    player.setSubtitleStream(2);
    assert.deepEqual(native.lastCall('playerSetSubtitle'), [2]);
    player.addSubtitleStream('/subs/1.ass');
    assert.deepEqual(native.lastCall('playerAddSubtitle'), ['/subs/1.ass']);
    player.setAudioStream(0);
    assert.deepEqual(native.lastCall('playerSetAudio'), [0]);
    player.addAudioStream('/audio/1.mka');
    assert.deepEqual(native.lastCall('playerAddAudio'), ['/audio/1.mka']);
    player.setAspectMode('cover');
    assert.deepEqual(native.lastCall('playerSetAspectMode'), ['cover']);
});

test('the subtitle and audio delays are converted from milliseconds to seconds', () => {
    // jellyfin-web works in milliseconds; mpv takes seconds.
    const { player, native } = setup();
    player.setSubtitleDelay(-1500);
    assert.deepEqual(native.lastCall('playerSetSubtitleDelay'), [-1.5]);
    player.setAudioDelay(250);
    assert.deepEqual(native.lastCall('playerSetAudioDelay'), [0.25]);
});

test('the video rectangle is ignored, because mpv always fills its window', () => {
    const { player, native } = setup();
    const before = native.calls.length;
    player.setVideoRectangle(0, 0, 1920, 1080);
    assert.equal(native.calls.length, before);
});

test('the position and duration are read back from what the native side pushed', () => {
    const { win, player } = setup();
    win._nativeUpdatePosition(42_000);
    win._nativeUpdateDuration(1_800_000);
    assert.equal(player.getPosition(), 42_000);
    assert.equal(player.getDuration(), 1_800_000);
    const seen = [];
    player.getPosition((ms) => seen.push(ms));
    player.getDuration((ms) => seen.push(ms));
    assert.deepEqual(seen, [42_000, 1_800_000]);
});

test('without the native bridge every player call is a quiet no-op', () => {
    // The shim is injected into every frame; one that never gets the bridge
    // must not throw its way through jellyfin-web's player setup.
    const { win, player } = setup({ noNative: true });
    player.load('http://server/a.mkv', { startMilliseconds: 0 }, {}, 1, 1, 0, '', '');
    player.stop();
    player.pause();
    player.play();
    player.seekTo(1000);
    player.setVolume(50);
    player.setMuted(true);
    player.setPlaybackRate(1000);
    player.setSubtitleStream(1);
    player.addSubtitleStream('/subs/1.ass');
    player.setAudioStream(1);
    player.addAudioStream('/audio/1.mka');
    player.setSubtitleDelay(0);
    player.setAudioDelay(0);
    player.setAspectMode('auto');
    win.api.system.exit();
    win.api.settings.setValue('playback', 'hwdec', 'no');
    assert.equal(win.jmpNative, undefined);
});

// ---- the native callbacks -------------------------------------------------

test('the native side fires a player signal by name', () => {
    const { win, player } = setup();
    const seen = [];
    player.buffering.connect((...args) => seen.push(args));
    win._nativeEmit('buffering', 42);
    assert.deepEqual(seen, [[42]]);
});

test('an unknown signal name is logged rather than thrown', () => {
    const { win } = setup();
    win._nativeEmit('noSuchSignal');
    assert.ok(win.console.lines.some((l) => l.level === 'error' && String(l.args[1]).includes('noSuchSignal')));
});

test('a position push updates the cached position and fires positionUpdate', () => {
    const { win, player, shim } = setup();
    const seen = [];
    player.positionUpdate.connect((ms) => seen.push(ms));
    win._nativeUpdatePosition(1234);
    assert.deepEqual(seen, [1234]);
    assert.equal(shim.playerState.position, 1234);
});

test('a duration push updates the cached duration and fires updateDuration', () => {
    const { win, player, shim } = setup();
    const seen = [];
    player.updateDuration.connect((ms) => seen.push(ms));
    win._nativeUpdateDuration(60_000);
    assert.deepEqual(seen, [60_000]);
    assert.equal(shim.playerState.duration, 60_000);
});

test('buffered ranges are stored for the scrubber, and cleared when empty', () => {
    const { win } = setup();
    assert.deepEqual(win._bufferedRanges, []);
    win._nativeUpdateBufferedRanges([{ start: 0, end: 30 }]);
    assert.deepEqual(win._bufferedRanges, [{ start: 0, end: 30 }]);
    win._nativeUpdateBufferedRanges(null);
    assert.deepEqual(win._bufferedRanges, [], 'never undefined: jellyfin-web iterates it');
});

test('the mpv stats snapshot is stored while the panel is open and nulled after', () => {
    const { win } = setup();
    assert.equal(win._mpvStats, null);
    win._nativeUpdateStats({ fps: 23.976 });
    assert.deepEqual(win._mpvStats, { fps: 23.976 });
    win._nativeUpdateStats(undefined);
    assert.equal(win._mpvStats, null);
});

test('a fullscreen change from the native side updates the flag and the player', () => {
    const { win } = setup();
    const events = installPlayerInstance(win);
    win._nativeFullscreenChanged(true);
    assert.equal(win._isFullscreen, true);
    assert.deepEqual(events.names(), ['fullscreenchange']);
    win._nativeFullscreenChanged(false);
    assert.equal(win._isFullscreen, false);
    assert.equal(events.names().length, 2);
});

test('a fullscreen change with no player yet is not an error', () => {
    const { win } = setup();
    win._nativeFullscreenChanged(true);
    assert.equal(win._isFullscreen, true);
});

test('media session commands arrive as input signals', () => {
    const { win } = setup();
    const seen = [];
    win.api.input.hostInput.connect((a) => seen.push(['hostInput', a]));
    win.api.input.rateChanged.connect((r) => seen.push(['rate', r]));
    win.api.input.positionSeek.connect((ms) => seen.push(['seek', ms]));
    win._nativeHostInput(['play_pause']);
    win._nativeSetRate(1.5);
    win._nativeSeek(30_000);
    assert.deepEqual(seen, [['hostInput', ['play_pause']], ['rate', 1.5], ['seek', 30_000]]);
    win.api.input.executeActions(); // jellyfin-web calls it; it does nothing
});

test('the settings signals exist for jellyfin-web to connect to', () => {
    const { win } = setup();
    const seen = [];
    win.api.settings.sectionValueUpdate.connect((...a) => seen.push(a));
    win.api.settings.groupUpdate.connect((...a) => seen.push(a));
    win.api.settings.sectionValueUpdate('playback', {});
    win.api.settings.groupUpdate('playback', {});
    assert.equal(seen.length, 2);
});

// ---- api.system and api.settings ------------------------------------------

test('an external url is opened in the system browser, downloads included', () => {
    const { win } = setup();
    win.api.system.openExternalUrl('https://jellyfin.org/');
    win.NativeShell.openUrl('https://example.com/', '_blank');
    win.NativeShell.downloadFile({ url: 'https://server/item.mkv' });
    assert.deepEqual(win.opened, [
        ['https://jellyfin.org/', '_blank'],
        ['https://example.com/', '_blank'],
        ['https://server/item.mkv', '_blank']
    ]);
});

test('exiting from the web UI asks the native side to quit', () => {
    const { win, native } = setup();
    win.api.system.exit();
    assert.deepEqual(native.names().slice(-1), ['appExit']);
    win.NativeShell.AppHost.exit();
    assert.equal(native.callsTo('appExit').length, 2);
});

test('cancelling the connectivity probe aborts it when one is running', () => {
    const { win } = setup();
    win.api.system.cancelServerConnectivity(); // nothing running: harmless
    let aborted = 0;
    win.jmpCheckServerConnectivity = { abort: () => { aborted += 1; } };
    win.api.system.cancelServerConnectivity();
    assert.equal(aborted, 1);
});

test('opening the client settings goes through the injected opener', () => {
    const { win } = setup();
    let opened = 0;
    win._openClientSettings = () => { opened += 1; };
    win.NativeShell.openClientSettings();
    assert.equal(opened, 1);
});

test('a setting value is serialized the way the native parser expects', () => {
    const { win, native } = setup();
    const set = (value) => {
        win.api.settings.setValue('advanced', 'key', value);
        return native.lastCall('setSettingValue')[2];
    };
    assert.equal(set(true), 'true');
    assert.equal(set(false), 'false');
    assert.equal(set(null), null, 'null clears the setting');
    assert.equal(set(42), '42');
    assert.equal(set('debug'), 'debug');
    assert.equal(set(['a', 'b']), '["a","b"]');
    assert.deepEqual(native.lastCall('setSettingValue').slice(0, 2), ['advanced', 'key']);
});

test('setting a value calls back even when the native side is missing', () => {
    const { win } = setup({ noNative: true });
    let called = 0;
    win.api.settings.setValue('advanced', 'key', 'value', () => { called += 1; });
    assert.equal(called, 1);
});

// ---- NativeShell ----------------------------------------------------------

test('the three plugins are advertised and each name resolves to its factory', () => {
    const { win } = setup();
    assert.deepEqual(win.NativeShell.getPlugins(), ['mpvVideoPlayer', 'mpvAudioPlayer', 'inputPlugin']);
    win._mpvVideoPlayer = class {};
    win._mpvAudioPlayer = class {};
    win._inputPlugin = class {};
    assert.equal(win.mpvVideoPlayer(), win._mpvVideoPlayer);
    assert.equal(win.mpvAudioPlayer(), win._mpvAudioPlayer);
    assert.equal(win.inputPlugin(), win._inputPlugin);
});

test('the app host introduces the app and the device to jellyfin-web', async () => {
    const { win } = setup({ version: '0.5.0', settings: { deviceName: 'Living Room' } });
    assert.deepEqual(await win.NativeShell.AppHost.init(), {
        deviceName: 'Living Room',
        appName: 'Astrofin',
        appVersion: '0.5.0'
    });
    assert.equal(win.NativeShell.AppHost.appName(), 'Astrofin');
    assert.equal(win.NativeShell.AppHost.appVersion(), '0.5.0');
    assert.equal(win.NativeShell.AppHost.deviceName(), 'Living Room');
    assert.equal(win.NativeShell.AppHost.getDefaultLayout(), 'desktop');
});

test('the supported features are matched case-insensitively', () => {
    const { win } = setup();
    const host = win.NativeShell.AppHost;
    assert.equal(host.supports('FileInput'), true);
    assert.equal(host.supports('fullscreenchange'), true);
    assert.equal(host.supports('ClientSettings'), true);
    assert.equal(host.supports('ExitMenu'), true);
    assert.equal(host.supports('Screensaver'), false);
    assert.equal(host.supports('Sync'), false);
});

test('the device profile is the one mpv built at startup, sync profile included', () => {
    const profile = { Name: 'Astrofin', DirectPlayProfiles: [{ Container: 'mkv' }] };
    const { win, shim } = setup({ deviceProfile: profile });
    assert.deepEqual(win.NativeShell.AppHost.getDeviceProfile(), profile);
    assert.equal(win.NativeShell.AppHost.getSyncProfile, win.NativeShell.AppHost.getDeviceProfile);
    assert.deepEqual(shim.getDeviceProfile(), profile);
});

test('the api promises resolve for the jellyfin-web startup gate', async () => {
    const { win } = setup();
    await win.initCompleted;
    assert.equal(await win.apiPromise, win.api);
});

// ---- document listeners ---------------------------------------------------

test('entering and leaving fullscreen tells the video player', () => {
    const ctx = setup();
    const events = installPlayerInstance(ctx.win);
    ctx.doc.fullscreenElement = ctx.doc.body;
    ctx.doc.dispatchEvent({ type: 'fullscreenchange' });
    assert.equal(ctx.win._isFullscreen, true);
    assert.deepEqual(events.names(), ['fullscreenchange']);

    ctx.doc.fullscreenElement = null;
    ctx.doc.dispatchEvent({ type: 'fullscreenchange' });
    assert.equal(ctx.win._isFullscreen, false);
    assert.equal(events.names().length, 2);
});

test('a fullscreenchange that changes nothing is not passed on', () => {
    const ctx = setup();
    const events = installPlayerInstance(ctx.win);
    ctx.doc.dispatchEvent({ type: 'fullscreenchange' }); // already false
    assert.deepEqual(events.names(), []);
});

test('escape leaves fullscreen, and does nothing when there is none to leave', () => {
    const ctx = setup();
    ctx.doc.dispatchEvent({ type: 'keydown', key: 'Escape' });
    assert.deepEqual(ctx.native.callsTo('toggleFullscreen'), []);
    ctx.win._isFullscreen = true;
    ctx.doc.dispatchEvent({ type: 'keydown', key: 'a' });
    assert.deepEqual(ctx.native.callsTo('toggleFullscreen'), []);
    ctx.doc.dispatchEvent({ type: 'keydown', key: 'Escape' });
    assert.equal(ctx.native.callsTo('toggleFullscreen').length, 1);
});

// Double-clicking the video toggles fullscreen through jellyfin-web's own
// dblclick handler now that every platform delivers a real DOM dblclick (the
// native side recovers the click count, adf6a92). The shim used to ALSO run a
// mousedown-pair detector that called toggleFullscreen itself, so a single
// double-click toggled twice and cancelled out (the live bug: fullscreen=true
// then =false). The guard below is that the shim contributes no such path.
function mousedown(ctx, at, extra = {}) {
    ctx.win.Date = { now: () => at };
    const target = extra.target || ctx.pageEl;
    ctx.doc.dispatchEvent(Object.assign({
        type: 'mousedown', button: 0, clientX: 100, clientY: 100, target
    }, extra));
}

function withVideoPage(opts) {
    const ctx = setup(opts);
    ctx.pageEl = ctx.doc.createElement('div');
    ctx.pageEl.classList.add('mainAnimatedPage');
    ctx.doc.body.appendChild(ctx.pageEl);
    const dlg = ctx.doc.createElement('div');
    dlg.classList.add('videoPlayerContainer');
    ctx.doc.body.appendChild(dlg);
    return ctx;
}

test('the shim installs no mousedown fullscreen toggle — the platform delivers dblclick', () => {
    const ctx = setup();
    assert.ok(!ctx.doc.listeners.some((l) => l.type === 'mousedown'),
        'a mousedown listener here would double-toggle against the native dblclick');
});

test('a fast mousedown pair over a playing video toggles nothing from the shim', () => {
    // The old detector fired here; jellyfin-web's dblclick owns the toggle now.
    const ctx = withVideoPage();
    mousedown(ctx, 1000);
    mousedown(ctx, 1200);
    assert.deepEqual(ctx.native.callsTo('toggleFullscreen'), [],
        'our code adds no second toggle on top of the platform dblclick');
});

// ---- DOMContentLoaded -----------------------------------------------------

test('the injected stylesheet hides the cursor when the mouse goes idle', () => {
    const ctx = setup();
    fireDomContentLoaded(ctx);
    const css = injectedCss(ctx);
    assert.match(css, /body\.mouseIdle[^]*cursor: none/);
    assert.match(css, /@keyframes mpv-video-zoomin/, 'the dialog animation lives here too');
});

test('the scrollbar rules are only injected when the setting is on', () => {
    const on = setup({ settings: {} });
    fireDomContentLoaded(on);
    assert.match(injectedCss(on), /scrollbar-width: none/);

    const off = setup({ settings: { hideScrollbar: false } });
    fireDomContentLoaded(off);
    assert.doesNotMatch(injectedCss(off), /scrollbar-width/);
});

test('macOS offsets the UI under the traffic lights and follows the OSD', () => {
    const ctx = setup({ platform: 'MacIntel' });
    fireDomContentLoaded(ctx);
    const css = injectedCss(ctx);
    assert.match(css, /--mac-titlebar-height: 22px/);
    assert.match(css, /\.skinHeader \{ padding-top/);
    assert.match(css, /\.MuiAppBar-positionFixed/, 'the dashboard has its own header');

    // jellyfin-web's own Events system, not a DOM event.
    ctx.doc._callbacks.SHOW_VIDEO_OSD.forEach((fn) => fn({}, true));
    assert.deepEqual(ctx.native.lastCall('setOsdVisible'), [true]);
    ctx.doc._callbacks.SHOW_VIDEO_OSD.forEach((fn) => fn({}, 0));
    assert.deepEqual(ctx.native.lastCall('setOsdVisible'), [false], 'always a boolean');
});

test('a windows build gets none of the macOS titlebar rules', () => {
    const ctx = setup({ platform: 'Win32' });
    fireDomContentLoaded(ctx);
    assert.doesNotMatch(injectedCss(ctx), /mac-titlebar-height/);
    assert.equal(ctx.doc._callbacks, undefined);
});

test('the theme colour is sent to the titlebar and re-sent when it changes', () => {
    const ctx = setup();
    const meta = ctx.doc.createElement('meta');
    meta.setAttribute('name', 'theme-color');
    meta.content = '#101418';
    ctx.doc.head.appendChild(meta);

    fireDomContentLoaded(ctx);
    assert.deepEqual(ctx.native.lastCall('themeColor'), ['#101418']);

    meta.content = '#ffffff';
    ctx.win.observers.at(-1)._fire([{ type: 'attributes' }]);
    assert.deepEqual(ctx.native.lastCall('themeColor'), ['#ffffff']);
    assert.equal(ctx.native.callsTo('themeColor').length, 2);
});

test('a theme colour tag added after load is picked up once', () => {
    const ctx = setup();
    fireDomContentLoaded(ctx);
    assert.deepEqual(ctx.native.callsTo('themeColor'), [], 'nothing to send yet');

    const other = ctx.doc.createElement('meta');
    other.setAttribute('name', 'viewport');
    ctx.doc.head.appendChild(other);
    assert.deepEqual(ctx.native.callsTo('themeColor'), [], 'only theme-color counts');

    const meta = ctx.doc.createElement('meta');
    meta.setAttribute('name', 'theme-color');
    meta.content = '#101418';
    ctx.doc.head.appendChild(meta);
    assert.deepEqual(ctx.native.lastCall('themeColor'), ['#101418']);

    // The watcher on <head> is gone; the one on the tag itself took over.
    meta.content = '#000000';
    ctx.win.observers.at(-1)._fire([{ type: 'attributes' }]);
    assert.deepEqual(ctx.native.lastCall('themeColor'), ['#000000']);
});

test('an empty theme colour is not forwarded', () => {
    const ctx = setup();
    const meta = ctx.doc.createElement('meta');
    meta.setAttribute('name', 'theme-color');
    ctx.doc.head.appendChild(meta);
    fireDomContentLoaded(ctx);
    assert.deepEqual(ctx.native.callsTo('themeColor'), []);
});
