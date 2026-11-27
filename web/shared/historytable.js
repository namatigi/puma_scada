// History table widget (GET /api/history/table): time presets, search,
// sortable columns, CSV export and a Refresh bar at the bottom, laid out like
// Movicon's Historical Viewer.
//
//   const t = createHistoryTable(host, { api, project, source: 'FM01' });
//   t.refresh(); t.destroy();

import { h } from './svg.js';
import { formatDateTime, formatValue } from './format.js';
import { PRESETS, unitForTag } from './trend.js';

/** Column definitions: key in the response, header, how to render, sort key. */
const COLUMNS = [
  { key: 'tag', title: 'Tag Name' },
  { key: 'value', title: 'Value', numeric: true },
  { key: 'unit', title: 'Unit' },
  { key: 'description', title: 'Description' },
  { key: 'time', title: 'Record Date Time', numeric: true },
  { key: 'quality', title: 'Quality' },
];

/** Converts the API's {columns, rows} into objects keyed by column name. */
export function rowsToObjects(res) {
  const cols = res?.columns ?? [];
  return (res?.rows ?? []).map((r) => Object.fromEntries(cols.map((c, i) => [c, r[i]])));
}

/** Stable sort of row objects by key; numbers numerically, everything else as text. */
export function sortRows(rows, key, dir = 'desc') {
  const k = dir === 'asc' ? 1 : -1;
  return rows.map((r, i) => [r, i]).sort(([a, ia], [b, ib]) => {
    const va = a[key];
    const vb = b[key];
    let c;
    if (typeof va === 'number' && typeof vb === 'number') c = va - vb;
    else c = String(va ?? '').localeCompare(String(vb ?? ''), undefined, { numeric: true });
    return c ? c * k : ia - ib;
  }).map(([r]) => r);
}

