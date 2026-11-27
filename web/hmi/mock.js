// In-browser mock of the runtime for development and demos (index.html?mock).
// Implements the same `api` (shared/api.js) and `client` (shared/client.js)
// interfaces from a bundle JSON: simulated flow-meter values, writes, history,
// history tables and ISA-18.2 alarms. Values are deterministic functions of
// time, so any history range can be answered without storing anything.

import { createEmitter, WRITE_MODES } from '../shared/client.js';
import { parseInputValue, truthy, toNumber } from '../shared/format.js';

const HOUR = 3600e3;
const DENSITY = { 1: 962.4, 2: 838.7, 3: 801.2, 4: 742.9, 5: 871.5 };
const BASE_FLOW = { 1: 96, 2: 84, 3: 72, 4: 101, 5: 60 };
const HISTORY_DAYS = 45;

/** Cheap deterministic noise in [-1, 1] for (seed, t). */
function noise(seed, t) {
  const x = Math.sin(seed * 12.9898 + t * 78.233) * 43758.5453;
  return (x - Math.floor(x)) * 2 - 1;
}
function hashName(name) {
  let hsh = 0;
  for (const ch of name) hsh = (hsh * 31 + ch.charCodeAt(0)) | 0;
  return Math.abs(hsh % 997) + 1;
}

const DEFAULT_ALARMS = [
  { name: 'FM01_HighFlow', tag: 'FM_01_MassFlow', when: 'above', limit: 104, deadband: 2, severity: 'high', message: 'HFO mass flow above 104 mTon/hr', area: 'Manifold' },
  { name: 'FM02_HighFlow', tag: 'FM_02_MassFlow', when: 'above', limit: 86, deadband: 2, severity: 'medium', message: 'Gasoil mass flow above 86 mTon/hr', area: 'Manifold' },
  { name: 'FM03_HighTemp', tag: 'FM_03_Temp', when: 'above', limit: 25.5, deadband: 0.3, severity: 'low', message: 'JET A-1 product temperature above 25.5 °C', area: 'Manifold' },
  { name: 'FM01_PressureComms', tag: 'FM_01_External_Pressure', when: 'bad-quality', severity: 'critical', message: 'HFO external pressure transmitter not answering', area: 'Manifold' },
  { name: 'FM04_Stopped', tag: 'FM_04_START', when: 'is-false', severity: 'medium', message: 'Gasoline meter run stopped', area: 'Manifold' },
];

