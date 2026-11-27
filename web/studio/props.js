// Properties dock: a categorized / alphabetical property grid. The active
// document supplies the rows (see screen-props.js, tags-doc.js); this module
// only draws editors and reports changes through each row's onChange.
//
// Row: {key, label, cat, type, value, mixed, options, desc, dim, readonly, onChange, placeholder, wide}
// type: text | number | bool | enum | tag | colour | textarea | commands | readonly | custom

import { h, iconButton, tagInput, colourInput } from './ui.js';
import { icon } from './icons.js';

export const CATEGORIES = ['General', 'Tag', 'Style', 'Fonts', 'Layout', 'Visibility', 'User Access', 'Animations', 'Commands'];

export function createPropertiesPanel(root, { getTags }) {
  let target = null; // {title, subtitle, rows}
  let alpha = false;
  let filter = '';
  let activeKey = null;
  let selectedKey = null;
  const collapsed = new Set(loadCollapsed());

  const search = h('input', { class: 'st-search', placeholder: 'Search properties', spellcheck: 'false' });
  const catBtn = iconButton('categorized', 'Categorized', () => { alpha = false; sync(); render(); });
  const alphaBtn = iconButton('alpha', 'Alphabetical', () => { alpha = true; sync(); render(); });
  const titleEl = h('div', { class: 'st-props-target' });
  const head = h('div', { class: 'st-props-head' }, titleEl, h('div', { class: 'st-props-tools' }, catBtn, alphaBtn, search));
  const body = h('div', { class: 'st-pane-body st-pgrid', role: 'grid' });
  const desc = h('div', { class: 'st-pdesc' });
  root.append(h('div', { class: 'st-pane-title' }, h('span', { class: 'grow', text: 'Properties' })), head, body, desc);

  search.addEventListener('input', () => { filter = search.value.trim().toLowerCase(); render(); });
  body.addEventListener('focusin', (e) => {
    const row = e.target.closest('.st-prow');
    if (row) { activeKey = row.dataset.key; select(row.dataset.key); }
  });
  body.addEventListener('focusout', () => { setTimeout(() => { if (!body.contains(document.activeElement)) activeKey = null; }, 0); });

  function sync() {
    catBtn.classList.toggle('on', !alpha);
    alphaBtn.classList.toggle('on', alpha);
  }

  function select(key) {
    selectedKey = key;
    body.querySelectorAll('.st-prow').forEach((r) => r.classList.toggle('selected', r.dataset.key === key));
    const row = target?.rows.find((r) => r.key === key);
    desc.replaceChildren(...(row ? [h('b', { text: row.label }), document.createTextNode(row.desc ?? '')] : []));
  }

  function render() {
    titleEl.replaceChildren();
    if (!target) {
      body.replaceChildren(h('div', { class: 'st-empty', text: 'Nothing selected.' }));
      desc.replaceChildren();
      return;
    }
    titleEl.append(h('b', { text: target.title ?? '' }), h('span', { text: target.subtitle ?? '' }));
    const rows = target.rows.filter((r) => !filter || r.label.toLowerCase().includes(filter) || (r.key ?? '').toLowerCase().includes(filter));
    const out = [];
    if (alpha) {
      [...rows].sort((a, b) => a.label.localeCompare(b.label)).forEach((r) => out.push(rowEl(r)));
    } else {
      const cats = [...new Set([...CATEGORIES, ...rows.map((r) => r.cat)])];
      for (const cat of cats) {
        const inCat = rows.filter((r) => r.cat === cat);
        if (!inCat.length) continue;
        const open = filter ? true : !collapsed.has(cat);
        const header = h('div', { class: `st-cat${open ? ' open' : ''}`, role: 'row', 'aria-expanded': String(open) },
          h('span', { class: 'twisty' }, icon('chevron')), h('span', { text: cat }), h('span', { class: 'n', text: String(inCat.length) }));
        header.addEventListener('click', () => {
          if (collapsed.has(cat)) collapsed.delete(cat); else collapsed.add(cat);
          saveCollapsed([...collapsed]);
          render();
        });
        out.push(header);
        if (open) inCat.forEach((r) => out.push(rowEl(r)));
      }
    }
    body.replaceChildren(...(out.length ? out : [h('div', { class: 'st-empty', text: 'No property matches.' })]));
    if (selectedKey) select(selectedKey);
    if (activeKey) {
      const again = body.querySelector(`.st-prow[data-key="${CSS.escape(activeKey)}"] input, .st-prow[data-key="${CSS.escape(activeKey)}"] select, .st-prow[data-key="${CSS.escape(activeKey)}"] textarea`);
      again?.focus({ preventScroll: true });
    }
  }

  function rowEl(r) {
    const editor = editorFor(r);
    const cls = `st-prow${r.dim ? ' dim' : ''}${r.mixed ? ' mixed' : ''}${r.key === selectedKey ? ' selected' : ''}`;
    if (r.wide) {
      return h('div', { class: cls, 'data-key': r.key, style: { gridTemplateColumns: '1fr' } },
        h('div', { class: 'st-pname', text: r.label, title: r.desc ?? '' }), editor);
    }
    return h('div', { class: cls, 'data-key': r.key, role: 'row' },
      h('div', { class: 'st-pname', text: r.label, title: r.desc ?? r.label }),
      h('div', { class: 'st-pval' }, editor));
  }

  function editorFor(r) {
    const change = (v) => r.onChange?.(v);
    const ph = r.mixed ? '(mixed)' : r.placeholder ?? '';
    if (r.dim || r.type === 'readonly' || r.readonly) {
      return h('div', { class: 'ro', title: r.dim ? (r.desc ?? '') : String(r.value ?? ''), text: r.mixed ? '(mixed)' : String(r.value ?? '') });
    }
    switch (r.type) {
      case 'bool': {
        const cb = h('input', { type: 'checkbox' });
        cb.checked = Boolean(r.value);
        cb.indeterminate = Boolean(r.mixed);
        cb.addEventListener('change', () => change(cb.checked));
        return cb;
      }
      case 'enum': {
        const sel = h('select', {}, ...(r.mixed ? [h('option', { value: '\u0000', text: '(mixed)' })] : []),
          ...r.options.map((o) => h('option', { value: o.value ?? o, text: o.label ?? (o === '' ? '(none)' : o) })));
        sel.value = r.mixed ? '\u0000' : String(r.value ?? '');
        if (!r.mixed && sel.value !== String(r.value ?? '') && r.value) {
          sel.appendChild(h('option', { value: r.value, text: `${r.value} (unknown)` }));
          sel.value = r.value;
        }
        sel.addEventListener('change', () => { if (sel.value !== '\u0000') change(sel.value); });
        return sel;
      }
      case 'number': {
        const input = h('input', { type: 'number', step: r.step ?? 'any', placeholder: ph });
        if (!r.mixed && r.value !== undefined && r.value !== null) input.value = String(r.value);
        input.addEventListener('change', () => {
          if (input.value === '' && r.optional) { change(undefined); return; }
          const v = Number(input.value);
          const bad = !Number.isFinite(v) || (r.min !== undefined && v < r.min) || (r.integer && !Number.isInteger(v));
          input.classList.toggle('invalid', bad);
          if (!bad) change(v);
        });
        input.addEventListener('keydown', (e) => { if (e.key === 'Enter') input.blur(); });
        return input;
      }
      case 'tag':
        return tagInput({ value: r.mixed ? '' : r.value ?? '', tags: getTags, onChange: change, placeholder: r.mixed ? '(mixed)' : '(no tag)', filter: r.filter });
      case 'colour':
        return colourInput({ value: r.mixed ? '' : r.value ?? '', onChange: change, placeholder: r.mixed ? '(mixed)' : r.placeholder ?? 'default' });
      case 'textarea': {
        const ta = h('textarea', { spellcheck: 'false', placeholder: ph });
        ta.value = r.mixed ? '' : r.value ?? '';
        ta.addEventListener('change', () => {
          const res = change(ta.value);
          ta.classList.toggle('invalid', res === false);
        });
        return ta;
      }
      case 'custom':
        return r.render();
      default: {
        const input = h('input', { type: 'text', placeholder: ph, spellcheck: 'false', list: r.datalist ? datalistId(r.datalist) : undefined });
        input.value = r.mixed ? '' : r.value ?? '';
        input.addEventListener('change', () => {
          const res = change(input.value);
          input.classList.toggle('invalid', res === false);
        });
        input.addEventListener('keydown', (e) => { if (e.key === 'Enter') input.blur(); if (e.key === 'Escape') { input.value = r.value ?? ''; input.blur(); } });
        return input;
      }
    }
  }

  const lists = new Map();
  function datalistId(values) {
    const key = values.join('\n');
    if (!lists.has(key)) {
      const id = `st-dl-${lists.size}`;
      document.body.appendChild(h('datalist', { id }, values.map((v) => h('option', { value: v }))));
      lists.set(key, id);
    }
    return lists.get(key);
  }

  sync();
  render();
  return {
    /** Shows a new target. Same `id` keeps scroll position and focus. */
    show(next) {
      const keep = target && next && target.id === next.id;
      const scroll = body.scrollTop;
      target = next;
      if (!keep) { selectedKey = null; activeKey = null; }
      render();
      if (keep) body.scrollTop = scroll;
    },
    focusSearch: () => search.focus(),
    get target() { return target; },
  };
}

function loadCollapsed() {
  try {
    return JSON.parse(localStorage.getItem('moviclone.studio.props.collapsed') ?? '["Fonts","Visibility","User Access"]');
  } catch {
    return ['Fonts', 'Visibility', 'User Access'];
  }
}

function saveCollapsed(list) {
  try { localStorage.setItem('moviclone.studio.props.collapsed', JSON.stringify(list)); } catch { /* ignore */ }
}
