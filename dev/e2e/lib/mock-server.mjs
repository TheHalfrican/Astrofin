// A mock Jellyfin server: node:http plus the pinned jellyfin-web build.
//
// It is deliberately permissive — it authenticates one user, serves one movie
// library holding one item, and streams the generated clip — but it records
// every request, so `server.requests` / `server.unhandled` are the ground
// truth a scenario asserts on (e.g. "the client reported playback start").
//
// Routes are matched case-insensitively against the path with the query
// stripped, the way Jellyfin's own ASP.NET routing behaves. Anything that
// falls through lands in `unhandled` and answers with an empty result of the
// shape jellyfin-web tolerates, so an unmodelled endpoint shows up in the log
// instead of breaking the page.

import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { createHash } from 'node:crypto';

import { CLIP_PATH, WEB_ROOT } from './paths.mjs';
import * as fx from './fixtures.mjs';

const MIME = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.mjs': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
    '.json': 'application/json; charset=utf-8',
    '.png': 'image/png',
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.svg': 'image/svg+xml',
    '.ico': 'image/x-icon',
    '.woff': 'font/woff',
    '.woff2': 'font/woff2',
    '.ttf': 'font/ttf',
    '.map': 'application/json; charset=utf-8',
    '.webmanifest': 'application/manifest+json',
    '.mp4': 'video/mp4',
    '.txt': 'text/plain; charset=utf-8',
};

/** A 1x1 transparent PNG, for every image the client asks for. */
const PIXEL_PNG = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
    'base64',
);

