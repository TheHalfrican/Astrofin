// Minimal Chrome DevTools Protocol client.
//
// Node's global `WebSocket` (>= 22) is the whole transport, so the harness
// needs no npm dependency. Only what the scenarios use is implemented:
// target discovery over the HTTP endpoint, one socket per target, request /
// response correlation by id, and `Runtime.evaluate` with awaited promises.

const DEFAULT_TIMEOUT_MS = 20_000;

/** Sleep helper — used only where CDP gives no event to wait on. */
export function delay(ms) {
    return new Promise((r) => setTimeout(r, ms));
}

/**
 * Poll `fn` until it returns a truthy value. Rejects with `message` after
 * `timeoutMs`. Every wait in the suite goes through here so a hung app fails
 * a scenario instead of the whole run.
 */
export async function waitFor(fn, { timeoutMs = DEFAULT_TIMEOUT_MS, intervalMs = 250, message = 'condition' } = {}) {
    const deadline = Date.now() + timeoutMs;
    let last;
    for (;;) {
        try {
            const v = await fn();
            if (v) return v;
            last = undefined;
        } catch (e) {
            last = e;
        }
        if (Date.now() >= deadline) {
            const suffix = last ? ` (last error: ${last.message})` : '';
            throw new Error(`timed out after ${timeoutMs} ms waiting for ${message}${suffix}`);
        }
        await delay(intervalMs);
    }
}

/** GET the debugger's HTTP endpoint and parse the JSON body. */
export async function fetchJson(port, route, timeoutMs = 4000) {
    const res = await fetch(`http://127.0.0.1:${port}${route}`, {
        signal: AbortSignal.timeout(timeoutMs),
    });
    if (!res.ok) throw new Error(`GET ${route} -> ${res.status}`);
    return res.json();
}

/** Every debuggable target CEF currently exposes. */
export async function listTargets(port) {
    return fetchJson(port, '/json/list');
}

/** The debugger's own version banner; the cheapest liveness probe. */
export async function debuggerVersion(port) {
    return fetchJson(port, '/json/version');
}

/**
 * Wait until a target whose `url`/`title` satisfies `predicate` exists, then
 * attach to it.
 */
export async function attachToTarget(port, predicate, opts = {}) {
    const target = await waitFor(
        async () => (await listTargets(port)).find(predicate),
        { message: opts.message ?? 'a matching debug target', ...opts },
    );
    const session = new CdpSession(target);
    await session.open();
    return session;
}

export class CdpSession {
    constructor(target) {
        this.target = target;
        this.ws = null;
        this.nextId = 1;
        this.pending = new Map();
        this.events = [];
        this.listeners = new Set();
    }

    get url() {
        return this.target.url;
    }

    async open() {
        const ws = new WebSocket(this.target.webSocketDebuggerUrl);
        this.ws = ws;
        await new Promise((resolve, reject) => {
            const onOpen = () => {
                ws.removeEventListener('error', onError);
                resolve();
            };
            const onError = (e) => {
                ws.removeEventListener('open', onOpen);
                reject(new Error(`CDP socket failed: ${e?.message ?? 'error'}`));
            };
            ws.addEventListener('open', onOpen, { once: true });
            ws.addEventListener('error', onError, { once: true });
        });
        ws.addEventListener('message', (ev) => this.#onMessage(ev));
        ws.addEventListener('close', () => {
            for (const [, p] of this.pending) p.reject(new Error('CDP socket closed'));
            this.pending.clear();
        });
        return this;
    }

    #onMessage(ev) {
        let msg;
        try {
            msg = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data));
        } catch {
            return;
        }
        if (msg.id != null && this.pending.has(msg.id)) {
            const { resolve, reject } = this.pending.get(msg.id);
            this.pending.delete(msg.id);
            if (msg.error) reject(new Error(`${msg.error.message} (${msg.error.code})`));
            else resolve(msg.result);
            return;
        }
        if (msg.method) {
            this.events.push(msg);
            for (const fn of this.listeners) fn(msg);
        }
    }

    /** Subscribe to protocol events; returns an unsubscribe function. */
    on(fn) {
        this.listeners.add(fn);
        return () => this.listeners.delete(fn);
    }

    send(method, params = {}, timeoutMs = DEFAULT_TIMEOUT_MS) {
        if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
            return Promise.reject(new Error(`CDP session for ${this.target.url} is not open`));
        }
        const id = this.nextId++;
        const p = new Promise((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            setTimeout(() => {
                if (this.pending.delete(id)) reject(new Error(`CDP ${method} timed out after ${timeoutMs} ms`));
            }, timeoutMs).unref?.();
        });
        this.ws.send(JSON.stringify({ id, method, params }));
        return p;
    }

    /**
     * Evaluate `expression` in the page and return its value. Promises are
     * awaited; a thrown exception becomes a rejected promise here so a broken
     * page assertion reads like a normal JS error.
     */
    async evaluate(expression, { timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
        const r = await this.send(
            'Runtime.evaluate',
            {
                expression,
                returnByValue: true,
                awaitPromise: true,
                allowUnsafeEvalBlockedByCSP: true,
            },
            timeoutMs,
        );
        if (r.exceptionDetails) {
            const d = r.exceptionDetails;
            throw new Error(`page threw: ${d.exception?.description ?? d.text}`);
        }
        return r.result?.value;
    }

    /** `evaluate`, retried until it yields a truthy value. */
    waitForExpression(expression, opts = {}) {
        return waitFor(() => this.evaluate(expression), {
            message: opts.message ?? `expression ${JSON.stringify(expression)}`,
            ...opts,
        });
    }

    close() {
        try {
            this.ws?.close();
        } catch {
            /* already gone */
        }
    }
}
