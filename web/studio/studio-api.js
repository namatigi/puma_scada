// REST client for the studio routes (docs/PROTOCOL.md). Unlike shared/api.js it
// keeps the diagnostics of refused saves, which the Problems panel needs.

export class StudioApiError extends Error {
  constructor(message, status, body) {
    super(message);
    this.name = 'StudioApiError';
    this.status = status;
    this.body = body;
  }
}

export function createStudioApi(base = '', { timeoutMs = 20000, fetchImpl = (...a) => fetch(...a) } = {}) {
  async function request(method, path, body) {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), timeoutMs);
    let res;
    try {
      res = await fetchImpl(`${base}${path}`, {
        method,
        headers: body === undefined ? {} : { 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: ctrl.signal,
        cache: 'no-store',
      });
    } catch (err) {
      throw new StudioApiError(err.name === 'AbortError' ? `${method} ${path} timed out` : `${method} ${path}: ${err.message}`, 0);
    } finally {
      clearTimeout(timer);
    }
    const text = await res.text();
    let data = null;
    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        throw new StudioApiError(`${method} ${path}: HTTP ${res.status}${res.ok ? ' (invalid JSON)' : ''}`, res.status);
      }
    }
    if (!res.ok || data?.ok === false) throw new StudioApiError(data?.error ?? `${method} ${path}: HTTP ${res.status}`, res.status, data);
    return data;
  }
  const enc = encodeURIComponent;
  return {
    project: () => request('GET', '/api/studio/project'),
    saveScreen: (screen) => request('PUT', `/api/studio/screens/${enc(screen.name)}`, screen),
    deleteScreen: (name) => request('DELETE', `/api/studio/screens/${enc(name)}`),
    saveTags: (tags) => request('PUT', '/api/studio/tags', tags),
    saveAlarms: (alarms) => request('PUT', '/api/studio/alarms', alarms),
    saveMeta: (meta) => request('PUT', '/api/studio/meta', meta),
    /** Is a runtime (gateway) answering on this origin? */
    async runtimeAlive() {
      try {
        const res = await fetchImpl(`${base}/api/diagnostics`, { cache: 'no-store' });
        return res.ok;
      } catch {
        return false;
      }
    },
    diagnostics: () => request('GET', '/api/diagnostics'),
  };
}
