// Small UI building blocks: dialogs, context menus, the searchable tag picker
// and splitters. Everything is plain DOM so it works offline without a framework.

import { h } from '../shared/svg.js';
import { icon } from './icons.js';

export { h };

/** Toolbar-style icon button. */
export function iconButton(name, title, onClick, { label, cls = '' } = {}) {
  const b = h('button', { class: `st-tb ${cls}`.trim(), type: 'button', title, 'aria-label': title });
  b.appendChild(icon(name));
  if (label) b.appendChild(h('span', { class: 'st-tb-label', text: label }));
  if (onClick) b.addEventListener('click', onClick);
  return b;
}

/** Modal with a form. Resolves to the field values, or null when cancelled. */
export function formDialog({ title, fields, okText = 'OK', validate, text }) {
  return new Promise((resolve) => {
    const inputs = {};
    const err = h('div', { class: 'err' });
    const rows = fields.map((f) => {
      let input;
      if (f.options) {
        input = h('select', { name: f.name }, f.options.map((o) => h('option', { value: o.value ?? o, text: o.label ?? o })));
        input.value = f.value ?? '';
      } else {
        input = h('input', { name: f.name, type: f.type ?? 'text', value: f.value ?? '', placeholder: f.placeholder ?? '', spellcheck: 'false', autocomplete: f.type === 'password' ? 'new-password' : 'off' });
      }
      inputs[f.name] = input;
      return h('label', {}, h('span', { text: f.label }), input);
    });
    const ok = h('button', { class: 'st-btn primary', type: 'submit', text: okText });
    const cancel = h('button', { class: 'st-btn', type: 'button', text: 'Cancel' });
    const form = h('form', {}, text ? h('p', { text }) : null, ...rows, err, h('div', { class: 'actions' }, cancel, ok));
    const back = h('div', { class: 'st-modal-back' }, h('div', { class: 'st-dialog', role: 'dialog', 'aria-label': title }, h('h2', { text: title }), form));
    const close = (v) => { back.remove(); resolve(v); };
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      // Passwords are taken as typed: spaces are part of them.
      const values = Object.fromEntries(Object.entries(inputs).map(([k, i]) => [k, i.type === 'number' ? Number(i.value) : i.type === 'password' ? i.value : i.value.trim()]));
      const problem = validate?.(values);
      if (problem) { err.textContent = problem; return; }
      close(values);
    });
    cancel.addEventListener('click', () => close(null));
    back.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(null); } });
    back.addEventListener('pointerdown', (e) => { if (e.target === back) close(null); });
    document.body.appendChild(back);
    const first = Object.values(inputs)[0];
    (first ?? ok).focus();
    first?.select?.();
  });
}

export function confirmDialog({ title, text, okText = 'OK', danger = false }) {
  return new Promise((resolve) => {
    const ok = h('button', { class: `st-btn primary${danger ? ' danger' : ''}`, type: 'submit', text: okText });
    const cancel = h('button', { class: 'st-btn', type: 'button', text: 'Cancel' });
    const form = h('form', {}, h('p', { text }), h('div', { class: 'actions' }, cancel, ok));
    const back = h('div', { class: 'st-modal-back' }, h('div', { class: 'st-dialog', role: 'alertdialog', 'aria-label': title }, h('h2', { text: title }), form));
    const close = (v) => { back.remove(); resolve(v); };
    form.addEventListener('submit', (e) => { e.preventDefault(); close(true); });
    cancel.addEventListener('click', () => close(false));
    back.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(false); } });
    document.body.appendChild(back);
    ok.focus();
  });
}

let openMenu = null;

export function closeMenus() {
  openMenu?.close();
  openMenu = null;
}

/**
 * Shows a dropdown / context menu at (x, y).
 * items: [{label, icon, kbd, run, disabled, checked}] or '-' separators.
 */
