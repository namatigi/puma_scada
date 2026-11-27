// Project settings (editable), driver and data (read-only) and Cross Reference documents.

import { h } from './ui.js';
import { icon } from './icons.js';
import { whereUsed, usageCounts } from './model.js';

function field(label, input, hint) {
  return [h('label', { text: label }), input, hint ? h('div', { class: 'hint', text: hint }) : null];
}

export function createSettingsDoc(root, ctx) {
  const { store } = ctx;
  const body = h('div', { class: 'st-form-doc' });
  const doc = h('div', { class: 'st-doc' }, body);
  root.appendChild(doc);

  const edit = (label, fn) => store.edit('meta', label, fn);
  function input(value, onChange, attrs = {}) {
    const el = h('input', { type: attrs.type ?? 'text', spellcheck: 'false', ...attrs });
    el.value = value ?? '';
    el.addEventListener('change', () => onChange(attrs.type === 'number' ? Number(el.value) : el.value));
    return el;
  }

  function render() {
    const m = store.project.meta;
    const screens = store.project.screens.map((s) => s.name);
    const main = h('select', {}, h('option', { value: '', text: '(none)' }), ...screens.map((n) => h('option', { value: n, text: n })));
    main.value = m.main_screen ?? '';
    main.addEventListener('change', () => edit('Startup screen', (x) => { if (main.value) x.main_screen = main.value; else delete x.main_screen; }));
    const desc = h('textarea', { spellcheck: 'true' });
    desc.value = m.description ?? '';
    desc.addEventListener('change', () => edit('Description', (x) => { if (desc.value.trim()) x.description = desc.value; else delete x.description; }));
    const ms = (k, label, hint) => field(label, input(m.sampling?.[k], (v) => {
      if (!Number.isInteger(v) || v <= 0) { store.log(`${label} must be a positive whole number`, 'warn'); render(); return; }
      edit(label, (x) => { x.sampling = { ...x.sampling, [k]: v }; });
    }, { type: 'number', min: 1, step: 1 }), hint);
    const reports = h('textarea', { spellcheck: 'false' });
    reports.value = (m.reports ?? []).join('\n');
    reports.addEventListener('change', () => edit('Reports', (x) => {
      const list = reports.value.split(/\n+/).map((s) => s.trim()).filter(Boolean);
      if (list.length) x.reports = list; else delete x.reports;
    }));
    body.replaceChildren(...[
      h('h2', { text: 'Project Settings' }),
      h('div', { class: 'st-form' },
        ...field('Project name', input(m.name, (v) => { if (!v.trim()) { store.log('Project name must not be empty', 'warn'); render(); return; } edit('Project name', (x) => { x.name = v.trim(); }); })),
        ...field('Description', desc),
        ...field('Culture', input(m.culture, (v) => edit('Culture', (x) => { x.culture = v.trim() || 'en-us'; }), { list: 'st-cultures' }), 'Number and date format for displays, e.g. en-us, it-it, de-de.'),
        ...field('Startup screen', main)),
      h('h3', { text: 'Sampling' }),
      h('div', { class: 'st-form' },
        ...ms('fast_ms', 'Fast poll (ms)', 'Poll period for tags on the visible screen.'),
        ...ms('slow_ms', 'Slow poll (ms)', 'Poll period for tags nobody is looking at.'),
        ...ms('publish_ms', 'Publish (ms)', 'How often clients receive batched value changes.')),
      h('h3', { text: 'Reports' }),
      h('div', { class: 'st-form' }, ...field('Report layouts', reports, 'One name per line. Layouts themselves are not converted yet.')),
      m.origin ? h('h3', { text: 'Imported from' }) : null,
      m.origin ? h('table', { class: 'st-kv' }, h('tbody', {}, [['Tool', m.origin.tool], ['Version', m.origin.version], ['Project id', m.origin.project_id]].map(([k, v]) => h('tr', {}, h('td', { text: k }), h('td', { text: v }))))) : null,
    ].filter(Boolean));
    if (!document.getElementById('st-cultures')) {
      document.body.appendChild(h('datalist', { id: 'st-cultures' }, ['en-us', 'en-gb', 'it-it', 'de-de', 'fr-fr', 'es-es', 'pt-br', 'nl-nl'].map((c) => h('option', { value: c }))));
    }
  }

  const off = store.on('changed', ({ part }) => { if (part === 'meta' && !doc.contains(document.activeElement)) render(); });
  const off2 = store.on('structure', render);
  render();
  return { kind: 'settings', part: 'meta', el: doc, focus: () => {}, destroy() { off(); off2(); doc.remove(); } };
}

function note(text) {
  return h('div', { class: 'st-readonly-note' }, icon('lock'), h('span', { text }));
}

function table(headers, rows) {
  return h('table', { class: 'st-table', style: { minWidth: '0', marginBottom: '8px' } },
    h('thead', {}, h('tr', {}, headers.map((t) => h('th', { text: t })))),
    h('tbody', {}, rows.map((r) => h('tr', {}, r.map((c) => h('td', { class: 'ro', text: c === undefined || c === null ? '' : String(c) }))))));
}

