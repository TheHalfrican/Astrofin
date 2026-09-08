// Unit tests for the playback source badge. Run with:
//
//     node --test src/web/playback-source.test.js
//
// (or `just test-js`). playback-source.js talks to a document, a clock, a
// playbackManager and ApiClient.getSessions, so all four are faked here — the
// same shape as input-plugin.test.js's fake playbackManager, plus a tiny DOM
// that understands descendant class selectors, which is the only kind the
// module uses.
const test = require('node:test');
const assert = require('node:assert');

// ---- fake DOM ------------------------------------------------------------

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

    get firstChild() {
        return this.children[0] || null;
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

    appendChild(child) {
        if (child.parentNode) child.parentNode.removeChild(child);
        child.parentNode = this;
        this.children.push(child);
        return child;
    }
    removeChild(child) {
        const i = this.children.indexOf(child);
        if (i !== -1) this.children.splice(i, 1);
        child.parentNode = null;
        return child;
    }

    // Depth-first, document order.
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

    // Walks up from a node checking the ancestor compounds, right to left.
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

// ---- fake clock ----------------------------------------------------------

function makeTimers() {
    let next = 1;
    const pending = new Map();
    return {
        set(fn, delay) {
            const id = next++;
            pending.set(id, { fn, delay });
            return id;
        },
        clear(id) {
            pending.delete(id);
        },
        pending() {
            return [...pending.entries()].map(([id, t]) => ({ id, delay: t.delay }));
        },
        // Fire every timer currently queued (callbacks may queue more).
        flush() {
            const now = [...pending.entries()];
            pending.clear();
            for (const [, t] of now) t.fn();
        }
    };
}

// ---- fake jellyfin-web events -------------------------------------------

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

// ---- harness -------------------------------------------------------------

const PLAYER = { name: 'MPV Video Player' };

const SOURCE_STREAM = {
    Type: 'Video',
    Codec: 'hevc',
    Width: 1440,
    Height: 1080,
    BitDepth: 10,
    RealFrameRate: 23.976
};

function makeState(playMethod, extra) {
    return Object.assign(
        {
            PlayState: { PlayMethod: playMethod, PositionTicks: 600 * 10000000 },
            NowPlayingItem: { Id: 'a', Name: 'Item A', RunTimeTicks: 3600 * 10000000 },
            MediaSource: { MediaStreams: [{ Type: 'Audio', Codec: 'eac3' }, SOURCE_STREAM] }
        },
        extra || {}
    );
}

function makePm(state) {
    return {
        _state: state,
        getPlayerState() {
            return this._state;
        },
        currentMediaSource() {
            return this._state ? this._state.MediaSource : null;
        },
        // ms; 600 s in, so a CompletionPercentage of 17 % of a 1 h item is
        // 12 s of lead.
        currentTime() {
            return 600 * 1000;
        }
    };
}

function load(sessions) {
    delete require.cache[require.resolve('./playback-source.js')];
    const doc = new Document();
    const timers = makeTimers();
    const calls = { getSessions: 0 };
    const win = {
        document: doc,
        Events,
        setTimeout: timers.set,
        clearTimeout: timers.clear,
        ApiClient: {
            deviceId: () => 'dev-1',
            getSessions: () => {
                calls.getSessions += 1;
                const next = typeof sessions === 'function' ? sessions() : sessions;
                if (next instanceof Error) throw next;
                return Promise.resolve(next || []);
            }
        },
        jmpInfo: { settings: { playback: {} } }
    };
    global.window = win;
    global.console.debug = () => {};
    const api = require('./playback-source.js');
    return { api, doc, timers, win, calls };
}

function mountOsdHeader(doc) {
    const skin = doc.createElement('div');
    skin.className = 'skinHeader osdHeader';
    const top = doc.createElement('div');
    top.className = 'headerTop';
    const left = doc.createElement('div');
    left.className = 'headerLeft';
    const title = doc.createElement('h3');
    title.className = 'pageTitle';
    left.appendChild(title);
    top.appendChild(left);
    skin.appendChild(top);
    doc.body.appendChild(skin);
    return { skin, left, title };
}

function badgeOf(doc) {
    return doc.querySelector('.af-source-badge');
}

function labelOf(doc) {
    const b = badgeOf(doc);
    return b ? b.querySelector('.af-source-badge-label').textContent : null;
}

function levelClassOf(doc) {
    const b = badgeOf(doc);
    if (!b) return null;
    return b.className
        .split(/\s+/)
        .filter((c) => c.startsWith('af-source-badge--'))
        .join(' ');
}

function popoverLinesOf(doc) {
    const b = badgeOf(doc);
    if (!b) return [];
    return b.querySelector('.af-source-popover').children.map((c) => c.textContent);
}

function session(transcodingInfo, deviceId) {
    return { DeviceId: deviceId || 'dev-1', TranscodingInfo: transcodingInfo };
}

const HW_INFO = {
    HardwareAccelerationType: 'nvenc',
    VideoCodec: 'h264',
    AudioCodec: 'aac',
    Bitrate: 8000000,
    Framerate: 23.976,
    CompletionPercentage: 17,
    Width: 1920,
    Height: 1080,
    TranscodeReasons: ['VideoCodecNotSupported']
};

const CPU_INFO = Object.assign({}, HW_INFO, { HardwareAccelerationType: null });

// ---- classification ------------------------------------------------------

test('classify covers every level', () => {
    const { api } = load();
    assert.strictEqual(api.classify({ playMethod: 'DirectPlay' }), 'direct');
    assert.strictEqual(api.classify({ playMethod: 'DirectStream' }), 'stream');
    // Transcode before the first /Sessions answer: warm, but unqualified.
    assert.strictEqual(api.classify({ playMethod: 'Transcode' }), 'transcode');
    assert.strictEqual(api.classify({ playMethod: 'Transcode', transcodingInfo: HW_INFO }), 'transcode');
    assert.strictEqual(api.classify({ playMethod: 'Transcode', transcodingInfo: CPU_INFO }), 'cpu');
    assert.strictEqual(
        api.classify({ playMethod: 'Transcode', transcodingInfo: { HardwareAccelerationType: 'none' } }),
        'cpu'
    );
    // struggling outranks cpu, and outranks a healthy-looking hardware encode.
    assert.strictEqual(
        api.classify({ playMethod: 'Transcode', transcodingInfo: CPU_INFO, struggling: true }),
        'struggling'
    );
    assert.strictEqual(
        api.classify({ playMethod: 'Transcode', transcodingInfo: HW_INFO, struggling: true }),
        'struggling'
    );
    assert.strictEqual(api.classify({ playMethod: null }), null);
    assert.strictEqual(api.classify({}), null);
});

test('any non-empty HardwareAccelerationType other than none counts as hardware', () => {
    const { api } = load();
    for (const raw of ['nvenc', 'NVENC', ' qsv ', 'vaapi', 'videotoolbox', 'amf', 'rkmpp']) {
        assert.ok(api.hardwareName(raw), raw);
    }
    for (const raw of [null, undefined, '', '   ', 'none', 'None', 'NONE', 7]) {
        assert.strictEqual(api.hardwareName(raw), null, String(raw));
    }
});

// ---- struggling ----------------------------------------------------------

test('a slow transcode needs two consecutive polls under 0.95x', () => {
    const { api } = load();
    const t = api.newTracker();
    assert.strictEqual(api.trackThroughput(t, { transcodeFps: 20, sourceFps: 23.976 }), false);
    assert.strictEqual(api.trackThroughput(t, { transcodeFps: 20, sourceFps: 23.976 }), true);
});

test('one healthy poll between two slow ones resets the slow counter', () => {
    const { api } = load();
    const t = api.newTracker();
    api.trackThroughput(t, { transcodeFps: 20, sourceFps: 23.976 });
    assert.strictEqual(api.trackThroughput(t, { transcodeFps: 24, sourceFps: 23.976 }), false);
    assert.strictEqual(api.trackThroughput(t, { transcodeFps: 20, sourceFps: 23.976 }), false);
    assert.strictEqual(api.trackThroughput(t, { transcodeFps: 20, sourceFps: 23.976 }), true);
});

test('a lead that shrinks under 20 s on two consecutive polls is struggling', () => {
    const { api } = load();
    const t = api.newTracker();
    // First sample only establishes the baseline; nothing has shrunk yet.
    assert.strictEqual(api.trackThroughput(t, { lead: 18 }), false);
    assert.strictEqual(api.trackThroughput(t, { lead: 15 }), false);
    assert.strictEqual(api.trackThroughput(t, { lead: 11 }), true);
});

test('a comfortable lead never counts, even while shrinking', () => {
    const { api } = load();
    const t = api.newTracker();
    for (const lead of [400, 300, 200, 100, 40]) {
        assert.strictEqual(api.trackThroughput(t, { lead }), false, String(lead));
    }
});

test('missing frame rates and leads leave the tracker untouched', () => {
    const { api } = load();
    const t = api.newTracker();
    for (let i = 0; i < 5; i++) {
        assert.strictEqual(api.trackThroughput(t, { transcodeFps: null, sourceFps: 0, lead: null }), false);
    }
});

// ---- labels --------------------------------------------------------------

test('badge labels read as the design specifies them', () => {
    const { api } = load();
    assert.strictEqual(api.badgeLabel('direct'), 'DIRECT PLAY');
    assert.strictEqual(api.badgeLabel('stream'), 'DIRECT STREAM');
    assert.strictEqual(api.badgeLabel('transcode'), 'TRANSCODING');
    assert.strictEqual(api.badgeLabel('transcode', 'nvenc'), 'TRANSCODING · NVENC');
    assert.strictEqual(api.badgeLabel('cpu'), 'TRANSCODING · CPU');
    assert.strictEqual(api.badgeLabel('struggling'), "SERVER CAN'T KEEP UP");
});

test('transcode reasons translate, and unknown ones split on capitals', () => {
    const { api } = load();
    assert.strictEqual(api.reasonText('VideoCodecNotSupported'), 'The video codec is not supported');
    assert.strictEqual(api.reasonText('ContainerBitrateExceedsLimit'), "The video's bitrate exceeds the limit");
    assert.strictEqual(api.reasonText('SomeFutureServerReason'), 'Some future server reason');
    assert.strictEqual(api.reasonText('HDRToneMappingNotSupported'), 'Hdr tone mapping not supported');
    assert.strictEqual(api.reasonText(''), '');
});

// ---- badge in the DOM ----------------------------------------------------

test('a direct-play start renders the calm badge next to the title', () => {
    const { api, doc } = load();
    const header = mountOsdHeader(doc);
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]);

    const badge = badgeOf(doc);
    assert.ok(badge);
    // A sibling of h3.pageTitle, never a child: setTitle rewrites innerText.
    assert.strictEqual(badge.parentNode, header.left);
    assert.strictEqual(header.title.children.length, 0);
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--direct');
    assert.strictEqual(badge.getAttribute('tabindex'), '0');
    assert.strictEqual(badge.hidden, false);
});