export function showMenu(items, x, y, { onClose } = {}) {
  closeMenus();
  const menu = h('div', { class: 'st-menu', role: 'menu' });
  for (const it of items) {
    if (it === '-') { menu.appendChild(h('div', { class: 'st-menu-sep' })); continue; }
    const disabled = typeof it.disabled === 'function' ? it.disabled() : it.disabled;
    const checked = typeof it.checked === 'function' ? it.checked() : it.checked;
    const row = h('div', { class: `st-menu-item${disabled ? ' disabled' : ''}${checked ? ' checked' : ''}`, role: 'menuitem', title: it.title ?? '' },
      it.icon ? icon(it.icon) : h('span', { class: 'st-check' }),
      h('span', { text: it.label }), h('kbd', { text: it.kbd ?? '' }), h('span'));
    if (!disabled) {
      row.addEventListener('click', (e) => {
        e.stopPropagation();
        closeMenus();
        it.run?.();
      });
    }
    menu.appendChild(row);
  }
  document.body.appendChild(menu);
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(0, Math.min(x, window.innerWidth - r.width - 4))}px`;
  menu.style.top = `${Math.max(0, Math.min(y, window.innerHeight - r.height - 4))}px`;
  const away = (e) => { if (!menu.contains(e.target)) closeMenus(); };
  const esc = (e) => { if (e.key === 'Escape') closeMenus(); };
  setTimeout(() => {
    document.addEventListener('pointerdown', away, true);
    document.addEventListener('keydown', esc, true);
  }, 0);
  openMenu = {
    el: menu,
    close() {
      menu.remove();
      document.removeEventListener('pointerdown', away, true);
      document.removeEventListener('keydown', esc, true);
      onClose?.();
    },
  };
  return openMenu;
}

/**
 * Searchable tag list anchored under `anchor`. Resolves to a tag name, '' for
 * "no tag" or null when dismissed.
 */
export function pickTag(anchor, tags, { current = '', allowNone = true, filter } = {}) {
  return new Promise((resolve) => {
    closeMenus();
    const search = h('input', { class: 'st-search', placeholder: 'Search tags (name, description, type)…', value: '', spellcheck: 'false' });
    const list = h('div', { class: 'st-pop-list', role: 'listbox' });
    const foot = h('div', { class: 'st-pop-foot' });
    const pop = h('div', { class: 'st-pop' }, search, list, foot);
    let active = 0;
    let shown = [];
    const pool = filter ? tags.filter(filter) : tags;
    const render = () => {
      const q = search.value.trim().toLowerCase();
      shown = pool.filter((t) => !q || t.name.toLowerCase().includes(q) || (t.description ?? '').toLowerCase().includes(q) || t.type === q);
      if (allowNone && !q) shown = [{ name: '', type: '', description: 'Remove the tag binding' }, ...shown];
      list.replaceChildren(...shown.slice(0, 300).map((t, i) => {
        const row = h('div', { class: `st-pop-item${i === active ? ' active' : ''}`, role: 'option' },
          h('span', { text: t.name || '(none)' }), h('span', { class: 'ty', text: t.type ? `${t.type} · ${t.access ?? 'read'}` : '' }),
          t.description ? h('small', { text: t.description }) : null);
        row.addEventListener('pointerdown', (e) => { e.preventDefault(); done(t.name); });
        return row;
      }));
      foot.textContent = `${shown.filter((t) => t.name).length} of ${pool.length} tags · Enter to pick, Esc to close`;
      list.children[active]?.scrollIntoView({ block: 'nearest' });
    };
    const done = (v) => { pop.remove(); document.removeEventListener('pointerdown', away, true); resolve(v); };
    const away = (e) => { if (!pop.contains(e.target)) done(null); };
    search.addEventListener('input', () => { active = 0; render(); });
    search.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') { active = Math.min(shown.length - 1, active + 1); render(); e.preventDefault(); }
      else if (e.key === 'ArrowUp') { active = Math.max(0, active - 1); render(); e.preventDefault(); }
      else if (e.key === 'Enter') { e.preventDefault(); if (shown[active]) done(shown[active].name); }
      else if (e.key === 'Escape') { e.stopPropagation(); done(null); }
    });
    document.body.appendChild(pop);
    const r = anchor.getBoundingClientRect();
    const pr = pop.getBoundingClientRect();
    pop.style.left = `${Math.max(4, Math.min(r.left, window.innerWidth - pr.width - 4))}px`;
    pop.style.top = `${r.bottom + pr.height > window.innerHeight ? Math.max(4, r.top - pr.height) : r.bottom}px`;
    const idx = pool.findIndex((t) => t.name === current);
    active = idx >= 0 ? idx + (allowNone ? 1 : 0) : 0;
    render();
    setTimeout(() => document.addEventListener('pointerdown', away, true), 0);
    search.focus();
  });
}

/**
 * Tag input with a "…" button that opens the picker; free typing works too.
 * onChange(name) fires on pick, Enter or blur.
 */
export function tagInput({ value = '', tags, onChange, placeholder = '(no tag)', allowNone = true, filter }) {
  const input = h('input', { type: 'text', value, placeholder, spellcheck: 'false', autocomplete: 'off' });
  const known = () => new Set(tags().map((t) => t.name));
  const mark = () => input.classList.toggle('invalid', Boolean(input.value) && !known().has(input.value));
  const commit = () => { mark(); if (input.value !== value) { value = input.value; onChange(value); } };
  input.addEventListener('change', commit);
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); commit(); } });
  const open = async () => {
    const picked = await pickTag(input, tags(), { current: input.value, allowNone, filter });
    if (picked === null) return;
    input.value = picked;
    commit();
  };
  input.addEventListener('dblclick', open);
  const btn = iconButton('search', 'Browse tags', open);
  mark();
  const wrap = h('div', { class: 'st-picker' }, input, btn);
  wrap.input = input;
  return wrap;
}

/** Colour editor: swatch + text (blank = default). */
export function colourInput({ value = '', onChange, placeholder = 'default' }) {
  const text = h('input', { type: 'text', value, placeholder, spellcheck: 'false' });
  const hex = /^#[0-9a-f]{6}$/i;
  const swatch = h('input', { type: 'color', value: hex.test(value) ? value : '#808080', title: 'Pick a colour' });
  swatch.addEventListener('input', () => { text.value = swatch.value.toUpperCase(); });
  swatch.addEventListener('change', () => { text.value = swatch.value.toUpperCase(); onChange(text.value); });
  text.addEventListener('change', () => {
    if (hex.test(text.value)) swatch.value = text.value;
    onChange(text.value.trim());
  });
  return h('div', { class: 'st-color' }, swatch, text);
}

/** Drag-to-resize splitters (data-resize = CSS var target, data-side = which edge grows). */
export function initSplitters(root = document) {
  const vars = { explorer: '--explorer-w', properties: '--properties-w', 'flyout-right': '--flyout-w', bottom: '--bottom-h' };
  for (const [k, v] of Object.entries(vars)) {
    try {
      const saved = localStorage.getItem(`moviclone.studio.${k}`);
      if (saved) document.documentElement.style.setProperty(v, saved);
    } catch { /* storage may be unavailable */ }
  }
  root.querySelectorAll('.st-splitter').forEach((sp) => {
    sp.addEventListener('pointerdown', (e) => {
      const key = sp.dataset.resize;
      const cssVar = vars[key];
      if (!cssVar) return;
      e.preventDefault();
      sp.setPointerCapture(e.pointerId);
      sp.classList.add('dragging');
      const start = { x: e.clientX, y: e.clientY };
      const cur = parseFloat(getComputedStyle(document.documentElement).getPropertyValue(cssVar)) || 200;
      const move = (ev) => {
        let v;
        if (key === 'bottom') v = cur - (ev.clientY - start.y);
        else if (sp.dataset.side === 'left') v = cur + (ev.clientX - start.x);
        else v = cur - (ev.clientX - start.x);
        const min = key === 'bottom' ? 60 : 150;
        const max = key === 'bottom' ? window.innerHeight * 0.7 : window.innerWidth * 0.45;
        v = Math.max(min, Math.min(max, v));
        document.documentElement.style.setProperty(cssVar, `${Math.round(v)}px`);
      };
      const up = () => {
        sp.classList.remove('dragging');
        sp.removeEventListener('pointermove', move);
        sp.removeEventListener('pointerup', up);
        try {
          localStorage.setItem(`moviclone.studio.${key}`, getComputedStyle(document.documentElement).getPropertyValue(cssVar).trim());
        } catch { /* ignore */ }
        window.dispatchEvent(new Event('resize'));
      };
      sp.addEventListener('pointermove', move);
      sp.addEventListener('pointerup', up);
    });
  });
}

/** Debounce helper. */
export function debounce(fn, ms) {
  let t = null;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}
