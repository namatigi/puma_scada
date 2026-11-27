// Sign-in for the HMI and the studio (docs/PROTOCOL.md, "Sign-in"): the card
// shown before anything opens, the first-administrator setup on a new
// installation, and the dialog shown when a session ends while someone is
// working (the page underneath keeps its state).

import { h } from './svg.js';

export class AuthApiError extends Error {
  constructor(message, status, body) {
    super(message);
    this.name = 'AuthApiError';
    this.status = status;
    this.body = body;
  }
}

/** The `/api/auth/…` routes. */
export function createAuthApi(base = '', { fetchImpl = (...a) => fetch(...a) } = {}) {
  async function request(method, path, body) {
    let res;
    try {
      res = await fetchImpl(`${base}${path}`, {
        method,
        headers: body === undefined ? {} : { 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        cache: 'no-store',
      });
    } catch (err) {
      throw new AuthApiError(`${method} ${path}: ${err.message}`, 0);
    }
    let data = null;
    try { data = await res.json(); } catch { /* empty or not JSON */ }
    if (!res.ok || data?.ok === false) throw new AuthApiError(data?.error ?? `${method} ${path}: HTTP ${res.status}`, res.status, data);
    return data;
  }
  const enc = encodeURIComponent;
  return {
    session: () => request('GET', '/api/auth/session'),
    signIn: (name, password) => request('POST', '/api/auth/session', { name, password }),
    signOut: () => request('DELETE', '/api/auth/session'),
    setup: (name, password) => request('POST', '/api/auth/setup', { name, password }),
    changePassword: (current, next) => request('PUT', '/api/auth/password', { current, new: next }),
    users: () => request('GET', '/api/auth/users'),
    /** Creates a user ({role, password}) or changes their role and/or password. */
    saveUser: (name, change) => request('PUT', `/api/auth/users/${enc(name)}`, change),
    deleteUser: (name) => request('DELETE', `/api/auth/users/${enc(name)}`),
  };
}

export const ROLE_INFO = {
  viewer: 'Watches the HMI; opens the studio read-only.',
  operator: 'Viewer, and writes values and acknowledges or shelves alarms in the HMI.',
  engineer: 'Operator, and edits and saves the project in the studio.',
  admin: 'Engineer, and manages users.',
};
export const ROLES = Object.keys(ROLE_INFO);

/** Whether `role` includes `need` (viewer < operator < engineer < admin). */
export function roleAtLeast(role, need) {
  return ROLES.indexOf(role) >= ROLES.indexOf(need) && ROLES.includes(need);
}

const field = (label, attrs) => {
  const input = h('input', { spellcheck: 'false', autocapitalize: 'off', ...attrs });
  return [h('label', {}, h('span', { text: label }), input), input];
};

/**
 * Shows the sign-in card until someone signs in.
 * @param {object} api an object with session(), signIn(), setup() (createAuthApi)
 * @param {{setup?:boolean, can_setup?:boolean}} session GET /api/auth/session
 * @param {{modal?:boolean, name?:string, reason?:string, product?:string, allowGuest?:boolean, title?:string}} [opts]
 *   modal: over the open page (session ended) instead of a full page;
 *   allowGuest: offer "Watch without signing in" (resolves null)
 * @returns {Promise<{name:string, role:string}|null>}
 */
export function signIn(api, session, { modal = false, name = '', reason = '', product = 'Moviclone', allowGuest = false, title = '' } = {}) {
  return new Promise((resolve) => {
    const back = h('div', { class: `mc-signin-back${modal ? ' mc-modal' : ''}` });
    const card = h('div', { class: 'mc-signin', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'mc-signin-title' });
    back.appendChild(card);
    document.body.appendChild(back);

    const head = (title, text) => [
      h('div', { class: 'mc-signin-brand' }, h('div', { class: 'mc-signin-mark', 'aria-hidden': 'true' }),
        h('div', {}, h('div', { class: 'mc-signin-product', text: product }), h('h1', { id: 'mc-signin-title', text: title }))),
      text ? h('p', { class: 'mc-signin-text', text }) : null,
    ].filter(Boolean);
    const done = (user) => { back.remove(); resolve(user); };
    const guestBtn = () => (allowGuest
      ? h('button', { class: 'mc-btn', type: 'button', text: 'Watch without signing in', onclick: () => done(null) })
      : null);

    if (session.setup && !session.can_setup) {
      const retry = h('button', { class: 'mc-btn mc-btn-primary', type: 'button', text: 'Check again' });
      retry.addEventListener('click', async () => {
        try {
          const s = await api.session();
          if (s.user) return done(s.user);
          if (!s.setup || s.can_setup) { back.remove(); resolve(signIn(api, s, { modal, name, reason, product, allowGuest, title })); }
        } catch { /* stays */ }
        return undefined;
      });
      card.append(...head('No users yet',
        'Create the first administrator on the computer the runtime runs on: open this page there (http://127.0.0.1:<port>/), or run "movi users add <name> --role admin --data <data folder>". Then sign in here.'),
      h('div', { class: 'mc-signin-actions' }, guestBtn(), retry));
      retry.focus();
      return;
    }

    const setup = Boolean(session.setup);
    const [nameRow, nameInput] = field('User name', { name: 'username', autocomplete: 'username', value: name, required: true, maxlength: 64 });
    const [pwRow, pwInput] = field('Password', { name: 'password', type: 'password', autocomplete: setup ? 'new-password' : 'current-password', required: true });
    const [againRow, againInput] = setup ? field('Repeat password', { name: 'password2', type: 'password', autocomplete: 'new-password', required: true }) : [null, null];
    const err = h('div', { class: 'mc-signin-err', role: 'alert' });
    const ok = h('button', { class: 'mc-btn mc-btn-primary', type: 'submit', text: setup ? 'Create administrator' : 'Sign in' });
    const form = h('form', { class: 'mc-signin-form' }, nameRow, pwRow, againRow, err, h('div', { class: 'mc-signin-actions' }, guestBtn(), ok));
    card.append(...head(setup ? 'Create the administrator' : title || (modal ? 'Sign in again' : 'Sign in'),
      setup ? 'There are no users yet. The first user is an administrator, who can then add operators, engineers and viewers.'
        : reason), form);

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      err.textContent = '';
      const n = nameInput.value.trim();
      const pw = pwInput.value;
      if (setup) {
        if (pw.length < 8) { err.textContent = 'Use at least 8 characters.'; pwInput.focus(); return; }
        if (pw !== againInput.value) { err.textContent = 'The passwords differ.'; againInput.focus(); return; }
      }
      ok.disabled = true;
      try {
        const res = setup ? await api.setup(n, pw) : await api.signIn(n, pw);
        done(res.user);
      } catch (ex) {
        err.textContent = ex.message;
        pwInput.value = '';
        if (againInput) againInput.value = '';
        pwInput.focus();
      } finally {
        ok.disabled = false;
      }
    });
    (name ? pwInput : nameInput).focus();
  });
}

