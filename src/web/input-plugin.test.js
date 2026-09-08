// Unit tests for the input plugin's playbackManager bridge. Run with:
//
//     node --test src/web/input-plugin.test.js
//
// (or `just test-js`). The fake playbackManager below mirrors the one
// behaviour of jellyfin-web 10.11 that matters here: `currentTime`,
// `getPlayerState` and `duration` default their player argument to
// `_currentPlayer`, and the tick helper underneath throws
// "player cannot be null" when that is null. `_currentPlayer` is null
// from onPlaybackStopped until the next item's onPlaybackStarted, and
// mpv's load-paused `pause` and first `playing` land inside that window.
const test = require('node:test');
const assert = require('node:assert');

// Minimal copy of jellyfin-web's utils/events: callbacks keyed per
// object, `off` without the function removes nothing.
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

function makeNative() {
    const calls = [];
    const record = (name) => (...args) => calls.push([name, ...args]);
    return {
        calls,
        notifyPlaybackState: record('state'),
        notifyPosition: record('position'),
        notifyRateChange: record('rate'),
        notifyQueueChange: record('queue'),
        notifyMetadata: record('metadata'),
        notifyArtwork: record('artwork'),
        notifySeek: record('seek')
    };
}

function makePlayer(timeMs) {
    return {
        name: 'MPV Video Player',
        isLocalPlayer: true,
        _t: timeMs,
        currentTime() { return this._t; },
        getPlaybackRate() { return 1.25; }
    };
}

function makePlaybackManager() {
    const pm = {
        _currentPlayer: null,
        _playQueueManager: {
            getPlaylist() { return [{ Id: 'a' }, { Id: 'b' }]; },
            getCurrentPlaylistIndex() { return 0; }
        },
        getCurrentPlayer() { return this._currentPlayer; },
        getCurrentTicks(player) {
            if (!player) throw new Error('player cannot be null');
            return Math.floor(10000 * player.currentTime());
        },
        currentTime(player = this._currentPlayer) {
            return this.getCurrentTicks(player) / 10000;
        },
        getPlayerState(player = this._currentPlayer) {
            if (!player) throw new Error('player cannot be null');
            return { NowPlayingItem: { Id: 'a', Name: 'Item A', MediaType: 'Video' } };
        },
        duration(player = this._currentPlayer) {
            if (!player) throw new Error('player cannot be null');
            return 600 * 10000 * 1000;
        },
        seekPercent() {}
    };
    return pm;
}

function load() {
    delete require.cache[require.resolve('./input-plugin.js')];
    const native = makeNative();
    const connectNoop = { connect() {} };
    global.window = {
        Events,
        jmpNative: native,
        api: { input: { hostInput: connectNoop, positionSeek: connectNoop, rateChanged: connectNoop } }
    };
    global.console.debug = () => {};
    require('./input-plugin.js');
    return { Plugin: global.window._inputPlugin, native };
}

test('pause and playing during the stop-to-start window do not throw', () => {
    const { Plugin, native } = load();
    const pm = makePlaybackManager();
    const player = makePlayer(1234);
    const plugin = new Plugin({ playbackManager: pm, inputManager: null });

    // Item A starts: playbackManager has a current player.
    pm._currentPlayer = player;
    Events.trigger(pm, 'playbackstart', [player]);
    assert.strictEqual(plugin.attachedPlayer, player);

    // OSD back: onPlaybackStopped clears the current player...
    Events.trigger(pm, 'playbackstop', [{}]);
    pm._currentPlayer = null;
    native.calls.length = 0;

    // ...and the next item's load-paused signal plus first frame arrive
    // before onPlaybackStarted sets it again.
    player._t = 0;
    assert.doesNotThrow(() => Events.trigger(player, 'pause'));
    assert.doesNotThrow(() => Events.trigger(player, 'playing'));
    assert.doesNotThrow(() => Events.trigger(player, 'ratechange'));
    assert.doesNotThrow(() => Events.trigger(player, 'timeupdate'));

    const states = native.calls.filter(c => c[0] === 'state').map(c => c[1]);
    assert.deepStrictEqual(states, ['Paused', 'Playing']);
    // The position and rate are still reported, resolved through the
    // attached player rather than the manager's null default.
    assert.ok(native.calls.some(c => c[0] === 'position' && c[1] === 0));
    assert.ok(native.calls.some(c => c[0] === 'rate' && c[1] === 1.25));
});

test('queue state and position tracking survive having no player at all', () => {
    const { Plugin, native } = load();
    const pm = makePlaybackManager();
    const plugin = new Plugin({ playbackManager: pm, inputManager: null });

    assert.doesNotThrow(() => plugin.updateQueueState());
    assert.doesNotThrow(() => plugin.startPositionUpdates());
    assert.doesNotThrow(() => plugin.checkPositionDrift());
    assert.strictEqual(plugin._currentTimeMs(), null);
    // No position is invented when nobody can be asked.
    assert.ok(!native.calls.some(c => c[0] === 'position'));
    // The queue is still reported: canNext from the playlist, canPrev
    // from a null state.
    assert.deepStrictEqual(native.calls.filter(c => c[0] === 'queue'), [['queue', true, false]]);
});

test('destroy really unbinds the player and manager handlers', () => {
    const { Plugin, native } = load();
    const pm = makePlaybackManager();
    const player = makePlayer(50);
    const plugin = new Plugin({ playbackManager: pm, inputManager: null });
    pm._currentPlayer = player;
    Events.trigger(pm, 'playbackstart', [player]);

    plugin.destroy();
    native.calls.length = 0;
    Events.trigger(player, 'pause');
    Events.trigger(player, 'playing');
    Events.trigger(pm, 'playbackstart', [player]);
    Events.trigger(pm, 'playbackstop', [{}]);

    assert.deepStrictEqual(native.calls, []);
    assert.strictEqual(plugin.attachedPlayer, null);
    for (const name of ['playing', 'pause', 'ratechange', 'timeupdate']) {
        assert.deepStrictEqual(player._callbacks[name], [], name);
    }
});
