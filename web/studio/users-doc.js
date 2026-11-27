// Users document (administrators): who may sign in to the HMI and the studio,
// and with which role. Changes are saved at once through /api/auth/users;
// there is no working copy, because user accounts are not part of the project.

import { toast } from '../shared/toast.js';
import { h, formDialog, confirmDialog } from './ui.js';
import { ROLE_INFO, ROLES } from '../shared/signin.js';

export { ROLE_INFO };

/** Asks for a new password twice. Resolves to the password, or null. */
export async function askPassword(title, { current = false, text } = {}) {
  const fields = [
    ...(current ? [{ name: 'current', label: 'Current password', type: 'password' }] : []),
    { name: 'password', label: 'New password', type: 'password' },
    { name: 'again', label: 'Repeat it', type: 'password' },
  ];
  const v = await formDialog({
    title, fields, okText: 'Set password', text,
    validate: (x) => (current && !x.current ? 'Enter your current password'
      : x.password.length < 8 ? 'Use at least 8 characters'
        : x.password !== x.again ? 'The passwords differ' : null),
  });
  return v ? (current ? { current: v.current, password: v.password } : v.password) : null;
}

export function createUsersDoc(root, { api, me }) {
  const add = h('button', { class: 'st-btn primary', type: 'button', text: 'Add user…' });
  const refresh = h('button', { class: 'st-btn', type: 'button', text: 'Refresh' });
  const tools = h('div', { class: 'st-grid-tools' }, add, refresh,
    h('span', { class: 'st-users-note', text: 'Sign-in for the HMI and the studio. Stored in users.toml in the runtime’s data folder, not in the project. Changes apply at once.' }));
  const wrap = h('div', { class: 'st-grid-wrap' });
  const doc = h('div', { class: 'st-doc' }, tools, wrap);
  root.appendChild(doc);
  let users = [];

  async function load() {
    try {
      users = (await api.users()).users;
      render();
    } catch (err) {
      wrap.replaceChildren(h('div', { class: 'st-empty', text: `Cannot list users: ${err.message}` }));
    }
  }

  async function act(fn, done) {
    try {
      await fn();
      if (done) toast(done, { kind: 'ok' });
    } catch (err) {
      toast(err.message, { kind: 'error' });
    }
    await load();
  }

  function render() {
    const admins = users.filter((u) => u.role === 'admin').length;
    const rows = users.map((u) => {
      const self = u.name.toLowerCase() === me().name.toLowerCase();
      const role = h('select', { 'aria-label': `Role of ${u.name}` }, ROLES.map((r) => h('option', { value: r, text: r, title: ROLE_INFO[r] })));
      role.value = u.role;
      role.disabled = u.role === 'admin' && admins === 1;
      role.title = role.disabled ? 'The only administrator; make someone else admin first' : ROLE_INFO[u.role];
      role.addEventListener('change', () => act(() => api.saveUser(u.name, { role: role.value }), `${u.name} is now ${role.value}`));
      const pw = h('button', { class: 'st-btn', type: 'button', text: 'Set password…' });
      pw.addEventListener('click', async () => {
        const p = await askPassword(`Password of ${u.name}`, { text: self ? undefined : `${u.name}'s other studio sessions end; tell them the new password.` });
        if (p) act(() => api.saveUser(u.name, { password: p }), `Password of ${u.name} changed`);
      });
      const del = h('button', { class: 'st-btn danger', type: 'button', text: 'Remove', disabled: self, title: self ? 'You cannot remove yourself' : `Remove ${u.name}` });
      del.addEventListener('click', async () => {
        if (await confirmDialog({ title: 'Remove user', text: `Remove ${u.name}? They are signed out of the studio at once.`, okText: 'Remove', danger: true })) {
          act(() => api.deleteUser(u.name), `${u.name} removed`);
        }
      });
      return h('tr', {},
        h('td', { class: 'ro', text: self ? `${u.name} (you)` : u.name }),
        h('td', {}, role),
        h('td', { class: 'ro st-users-role', text: ROLE_INFO[u.role] }),
        h('td', { class: 'st-users-actions' }, pw, del));
    });
    wrap.replaceChildren(h('table', { class: 'st-table st-users' },
      h('thead', {}, h('tr', {}, ['User', 'Role', 'May', ''].map((x) => h('th', { text: x })))),
      h('tbody', {}, rows)));
  }

  add.addEventListener('click', async () => {
    const v = await formDialog({
      title: 'Add user',
      fields: [
        { name: 'name', label: 'User name' },
        { name: 'role', label: 'Role', options: ROLES.map((r) => ({ value: r, label: `${r} – ${ROLE_INFO[r]}` })), value: 'operator' },
        { name: 'password', label: 'Password', type: 'password' },
        { name: 'again', label: 'Repeat it', type: 'password' },
      ],
      okText: 'Add',
      validate: (x) => (!/^[A-Za-z0-9._-]{1,64}$/.test(x.name) ? 'Letters, digits, ".", "_" and "-" only'
        : users.some((u) => u.name.toLowerCase() === x.name.toLowerCase()) ? `${x.name} already exists`
          : x.password.length < 8 ? 'Use at least 8 characters'
            : x.password !== x.again ? 'The passwords differ' : null),
    });
    if (v) act(() => api.saveUser(v.name, { role: v.role, password: v.password }), `${v.name} added as ${v.role}`);
  });
  refresh.addEventListener('click', load);
  load();
  return { kind: 'users', part: null, el: doc, focus: () => add.focus(), destroy() { doc.remove(); } };
}
