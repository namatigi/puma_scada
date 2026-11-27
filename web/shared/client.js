// WebSocket client for /ws (docs/PROTOCOL.md).
//
//   const c = createWsClient({ url });
//   c.on('values', (changes) => …); c.on('alarms', (list) => …);
//   c.on('status', (s) => …)  // 'connecting' | 'open' | 'closed'
//   c.on('hello', (msg) => …); c.on('reload', () => …);
//   c.on('signed-out', () => …)  // the server ended the socket: its sign-in session ended
//   c.setTags(['A','B']);       // desired subscription set; diffs are sent
//   await c.write('A', 1, 'pulse')  // -> {ok, error?}
//   c.connect(); c.close();
//
// The client reconnects with exponential backoff and jitter, re-subscribes after
// every reconnect, and fails pending writes when the socket drops.

export const WRITE_MODES = ['set', 'toggle', 'increase', 'decrease', 'pulse'];

/** Small event emitter used by the clients and widgets. */
export function createEmitter() {
  const handlers = new Map();
  return {
    on(type, fn) {
      if (!handlers.has(type)) handlers.set(type, new Set());
      handlers.get(type).add(fn);
      return () => handlers.get(type)?.delete(fn);
    },
    emit(type, payload) {
      for (const fn of handlers.get(type) ?? []) {
        try {
          fn(payload);
        } catch (err) {
          console.error(`[moviclone] ${type} handler failed`, err);
        }
      }
    },
  };
}

/** Reconnect delay for attempt n (0-based): 0.5 s doubling to 10 s, ±25 % jitter. */
export function backoffDelay(attempt, random = Math.random) {
  const base = Math.min(10000, 500 * 2 ** Math.min(attempt, 10));
  return Math.round(base * (0.75 + random() * 0.5));
}

/** Default WebSocket URL for the page's origin. */
export function defaultWsUrl(loc = window.location) {
  return `${loc.protocol === 'https:' ? 'wss' : 'ws'}://${loc.host}/ws`;
}

/**
 * @param {{url?:string, writeTimeoutMs?:number, pingMs?:number, WebSocketImpl?:typeof WebSocket}} [opts]
 */
/**
 * How long the client waits for a write-result. Longer than the server's own
 * bound (tagdb waits 10 s for the device; a pulse is write + 0.5 s + write), so
 * a slow write that does succeed is never reported as failed.
 */
export function writeTimeoutFor(mode, base = 12000) {
  return mode === 'pulse' ? 2 * base : base;
}