/** CSV text (RFC 4180 quoting) for rows/columns. */
export function toCsv(rows, columns) {
  const q = (v) => {
    const t = v === null || v === undefined ? '' : String(v);
    return /[",\n\r]/.test(t) ? `"${t.replace(/"/g, '""')}"` : t;
  };
  return [columns.map((c) => q(c.title)).join(','), ...rows.map((r) => columns.map((c) => q(c.csv ? c.csv(r) : r[c.key])).join(','))].join('\r\n');
}

/**
 * @param {HTMLElement} host
 * @param {{api:object, project?:object, source:string, limit?:number}} opts
 */
export function createHistoryTable(host, opts) {
  const { api, project, source } = opts;
  const tagDefs = new Map((project?.tags ?? []).map((t) => [t.name, t]));
  let limit = opts.limit ?? 500;
  let rangeMs = 3600e3;
  let rows = [];
  let sortKey = 'time';
  let sortDir = 'desc';
  let filter = '';
  let seq = 0;
  let lastLoaded = null;

  host.classList.add('mc-htable');
  const presetBtns = PRESETS.map(([name, ms]) => h('button', {
    type: 'button', class: `mc-tbtn${ms === rangeMs ? ' mc-on' : ''}`, text: name,
    onclick: () => {
      rangeMs = ms;
      presetBtns.forEach((b) => b.classList.toggle('mc-on', b.textContent === name));
      refresh();
    },
  }));
  const search = h('input', { type: 'search', class: 'mc-tinput mc-search', placeholder: 'Search', 'aria-label': 'Search rows' });
  search.addEventListener('input', () => {
    filter = search.value.trim().toLowerCase();
    renderBody();
  });
  const limitInput = h('input', { type: 'number', class: 'mc-tinput', min: 10, max: 10000, step: 10, value: limit, 'aria-label': 'Max records' });
  limitInput.addEventListener('change', () => {
    const n = Number(limitInput.value);
    limit = Number.isFinite(n) ? Math.max(10, Math.min(10000, Math.round(n))) : 500;
    limitInput.value = limit;
    refresh();
  });
  const head = h('thead');
  const tbody = h('tbody');
  const table = h('table', { class: 'mc-table' }, head, tbody);
  const scroller = h('div', { class: 'mc-table-scroll' }, table);
  const status = h('span', { class: 'mc-htable-status' });
  const refreshBtn = h('button', { type: 'button', class: 'mc-refresh', onclick: () => refresh() }, h('span', { class: 'mc-refresh-icon', 'aria-hidden': 'true', text: '⟳' }), 'Refresh');
  const csvBtn = h('button', { type: 'button', class: 'mc-tbtn', text: 'CSV', title: 'Export the visible rows as CSV', onclick: exportCsv });
  host.append(
    h('div', { class: 'mc-htable-toolbar' },
      h('div', { class: 'mc-tgroup' }, presetBtns),
      h('label', { class: 'mc-tgroup mc-tlabel' }, 'Max records', limitInput),
      h('span', { class: 'mc-htable-source', text: `Source: ${source}` }),
      search),
    scroller,
    h('div', { class: 'mc-refresh-bar' }, status, refreshBtn, csvBtn),
  );

  const columns = COLUMNS.map((c) => ({
    ...c,
    csv: c.key === 'time' ? (r) => formatDateTime(r.time, { ms: true }) : undefined,
  }));

  function renderHead() {
    head.replaceChildren(h('tr', {}, columns.map((c) => {
      const active = c.key === sortKey;
      const th = h('th', {
        scope: 'col', class: `${c.numeric ? 'mc-num' : ''}${active ? ' mc-sorted' : ''}`,
        'aria-sort': active ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none', tabindex: 0,
      }, c.title, h('span', { class: 'mc-sort', text: active ? (sortDir === 'asc' ? '▲' : '▼') : '' }));
      const toggle = () => {
        if (sortKey === c.key) sortDir = sortDir === 'asc' ? 'desc' : 'asc';
        else {
          sortKey = c.key;
          sortDir = c.numeric ? 'desc' : 'asc';
        }
        renderHead();
        renderBody();
      };
      th.addEventListener('click', toggle);
      th.addEventListener('keydown', (e) => { if (e.key === 'Enter') toggle(); });
      return th;
    })));
  }

  function renderBody() {
    const visible = sortRows(rows.filter((r) => !filter || `${r.tag} ${r.description} ${r.value}`.toLowerCase().includes(filter)), sortKey, sortDir);
    const frag = document.createDocumentFragment();
    for (const r of visible) {
      frag.appendChild(h('tr', { class: r.quality && r.quality !== 'good' ? `mc-row-${r.quality}` : '' },
        h('td', { text: r.tag }),
        h('td', { class: 'mc-num', text: formatValue(r.value, { format: '#0.000###' }) }),
        h('td', { text: r.unit }),
        h('td', { text: r.description }),
        h('td', { class: 'mc-num', text: formatDateTime(r.time, { ms: true }) }),
        h('td', { class: `mc-q mc-q-${r.quality}`, text: r.quality ?? '' })));
    }
    if (!visible.length) frag.appendChild(h('tr', { class: 'mc-empty' }, h('td', { colspan: columns.length, text: rows.length ? 'No rows match the search' : 'No records in this period' })));
    tbody.replaceChildren(frag);
    status.textContent = `${visible.length} of ${rows.length} records${lastLoaded ? ` · loaded ${formatDateTime(lastLoaded)}` : ''}`;
  }

  async function refresh() {
    if (!api) return;
    const my = ++seq;
    status.textContent = 'Loading…';
    host.classList.add('mc-loading');
    const to = Date.now();
    const from = rangeMs === null ? 0 : to - rangeMs;
    try {
      const res = await api.historyTable({ source, from, to, limit });
      if (my !== seq) return;
      rows = rowsToObjects(res).map((r) => ({
        ...r,
        description: tagDefs.get(r.tag)?.description ?? '',
        unit: unitForTag(project, r.tag),
      }));
      lastLoaded = Date.now();
      renderBody();
    } catch (err) {
      if (my !== seq) return;
      status.textContent = `Could not load records: ${err.message}`;
    } finally {
      if (my === seq) host.classList.remove('mc-loading');
    }
  }

  function exportCsv() {
    const visible = sortRows(rows.filter((r) => !filter || `${r.tag} ${r.description} ${r.value}`.toLowerCase().includes(filter)), sortKey, sortDir);
    const blob = new Blob([toCsv(visible, columns)], { type: 'text/csv;charset=utf-8' });
    const a = h('a', { href: URL.createObjectURL(blob), download: `${source}-${formatDateTime(Date.now()).replace(/[: ]/g, '-')}.csv` });
    document.body.appendChild(a);
    a.click();
    setTimeout(() => {
      URL.revokeObjectURL(a.href);
      a.remove();
    }, 1000);
  }

  renderHead();
  renderBody();
  refresh();

  return {
    refresh,
    get rows() { return rows; },
    destroy() {
      seq += 1;
      host.replaceChildren();
      host.classList.remove('mc-htable', 'mc-loading');
    },
  };
}