test('badge creation is idempotent and the content is rewritten per item', () => {
    const { api, doc } = load();
    const header = mountOsdHeader(doc);
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);

    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]);
    const first = badgeOf(doc);
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectStream')]);

    assert.strictEqual(doc.querySelectorAll('.af-source-badge').length, 1);
    assert.strictEqual(badgeOf(doc), first, 'the same node is reused');
    assert.strictEqual(header.left.children.filter((c) => c.classList.contains('af-source-badge')).length, 1);
    assert.strictEqual(labelOf(doc), 'DIRECT STREAM');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--stream', 'the old level class is dropped');
});

test('a transcode shows plain TRANSCODING, then the accelerator after the poll', async () => {
    const { api, doc, timers, calls } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);

    assert.strictEqual(labelOf(doc), 'TRANSCODING');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--transcode');
    assert.deepStrictEqual(timers.pending().map((t) => t.delay), [4000], 'first poll is 4 s out');

    await api.pollOnce();
    assert.strictEqual(calls.getSessions, 1);
    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--transcode');
});

test('a software transcode reads as CPU and turns the badge danger', async () => {
    const { api, doc } = load([session(CPU_INFO)]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();

    assert.strictEqual(labelOf(doc), 'TRANSCODING · CPU');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--cpu');
});

test('the session for this device wins over the first one in the list', async () => {
    const { api, doc } = load([
        session(CPU_INFO, 'someone-else'),
        session(HW_INFO, 'dev-1')
    ]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();

    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC');
});

// Measured against Jellyfin 10.11: /Sessions returns several records for one
// deviceId — the live one plus stale ones from earlier app runs, whose
// TranscodingInfo is null and whose PlayMethod is frozen at whatever it was.
test('a stale session for the same device does not shadow the live one', async () => {
    const { api, doc } = load([
        { DeviceId: 'dev-1', TranscodingInfo: null },
        { DeviceId: 'dev-1', TranscodingInfo: null },
        session(HW_INFO, 'dev-1')
    ]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();

    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC');
});

test('a poll that finds no TranscodingInfo keeps the last one measured', async () => {
    let payload = [session(HW_INFO, 'dev-1')];
    const { api, doc } = load(() => payload);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();
    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC');

    // The transcoder finished; the server drops the block entirely.
    payload = [{ DeviceId: 'dev-1', TranscodingInfo: null }];
    await api.pollOnce();
    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC', 'the badge does not flap back');

    // ...and the next item starts clean.
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    assert.strictEqual(labelOf(doc), 'TRANSCODING');
});

test('a slow transcode escalates to SERVER CAN\'T KEEP UP after two polls', async () => {
    const slow = Object.assign({}, HW_INFO, { Framerate: 12 });
    const { api, doc } = load([session(slow)]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);

    await api.pollOnce();
    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC', 'one slow sample is not enough');
    await api.pollOnce();
    assert.strictEqual(labelOf(doc), "SERVER CAN'T KEEP UP");
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--struggling');
});

test('the badge is removed when playback stops', () => {
    const { api, doc } = load();
    api.attach(makePm(makeState('DirectPlay')));
    mountOsdHeader(doc);
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]);
    assert.ok(badgeOf(doc));

    Events.trigger(pm, 'playbackstop', [{}]);
    assert.strictEqual(badgeOf(doc), null);
});

test('an unknown play method hides the badge rather than labelling it', () => {
    const { api, doc } = load();
    mountOsdHeader(doc);
    const pm = makePm(makeState(null));
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState(null)]);
    const badge = badgeOf(doc);
    assert.ok(badge);
    assert.strictEqual(badge.hidden, true);
});