export function createWsClient({ url = defaultWsUrl(), writeTimeoutMs = 12000, pingMs = 15000, WebSocketImpl = WebSocket } = {}) {
  const ev = createEmitter();
  const wanted = new Set();
  const subscribed = new Set();
  const pending = new Map();
  /** Latest {v,q,ts} per tag, kept across reconnects so screens can repaint at once. */
  const values = new Map();
  let ws = null;
  let status = 'closed';
  let attempt = 0;
  let retryTimer = null;
  let pingTimer = null;
  let lastMessageAt = 0;
  let nextId = 1;
  let stopped = true;
  let serverInfo = null;

  function setStatus(s) {
    if (s === status) return;
    status = s;
    ev.emit('status', s);
  }

  function send(msg) {
    if (ws && ws.readyState === 1) {
      ws.send(JSON.stringify(msg));
      return true;
    }
    return false;
  }

  function syncSubscriptions() {
    if (status !== 'open') return;
    const add = [...wanted].filter((t) => !subscribed.has(t));
    const remove = [...subscribed].filter((t) => !wanted.has(t));
    if (add.length && send({ op: 'subscribe', tags: add })) add.forEach((t) => subscribed.add(t));
    if (remove.length && send({ op: 'unsubscribe', tags: remove })) remove.forEach((t) => subscribed.delete(t));
  }

  function failPending(reason) {
    for (const [, p] of pending) {
      clearTimeout(p.timer);
      p.resolve({ ok: false, error: reason });
    }
    pending.clear();
  }

  function onMessage(e) {
    lastMessageAt = Date.now();
    let msg;
    try {
      msg = JSON.parse(e.data);
    } catch {
      console.warn('[moviclone] ignoring non-JSON message from server');
      return;
    }
    switch (msg.type) {
      case 'hello':
        serverInfo = msg;
        ev.emit('hello', msg);
        break;
      case 'values': {
        const changes = msg.values ?? {};
        for (const [tag, sample] of Object.entries(changes)) values.set(tag, sample);
        ev.emit('values', changes);
        break;
      }
      case 'write-result': {
        const p = pending.get(msg.id);
        if (p) {
          clearTimeout(p.timer);
          pending.delete(msg.id);
          p.resolve(msg.ok ? { ok: true } : { ok: false, error: msg.error ?? 'write refused' });
        }
        break;
      }
      case 'alarms':
        ev.emit('alarms', msg.alarms ?? []);
        break;
      case 'signed-out':
        ev.emit('signed-out', msg);
        break;
      case 'reload':
        ev.emit('reload', msg);
        break;
      case 'pong':
        break;
      default:
        ev.emit('message', msg);
    }
  }

  function scheduleReconnect() {
    if (stopped || retryTimer) return;
    const delay = backoffDelay(attempt);
    attempt += 1;
    ev.emit('retry', { delay, attempt });
    retryTimer = setTimeout(() => {
      retryTimer = null;
      open();
    }, delay);
  }

  function open() {
    if (stopped) return;
    setStatus('connecting');
    try {
      ws = new WebSocketImpl(url);
    } catch (err) {
      console.warn(`[moviclone] cannot open ${url}: ${err.message}`);
      setStatus('closed');
      scheduleReconnect();
      return;
    }
    const sock = ws;
    sock.onopen = () => {
      if (sock !== ws) return;
      attempt = 0;
      lastMessageAt = Date.now();
      subscribed.clear();
      setStatus('open');
      syncSubscriptions();
      clearInterval(pingTimer);
      pingTimer = setInterval(() => {
        // A silent socket (half-open TCP) is dropped so the reconnect logic takes over.
        if (Date.now() - lastMessageAt > pingMs * 2.5) {
          sock.close();
          return;
        }
        send({ op: 'ping' });
      }, pingMs);
    };
    sock.onmessage = onMessage;
    sock.onerror = () => {
      // onclose follows; nothing to do here, but keep the console quiet.
    };
    sock.onclose = () => {
      if (sock !== ws) return;
      clearInterval(pingTimer);
      ws = null;
      subscribed.clear();
      failPending('connection lost');
      setStatus('closed');
      scheduleReconnect();
    };
  }

  return {
    kind: 'ws',
    on: ev.on,
    get status() { return status; },
    get server() { return serverInfo; },
    values,
    connect() {
      if (!stopped) return;
      stopped = false;
      open();
    },
    close() {
      stopped = true;
      clearTimeout(retryTimer);
      retryTimer = null;
      clearInterval(pingTimer);
      if (ws) ws.close();
      ws = null;
      failPending('client closed');
      setStatus('closed');
    },
    /** Drops the socket and opens a new one at once (e.g. after signing in as someone else). */
    reconnect() {
      if (stopped) return;
      clearTimeout(retryTimer);
      retryTimer = null;
      clearInterval(pingTimer);
      const old = ws;
      ws = null;
      if (old) { old.onclose = null; old.close(); }
      subscribed.clear();
      failPending('reconnecting');
      attempt = 0;
      open();
    },
    /** Replaces the desired subscription set. */
    setTags(tags) {
      wanted.clear();
      for (const t of tags) if (t) wanted.add(t);
      syncSubscriptions();
    },
    /**
     * Writes a tag. Resolves (never rejects) with {ok:true} or {ok:false,error}.
     * @param {string} tag @param {*} value @param {string} [mode]
     */
    write(tag, value, mode = 'set') {
      if (!WRITE_MODES.includes(mode)) return Promise.resolve({ ok: false, error: `unknown write mode "${mode}"` });
      const id = nextId;
      nextId += 1;
      if (!send({ op: 'write', id, tag, value, mode })) return Promise.resolve({ ok: false, error: 'not connected' });
      return new Promise((resolve) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          resolve({ ok: false, error: 'no answer from server' });
        }, writeTimeoutFor(mode, writeTimeoutMs));
        pending.set(id, { resolve, timer });
      });
    },
  };
}
