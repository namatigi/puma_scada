// Side panels behind the vertical edge tabs: ToolBox and Symbol Library on the
// right, Tag Browser and Runtime Status on the left. Items are dragged onto a
// screen canvas (HTML5 drag and drop) or double-clicked to add at the centre.

import { h } from './ui.js';
import { icon } from './icons.js';
import { s } from '../shared/svg.js';
import { listSymbols, drawSymbol } from '../shared/symbols.js';
import { ELEMENT_KINDS } from './model.js';
import { DRAG_ELEMENT, DRAG_TAG } from './screen-editor.js';
import { formatValue, formatAge } from '../shared/format.js';

const TOOL_VARIANTS = [
  { kind: 'label', title: 'Text', hint: 'label' },
  { kind: 'display', title: 'Display / Edit Box', hint: 'display' },
  { kind: 'button', title: 'Button', hint: 'button' },
  { kind: 'pipeline', title: 'Pipeline (horizontal)', hint: 'pipeline' },
  { kind: 'pipeline', variant: 'vertical', title: 'Pipeline (vertical)', hint: 'pipeline' },
  { kind: 'pipeline', variant: 'elbow', title: 'Pipeline (elbow)', hint: 'pipeline' },
  { kind: 'symbol', title: 'Symbol', hint: 'symbol' },
  { kind: 'trend', title: 'Trend / Data Analysis', hint: 'trend' },
  { kind: 'history-table', title: 'Historical Log Viewer', hint: 'history-table' },
  { kind: 'alarm-list', title: 'Alarm Window', hint: 'alarm-list' },
];

function dragPayload(el, type, data, onAdd) {
  el.setAttribute('draggable', 'true');
  el.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData(type, typeof data === 'string' ? data : JSON.stringify(data));
    e.dataTransfer.setData('text/plain', typeof data === 'string' ? data : JSON.stringify(data));
    e.dataTransfer.effectAllowed = 'copy';
  });
  el.addEventListener('dblclick', () => onAdd(data));
}

function group(title, open, children) {
  const wrap = h('div', { class: 'st-tool-group' });
  const head = h('div', { class: `st-cat${open ? ' open' : ''}` }, h('span', { class: 'twisty' }, icon('chevron')), h('span', { text: title }), h('span', { class: 'n', text: String(children.length) }));
  const body = h('div', { hidden: !open }, children);
  head.addEventListener('click', () => {
    body.hidden = !body.hidden;
    head.classList.toggle('open', !body.hidden);
  });
  wrap.append(head, body);
  return wrap;
}

export function createToolbox(root, { onAdd }) {
  const search = h('input', { class: 'st-search', placeholder: 'Search ToolBox', spellcheck: 'false' });
  const body = h('div', { class: 'st-pane-body' });
  root.append(h('div', { class: 'st-pane-title' }, h('span', { class: 'grow', text: 'ToolBox' })), h('div', { class: 'st-pane-tools' }, search), body);
  const render = () => {
    const q = search.value.trim().toLowerCase();
    const groups = new Map();
    for (const t of TOOL_VARIANTS) {
      if (q && !t.title.toLowerCase().includes(q) && !t.kind.includes(q)) continue;
      const info = ELEMENT_KINDS.find((k) => k.kind === t.kind);
      const item = h('div', { class: 'st-tool', title: `Drag onto a screen (or double-click) to add a ${t.title}` },
        icon(info.icon), h('span', { text: t.title }), h('span', { class: 'hint', text: t.kind }));
      dragPayload(item, DRAG_ELEMENT, { kind: t.kind, variant: t.variant }, onAdd);
      if (!groups.has(info.group)) groups.set(info.group, []);
      groups.get(info.group).push(item);
    }
    body.replaceChildren(...[...groups].map(([g, items]) => group(g, true, items)));
  };
  search.addEventListener('input', render);
  render();
}