/**
 * "Change your password" card over the page. Resolves true when changed.
 * @param {object} api createAuthApi()
 */
export function changePasswordDialog(api, { product = 'Moviclone' } = {}) {
  return new Promise((resolve) => {
    const back = h('div', { class: 'mc-signin-back mc-modal' });
    const card = h('div', { class: 'mc-signin', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'mc-pw-title' });
    back.appendChild(card);
    document.body.appendChild(back);
    const [curRow, cur] = field('Current password', { type: 'password', autocomplete: 'current-password', required: true });
    const [newRow, next] = field('New password', { type: 'password', autocomplete: 'new-password', required: true });
    const [againRow, again] = field('Repeat it', { type: 'password', autocomplete: 'new-password', required: true });
    const err = h('div', { class: 'mc-signin-err', role: 'alert' });
    const ok = h('button', { class: 'mc-btn mc-btn-primary', type: 'submit', text: 'Change password' });
    const cancel = h('button', { class: 'mc-btn', type: 'button', text: 'Cancel' });
    const form = h('form', { class: 'mc-signin-form' }, curRow, newRow, againRow, err, h('div', { class: 'mc-signin-actions' }, cancel, ok));
    card.append(
      h('div', { class: 'mc-signin-brand' }, h('div', { class: 'mc-signin-mark', 'aria-hidden': 'true' }),
        h('div', {}, h('div', { class: 'mc-signin-product', text: product }), h('h1', { id: 'mc-pw-title', text: 'Change your password' }))),
      h('p', { class: 'mc-signin-text', text: 'Your other sessions (other browsers and tablets) are signed out.' }),
      form);
    const close = (v) => { back.remove(); resolve(v); };
    cancel.addEventListener('click', () => close(false));
    back.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(false); });
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      err.textContent = '';
      if (next.value.length < 8) { err.textContent = 'Use at least 8 characters.'; next.focus(); return; }
      if (next.value !== again.value) { err.textContent = 'The new passwords differ.'; again.focus(); return; }
      ok.disabled = true;
      try {
        await api.changePassword(cur.value, next.value);
        close(true);
      } catch (ex) {
        err.textContent = ex.message;
        ok.disabled = false;
      }
    });
    cur.focus();
  });
}
