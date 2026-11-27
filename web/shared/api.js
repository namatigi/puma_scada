// REST client for the runtime (docs/PROTOCOL.md). The HMI mock backend
// implements the same interface, so widgets only ever see an `api` object:
//
//   bundle(), values(), write(tag, value, mode),
//   history({tags, from, to, points}), historyTable({source, from, to, limit}),
//   alarms(), ack(name), ackAll(), shelve(name, minutes), unshelve(name),
//   journal(limit), diagnostics()

export class ApiError extends Error {
  constructor(message, status) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

/**
 * @param {string} [base] origin + prefix, '' for same origin
 * @param {{timeoutMs?:number, onSignedOut?:() => Promise<boolean>}} [opts]
 *   onSignedOut: called when a request is refused because the session ended
 *   (401); resolve true once signed in again and the request is sent again.
 */
export function createHttpApi(base = '', { timeoutMs = 15000, onSignedOut } = {}) {
  const self = { onSignedOut };
  async function request(method, path, body, retried = false) {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), timeoutMs);
    let res;
    try {
      res = await fetch(`${base}${path}`, {
        method,
        headers: body === undefined ? {} : { 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: ctrl.signal,
        cache: 'no-store',
      });
    } catch (err) {
      throw new ApiError(err.name === 'AbortError' ? `${method} ${path} timed out` : `${method} ${path}: ${err.message}`, 0);
    } finally {
      clearTimeout(timer);
    }
    let data = null;
    const text = await res.text();
    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        // Error pages are often HTML; report the status rather than the parse failure.
        throw new ApiError(res.ok ? `${method} ${path}: invalid JSON` : `${method} ${path}: HTTP ${res.status}`, res.status);
      }
    }
    if (res.status === 401 && data?.auth === 'required' && !retried && self.onSignedOut && (await self.onSignedOut())) {
      return request(method, path, body, true);
    }
    if (!res.ok) throw new ApiError(data?.error ?? `${method} ${path}: HTTP ${res.status}`, res.status);
    if (data && data.ok === false) throw new ApiError(data.error ?? 'request refused', res.status);
    return data;
  }

  const q = (params) => new URLSearchParams(Object.entries(params).filter(([, v]) => v !== undefined && v !== null)).toString();
  const enc = encodeURIComponent;

  return Object.assign(self, {
    kind: 'http',
    bundle: () => request('GET', '/api/bundle'),
    values: () => request('GET', '/api/values'),
    write: (tag, value, mode = 'set') => request('POST', '/api/write', { tag, value, mode }),
    history: ({ tags, from, to, points = 600 }) =>
      request('GET', `/api/history?${q({ tags: tags.join(','), from: Math.round(from), to: Math.round(to), points })}`),
    historyTable: ({ source, from, to, limit = 500 }) =>
      request('GET', `/api/history/table?${q({ source, from: Math.round(from), to: Math.round(to), limit })}`),
    alarms: () => request('GET', '/api/alarms'),
    ack: (name) => request('POST', `/api/alarms/${enc(name)}/ack`),
    ackAll: () => request('POST', '/api/alarms/ack-all'),
    shelve: (name, minutes = 30) => request('POST', `/api/alarms/${enc(name)}/shelve`, { minutes }),
    unshelve: (name) => request('POST', `/api/alarms/${enc(name)}/unshelve`),
    journal: (limit = 200) => request('GET', `/api/alarms/journal?${q({ limit })}`),
    diagnostics: () => request('GET', '/api/diagnostics'),
  });
}