// ---- popover -------------------------------------------------------------

test('the popover carries the codec path, the reasons, the speed and the lead', async () => {
    const { api, doc } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();

    // 17 % of a 3600 s item is 612 s; the playhead is at 600 s.
    assert.deepStrictEqual(popoverLinesOf(doc), [
        'HEVC 10-bit 1440x1080 → H264 1920x1080 8.0 Mbps',
        'The video codec is not supported',
        '1.0× realtime',
        '12 s ahead'
    ]);
});

test('the popover omits what is not known instead of printing placeholders', () => {
    const { api } = load();
    assert.deepStrictEqual(api.popoverLines({ source: { codec: 'h264', width: 1920, height: 1080 } }), [
        'H264 1920x1080'
    ]);
    assert.deepStrictEqual(api.popoverLines({}), []);
    assert.deepStrictEqual(
        api.popoverLines({ source: null, transcodingInfo: { VideoCodec: 'h264' }, lead: -8 }),
        ['H264', '8 s behind']
    );
});

test('a direct play popover is just the source description', () => {
    const { api, doc } = load();
    mountOsdHeader(doc);
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]);
    assert.deepStrictEqual(popoverLinesOf(doc), ['HEVC 10-bit 1440x1080']);
});

// ---- toast ---------------------------------------------------------------

