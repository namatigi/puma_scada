// Tags List document: every tag with its address, inline editing, add /
// duplicate / delete, filter, problem markers and a "where used" pane.

import { h, iconButton, confirmDialog } from './ui.js';
import { createGrid } from './grid.js';
import { whereUsed, usageCounts, renameTagRefs, isIdentifier, clone, parseItem } from './model.js';

export const DATA_TYPES = ['bool', 'i8', 'u8', 'i16', 'u16', 'i32', 'u32', 'i64', 'u64', 'f32', 'f64', 'string'];
export const ACCESS = ['read', 'read-write', 'write'];
export const AREAS = ['coil', 'discrete-input', 'input-register', 'holding-register'];
export const WRITES = ['multiple', 'single', 'mask'];

export function createTagsDoc(root, ctx) {
  const { store } = ctx;
  let filter = '';
  let usage = new Map();
  let showUses = true;
  const tags = () => store.project.tags;

  const search = h('input', { class: 'st-search', placeholder: 'Filter tags (name, description, driver, station, register…)', spellcheck: 'false', style: { maxWidth: '360px' } });
  const count = h('span', { class: 'count' });
  const usesBtn = iconButton('where', 'Where used (show or hide the pane)', () => { showUses = !showUses; side.hidden = !showUses; usesBtn.classList.toggle('on', showUses); renderSide(); }, { label: 'Where used' });
  usesBtn.classList.add('on');
  const tools = h('div', { class: 'st-grid-tools' },
    iconButton('plus', 'Add a tag (Insert)', () => addTag(), { label: 'Add tag' }),
    iconButton('duplicate', 'Duplicate the selected tags', () => duplicate(), { label: 'Duplicate' }),
    iconButton('delete', 'Delete the selected tags (Delete)', () => removeSelected(), { label: 'Delete' }),
    h('span', { class: 'st-tb-sep' }), search, count, h('span', { style: { flex: 1 } }), usesBtn);
  const split = h('div', { class: 'st-grid-split' });
  const side = h('div', { class: 'st-side' });
  const doc = h('div', { class: 'st-doc' }, tools, split);
  root.appendChild(doc);

  const drivers = () => store.project.drivers.map((d) => d.name);
  const stations = (row) => store.project.drivers.find((d) => d.name === row.address?.driver)?.station?.map((s) => s.name) ?? [];
  const historians = () => (store.project.data?.historian ?? []).map((x) => x.name);
  const noAddr = (row) => !row.address;

  const columns = [
    { key: 'name', label: 'Name', width: 190, get: (r) => r.name, validate: (v) => (isIdentifier(v.trim()) ? null : 'Letters, digits and _; must not start with a digit') },
    { key: 'type', label: 'Type', type: 'enum', options: DATA_TYPES, get: (r) => r.type },
    { key: 'access', label: 'Access', type: 'enum', options: ACCESS, get: (r) => r.access ?? 'read' },
    { key: 'driver', label: 'Driver', type: 'enum', options: () => ['', ...drivers()], get: (r) => r.address?.driver ?? '' },
    { key: 'station', label: 'Station', type: 'enum', options: (r) => stations(r), get: (r) => r.address?.station ?? '', disabled: noAddr },
    { key: 'area', label: 'Area', type: 'enum', options: AREAS, get: (r) => r.address?.area ?? '', disabled: noAddr },
    { key: 'register', label: 'Register', num: true, width: 70, get: (r) => r.address?.register ?? '', disabled: noAddr, validate: (v) => (v === '' || (Number.isInteger(Number(v)) && Number(v) >= 0 && Number(v) <= 65535) ? null : '0…65535') },
    { key: 'write', label: 'Write', type: 'enum', options: WRITES, get: (r) => r.address?.write ?? 'multiple', disabled: (r) => noAddr(r) || !['coil', 'holding-register'].includes(r.address?.area), title: 'Write function: multiple (FC15/16), single (FC5/6) or mask (FC22)' },
    { key: 'historian', label: 'Historian', type: 'enum', options: () => ['', ...historians()], get: (r) => r.historian ?? '' },
    { key: 'description', label: 'Description', width: 220, get: (r) => r.description ?? '' },
    { key: 'used', label: 'Used', ro: true, num: true, get: (r) => usage.get(r.name) ?? 0, title: 'References in screens, alarms and data loggers' },
  ];

  const problems = () => {
    const map = new Map();
    for (const d of store.diagnostics) {
      if (d.file !== 'tags.toml') continue;
      const { type, name } = parseItem(d.item);
      if (type !== 'tag') continue;
      if (!map.has(name)) map.set(name, []);
      map.get(name).push(d);
    }
    return map;
  };

  let probs = new Map();
  const grid = createGrid(split, {
    columns,
    rows: () => tags().map((t, i) => ({ ...t, _i: i })).filter((t) => !filter || matches(t)),
    onEdit: (i, col, v) => editCell(i, col.key, v),
    onSelect: () => { renderSide(); ctx.onSelection?.(); },
    rowState: (r) => {
      const list = probs.get(r.name);
      if (!list) return {};
      const err = list.some((d) => d.severity === 'error');
      return { cls: err ? 'err' : 'warn', title: list.map((d) => `${d.code}: ${d.message}`).join('\n') };
    },
  });
  split.appendChild(side);

  function matches(t) {
    const q = filter;
    const a = t.address ?? {};
    return [t.name, t.description, t.type, t.access, a.driver, a.station, a.area, a.register, t.historian].some((x) => x !== undefined && String(x).toLowerCase().includes(q));
  }

  function editCell(i, key, v) {
    const t0 = tags()[i];
    if (!t0) return;
    if (key === 'name') {
      const name = String(v).trim();
      if (name === t0.name) return;
      if (tags().some((t, j) => j !== i && t.name === name)) { store.log(`Tag "${name}" already exists`, 'warn'); grid.render(); return; }
      store.edit('tags', `Rename tag ${t0.name}`, (list) => { list[i].name = name; });
      // References follow the tag (each screen keeps its own undo history).
      const probe = clone(store.project);
      const touched = renameTagRefs(probe, t0.name, name);
      for (const scr of touched.screens) {
        store.edit(`screen:${scr}`, `Rename tag ${t0.name} → ${name}`, (s) => { renameTagRefs({ screens: [s], alarms: [] }, t0.name, name); });
      }
      if (touched.alarms) store.edit('alarms', `Rename tag ${t0.name} → ${name}`, (list) => { renameTagRefs({ screens: [], alarms: list }, t0.name, name); });
      if (touched.screens.length || touched.alarms) store.log(`Renamed tag ${t0.name} → ${name}; updated ${touched.screens.length} screen(s)${touched.alarms ? ' and alarms' : ''}`);
      return;
    }
    store.edit('tags', `Edit ${key} of ${t0.name}`, (list) => {
      const t = list[i];
      const setOpt = (k, val, def) => { if (val === '' || val === null || val === undefined || val === def) delete t[k]; else t[k] = val; };
      if (key === 'type') t.type = v;
      else if (key === 'access') setOpt('access', v, 'read');
      else if (key === 'historian') setOpt('historian', v, '');
      else if (key === 'description') setOpt('description', String(v).trim(), '');
      else if (key === 'driver') {
        if (!v) delete t.address;
        else {
          const drv = store.project.drivers.find((d) => d.name === v);
          const station = drv?.station?.find((s) => s.name === t.address?.station)?.name ?? drv?.station?.[0]?.name ?? '';
          t.address = { kind: 'modbus', driver: v, station, area: t.address?.area ?? (t.type === 'bool' ? 'coil' : 'holding-register'), register: t.address?.register ?? 1, ...(t.address?.write ? { write: t.address.write } : {}) };
        }
      } else if (t.address) {
        if (key === 'register') t.address.register = v === null ? 0 : v;
        else if (key === 'write') { if (v === 'multiple') delete t.address.write; else t.address.write = v; }
        else t.address[key] = v;
        if (key === 'area' && !['coil', 'holding-register'].includes(v)) delete t.address.write;
      }
    });
  }

  function uniqueName(base) {
    const names = new Set(tags().map((t) => t.name));
    if (!names.has(base)) return base;
    let n = 1;
    const stem = base.replace(/_?\d+$/, '');
    while (names.has(`${stem}_${n}`)) n += 1;
    return `${stem}_${n}`;
  }

  function addTag() {
    const sel = grid.selection();
    const template = tags()[sel[0]] ?? tags().at(-1);
    const t = { name: uniqueName('NewTag'), type: template?.type ?? 'f32' };
    if (template?.address) t.address = { ...clone(template.address), register: nextRegister(template.address) };
    store.edit('tags', `Add tag ${t.name}`, (list) => { list.push(t); });
    const i = tags().length - 1;
    filter = '';
    search.value = '';
    grid.select([i]);
    grid.focusCell(i, 'name');
  }

  function nextRegister(addr) {
    const same = tags().filter((t) => t.address?.driver === addr.driver && t.address?.station === addr.station && t.address?.area === addr.area);
    const end = Math.max(0, ...same.map((t) => t.address.register + ({ bool: 1, i8: 1, u8: 1, i16: 1, u16: 1, i32: 2, u32: 2, f32: 2, i64: 4, u64: 4, f64: 4, string: 16 }[t.type] ?? 1)));
    return Math.min(65535, end);
  }

  function duplicate() {
    const sel = grid.selection().sort((a, b) => a - b);
    if (!sel.length) return;
    const copies = sel.map((i) => ({ ...clone(tags()[i]), name: '' }));
    store.edit('tags', `Duplicate ${sel.length} tag(s)`, (list) => {
      const names = new Set(list.map((t) => t.name));
      for (const [k, c] of copies.entries()) {
        let n = 1;
        const stem = tags()[sel[k]].name;
        while (names.has(`${stem}_copy${n > 1 ? n : ''}`)) n += 1;
        c.name = `${stem}_copy${n > 1 ? n : ''}`;
        delete c.legacy_id;
        names.add(c.name);
        list.push(c);
      }
    });
    const start = tags().length - copies.length;
    grid.select(copies.map((_, k) => start + k));
  }

  async function removeSelected() {
    const sel = grid.selection();
    if (!sel.length) return;
    const names = sel.map((i) => tags()[i]?.name).filter(Boolean);
    const used = names.filter((n) => (usage.get(n) ?? 0) > 0);
    if (used.length) {
      const ok = await confirmDialog({ title: 'Delete tags', text: `${used.join(', ')} ${used.length === 1 ? 'is' : 'are'} still used. Delete anyway? References will show up in Problems. (Undo with Ctrl+Z.)`, okText: 'Delete', danger: true });
      if (!ok) return;
    }
    const drop = new Set(sel);
    store.edit('tags', `Delete ${names.length} tag(s)`, (list) => list.filter((_, i) => !drop.has(i)));
    grid.select([]);
  }

  function renderSide() {
    if (!showUses) return;
    const sel = grid.selection();
    const t = tags()[sel[0]];
    if (!t) { side.replaceChildren(h('h3', { text: 'Where used' }), h('div', { class: 'st-empty', text: 'Select a tag to see where it is used.' })); return; }
    const uses = whereUsed(store.project, t.name);
    const tagProblems = probs.get(t.name) ?? [];
    side.replaceChildren(...[h('h3', { text: `Where used: ${t.name}` }),
      uses.length ? h('div', { class: 'st-usage' }, uses.map((u) => {
        const row = h('div', { class: 'st-usage-row', title: 'Open' },
          h('span', { class: 'where', text: u.kind === 'element' ? `${u.screen} / ${u.id}` : `${u.kind} ${u.id}` }), h('span', { class: 'role', text: u.role }));
        row.addEventListener('click', () => ctx.reveal(u));
        return row;
      })) : h('div', { class: 'st-empty', text: 'Not used on any screen, alarm or data logger.' }),
      tagProblems.length ? h('h3', { text: 'Problems' }) : null,
      ...tagProblems.map((d) => h('div', { class: 'st-empty', text: `${d.code}: ${d.message}` }))].filter(Boolean));
  }

  function refresh() {
    usage = usageCounts(store.project);
    probs = problems();
    count.textContent = `${filter ? `${tags().filter(matches).length} of ` : ''}${tags().length} tags`;
    grid.render();
    renderSide();
  }

  search.addEventListener('input', () => { filter = search.value.trim().toLowerCase(); refresh(); });
  doc.addEventListener('keydown', (e) => {
    if (e.target.closest('input, select')) return;
    if (e.key === 'Insert') { e.preventDefault(); addTag(); }
  });

  const offs = [
    store.on('changed', () => refresh()),
    store.on('diagnostics', () => refresh()),
  ];
  refresh();

  return {
    kind: 'tags',
    part: 'tags',
    el: doc,
    focus: () => grid.el.focus({ preventScroll: true }),
    refresh,
    revealTag(name) {
      const i = tags().findIndex((t) => t.name === name);
      if (i < 0) return;
      if (filter && !matches(tags()[i])) { filter = ''; search.value = ''; refresh(); }
      grid.select([i], { flash: true });
    },
    selectedTags: () => grid.selection().map((i) => tags()[i]).filter(Boolean),
    hasSelection: () => grid.selection().length > 0,
    deleteSelection: removeSelected,
    duplicate,
    add: addTag,
    focusSearch: () => search.focus(),
    destroy() { offs.forEach((f) => f()); doc.remove(); },
  };
}
