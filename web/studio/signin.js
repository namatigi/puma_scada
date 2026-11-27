// Sign-in for the studio: the page shown before the project opens, the
// first-administrator setup on a new installation, and the dialog shown when a
// session ends while someone is working (their unsaved edits stay in memory).

import { h } from './ui.js';

const field = (label, attrs) => {
  const input = h('input', { spellcheck: 'false', autocapitalize: 'off', ...attrs });
  return [h('label', {}, h('span', { text: label }), input), input];
};

/**
 * Shows the sign-in card until someone is signed in.
 * @param {object} api the studio API client
 * @param {{setup?:boolean, can_setup?:boolean}} session GET /api/studio/session
 * @param {{modal?:boolean, name?:string, reason?:string}} [opts] modal: over the
 *   open studio (session ended) instead of the start page
 * @returns {Promise<{name:string, role:string}>}
 */
export function signIn(api, session, { modal = false, name = '', reason = '' } = {}) {
  return new Promise((resolve) => {
    const back = h('div', { class: `st-signin-back${modal ? ' modal' : ''}` });
    const card = h('div', { class: 'st-signin', role: 'dialog', 'aria-modal': 'true' });
    back.appendChild(card);
    document.body.appendChild(back);

    const head = (title, text) => [
      h('div', { class: 'st-signin-brand' }, h('div', { class: 'st-logo st-logo-big', 'aria-hidden': 'true' }), h('div', {}, h('div', { class: 'st-signin-product', text: 'Moviclone Studio' }), h('h1', { text: title }))),
      text ? h('p', { class: 'st-signin-text', text }) : null,
    ].filter(Boolean);
    const done = (user) => { back.remove(); resolve(user); };

    if (session.setup && !session.can_setup) {
      const retry = h('button', { class: 'st-btn primary', type: 'button', text: 'Check again' });
      retry.addEventListener('click', async () => {
        try {
          const s = await api.session();
          if (s.user) return done(s.user);
          if (!s.setup || s.can_setup) { back.remove(); resolve(signIn(api, s, { modal, name, reason })); }
        } catch { /* stays */ }
      });
      card.append(...head('No studio users yet',
        'Create the first administrator on the computer the runtime runs on: open the studio there (http://127.0.0.1:<port>/studio/), or run "movi users add <name> --role admin --data <data folder>". Then sign in here.'),
      h('div', { class: 'st-signin-actions' }, retry));
      retry.focus();
      return;
    }

    const setup = Boolean(session.setup);
    const [nameRow, nameInput] = field('User name', { name: 'username', autocomplete: 'username', value: name, required: true, maxlength: 64 });
    const [pwRow, pwInput] = field('Password', { name: 'password', type: 'password', autocomplete: setup ? 'new-password' : 'current-password', required: true });
    const [againRow, againInput] = setup ? field('Repeat password', { name: 'password2', type: 'password', autocomplete: 'new-password', required: true }) : [null, null];
    const err = h('div', { class: 'st-signin-err', role: 'alert' });
    const ok = h('button', { class: 'st-btn primary', type: 'submit', text: setup ? 'Create administrator' : 'Sign in' });
    const form = h('form', { class: 'st-signin-form' }, nameRow, pwRow, againRow, err, h('div', { class: 'st-signin-actions' }, ok));
    card.append(...head(setup ? 'Create the administrator' : modal ? 'Sign in again' : 'Sign in',
      setup ? 'This studio has no users yet. The first user is an administrator, who can then add engineers (edit and save) and viewers (read only).'
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