export function createMockBackend(bundle, { now = () => Date.now() } = {}) {
  const tags = new Map((bundle.tags ?? []).map((t) => [t.name, t]));
  const startedAt = now();
  /** Operator-written values (read-write tags). */
  const overrides = new Map();
  /** Totaliser reset time per meter. */
  const resetAt = new Map();
  const quality = new Map([['FM_01_External_Pressure', 'bad'], ['FM_03_External_Temperature', 'uncertain']]);

  // ---------------------------------------------------------------- signal model
  function meterOf(name) {
    const m = /^FM_0?(\d+)_/.exec(name);
    return m ? Number(m[1]) : 0;
  }

  function running(n, t) {
    const v = overrides.get(`FM_0${n}_START`);
    if (v !== undefined && (v.since ?? 0) <= t) return truthy(v.value);
    return true;
  }

  function flowAt(n, t) {
    if (!running(n, t)) return 0;
    const b = BASE_FLOW[n] ?? 80;
    const slow = Math.sin(t / (HOUR * 0.37) + n * 1.7) * 14;
    const fast = Math.sin(t / 290e3 + n) * 3 + Math.sin(t / 61e3 + n * 2) * 0.8;
    return Math.max(0, b + slow + fast + noise(n, Math.floor(t / 1000)) * 0.15);
  }

  /** Integral of the mean flow (closed form), in tonnes, from t0 to t. */
  function massBetween(n, t0, t) {
    return ((BASE_FLOW[n] ?? 80) * Math.max(0, t - t0)) / HOUR;
  }

  function valueAt(name, t) {
    const tag = tags.get(name);
    const o = overrides.get(name);
    if (o !== undefined && o.since <= t) return o.value;
    const n = meterOf(name);
    const kind = name.replace(/^FM_0?\d+_/, '');
    const seed = hashName(name);
    const sec = Math.floor(t / 1000);
    const density = DENSITY[n] ?? 850;
    const epoch = startedAt - HISTORY_DAYS * 24 * HOUR;
    switch (kind) {
      case 'MassFlow': return round(flowAt(n, t), 4);
      case 'Volume_Rate': return round((flowAt(n, t) * 1000) / density, 4);
      case 'Density': return round(density + Math.sin(t / (HOUR * 2) + n) * 1.8 + Math.sin(t / 420e3) * 0.12 + noise(seed, sec) * 0.004, 5);
      case 'Temp': return round(24 + n * 0.6 + Math.sin(t / (HOUR * 3) + n) * 2.2 + Math.sin(t / 530e3 + n) * 0.15 + noise(seed, sec) * 0.003, 4);
      case 'Mass_Total': return round(massBetween(n, Math.max(epoch, resetAt.get(n) ?? epoch), t), 4);
      case 'Vol_Total':
      case 'Vol': return round((massBetween(n, Math.max(epoch, resetAt.get(n) ?? epoch), t) * 1000) / density, 4);
      case 'Mass_Inv': return round(125000 + n * 9000 + massBetween(n, epoch, t), 3);
      case 'Vol_Inv': return round(((125000 + n * 9000 + massBetween(n, epoch, t)) * 1000) / density, 3);
      case 'External_Temp':
      case 'External_Temperature': return round(21.5 + n * 0.4 + Math.sin(t / (HOUR * 5)) * 1.5, 3);
      case 'External_Pressure': return round(3.4 + n * 0.15 + Math.sin(t / (HOUR * 1.3) + n) * 0.08, 4);
      case 'START': return true;
      case 'Reset_Total': return false;
      default:
        if (tag?.type === 'bool') return false;
        if (tag?.type === 'string') return '';
        return round(1 + Math.sin(t / HOUR + seed) * 0.5, 3);
    }
  }

  const round = (v, d) => Math.round(v * 10 ** d) / 10 ** d;

  function sample(name, t = now()) {
    if (!tags.has(name)) return { v: null, q: 'bad', ts: t };
    const q = quality.get(name) ?? 'good';
    // A tag whose device does not answer keeps its last good timestamp.
    const ts = q === 'bad' ? startedAt - 12 * 60e3 : Math.floor(t / 1000) * 1000;
    return { v: q === 'bad' ? round(valueAt(name, startedAt - 12 * 60e3), 3) : valueAt(name, t), q, ts };
  }

  // ---------------------------------------------------------------- writes
  function write(tagName, rawValue, mode = 'set') {
    const tag = tags.get(tagName);
    if (!tag) return { ok: false, error: `unknown tag "${tagName}"` };
    if (!tag.access || tag.access === 'read') return { ok: false, error: `tag "${tagName}" is read-only` };
    if (!WRITE_MODES.includes(mode)) return { ok: false, error: `unknown write mode "${mode}"` };
    let value;
    try {
      value = typeof rawValue === 'string' ? parseInputValue(rawValue, tag.type) : rawValue;
    } catch (err) {
      return { ok: false, error: err.message };
    }
    const t = now();
    const current = sample(tagName, t).v;
    if (mode === 'toggle') value = tag.type === 'bool' ? !truthy(current) : (toNumber(current) ? 0 : 1);
    else if (mode === 'increase') value = (toNumber(current) ?? 0) + (toNumber(value) ?? 1);
    else if (mode === 'decrease') value = (toNumber(current) ?? 0) - (toNumber(value) ?? 1);
    if (/_Reset_Total$/.test(tagName)) {
      if (truthy(value)) resetAt.set(meterOf(tagName), t);
      pushChanged([tagName, ...[...tags.keys()].filter((n) => meterOf(n) === meterOf(tagName))]);
      return { ok: true };
    }
    if (mode === 'pulse') {
      overrides.set(tagName, { value, since: t });
      setTimeout(() => {
        overrides.set(tagName, { value: current, since: now() });
        pushChanged([tagName]);
      }, 1000);
    } else overrides.set(tagName, { value, since: t });
    pushChanged([tagName]);
    return { ok: true };
  }

  // ---------------------------------------------------------------- alarms (ISA-18.2)
  const alarmDefs = (bundle.alarms?.length ? bundle.alarms : DEFAULT_ALARMS).filter((a) => a.enabled !== false);
  const alarmState = new Map(alarmDefs.map((a) => [a.name, { state: 'normal', since: null, value: null, acked_by: null, shelved_until: null }]));
  const journal = [];

  function condition(def, sm) {
    const v = toNumber(sm.v);
    const st = alarmState.get(def.name);
    const active = st.state === 'active-unacked' || st.state === 'active-acked';
    const db = active ? def.deadband ?? 0 : 0;
    switch (def.when) {
      case 'above': return v !== null && v > def.limit - db;
      case 'below': return v !== null && v < def.limit + db;
      case 'equals': return v === def.value;
      case 'is-true': return truthy(sm.v);
      case 'is-false': return !truthy(sm.v);
      case 'bad-quality': return sm.q === 'bad';
      default: return false;
    }
  }

  function logEvent(name, event, value) {
    journal.unshift({ ts: now(), name, event, value, user: event === 'acked' || event === 'shelved' || event === 'unshelved' ? 'operator' : null });
    if (journal.length > 1000) journal.length = 1000;
  }

  function evaluateAlarms() {
    let changed = false;
    const t = now();
    for (const def of alarmDefs) {
      const st = alarmState.get(def.name);
      const sm = sample(def.tag, t);
      if (st.state === 'shelved') {
        if (st.shelved_until && t >= st.shelved_until) {
          st.state = 'normal';
          st.shelved_until = null;
          logEvent(def.name, 'unshelved', sm.v);
          changed = true;
        } else continue;
      }
      const on = condition(def, sm);
      const prev = st.state;
      if (on && (prev === 'normal' || prev === 'cleared-unacked')) {
        st.state = 'active-unacked';
        st.since = t;
        st.acked_by = null;
        logEvent(def.name, 'activated', sm.v);
      } else if (!on && prev === 'active-unacked') {
        st.state = def.ack_required === false ? 'normal' : 'cleared-unacked';
        logEvent(def.name, 'cleared', sm.v);
      } else if (!on && prev === 'active-acked') {
        st.state = 'normal';
        logEvent(def.name, 'cleared', sm.v);
      }
      if (st.state !== prev) changed = true;
      st.value = sm.v;
    }
    if (changed) ev.emit('alarms', alarmList());
  }

  function alarmList() {
    return alarmDefs.map((def) => {
      const st = alarmState.get(def.name);
      return {
        name: def.name, tag: def.tag, message: def.message, severity: def.severity ?? 'medium', area: def.area ?? null,
        state: st.state, value: st.value, since: st.since, acked_by: st.acked_by, shelved_until: st.shelved_until,
      };
    });
  }

  function ack(name) {
    const st = alarmState.get(name);
    if (!st) throw new Error(`unknown alarm "${name}"`);
    if (st.state === 'active-unacked') st.state = 'active-acked';
    else if (st.state === 'cleared-unacked') st.state = 'normal';
    else return;
    st.acked_by = 'operator';
    logEvent(name, 'acked', st.value);
    ev.emit('alarms', alarmList());
  }

  // ---------------------------------------------------------------- WebSocket-like client
  const ev = createEmitter();
  const subscribed = new Set();
  const lastSent = new Map();
  const values = new Map();
  let status = 'closed';
  let timers = [];

  function pushChanged(names) {
    if (status !== 'open') return;
    const out = {};
    for (const n of names) {
      if (!subscribed.has(n)) continue;
      const sm = sample(n);
      out[n] = sm;
      lastSent.set(n, JSON.stringify([sm.v, sm.q]));
      values.set(n, sm);
    }
    if (Object.keys(out).length) ev.emit('values', out);
  }

  function publish() {
    const out = {};
    for (const n of subscribed) {
      const sm = sample(n);
      const key = JSON.stringify([sm.v, sm.q]);
      if (lastSent.get(n) === key) continue;
      lastSent.set(n, key);
      out[n] = sm;
      values.set(n, sm);
    }
    if (Object.keys(out).length) ev.emit('values', out);
  }

  const client = {
    kind: 'mock',
    on: ev.on,
    values,
    get status() { return status; },
    get server() { return { project: bundle.meta?.name, main_screen: bundle.meta?.main_screen, version: 'mock' }; },
    connect() {
      if (status !== 'closed') return;
      status = 'connecting';
      ev.emit('status', status);
      setTimeout(() => {
        status = 'open';
        ev.emit('status', status);
        ev.emit('hello', { type: 'hello', project: bundle.meta?.name, main_screen: bundle.meta?.main_screen, server_time: now(), version: 'mock' });
        evaluateAlarms();
        ev.emit('alarms', alarmList());
        pushChanged([...subscribed]);
        timers = [
          setInterval(publish, bundle.meta?.sampling?.publish_ms ?? 250),
          setInterval(evaluateAlarms, 1000),
        ];
      }, 150);
    },
    close() {
      timers.forEach(clearInterval);
      timers = [];
      status = 'closed';
      ev.emit('status', status);
    },
    setTags(list) {
      const next = new Set(list);
      const added = [...next].filter((tg) => !subscribed.has(tg));
      subscribed.clear();
      next.forEach((tg) => subscribed.add(tg));
      for (const tg of [...lastSent.keys()]) if (!next.has(tg)) lastSent.delete(tg);
      pushChanged(added);
    },
    write(tag, value, mode = 'set') {
      return new Promise((resolve) => setTimeout(() => resolve(write(tag, value, mode)), 60));
    },
  };

  // ---------------------------------------------------------------- REST-like api
  const delay = (v, ms = 120) => new Promise((resolve) => setTimeout(() => resolve(v), ms));
  const historyStart = () => startedAt - HISTORY_DAYS * 24 * HOUR;

  function series(tag, from, to, points) {
    const f = Math.max(from, historyStart());
    const out = [];
    if (to <= f) return out;
    const n = Math.max(2, Math.min(5000, points));
    const step = (to - f) / (n - 1);
    for (let i = 0; i < n; i += 1) {
      const t = Math.round(f + i * step);
      const q = quality.get(tag);
      out.push([t, q === 'bad' && t > startedAt - 12 * 60e3 ? null : valueAt(tag, t)]);
    }
    return out;
  }

  function sourceTags(source) {
    const logger = bundle.data?.logger?.find((l) => l.name === source);
    if (logger) return logger.column.map((c) => c.tag);
    return (bundle.tags ?? []).filter((t) => t.historian === source).map((t) => t.name);
  }

  const api = {
    kind: 'mock',
    bundle: () => delay(bundle, 0),
    values: () => delay(Object.fromEntries([...tags.keys()].map((n) => [n, sample(n)]))),
    write: async (tag, value, mode) => {
      const r = write(tag, value, mode);
      if (!r.ok) throw new Error(r.error);
      return r;
    },
    history: ({ tags: list, from, to, points = 600 }) =>
      delay({ series: list.map((tg) => ({ tag: tg, points: tags.has(tg) ? series(tg, from, to, points) : [] })) }, 200),
    historyTable: ({ source, from, to, limit = 500 }) => {
      const list = sourceTags(source);
      const rows = [];
      const f = Math.max(from, historyStart());
      // One record per tag every 30 s (on-change logging of a busy meter), newest first.
      const step = Math.max(30e3, Math.ceil((to - f) / Math.max(1, limit / Math.max(1, list.length)) / 30e3) * 30e3);
      for (let t = Math.floor(to / step) * step; t >= f && rows.length < limit; t -= step) {
        for (const tg of list) {
          if (rows.length >= limit) break;
          const q = quality.get(tg) ?? 'good';
          rows.push([t, tg, q === 'bad' ? null : valueAt(tg, t), q]);
        }
      }
      return delay({ columns: ['time', 'tag', 'value', 'quality'], rows }, 180);
    },
    alarms: () => delay({ alarms: alarmList() }),
    ack: async (name) => { ack(name); return delay({ ok: true }, 40); },
    ackAll: async () => {
      for (const def of alarmDefs) {
        const st = alarmState.get(def.name);
        if (st.state === 'active-unacked' || st.state === 'cleared-unacked') ack(def.name);
      }
      return delay({ ok: true }, 40);
    },
    shelve: async (name, minutes = 30) => {
      const st = alarmState.get(name);
      if (!st) throw new Error(`unknown alarm "${name}"`);
      st.state = 'shelved';
      st.shelved_until = now() + minutes * 60e3;
      logEvent(name, 'shelved', st.value);
      ev.emit('alarms', alarmList());
      return delay({ ok: true }, 40);
    },
    unshelve: async (name) => {
      const st = alarmState.get(name);
      if (!st) throw new Error(`unknown alarm "${name}"`);
      st.state = 'normal';
      st.shelved_until = null;
      logEvent(name, 'unshelved', st.value);
      evaluateAlarms();
      ev.emit('alarms', alarmList());
      return delay({ ok: true }, 40);
    },
    journal: (limit = 200) => delay({ events: journal.slice(0, limit) }),
    diagnostics: () => delay({ drivers: [{ name: 'flow-meters (simulated)', connected: true, requests: Math.round((now() - startedAt) / 500), errors: 0, last_error: null }], uptime_s: Math.round((now() - startedAt) / 1000) }),
  };

  return { api, client, sample, valueAt };
}