function toastTexts(doc) {
    const c = doc.querySelector('.toastContainer');
    return c ? c.children.map((t) => t.textContent) : [];
}

async function runToastCase(setting, info) {
    const { api, doc, win, timers } = load([session(info)]);
    mountOsdHeader(doc);
    win.jmpInfo.settings.playback.transcodeNotice = setting;
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    assert.deepStrictEqual(toastTexts(doc), [], 'nothing is said before the first poll');
    await api.pollOnce();
    return { api, doc, timers, pm, state };
}

test('the default cpu setting warns about a software transcode only', async () => {
    const cpu = await runToastCase('cpu', CPU_INFO);
    assert.deepStrictEqual(toastTexts(cpu.doc), [
        'The server is transcoding this on its CPU: video codec not supported'
    ]);

    const hw = await runToastCase('cpu', HW_INFO);
    assert.deepStrictEqual(toastTexts(hw.doc), [], 'a hardware transcode is not worth interrupting for');
});

test('the cpu setting also warns when the server cannot keep up', async () => {
    const slow = Object.assign({}, HW_INFO, { Framerate: 12 });
    const { api, doc } = await runToastCase('cpu', slow);
    assert.deepStrictEqual(toastTexts(doc), []);
    await api.pollOnce();
    assert.deepStrictEqual(toastTexts(doc), ["The server can't keep up with this transcode"]);
});