export function createDriverDoc(root, ctx, name) {
  const { store } = ctx;
  const body = h('div', { class: 'st-form-doc' });
  const doc = h('div', { class: 'st-doc' }, body);
  root.appendChild(doc);
  function render() {
    const d = store.project.drivers.find((x) => x.name === name);
    if (!d) { body.replaceChildren(h('div', { class: 'st-empty', text: `Driver ${name} no longer exists.` })); return; }
    const tags = store.project.tags.filter((t) => t.address?.driver === name);
    body.replaceChildren(
      h('h2', { text: `Driver: ${d.name}` }),
      note(`Read-only here: edit drivers/${d.name}.toml and reload. Driver editing comes in a later version.`),
      h('table', { class: 'st-kv' }, h('tbody', {}, [
        ['Kind', d.kind], ['Description', d.description], ['Host', d.host], ['Port', d.port], ['Poll (ms)', d.poll_ms],
        ['Timeout (ms)', d.timeout_ms], ['Keep connection open', d.keep_open ? 'yes' : 'no'], ['Tags', tags.length],
      ].map(([k, v]) => h('tr', {}, h('td', { text: k }), h('td', { text: v === undefined ? '' : String(v) }))))),
      h('h3', { text: 'Stations' }),
      table(['Name', 'Unit id', 'Addressing', 'Max retries', 'Tags'], (d.station ?? []).map((s) => [s.name, s.unit_id, s.addressing ?? 'one-based', s.max_retries, tags.filter((t) => t.address.station === s.name).length])),
    );
  }
  render();
  const off = store.on('changed', ({ part }) => { if (part === 'tags') render(); });
  return { kind: 'driver', part: null, el: doc, focus: () => {}, destroy() { off(); doc.remove(); } };
}

export function createDataDoc(root, ctx) {
  const { store } = ctx;
  const body = h('div', { class: 'st-form-doc' });
  const doc = h('div', { class: 'st-doc' }, body);
  root.appendChild(doc);
  function render() {
    const d = store.project.data ?? {};
    const tagsFor = (hname) => store.project.tags.filter((t) => t.historian === hname).length;
    body.replaceChildren(
      h('h2', { text: 'Historians, Data Loggers and Databases' }),
      note('Read-only here: edit data.toml and reload. Assign tags to historians in the Tags List.'),
      h('h3', { text: 'Historians' }),
      table(['Name', 'Enabled', 'Database', 'Keep (days)', 'Tags'], (d.historian ?? []).map((x) => [x.name, x.enabled === false ? 'no' : 'yes', x.database, Math.round(x.max_age_s / 86400), tagsFor(x.name)])),
      h('h3', { text: 'Data Loggers' }),
      table(['Name', 'Enabled', 'Database', 'Table', 'Record', 'Keep (days)', 'Columns'], (d.logger ?? []).map((x) => [x.name, x.enabled === false ? 'no' : 'yes', x.database, x.table, typeof x.record === 'string' ? x.record : `every ${x.record?.['every-s']} s`, Math.round(x.max_age_s / 86400), (x.column ?? []).map((c) => c.tag).join(', ')])),
      h('h3', { text: 'Databases' }),
      table(['Name', 'Provider', 'Server', 'Database', 'User', 'Password', 'Integrated auth'], (d.database ?? []).map((x) => [x.name, x.provider, x.server, x.database, x.user, x.password ? `${x.password} (secret reference)` : '', x.integrated_auth ? 'yes' : 'no'])),
    );
  }
  render();
  return { kind: 'data', part: null, el: doc, focus: () => {}, destroy() { doc.remove(); } };
}

export function createXrefDoc(root, ctx) {
  const { store } = ctx;
  let filter = '';
  const search = h('input', { class: 'st-search', placeholder: 'Filter tags or screens', spellcheck: 'false', style: { maxWidth: '360px' } });
  const tools = h('div', { class: 'st-grid-tools' }, search);
  const wrap = h('div', { class: 'st-grid-wrap' });
  const doc = h('div', { class: 'st-doc' }, tools, wrap);
  root.appendChild(doc);
  function render() {
    const counts = usageCounts(store.project);
    const rows = [];
    for (const t of store.project.tags) {
      const uses = whereUsed(store.project, t.name);
      const text = [t.name, t.description, ...uses.map((u) => `${u.screen ?? ''} ${u.id}`)].join(' ').toLowerCase();
      if (filter && !text.includes(filter)) continue;
      if (!uses.length) {
        rows.push(h('tr', {}, h('td', { class: 'ro', text: t.name }), h('td', { class: 'ro', text: t.type }), h('td', { class: 'ro', text: '—' }), h('td', { class: 'ro', text: t.historian ? `historian ${t.historian}` : 'unused' }), h('td', { class: 'ro', text: '' })));
        continue;
      }
      uses.forEach((u, k) => {
        const tr = h('tr', { style: { cursor: 'pointer' }, title: 'Open' },
          h('td', { class: 'ro', text: k ? '' : t.name }), h('td', { class: 'ro', text: k ? '' : t.type }),
          h('td', { class: 'ro', text: u.kind === 'element' ? u.screen : u.kind }), h('td', { class: 'ro', text: u.id }), h('td', { class: 'ro', text: u.role }));
        tr.addEventListener('click', () => ctx.reveal(u));
        rows.push(tr);
      });
    }
    const unused = store.project.tags.filter((t) => !(counts.get(t.name) > 0)).length;
    wrap.replaceChildren(h('table', { class: 'st-table' },
      h('thead', {}, h('tr', {}, ['Tag', 'Type', 'Screen / file', 'Object', 'Used as'].map((x) => h('th', { text: x })))),
      h('tbody', {}, rows)), h('div', { class: 'st-empty', text: `${store.project.tags.length} tags, ${unused} not referenced by screens, alarms or loggers.` }));
  }
  search.addEventListener('input', () => { filter = search.value.trim().toLowerCase(); render(); });
  const off = store.on('changed', render);
  render();
  return { kind: 'xref', part: null, el: doc, focus: () => search.focus(), setFilter(v) { search.value = v; filter = v.toLowerCase(); render(); }, destroy() { off(); doc.remove(); } };
}