export function createSymbolLibrary(root, { onAdd, symbolMap }) {
  const search = h('input', { class: 'st-search', placeholder: 'Search Symbol Library', spellcheck: 'false' });
  const body = h('div', { class: 'st-pane-body' });
  root.append(h('div', { class: 'st-pane-title' }, h('span', { class: 'grow', text: 'Symbol Library' })), h('div', { class: 'st-pane-tools' }, search), body);
  const thumb = (name, params = {}) => {
    const def = listSymbols().find((x) => x.name === name);
    const [w, hh] = def?.size ?? [80, 60];
    const svg = s('svg', { viewBox: `${-w * 0.1} ${-hh * 0.1} ${w * 1.2} ${hh * 1.2}`, preserveAspectRatio: 'xMidYMid meet' });
    try {
      svg.appendChild(drawSymbol(name, { w, h: hh, params, state: def?.states?.[0] ?? 'static' }));
    } catch (err) {
      console.warn(`[studio] symbol ${name} preview failed`, err);
    }
    return svg;
  };
  const render = () => {
    const q = search.value.trim().toLowerCase();
    const byCat = new Map();
    for (const sym of listSymbols()) {
      if (q && !sym.name.includes(q) && !sym.title.toLowerCase().includes(q) && !sym.category.toLowerCase().includes(q)) continue;
      const tile = h('div', { class: 'st-sym', title: `${sym.title}\n${sym.name} · ${sym.size.join(' × ')}${sym.states.length ? `\nstates: ${sym.states.join(', ')}` : ''}\nDrag onto a screen or double-click.` },
        thumb(sym.name), h('span', { text: sym.title }), h('small', { text: sym.name }));
      dragPayload(tile, DRAG_ELEMENT, { kind: 'symbol', symbol: sym.name, size: sym.size }, onAdd);
      if (!byCat.has(sym.category)) byCat.set(sym.category, []);
      byCat.get(sym.category).push(tile);
    }
    // Product-coloured meters as ready-made variants (Petrogas products).
    const products = [['Gasoline', '#FF4D4D'], ['Jet A-1', '#FFFFFF'], ['Gasoil', '#555555'], ['HFO', '#111111']];
    const variants = products
      .filter(([n]) => !q || n.toLowerCase().includes(q) || 'coriolis meter'.includes(q))
      .map(([n, c]) => {
        const ref = `coriolis-meter?product=${encodeURIComponent(c)}`;
        const tile = h('div', { class: 'st-sym', title: `Coriolis meter, ${n} (${c})` }, thumb('coriolis-meter', { product: c }), h('span', { text: `Meter · ${n}` }), h('small', { text: c }));
        dragPayload(tile, DRAG_ELEMENT, { kind: 'symbol', symbol: ref, size: [104, 125] }, onAdd);
        return tile;
      });
    const mapped = Object.entries(symbolMap?.symbols ?? {}).filter(([k]) => !q || k.toLowerCase().includes(q)).map(([k, v]) => {
      const tile = h('div', { class: 'st-sym', title: `Imported ${k} → ${v.symbol}` }, thumb(v.symbol, v.params), h('span', { text: k.replace('movicon:', '') }), h('small', { text: `→ ${v.symbol}` }));
      dragPayload(tile, DRAG_ELEMENT, { kind: 'symbol', symbol: k }, onAdd);
      return tile;
    });
    const sections = [...byCat].map(([cat, tiles]) => group(cat, true, [h('div', { class: 'st-symbols' }, tiles)]));
    if (variants.length) sections.push(group('Product meters', true, [h('div', { class: 'st-symbols' }, variants)]));
    if (mapped.length) sections.push(group('Imported (Movicon names)', false, [h('div', { class: 'st-symbols' }, mapped)]));
    body.replaceChildren(...sections);
  };
  search.addEventListener('input', render);
  render();
}

export function createTagBrowser(root, { store, onAdd, onWhereUsed, getValues }) {
  const search = h('input', { class: 'st-search', placeholder: 'Search tags', spellcheck: 'false' });
  const body = h('div', { class: 'st-pane-body' });
  root.append(h('div', { class: 'st-pane-title' }, h('span', { class: 'grow', text: 'Tag Browser' })), h('div', { class: 'st-pane-tools' }, search), body);
  const render = () => {
    const q = search.value.trim().toLowerCase();
    const values = getValues();
    const tags = (store.project?.tags ?? []).filter((t) => !q || t.name.toLowerCase().includes(q) || (t.description ?? '').toLowerCase().includes(q));
    body.replaceChildren(...tags.map((t) => {
      const v = values.get(t.name);
      const row = h('div', { class: 'st-tool', title: `${t.name} (${t.type}, ${t.access ?? 'read'})${t.description ? `\n${t.description}` : ''}\nDrag onto a screen to add a display, or onto an object to bind it.` },
        icon('tag'), h('span', { text: t.name }), h('span', { class: 'hint', text: v ? formatValue(v.v) : t.type }));
      dragPayload(row, DRAG_TAG, t.name, () => onAdd({ kind: 'display', tag: t.name }));
      row.addEventListener('contextmenu', (e) => { e.preventDefault(); onWhereUsed(t.name); });
      return row;
    }));
    if (!tags.length) body.appendChild(h('div', { class: 'st-empty', text: 'No tag matches.' }));
  };
  search.addEventListener('input', render);
  store.on('loaded', render);
  store.on('changed', ({ part }) => { if (part === 'tags') render(); });
  render();
  return { render };
}

export function createRuntimeStatus(root, { getState }) {
  const body = h('div', { class: 'st-pane-body', style: { padding: '8px' } });
  root.append(h('div', { class: 'st-pane-title' }, h('span', { class: 'grow', text: 'Runtime Status' })), body);
  const render = () => {
    const st = getState();
    const rows = [
      ['Runtime', st.alive ? 'answering' : 'not reachable'],
      ['Live values', st.status],
      ['Tags with values', String(st.values.size)],
      ['Server', location.host],
    ];
    if (st.diag) {
      rows.push(['Uptime', formatAge((st.diag.uptime_s ?? 0) * 1000)]);
      for (const d of st.diag.drivers ?? []) {
        rows.push([`Driver ${d.name}`, `${d.connected ? 'connected' : 'disconnected'} · ${d.requests ?? 0} req · ${d.errors ?? 0} err`]);
        if (d.last_error) rows.push(['  last error', d.last_error]);
      }
    }
    body.replaceChildren(h('table', { class: 'st-kv' }, h('tbody', {}, rows.map(([k, v]) => h('tr', {}, h('td', { text: k }), h('td', { text: v }))))));
    if (!st.alive) body.appendChild(h('p', { class: 'st-empty', text: 'Start moviclone-rt on this host to see live values on the canvas and driver status here. The studio works without it.' }));
  };
  render();
  return { render };
}