test('the any setting warns about a hardware transcode too', async () => {
    const hw = await runToastCase('any', HW_INFO);
    assert.deepStrictEqual(toastTexts(hw.doc), [
        'The server is transcoding this with NVENC: video codec not supported'
    ]);
});

test('the off setting never warns', async () => {
    for (const info of [CPU_INFO, HW_INFO]) {
        const { doc } = await runToastCase('off', info);
        assert.deepStrictEqual(toastTexts(doc), []);
    }
});

test('an unset transcodeNotice behaves as cpu', async () => {
    const { api, doc, win, timers } = load([session(CPU_INFO)]);
    mountOsdHeader(doc);
    delete win.jmpInfo.settings.playback.transcodeNotice;
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();
    assert.strictEqual(toastTexts(doc).length, 1);
    assert.ok(timers.pending().length > 0, 'the toast schedules its own show/hide');
});

test('the toast fires once per playback session, and again on the next item', async () => {
    const { api, doc, pm, state } = await runToastCase('cpu', CPU_INFO);
    assert.strictEqual(toastTexts(doc).length, 1);
    await api.pollOnce();
    await api.pollOnce();
    assert.strictEqual(toastTexts(doc).length, 1, 'later polls do not repeat it');

    // Episode auto-advance: a fresh playbackstart resets the session.
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await api.pollOnce();
    assert.strictEqual(toastTexts(doc).length, 2);
});

// ---- polling lifecycle ---------------------------------------------------

