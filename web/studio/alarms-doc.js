// Alarms document: grid editor for alarms.toml (ISA-18.2 style conditions on tags).

import { h, iconButton, pickTag } from './ui.js';
import { createGrid } from './grid.js';
import { clone, parseItem } from './model.js';

export const CONDITIONS = ['above', 'below', 'equals', 'is-true', 'is-false', 'bad-quality'];
export const SEVERITIES = ['low', 'medium', 'high', 'critical'];

/** Converts an alarm to the condition `when`, keeping or defaulting the limit. */
export function setCondition(a, when) {
  const lim = a.limit ?? a.value ?? 0;
  delete a.limit;
  delete a.value;
  a.when = when;
  if (when === 'above' || when === 'below') a.limit = lim;
  if (when === 'equals') a.value = lim;
  return a;
}

export function createAlarmsDoc(root, ctx) {
  const { store } = ctx;
  let filter = '';
  const alarms = () => store.project.alarms;
  const search = h('input', { class: 'st-search', placeholder: 'Filter alarms', spellcheck: 'false', style: { maxWidth: '320px' } });
  const count = h('span', { class: 'count' });
  const tools = h('div', { class: 'st-grid-tools' },
    iconButton('plus', 'Add an alarm', (e) => addAlarm(e.currentTarget), { label: 'Add alarm' }),
    iconButton('duplicate', 'Duplicate the selected alarms', () => duplicate(), { label: 'Duplicate' }),
    iconButton('delete', 'Delete the selected alarms', () => removeSelected(), { label: 'Delete' }),
    h('span', { class: 'st-tb-sep' }), search, count);
  const split = h('div', { class: 'st-grid-split' });
  const doc = h('div', { class: 'st-doc' }, tools, split);
  root.appendChild(doc);

  const tagNames = () => store.project.tags.map((t) => t.name);
  const analog = (r) => ['above', 'below', 'equals'].includes(r.when);
  const columns = [
    { key: 'name', label: 'Name', width: 170, get: (r) => r.name, validate: (v) => (v.trim() ? null : 'Required') },
    { key: 'tag', label: 'Tag', width: 190, get: (r) => r.tag, custom: (input) => { input.setAttribute('list', tagList()); input.addEventListener('dblclick', async () => { const t = await pickTag(input, store.project.tags, { current: input.value, allowNone: false }); if (t) { input.value = t; input.dispatchEvent(new Event('change')); } }); }, validate: (v) => (tagNames().includes(v.trim()) ? null : 'Unknown tag (double-click to browse)') },
    { key: 'when', label: 'Condition', type: 'enum', options: CONDITIONS, get: (r) => r.when },
    { key: 'limit', label: 'Limit / value', num: true, width: 80, get: (r) => r.limit ?? r.value ?? '', disabled: (r) => !analog(r) },
    { key: 'deadband', label: 'Deadband', num: true, width: 70, get: (r) => r.deadband ?? 0, disabled: (r) => !analog(r), validate: (v) => (Number(v) >= 0 ? null : '≥ 0') },
    { key: 'delay_s', label: 'Delay (s)', num: true, width: 70, get: (r) => r.delay_s ?? 0, validate: (v) => (Number(v) >= 0 ? null : '≥ 0') },
    { key: 'severity', label: 'Severity', type: 'enum', options: SEVERITIES, get: (r) => r.severity ?? 'medium' },
    { key: 'message', label: 'Message', width: 260, get: (r) => r.message ?? '' },
    { key: 'area', label: 'Area', width: 110, get: (r) => r.area ?? '' },
    { key: 'ack_required', label: 'Ack', type: 'bool', get: (r) => r.ack_required !== false, title: 'Operators must acknowledge' },
    { key: 'enabled', label: 'Enabled', type: 'bool', get: (r) => r.enabled !== false },
  ];

  let listId = null;
  function tagList() {
    if (!listId) {
      listId = `st-alarm-tags-${Math.random().toString(36).slice(2, 7)}`;
      document.body.appendChild(h('datalist', { id: listId }));
    }
    document.getElementById(listId).replaceChildren(...tagNames().map((n) => h('option', { value: n })));
    return listId;
  }

  let probs = new Map();
  const grid = createGrid(split, {
    columns,
    rows: () => alarms().map((a, i) => ({ ...a, _i: i })).filter((a) => !filter || [a.name, a.tag, a.message, a.area, a.when, a.severity].some((x) => String(x ?? '').toLowerCase().includes(filter))),
    onEdit: edit,
    onSelect: () => ctx.onSelection?.(),
    rowState: (r) => {
      const list = probs.get(r.name);
      return list ? { cls: list.some((d) => d.severity === 'error') ? 'err' : 'warn', title: list.map((d) => `${d.code}: ${d.message}`).join('\n') } : {};
    },
  });

  function edit(i, col, v) {
    const a0 = alarms()[i];
    store.edit('alarms', `Edit ${col.key} of ${a0.name}`, (list) => {
      const a = list[i];
      const opt = (k, val, def) => { if (val === '' || val === null || val === undefined || val === def) delete a[k]; else a[k] = val; };
      switch (col.key) {
        case 'when': setCondition(a, v); break;
        case 'limit': if (a.when === 'equals') a.value = v ?? 0; else a.limit = v ?? 0; break;
        case 'severity': a.severity = v; break;
        case 'deadband': opt('deadband', v, 0); break;
        case 'delay_s': opt('delay_s', v, 0); break;
        case 'ack_required': opt('ack_required', v, true); break;
        case 'enabled': opt('enabled', v, true); break;
        case 'area': opt('area', String(v).trim(), ''); break;
        case 'name': case 'tag': a[col.key] = String(v).trim(); break;
        default: a[col.key] = v;
      }
    });
  }

  async function addAlarm(anchor) {
    const tag = await pickTag(anchor, store.project.tags, { allowNone: false });
    if (!tag) return;
    const t = store.project.tags.find((x) => x.name === tag);
    const names = new Set(alarms().map((a) => a.name));
    let name = `${tag}_Alarm`;
    for (let n = 2; names.has(name); n += 1) name = `${tag}_Alarm${n}`;
    const a = t?.type === 'bool'
      ? { name, tag, when: 'is-true', severity: 'medium', message: `${t.description ?? tag} active` }
      : { name, tag, when: 'above', limit: 100, severity: 'medium', message: `${t?.description ?? tag} high` };
    store.edit('alarms', `Add alarm ${name}`, (list) => { list.push(a); });
    grid.select([alarms().length - 1]);
    grid.focusCell(alarms().length - 1, 'message');
  }

  function duplicate() {
    const sel = grid.selection();
    if (!sel.length) return;
    store.edit('alarms', 'Duplicate alarms', (list) => {
      const names = new Set(list.map((a) => a.name));
      for (const i of sel) {
        const c = clone(list[i]);
        let n = 2;
        while (names.has(`${c.name}_${n}`)) n += 1;
        c.name = `${c.name}_${n}`;
        names.add(c.name);
        list.push(c);
      }
    });
  }

  function removeSelected() {
    const drop = new Set(grid.selection());
    if (!drop.size) return;
    store.edit('alarms', `Delete ${drop.size} alarm(s)`, (list) => list.filter((_, i) => !drop.has(i)));
    grid.select([]);
  }

  function refresh() {
    probs = new Map();
    for (const d of store.diagnostics) {
      if (d.file !== 'alarms.toml') continue;
      const { name } = parseItem(d.item);
      if (!probs.has(name)) probs.set(name, []);
      probs.get(name).push(d);
    }
    count.textContent = `${alarms().length} alarm(s)${alarms().length ? '' : ' — add one with “Add alarm”'}`;
    grid.render();
  }

  search.addEventListener('input', () => { filter = search.value.trim().toLowerCase(); refresh(); });
  const offs = [store.on('changed', ({ part }) => { if (part === 'alarms' || part === 'tags') refresh(); }), store.on('diagnostics', refresh)];
  refresh();
  return {
    kind: 'alarms',
    part: 'alarms',
    el: doc,
    focus: () => grid.el.focus({ preventScroll: true }),
    hasSelection: () => grid.selection().length > 0,
    deleteSelection: removeSelected,
    duplicate,
    revealAlarm(name) {
      const i = alarms().findIndex((a) => a.name === name);
      if (i >= 0) grid.select([i], { flash: true });
    },
    selectedAlarms: () => grid.selection().map((i) => alarms()[i]).filter(Boolean),
    destroy() { offs.forEach((f) => f()); doc.remove(); },
  };
}
