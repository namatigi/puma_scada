// Tabbed document area. Each document is created once and kept alive while its
// tab is open (canvas zoom and selection survive switching tabs).
// Closing a tab never discards edits: they stay in the working copy until saved.

import { h } from './ui.js';
import { icon } from './icons.js';

export function createDocuments(root, { onActivate, isDirty, startPage }) {
  const tabs = h('div', { class: 'st-doc-tabs', role: 'tablist' });
  const body = h('div', { class: 'st-doc-body' });
  root.append(tabs, body);
  /** @type {Map<string, {id, title, icon, holder:HTMLElement, api:object}>} */
  const docs = new Map();
  const order = [];
  let activeId = null;
  const start = h('div', { class: 'st-doc st-start-wrap', style: { display: 'flex' } });
  body.appendChild(start);

  function renderTabs() {
    tabs.replaceChildren(...order.map((id) => {
      const d = docs.get(id);
      const x = h('span', { class: 'x', title: 'Close (Ctrl+F4)' }, icon('close'));
      const t = h('button', {
        class: `st-doc-tab${id === activeId ? ' active' : ''}${isDirty(d) ? ' dirty' : ''}`, type: 'button', role: 'tab',
        title: d.tooltip ?? d.title, 'aria-selected': String(id === activeId),
      }, icon(d.icon), h('span', { class: 't', text: d.title }), x);
      t.addEventListener('click', (e) => { if (e.target.closest('.x')) close(id); else activate(id); });
      t.addEventListener('auxclick', (e) => { if (e.button === 1) close(id); });
      return t;
    }));
    start.hidden = order.length > 0;
    if (!order.length) start.replaceChildren(startPage());
  }

  function activate(id) {
    const d = docs.get(id);
    if (!d) return;
    activeId = id;
    for (const x of docs.values()) x.holder.hidden = x.id !== id;
    renderTabs();
    onActivate(d);
    requestAnimationFrame(() => { d.api.onShown?.(); d.api.focus?.(); });
  }

  function open(id, { title, icon: ic, tooltip, create }) {
    if (!docs.has(id)) {
      const holder = h('div', { class: 'st-doc-holder', style: { position: 'absolute', inset: '0', display: 'flex', flexDirection: 'column' } });
      body.appendChild(holder);
      const d = { id, title, icon: ic, tooltip, holder, api: null };
      docs.set(id, d);
      order.push(id);
      try {
        d.api = create(holder);
      } catch (err) {
        docs.delete(id);
        order.splice(order.indexOf(id), 1);
        holder.remove();
        throw err;
      }
    }
    activate(id);
    return docs.get(id).api;
  }

  function close(id) {
    const d = docs.get(id);
    if (!d) return;
    d.api.destroy?.();
    d.holder.remove();
    docs.delete(id);
    const i = order.indexOf(id);
    order.splice(i, 1);
    if (activeId === id) {
      activeId = null;
      const next = order[Math.min(i, order.length - 1)];
      if (next) activate(next);
      else { renderTabs(); onActivate(null); }
    } else renderTabs();
  }

  function rename(oldId, newId, title) {
    const d = docs.get(oldId);
    if (!d) return;
    docs.delete(oldId);
    d.id = newId;
    d.title = title;
    docs.set(newId, d);
    order[order.indexOf(oldId)] = newId;
    if (activeId === oldId) activeId = newId;
    renderTabs();
  }

  renderTabs();
  return {
    open,
    close,
    rename,
    activate,
    has: (id) => docs.has(id),
    get: (id) => docs.get(id)?.api ?? null,
    active: () => (activeId ? docs.get(activeId) : null),
    all: () => order.map((id) => docs.get(id)),
    renderTabs,
    cycle(dir) {
      if (order.length < 2) return;
      const i = order.indexOf(activeId);
      activate(order[(i + dir + order.length) % order.length]);
    },
  };
}
