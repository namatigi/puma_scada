// Optional live values while designing. The studio only opens the WebSocket
// when a runtime answers /api/diagnostics on this origin, so the dev server
// (no runtime) produces no connection errors; it re-checks every 30 s.

import { createWsClient } from '../shared/client.js';

export function createLive({ api, onValues, onStatus, onReload, onDiagnostics }) {
  let client = null;
  let enabled = true;
  let alive = false;
  let status = 'offline';
  let timer = null;
  let tags = new Set();
  const values = new Map();

  const setStatus = (s) => {
    status = s;
    onStatus?.({ status, alive });
  };

  async function probe() {
    const ok = await api.runtimeAlive();
    if (ok !== alive) {
      alive = ok;
      setStatus(ok ? 'runtime found' : 'offline');
    }
    if (ok) {
      try { onDiagnostics?.(await api.diagnostics()); } catch { /* optional */ }
    }
    if (ok && enabled && !client) connect();
    if (!ok && client) disconnect();
  }

  function connect() {
    client = createWsClient();
    client.on('status', (s) => setStatus(s === 'open' ? 'live' : s));
    client.on('values', (changes) => {
      for (const [k, v] of changes instanceof Map ? changes : Object.entries(changes)) values.set(k, v);
      onValues?.(changes);
    });
    client.on('reload', () => onReload?.());
    client.setTags(tags);
    client.connect();
  }

  function disconnect() {
    try { client?.close?.(); client?.disconnect?.(); } catch { /* already closed */ }
    client = null;
    setStatus(alive ? 'paused' : 'offline');
  }

  function start() {
    probe();
    timer = setInterval(probe, 30000);
  }

  return {
    start,
    get values() { return values; },
    get status() { return status; },
    get alive() { return alive; },
    get enabled() { return enabled; },
    setEnabled(on) {
      enabled = on;
      if (!on) disconnect();
      else if (alive && !client) connect();
      else probe();
    },
    setTags(next) {
      tags = new Set(next);
      client?.setTags(tags);
    },
    stop() { clearInterval(timer); disconnect(); },
  };
}