test('polling starts only for a transcode and stops on playbackstop', async () => {
    const { api, doc, timers, calls } = load([session(HW_INFO)]);
    mountOsdHeader(doc);

    const direct = makePm(makeState('DirectPlay'));
    api.attach(direct);
    Events.trigger(direct, 'playbackstart', [PLAYER, makeState('DirectPlay')]);
    assert.deepStrictEqual(timers.pending(), [], 'direct play never polls');

    api.detach();
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    assert.strictEqual(timers.pending().length, 1);

    // The 4 s timer fires: one poll, then the 10 s repeat is armed.
    timers.flush();
    await new Promise((r) => setImmediate(r));
    assert.strictEqual(calls.getSessions, 1);
    assert.deepStrictEqual(timers.pending().map((t) => t.delay), [10000]);

    Events.trigger(pm, 'playbackstop', [{}]);
    assert.strictEqual(api._timer(), null);
    assert.deepStrictEqual(timers.pending(), [], 'the pending poll is cancelled');

    timers.flush();
    await new Promise((r) => setImmediate(r));
    assert.strictEqual(calls.getSessions, 1, 'no poll runs after the stop');
});

test('detach unbinds both manager handlers', () => {
    const { api, doc } = load();
    mountOsdHeader(doc);
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    api.detach();
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]);
    assert.strictEqual(badgeOf(doc), null);
    for (const name of ['playbackstart', 'playbackstop']) {
        assert.deepStrictEqual(pm._callbacks[name], [], name);
    }
});

test('attaching twice does not double-bind', () => {
    const { api } = load();
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    api.attach(pm);
    assert.strictEqual(pm._callbacks.playbackstart.length, 1);
});

// ---- the play request, ahead of playbackstart ----------------------------

// What mpvVideoPlayer.play() is handed. Same item and same streams as
// makeState(), so a notePlayOptions/playbackstart pair describes one item.
function makeOptions(playMethod, itemId) {
    const id = itemId || 'a';
    return {
        playMethod,
        item: { Id: id, Name: 'Item A', RunTimeTicks: 3600 * 10000000 },
        mediaSource: {
            Id: id,
            RunTimeTicks: 3600 * 10000000,
            MediaStreams: [{ Type: 'Audio', Codec: 'eac3' }, SOURCE_STREAM]
        }
    };
}

test('notePlayOptions paints the badge before playbackstart ever fires', () => {
    const { api, doc, timers } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const pm = makePm(makeState('Transcode'));
    api.attach(pm);

    api.notePlayOptions(makeOptions('Transcode'));

    assert.strictEqual(labelOf(doc), 'TRANSCODING', 'plain: nothing is known about the transcoder yet');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--transcode');
    assert.strictEqual(badgeOf(doc).hidden, false);
    // The source description comes straight off the MediaSource.
    assert.deepStrictEqual(popoverLinesOf(doc), ['HEVC 10-bit 1440x1080']);
    // ...and the /Sessions clock is already ticking, from the play request.
    assert.deepStrictEqual(timers.pending().map((t) => t.delay), [4000]);
});

test('a provisional direct play is labelled and never polls', () => {
    const { api, doc, timers } = load();
    mountOsdHeader(doc);
    api.attach(makePm(makeState('DirectPlay')));

    api.notePlayOptions(makeOptions('DirectPlay'));
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--direct');
    assert.deepStrictEqual(timers.pending(), []);
});

test('a provisional badge falls back to the item streams and survives a bare options object', () => {
    const { api, doc } = load();
    mountOsdHeader(doc);
    api.attach(makePm(makeState('DirectStream')));

    api.notePlayOptions({
        playMethod: 'DirectStream',
        item: { Id: 'a', MediaStreams: [SOURCE_STREAM] },
        mediaSource: { Id: 'a' }
    });
    assert.strictEqual(labelOf(doc), 'DIRECT STREAM');
    assert.deepStrictEqual(popoverLinesOf(doc), ['HEVC 10-bit 1440x1080']);

    // Nothing but a play method: still a badge, still no throw.
    assert.doesNotThrow(() => api.notePlayOptions({ playMethod: 'DirectPlay' }));
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
    assert.deepStrictEqual(popoverLinesOf(doc), []);
});

