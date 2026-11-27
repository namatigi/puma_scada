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

/**
 * @param {object} [opts]
 * @param {() => Promise<boolean>} [opts.onSignedOut] called when a request is
 *   refused because the session ended (401); resolve true once signed in again
 *   and the request is sent once more.
 */
export function createStudioApi(base = '', { timeoutMs = 20000, fetchImpl = (...a) => fetch(...a), onSignedOut } = {}) {
  const api = {};
  // `version` is sent as If-Match: the server refuses the save with 409
  // ({conflict:true, version}) when the file changed since that version.
  async function request(method, path, body, version, retried = false) {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), timeoutMs);
    let res;
    try {
      res = await fetchImpl(`${base}${path}`, {
        method,
        headers: {
          ...(body === undefined ? {} : { 'content-type': 'application/json' }),
          ...(version ? { 'if-match': `"${version}"` } : {}),
        },
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
    const handler = api.onSignedOut ?? onSignedOut;
    if (res.status === 401 && data?.auth === 'required' && !retried && handler && !path.startsWith('/api/studio/session')) {
      if (await handler()) return request(method, path, body, version, true);
    }
    if (!res.ok || data?.ok === false) throw new StudioApiError(data?.error ?? `${method} ${path}: HTTP ${res.status}`, res.status, data);
    return data;
  }
  const enc = encodeURIComponent;
  return Object.assign(api, {
    // Sign-in (docs/PROTOCOL.md, "Studio users").
    session: () => request('GET', '/api/studio/session'),
    signIn: (name, password) => request('POST', '/api/studio/session', { name, password }),
    signOut: () => request('DELETE', '/api/studio/session'),
    setup: (name, password) => request('POST', '/api/studio/setup', { name, password }),
    changePassword: (current, next) => request('PUT', '/api/studio/password', { current, new: next }),
    users: () => request('GET', '/api/studio/users'),
    /** Creates a user ({role, password}) or changes their role and/or password. */
    saveUser: (name, change) => request('PUT', `/api/studio/users/${enc(name)}`, change),
    deleteUser: (name) => request('DELETE', `/api/studio/users/${enc(name)}`),

    project: () => request('GET', '/api/studio/project'),
    saveScreen: (screen, version) => request('PUT', `/api/studio/screens/${enc(screen.name)}`, screen, version),
    deleteScreen: (name, version) => request('DELETE', `/api/studio/screens/${enc(name)}`, undefined, version),
    saveTags: (tags, version) => request('PUT', '/api/studio/tags', tags, version),
    saveAlarms: (alarms, version) => request('PUT', '/api/studio/alarms', alarms, version),
    saveMeta: (meta, version) => request('PUT', '/api/studio/meta', meta, version),
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
  });
}