export class MockJellyfin {
    constructor(opts = {}) {
        this.webRoot = opts.webRoot ?? WEB_ROOT;
        this.clipPath = opts.clipPath ?? CLIP_PATH;
        this.host = opts.host ?? '127.0.0.1';
        /** Every request, in order: {method, path, query, body, handled}. */
        this.requests = [];
        /** The subset no route claimed — the list to grow the mock from. */
        this.unhandled = [];
        /** Playback reports posted by the client, newest last. */
        this.playbackReports = [];
        /** DisplayPreferences the client wrote back, by id. */
        this.displayPreferences = new Map();
        /** Capabilities the client posted. */
        this.capabilities = null;
        /** Live /Sessions record, mutated by the playback reports. */
        this.session = fx.sessionInfo();
        /** Open /socket connections, so a scenario can see the client connect. */
        this.socketCount = 0;
        /**
         * Every socket the server has accepted. `server.close()` waits for
         * them, and a socket the app upgraded to a WebSocket is no longer the
         * server's to close, so `stop()` destroys them itself — otherwise a
         * scenario that quits the app gracefully hangs in teardown.
         */
        this.sockets = new Set();
        this.server = http.createServer((req, res) => this.#dispatch(req, res));
        this.server.on('connection', (s) => this.#trackSocket(s));
        // jellyfin-web opens a WebSocket at /socket; answering the upgrade
        // handshake keeps its reconnect loop quiet even though the mock never
        // pushes a message.
        this.server.on('upgrade', (req, socket) => this.#upgrade(req, socket));
        this.port = null;
    }

    get origin() {
        return `http://${this.host}:${this.port}`;
    }

    async start() {
        if (!fs.existsSync(path.join(this.webRoot, 'index.html'))) {
            throw new Error(`no jellyfin-web build at ${this.webRoot}; run dev/e2e/setup.mjs`);
        }
        await new Promise((resolve, reject) => {
            this.server.once('error', reject);
            this.server.listen(0, this.host, resolve);
        });
        this.port = this.server.address().port;
        return this;
    }

    #trackSocket(socket) {
        this.sockets.add(socket);
        socket.on('close', () => this.sockets.delete(socket));
        socket.on('error', () => {});
    }

    async stop() {
        for (const s of this.sockets) s.destroy();
        this.sockets.clear();
        this.server.closeAllConnections?.();
        await new Promise((resolve) => {
            const bail = setTimeout(resolve, 5_000);
            bail.unref?.();
            this.server.close(() => {
                clearTimeout(bail);
                resolve();
            });
        });
    }

    /** Requests whose path matches, case-insensitively. */
    find(pattern) {
        const re = pattern instanceof RegExp ? pattern : new RegExp(pattern, 'i');
        return this.requests.filter((r) => re.test(r.path));
    }

    /** True once the client has posted a playback report of `kind`. */
    reported(kind) {
        return this.playbackReports.some((r) => r.kind === kind);
    }

    lastReport(kind) {
        return [...this.playbackReports].reverse().find((r) => r.kind === kind);
    }

    #upgrade(req, socket) {
        const key = req.headers['sec-websocket-key'];
        if (!key) {
            socket.destroy();
            return;
        }
        const accept = createHash('sha1')
            .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
            .digest('base64');
        socket.write(
            'HTTP/1.1 101 Switching Protocols\r\n' +
                'Upgrade: websocket\r\nConnection: Upgrade\r\n' +
                `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
        );
        this.socketCount += 1;
        this.requests.push({ method: 'GET', path: '/socket', query: {}, handled: true, upgrade: true });
        // The client only ever sends keep-alive frames the mock can ignore.
        this.#trackSocket(socket);
        socket.on('close', () => {
            this.socketCount -= 1;
        });
    }

    async #dispatch(req, res) {
        const url = new URL(req.url, this.origin);
        const record = {
            method: req.method,
            path: url.pathname,
            query: Object.fromEntries(url.searchParams),
            handled: false,
            body: null,
        };
        this.requests.push(record);
        try {
            const handled = await this.#route(req, res, url, record);
            record.handled = handled !== false;
            if (!record.handled) this.unhandled.push(record);
        } catch (e) {
            record.error = e.message;
            if (!res.headersSent) {
                res.writeHead(500, { 'Content-Type': 'text/plain' });
                res.end(String(e.message));
            }
        }
    }

    async #route(req, res, url, record) {
        const p = url.pathname.replace(/\/+$/, '') || '/';
        const lower = p.toLowerCase();
        const method = req.method.toUpperCase();

        // ---- static: the jellyfin-web build ----
        if (lower === '/' || lower === '/web' || lower === '/index.html') {
            res.writeHead(302, { Location: '/web/index.html' });
            res.end();
            return true;
        }
        if (lower.startsWith('/web/')) {
            return this.#serveWeb(res, p.slice('/web/'.length), method);
        }

        // ---- media ----
        if (/^\/(videos|audio)\/[^/]+\/(stream|universal)/i.test(lower)) {
            return this.#serveClip(req, res, method);
        }
        if (/\/images\//i.test(lower)) {
            res.writeHead(200, { 'Content-Type': 'image/png', 'Content-Length': PIXEL_PNG.length });
            res.end(method === 'HEAD' ? undefined : PIXEL_PNG);
            return true;
        }

        // ---- API ----
        const body = await readBody(req);
        record.body = body;
        const json = (value, status = 200) => sendJson(res, value, status, method);
        const noContent = () => {
            res.writeHead(204).end();
            return true;
        };

        switch (lower) {
            case '/system/info/public':
                return json(fx.publicSystemInfo());
            case '/system/info':
                return json(fx.systemInfo());
            case '/system/endpoint':
                return json({ IsLocal: true, IsInNetwork: true });
            case '/system/activitylog/entries':
                return json(fx.itemsResult([]));
            case '/system/configuration':
                return json({ EnableMetrics: false, IsStartupWizardCompleted: true });
            case '/branding/configuration':
                return json(fx.brandingConfiguration());
            case '/branding/css':
            case '/branding/css.css':
                res.writeHead(200, { 'Content-Type': 'text/css' }).end('');
                return true;
            case '/quickconnect/enabled':
                return json(false);
            case '/users/public':
                // Empty, so jellyfin-web shows the manual login form.
                return json([]);
            case '/users/authenticatebyname': {
                const creds = parseJson(body) ?? {};
                if (creds.Username !== fx.USERNAME || creds.Pw !== fx.PASSWORD) {
                    return json({ Message: 'Invalid username or password' }, 401);
                }
                return json(fx.authenticationResult());
            }
            case '/users/me':
                return json(fx.user());
            case '/users':
                return json([fx.user()]);
            case '/userviews':
                return json(fx.itemsResult([fx.library()]));
            case '/userviews/groupingoptions':
                return json([]);
            case '/sessions':
                return json([this.session]);
            case '/sessions/capabilities/full':
                this.capabilities = parseJson(body);
                return noContent();
            case '/sessions/playing':
                return this.#recordPlayback('start', body, noContent);
            case '/sessions/playing/progress':
            case '/sessions/playing/ping':
                return this.#recordPlayback('progress', body, noContent);
            case '/sessions/playing/stopped':
                return this.#recordPlayback('stopped', body, noContent);
            case '/plugins':
                return json([]);
            case '/auth/keys':
                return json(fx.itemsResult([]));
            case '/localization/options':
                return json([{ Name: 'English', Value: 'en-US' }]);
            case '/localization/cultures':
                return json([{ Name: 'English', DisplayName: 'English', TwoLetterISOLanguageName: 'en', ThreeLetterISOLanguageName: 'eng' }]);
            case '/localization/countries':
                return json([]);
            case '/livetv/programs/recommended':
            case '/livetv/channels':
            case '/livetv/recordings':
                return json(fx.itemsResult([]));
            case '/shows/nextup':
            case '/shows/upcoming':
                return json(fx.itemsResult([]));
            case '/library/mediafolders':
                return json(fx.itemsResult([fx.library()]));
            case '/library/virtualfolders':
                return json([]);
            case '/scheduledtasks':
                return json([]);
            case '/syncplay/list':
                return json([]);
            case '/playback/bitratetest':
                res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(Buffer.alloc(1024));
                return true;
            default:
                break;
        }

        // ---- parameterised API ----
        let m;
        if ((m = lower.match(/^\/users\/([^/]+)\/views$/))) {
            return json(fx.itemsResult([fx.library()]));
        }
        if ((m = lower.match(/^\/users\/([^/]+)\/items\/resume$/)) || lower === '/useritems/resume') {
            return json(fx.itemsResult([]));
        }
        // Latest is a bare array, not an ItemsResult — jellyfin-web's Latest
        // Media home section calls `.entries()` on it and throws otherwise.
        // It has to precede the `/Users/{id}/Items/{itemId}` route below,
        // which would happily read "Latest" as an item id.
        if (/^\/users\/[^/]+\/items\/latest$/.test(lower) || lower === '/items/latest') {
            return json([]);
        }
        if ((m = lower.match(/^\/users\/[^/]+\/items\/([^/]+)\/(intros|specialfeatures|localtrailers)$/))) {
            return json(fx.itemsResult([]));
        }
        if ((m = lower.match(/^\/users\/([^/]+)\/items\/([^/]+)$/))) {
            return json(this.#itemById(m[2]));
        }
        if ((m = lower.match(/^\/users\/([^/]+)\/items$/)) || lower === '/items') {
            return json(this.#itemQuery(url));
        }
        if ((m = lower.match(/^\/users\/([^/]+)$/)) && m[1] === fx.USER_ID.toLowerCase()) {
            return json(fx.user());
        }
        if ((m = lower.match(/^\/items\/([^/]+)\/playbackinfo$/))) {
            return json(this.#playbackInfo(m[1], parseJson(body)));
        }
        if ((m = lower.match(/^\/items\/([^/]+)\/(similar|thememedia|intros|specialfeatures|ancestors)$/))) {
            if (m[2] === 'ancestors') return json([fx.library()]);
            if (m[2] === 'thememedia') return json(fx.themeMedia(m[1]));
            return json(fx.itemsResult([]));
        }
        if ((m = lower.match(/^\/items\/([^/]+)$/))) {
            return json(this.#itemById(m[1]));
        }
        if ((m = lower.match(/^\/displaypreferences\/([^/]+)$/))) {
            if (method === 'POST') {
                this.displayPreferences.set(m[1], parseJson(body));
                return noContent();
            }
            return json(this.displayPreferences.get(m[1]) ?? fx.displayPreferences(m[1]));
        }
        if ((m = lower.match(/^\/users\/([^/]+)\/playeditems\/([^/]+)$/))) {
            return json(fx.userData({ Played: method === 'POST' }));
        }
        if (/^\/users\/[^/]+\/favoriteitems\//.test(lower)) {
            return json(fx.userData({ IsFavorite: method === 'POST' }));
        }
        if (/^\/(persons|genres|studios|artists|musicgenres|playlists|channels)/.test(lower)) {
            return json(fx.itemsResult([]));
        }
        if (/^\/videos\/[^/]+\/(hls1|master|main)/.test(lower)) {
            // The mock never transcodes; a client that asks has mis-negotiated.
            res.writeHead(404).end();
            return false;
        }

        // ---- fall-through: log it, answer with something inert ----
        if (method === 'POST' || method === 'DELETE') return noContent();
        sendJson(res, fx.itemsResult([]), 200, method);
        return false;
    }

    #recordPlayback(kind, body, noContent) {
        const info = parseJson(body) ?? {};
        this.playbackReports.push({ kind, at: Date.now(), info });
        this.session = fx.sessionInfo({
            NowPlayingItem: kind === 'stopped' ? undefined : fx.item(),
            PlayState: {
                CanSeek: true,
                IsPaused: Boolean(info.IsPaused),
                IsMuted: Boolean(info.IsMuted),
                PositionTicks: info.PositionTicks ?? 0,
                MediaSourceId: info.MediaSourceId ?? fx.MEDIA_SOURCE_ID,
                PlayMethod: info.PlayMethod ?? 'DirectPlay',
                RepeatMode: 'RepeatNone',
                PlaybackOrder: 'Default',
            },
            PlayMethod: info.PlayMethod ?? 'DirectPlay',
            TranscodingInfo: undefined,
        });
        return noContent();
    }

    #itemById(id) {
        if (id.toLowerCase() === fx.LIBRARY_ID.toLowerCase()) return fx.library();
        return fx.item();
    }

    #itemQuery(url) {
        const parentId = url.searchParams.get('ParentId') ?? url.searchParams.get('parentId');
        const ids = url.searchParams.get('Ids') ?? url.searchParams.get('ids');
        if (ids) {
            const wanted = ids.split(',').map((s) => s.trim().toLowerCase());
            return fx.itemsResult(wanted.includes(fx.ITEM_ID.toLowerCase()) ? [fx.item()] : []);
        }
        if (!parentId || parentId.toLowerCase() === fx.LIBRARY_ID.toLowerCase()) {
            return fx.itemsResult([fx.item()]);
        }
        return fx.itemsResult([]);
    }

    #playbackInfo(itemId, requestBody) {
        const src = fx.mediaSource();
        src.DirectStreamUrl = `/Videos/${fx.ITEM_ID}/stream.mp4?static=true&mediaSourceId=${fx.MEDIA_SOURCE_ID}`;
        src.TranscodingUrl = null;
        return {
            MediaSources: [src],
            PlaySessionId: `e2e-play-session-${this.playbackReports.length}`,
            ErrorCode: null,
            _requested: requestBody?.DeviceProfile ? 'with-profile' : 'no-profile',
        };
    }

    #serveWeb(res, rel, method) {
        const safe = path
            .normalize(decodeURIComponent(rel))
            .replace(/^([.][.][\\/])+/, '')
            .replace(/^[\\/]+/, '');
        const file = path.join(this.webRoot, safe);
        if (!file.startsWith(this.webRoot) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
            res.writeHead(404).end();
            return false;
        }
        const buf = fs.readFileSync(file);
        res.writeHead(200, {
            'Content-Type': MIME[path.extname(file).toLowerCase()] ?? 'application/octet-stream',
            'Content-Length': buf.length,
            'Cache-Control': 'no-store',
        });
        res.end(method === 'HEAD' ? undefined : buf);
        return true;
    }

    /** Range-aware clip streaming; mpv issues a ranged GET on every seek. */
    #serveClip(req, res, method) {
        if (!fs.existsSync(this.clipPath)) {
            res.writeHead(404).end();
            return false;
        }
        const size = fs.statSync(this.clipPath).size;
        const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? '');
        let start = 0;
        let end = size - 1;
        let status = 200;
        const headers = { 'Content-Type': 'video/mp4', 'Accept-Ranges': 'bytes' };
        if (range) {
            start = range[1] ? Number(range[1]) : 0;
            end = range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
            if (start >= size || start > end) {
                res.writeHead(416, { 'Content-Range': `bytes */${size}` }).end();
                return true;
            }
            status = 206;
            headers['Content-Range'] = `bytes ${start}-${end}/${size}`;
        }
        headers['Content-Length'] = end - start + 1;
        res.writeHead(status, headers);
        if (method === 'HEAD') {
            res.end();
            return true;
        }
        fs.createReadStream(this.clipPath, { start, end }).pipe(res);
        return true;
    }
}

function sendJson(res, value, status, method) {
    const buf = Buffer.from(JSON.stringify(value));
    res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': buf.length });
    res.end(method === 'HEAD' ? undefined : buf);
    return true;
}

function parseJson(body) {
    if (!body) return null;
    try {
        return JSON.parse(body);
    } catch {
        return null;
    }
}

function readBody(req) {
    if (req.method === 'GET' || req.method === 'HEAD') return Promise.resolve('');
    return new Promise((resolve) => {
        let data = '';
        req.setEncoding('utf8');
        req.on('data', (c) => (data += c));
        req.on('end', () => resolve(data));
        req.on('error', () => resolve(data));
    });
}

/** Start a mock on an ephemeral port. */
export async function startMock(opts) {
    return new MockJellyfin(opts).start();
}