test('the badge appears once the OSD header mounts, without a second playbackstart', () => {
    const { api, doc, timers } = load();
    api.attach(makePm(makeState('DirectPlay')));

    // Play is requested before the video view exists.
    api.notePlayOptions(makeOptions('DirectPlay'));
    assert.strictEqual(badgeOf(doc), null);
    assert.strictEqual(timers.pending().length, 1, 'a render retry is armed');

    mountOsdHeader(doc);
    timers.flush();
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
});

test('playbackstart refines the provisional badge in place', async () => {
    const { api, doc, win, timers } = load([session(CPU_INFO)]);
    mountOsdHeader(doc);
    win.jmpInfo.settings.playback.transcodeNotice = 'cpu';
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);

    api.notePlayOptions(makeOptions('Transcode'));
    const provisional = badgeOf(doc);
    await api.pollOnce();
    assert.strictEqual(labelOf(doc), 'TRANSCODING · CPU');
    assert.strictEqual(toastTexts(doc).length, 1);
    const clock = timers.pending();

    // ~20 s later, mpv finally has a frame.
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);

    assert.strictEqual(doc.querySelectorAll('.af-source-badge').length, 1, 'no second badge');
    assert.strictEqual(badgeOf(doc), provisional, 'the same node is refined');
    assert.strictEqual(labelOf(doc), 'TRANSCODING · CPU', 'what the poll learned is not thrown away');
    assert.strictEqual(toastTexts(doc).length, 1, 'the toast gate is not re-armed');
    assert.strictEqual(api._session().polled, true, 'the session is not reset');
    assert.deepStrictEqual(timers.pending(), clock, 'the poll clock is not pushed out again');

    // The player handle only arrives with playbackstart; the lead needs it.
    assert.strictEqual(api._session().player, PLAYER);
    await api.pollOnce();
    assert.deepStrictEqual(popoverLinesOf(doc), [
        'HEVC 10-bit 1440x1080 → H264 1920x1080 8.0 Mbps',
        'The video codec is not supported',
        '1.0× realtime',
        '12 s ahead'
    ]);
});

test('playbackstart re-labels when the play method turned out different', async () => {
    const { api, doc, timers } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const state = makeState('DirectPlay');
    const pm = makePm(state);
    api.attach(pm);

    api.notePlayOptions(makeOptions('Transcode'));
    assert.strictEqual(labelOf(doc), 'TRANSCODING');

    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
    assert.strictEqual(levelClassOf(doc), 'af-source-badge--direct');
    assert.deepStrictEqual(timers.pending(), [], 'the transcode poll is called off');
});

test('playbackstart for a different item than the one noted starts fresh', async () => {
    const { api, doc, timers } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const state = makeState('DirectPlay');
    const pm = makePm(state);
    api.attach(pm);

    // The user backed out and started something else before the first frame.
    api.notePlayOptions(makeOptions('Transcode', 'other-item'));
    await api.pollOnce();
    assert.strictEqual(labelOf(doc), 'TRANSCODING · NVENC');

    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
    assert.strictEqual(doc.querySelectorAll('.af-source-badge').length, 1);
    const s = api._session();
    assert.strictEqual(s.transcodingInfo, null, 'the noted session is discarded');
    assert.strictEqual(s.polled, false);
    assert.strictEqual(s.toastShown, false);
    assert.deepStrictEqual(timers.pending(), []);
});

test('notePlayOptions ignores a play method it does not recognise', () => {
    const { api, doc, timers } = load();
    mountOsdHeader(doc);
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]);
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');

    for (const opts of [undefined, null, {}, { playMethod: null }, { playMethod: 'Teleport' }, 'nonsense', 7]) {
        assert.doesNotThrow(() => api.notePlayOptions(opts), String(opts));
    }
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY', 'the live badge is left alone');
    assert.strictEqual(doc.querySelectorAll('.af-source-badge').length, 1);
    assert.deepStrictEqual(timers.pending(), [], 'and nothing is scheduled');
});

