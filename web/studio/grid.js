// Spreadsheet-style editable grid used by the Tags List and Alarms documents.
// Cells commit on change (Enter / Tab / blur); the owner applies the change to
// the store, which re-renders the grid. Focus is restored to the same cell.

import { h } from './ui.js';

/**
 * @param {HTMLElement} host
 * @param {object} o
 * @param {{key:string,label:string,width?:number,type?:string,options?:string[]|(()=>string[]),get:(row)=>any,set?:(row,v)=>void,ro?:boolean,num?:boolean,title?:string}[]} o.columns
 * @param {() => object[]} o.rows current rows (with stable index `_i`)
 * @param {(index:number, col:object, value:any) => void} o.onEdit
 * @param {(indices:number[]) => void} o.onSelect
 * @param {(row:object) => {cls?:string,title?:string}} [o.rowState]
 */
export function createGrid(host, o) {
  const wrap = h('div', { class: 'st-grid-wrap', tabindex: '-1' });
  host.appendChild(wrap);
  let sortKey = null;
  let sortDesc = false;
  let selected = new Set();
  let anchor = null;
  let active = null; // {i, key}

  wrap.addEventListener('input', () => { if (active) active.select = false; });
  wrap.addEventListener('focusin', (e) => {
    const td = e.target.closest('td[data-key]');
    if (td && (!active || active.i !== Number(td.parentElement.dataset.i) || active.key !== td.dataset.key)) active = { i: Number(td.parentElement.dataset.i), key: td.dataset.key };
  });

  function sorted(rows) {
    if (!sortKey) return rows;
    const col = o.columns.find((c) => c.key === sortKey);
    const val = (r) => col.sortValue?.(r) ?? col.get(r) ?? '';
    return [...rows].sort((a, b) => {
      const va = val(a);
      const vb = val(b);
      const c = typeof va === 'number' && typeof vb === 'number' ? va - vb : String(va).localeCompare(String(vb), undefined, { numeric: true });
      return sortDesc ? -c : c;
    });
  }

  function cellEditor(row, col) {
    const v = col.get(row);
    const commit = (value) => o.onEdit(row._i, col, value);
    if (col.ro) return h('span', { text: v === undefined || v === null ? '' : String(v) });
    if (col.type === 'enum') {
      const opts = typeof col.options === 'function' ? col.options(row) : col.options;
      const sel = h('select', { class: 'st-cell', 'aria-label': col.label }, ...opts.map((x) => h('option', { value: x, text: x === '' ? '—' : x })));
      if (v && !opts.includes(v)) sel.appendChild(h('option', { value: v, text: `${v} ?` }));
      sel.value = v ?? '';
      sel.disabled = Boolean(col.disabled?.(row));
      sel.addEventListener('change', () => commit(sel.value));
      return sel;
    }
    if (col.type === 'bool') {
      const cb = h('input', { class: 'st-cell', type: 'checkbox', 'aria-label': col.label });
      cb.checked = Boolean(v);
      cb.addEventListener('change', () => commit(cb.checked));
      return cb;
    }
    const input = h('input', { class: 'st-cell', type: col.num ? 'number' : 'text', step: 'any', spellcheck: 'false', 'aria-label': col.label, placeholder: col.placeholder ?? '' });
    input.value = v === undefined || v === null ? '' : String(v);
    input.disabled = Boolean(col.disabled?.(row));
    if (col.custom) col.custom(input, row);
    input.addEventListener('change', () => {
      const problem = col.validate?.(input.value, row);
      input.classList.toggle('invalid', Boolean(problem));
      input.title = problem ?? '';
      if (problem) return;
      commit(col.num ? (input.value === '' ? null : Number(input.value)) : input.value);
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') { input.value = v ?? ''; input.blur(); }
      if (e.key === 'Enter' || e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const tr = input.closest('tr');
        const next = e.key === 'ArrowUp' ? tr.previousElementSibling : tr.nextElementSibling;
        input.dispatchEvent(new Event('change'));
        const target = next?.querySelector(`td[data-key="${col.key}"] .st-cell`);
        if (target) { active = { i: Number(next.dataset.i), key: col.key }; target.focus(); target.select?.(); }
      }
    });
    return input;
  }

  // Rendering waits a frame so a Tab that triggered the commit has moved focus
  // (and `active`) to the next cell first.
  let pending = false;
  function render() {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => { pending = false; doRender(); });
  }

  function doRender() {
    const rows = sorted(o.rows());
    const head = h('tr', {}, h('th', { text: '' }), ...o.columns.map((c) => {
      const th = h('th', { class: `sortable${sortKey === c.key ? ` sorted${sortDesc ? ' desc' : ''}` : ''}`, text: c.label, title: c.title ?? `Sort by ${c.label}`, style: c.width ? { minWidth: `${c.width}px` } : {} });
      th.addEventListener('click', () => {
        if (sortKey === c.key) { if (sortDesc) { sortKey = null; sortDesc = false; } else sortDesc = true; } else { sortKey = c.key; sortDesc = false; }
        render();
      });
      return th;
    }));
    const body = rows.map((row, n) => {
      const st = o.rowState?.(row) ?? {};
      const rh = h('td', { class: `rowhead ${st.cls ?? ''}`, text: String(n + 1), title: st.title ?? 'Click to select, Shift/Ctrl for several' });
      const tr = h('tr', { class: selected.has(row._i) ? 'selected' : '', 'data-i': String(row._i) }, rh,
        ...o.columns.map((c) => h('td', { 'data-key': c.key, class: `${c.ro ? 'ro' : ''} ${c.num ? 'num-cell' : ''}`.trim(), title: c.cellTitle?.(row) ?? '' }, cellEditor(row, c))));
      tr.addEventListener('pointerdown', (e) => {
        const onHead = e.target === rh;
        if (!onHead && selected.has(row._i) && !e.shiftKey && !e.ctrlKey) return;
        if (e.shiftKey && anchor !== null) {
          const idx = rows.map((r) => r._i);
          const a = idx.indexOf(anchor);
          const b = idx.indexOf(row._i);
          selected = new Set(idx.slice(Math.min(a, b), Math.max(a, b) + 1));
        } else if (e.ctrlKey || e.metaKey) {
          if (selected.has(row._i)) selected.delete(row._i); else selected.add(row._i);
          anchor = row._i;
        } else {
          selected = new Set([row._i]);
          anchor = row._i;
        }
        wrap.querySelectorAll('tr[data-i]').forEach((r) => r.classList.toggle('selected', selected.has(Number(r.dataset.i))));
        o.onSelect([...selected]);
      });
      return tr;
    });
    const hadFocus = wrap.contains(document.activeElement);
    const scroll = wrap.scrollTop;
    const scrollX = wrap.scrollLeft;
    wrap.replaceChildren(h('table', { class: 'st-table' }, h('thead', {}, head), h('tbody', {}, body)));
    wrap.scrollTop = scroll;
    wrap.scrollLeft = scrollX;
    if (active && (hadFocus || !document.activeElement || document.activeElement === document.body)) {
      const cell = wrap.querySelector(`tr[data-i="${active.i}"] td[data-key="${active.key}"] .st-cell`);
      cell?.focus({ preventScroll: true });
      if (active.select) cell?.select?.();
    }
  }

  return {
    render,
    el: wrap,
    selection: () => [...selected],
    select(indices, { scroll = true, flash = false } = {}) {
      selected = new Set(indices);
      anchor = indices[0] ?? null;
      doRender();
      const tr = wrap.querySelector(`tr[data-i="${indices[0]}"]`);
      if (tr && scroll) tr.scrollIntoView({ block: 'center' });
      if (tr && flash) tr.classList.add('flash');
      o.onSelect([...selected]);
    },
    focusCell(i, key) {
      active = { i, key, select: true };
      doRender();
      const cell = wrap.querySelector(`tr[data-i="${i}"] td[data-key="${key}"] .st-cell`);
      cell?.focus();
      cell?.select?.();
    },
    remap(fn) { selected = new Set([...selected].map(fn).filter((x) => x !== null && x >= 0)); },
  };
}