// A play request that never becomes playback gets no playbackstop, so the
// pre-start poll loop has to stop itself.
test('polling started by the play request alone is bounded', async () => {
    const { api, doc, timers, calls } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);

    api.notePlayOptions(makeOptions('Transcode'));
    for (let i = 0; i < 30 && timers.pending().length; i++) {
        timers.flush();
        await new Promise((r) => setImmediate(r));
    }
    assert.strictEqual(calls.getSessions, 12);
    assert.deepStrictEqual(timers.pending(), [], 'the loop stops rather than running forever');

    // A late playbackstart re-arms it.
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    assert.deepStrictEqual(timers.pending().map((t) => t.delay), [4000]);
});

test('a stop after a provisional badge clears everything', () => {
    const { api, doc, timers } = load([session(HW_INFO)]);
    mountOsdHeader(doc);
    const pm = makePm(makeState('Transcode'));
    api.attach(pm);

    api.notePlayOptions(makeOptions('Transcode'));
    assert.ok(badgeOf(doc));

    Events.trigger(pm, 'playbackstop', [{}]);
    assert.strictEqual(badgeOf(doc), null);
    assert.deepStrictEqual(timers.pending(), []);
    assert.strictEqual(api._session().playMethod, null);
});

// ---- failure containment -------------------------------------------------

test('a getSessions that throws synchronously never throws out', async () => {
    const { api, doc } = load(() => new Error('boom'));
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await assert.doesNotReject(() => api.pollOnce());
    // The badge keeps the coarse answer it already had.
    assert.strictEqual(labelOf(doc), 'TRANSCODING');
});

test('a rejected getSessions never throws out, and the any toast still fires', async () => {
    const { api, doc, win } = load();
    mountOsdHeader(doc);
    win.jmpInfo.settings.playback.transcodeNotice = 'any';
    win.ApiClient.getSessions = () => Promise.reject(new Error('offline'));
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await assert.doesNotReject(() => api.pollOnce());
    assert.strictEqual(labelOf(doc), 'TRANSCODING');
    assert.deepStrictEqual(toastTexts(doc), ['The server is transcoding this']);
});

test('a garbage sessions payload is survivable', async () => {
    const { api, doc } = load(() => 'not an array');
    mountOsdHeader(doc);
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await assert.doesNotReject(() => api.pollOnce());
    assert.strictEqual(labelOf(doc), 'TRANSCODING');
});

test('no ApiClient at all is survivable', async () => {
    const { api, doc, win } = load();
    mountOsdHeader(doc);
    delete win.ApiClient;
    const state = makeState('Transcode');
    const pm = makePm(state);
    api.attach(pm);
    Events.trigger(pm, 'playbackstart', [PLAYER, state]);
    await assert.doesNotReject(() => api.pollOnce());
    assert.strictEqual(labelOf(doc), 'TRANSCODING');
});

test('a playbackManager that throws on every accessor does not stop the badge', () => {
    const { api, doc } = load();
    mountOsdHeader(doc);
    const pm = {
        getPlayerState() {
            throw new Error('player cannot be null');
        },
        currentMediaSource() {
            throw new Error('player cannot be null');
        },
        currentTime() {
            throw new Error('player cannot be null');
        }
    };
    api.attach(pm);
    assert.doesNotThrow(() => Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]));
    assert.strictEqual(labelOf(doc), 'DIRECT PLAY');
});

test('no OSD header yet means no badge and no throw', () => {
    const { api, doc } = load();
    const pm = makePm(makeState('DirectPlay'));
    api.attach(pm);
    assert.doesNotThrow(() => Events.trigger(pm, 'playbackstart', [PLAYER, makeState('DirectPlay')]));
    assert.strictEqual(badgeOf(doc), null);
});

test('attach without an Events bus is a no-op, not a throw', () => {
    const { api, win } = load();
    delete win.Events;
    assert.doesNotThrow(() => api.attach({}));
    assert.doesNotThrow(() => api.attach(null));
});
