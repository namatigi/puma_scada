// Window chrome: menu bar, toolbar and the Ctrl+K command palette. All three
// are views of the command registry, so they always agree on what is enabled.

import { h, showMenu, closeMenus } from './ui.js';
import { icon } from './icons.js';
import { fuzzyScore } from './commands.js';

/** Menu structure: command ids, '-' separators or {label, items} submenus are flattened. */
export const MENUS = [
  ['File', ['file.new-screen', '-', 'file.save', 'file.save-all', 'file.reload', '-', 'run.start', 'run.stop', '-', 'file.export']],
  ['Edit', ['edit.undo', 'edit.redo', '-', 'edit.cut', 'edit.copy', 'edit.paste', 'edit.duplicate', 'edit.delete', '-', 'edit.select-all', 'edit.find']],
  ['View', ['view.explorer', 'view.properties', 'view.toolbox', 'view.symbols', 'view.tag-browser', 'view.runtime', '-', 'view.problems', 'view.source', 'view.log', 'view.commands', 'view.animations', '-', 'view.zoom-in', 'view.zoom-out', 'view.zoom-fit', 'view.zoom-100', '-', 'view.live']],
  ['Options', ['options.grid', 'options.snap', 'options.guides', 'options.grid-size']],
  ['Project', ['project.tags', 'project.alarms', 'project.settings', 'project.data', 'project.xref', '-', 'project.validate', 'file.reload']],
  ['Design', ['format.front', 'format.back', 'format.forward', 'format.backward', '-', 'design.centre-h', 'design.centre-v', '-', 'design.where-used']],
  ['Format', ['format.align-left', 'format.align-center', 'format.align-right', '-', 'format.align-top', 'format.align-middle', 'format.align-bottom', '-', 'format.dist-h', 'format.dist-v', '-', 'format.same-w', 'format.same-h', 'format.same-size']],
  ['Tools', ['tools.palette', 'run.start', 'tools.hmi']],
  ['Help', ['help.shortcuts', 'help.about']],
];

export function createMenuBar(root, commands) {
  let openIndex = -1;
  const tops = MENUS.map(([label, ids], i) => {
    const top = h('div', { class: 'st-menu-top', role: 'menuitem', text: label, 'aria-haspopup': 'true' });
    const open = () => {
      tops.forEach((t) => t.classList.remove('open'));
      top.classList.add('open');
      openIndex = i;
      const r = top.getBoundingClientRect();
      showMenu(items(ids), r.left, r.bottom - 1, { onClose: () => { top.classList.remove('open'); if (openIndex === i) openIndex = -1; } });
    };
    top.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      if (openIndex === i) closeMenus(); else open();
    });
    top.addEventListener('pointerenter', () => { if (openIndex >= 0 && openIndex !== i) open(); });
    top.open = open;
    return top;
  });
  root.replaceChildren(...tops);

  function items(ids) {
    return ids.map((id) => {
      if (id === '-') return '-';
      const c = commands.get(id);
      if (!c) return { label: id, disabled: true };
      return {
        label: c.label, icon: c.icon, kbd: commands.kbd(id), title: c.title,
        disabled: () => !commands.enabled(id), checked: c.checked ? () => commands.checked(id) : undefined,
        run: () => commands.run(id),
      };
    });
  }
  return { open: (i) => tops[i]?.open() };
}

/** Toolbar layout: command ids, '|' separators, or {select} widgets. */
const TOOLBAR = [
  'edit.undo', 'edit.redo', '|', 'file.new-screen', 'file.reload', 'file.save', 'file.save-all', '|', 'run.start', 'run.stop', '|',
  'edit.cut', 'edit.copy', 'edit.paste', 'edit.delete', '|',
  'format.align-left', 'format.align-center', 'format.align-right', 'format.align-top', 'format.align-middle', 'format.align-bottom', '|',
  'format.dist-h', 'format.dist-v', '|', 'format.same-w', 'format.same-h', 'format.same-size', '|', 'format.front', 'format.back', '|',
  'options.grid', 'options.snap', '|', { font: true },
];

export function createToolbar(root, commands) {
  const buttons = [];
  const parts = [h('span', { class: 'st-tb-grip' })];
  for (const item of TOOLBAR) {
    if (item === '|') { parts.push(h('span', { class: 'st-tb-sep' })); continue; }
    if (item.font) {
      const note = 'Text styles are not in the project model yet';
      parts.push(
        h('select', { class: 'st-tb-select', disabled: true, title: note, style: { width: '120px' } }, h('option', { text: 'Segoe UI' })),
        h('select', { class: 'st-tb-select', disabled: true, title: note, style: { width: '52px', marginLeft: '3px' } }, h('option', { text: 'auto' })),
        tbButton({ icon: 'bold', label: 'Bold' }, note), tbButton({ icon: 'italic', label: 'Italic' }, note));
      continue;
    }
    const c = commands.get(item);
    if (!c) continue;
    const b = h('button', { class: 'st-tb', type: 'button', title: `${c.label}${commands.kbd(item) ? ` (${commands.kbd(item)})` : ''}`, 'aria-label': c.label, 'data-cmd': item }, icon(c.icon ?? 'symbol'));
    if (item === 'run.start') b.appendChild(h('span', { class: 'st-tb-label', text: 'Run' }));
    b.addEventListener('click', () => commands.run(item));
    buttons.push([item, b]);
    parts.push(b);
  }
  root.replaceChildren(...parts);
  function tbButton(c, title) {
    const b = h('button', { class: 'st-tb', type: 'button', title, disabled: true }, icon(c.icon));
    return b;
  }
  const refresh = () => {
    for (const [id, b] of buttons) {
      b.disabled = !commands.enabled(id);
      b.classList.toggle('on', commands.checked(id));
      const c = commands.get(id);
      if (id === 'edit.undo' || id === 'edit.redo') b.title = c.dynamicTitle?.() ?? c.label;
    }
  };
  refresh();
  return { refresh };
}

/**
 * Ctrl+K palette over commands plus dynamic entries (screens, tags, alarms).
 * `extra()` returns [{label, group, run, icon, detail}].
 */
export function openPalette(commands, extra = () => [], { initial = '' } = {}) {
  if (document.querySelector('.st-palette')) return;
  const input = h('input', { placeholder: 'Type a command, screen, tag or alarm…  (> commands only, # tags, @ screens)', value: initial, spellcheck: 'false', 'aria-label': 'Quick search' });
  const list = h('div', { class: 'st-palette-list', role: 'listbox' });
  const pal = h('div', { class: 'st-palette', role: 'dialog', 'aria-label': 'Quick Search Commands' }, input, list);
  const back = h('div', { class: 'st-modal-back', style: { paddingTop: '6vh' } }, pal);
  let active = 0;
  let shown = [];
  const entries = () => {
    const cmds = commands.list().map((c) => ({ label: c.label, group: c.group ?? 'Command', icon: c.icon, kbd: commands.kbd(c.id), run: () => commands.run(c.id), disabled: !commands.enabled(c.id), kind: 'command' }));
    return [...cmds, ...extra()];
  };
  const all = entries();
  const render = () => {
    let q = input.value.trim();
    let pool = all;
    if (q.startsWith('>')) { pool = all.filter((e) => e.kind === 'command'); q = q.slice(1).trim(); }
    else if (q.startsWith('#')) { pool = all.filter((e) => e.kind === 'tag'); q = q.slice(1).trim(); }
    else if (q.startsWith('@')) { pool = all.filter((e) => e.kind === 'screen'); q = q.slice(1).trim(); }
    shown = pool.map((e) => ({ e, score: fuzzyScore(`${e.label} ${e.detail ?? ''}`, q) }))
      .filter((x) => x.score >= 0)
      .sort((a, b) => a.score - b.score || (a.e.disabled - b.e.disabled) || a.e.label.localeCompare(b.e.label))
      .slice(0, 60)
      .map((x) => x.e);
    active = Math.min(active, Math.max(0, shown.length - 1));
    list.replaceChildren(...shown.map((e, i) => {
      const row = h('div', { class: `st-pal-item${i === active ? ' active' : ''}${e.disabled ? ' disabled' : ''}`, role: 'option' },
        e.icon ? icon(e.icon) : h('span', { style: { width: '16px' } }), h('span', { class: 'grp', text: e.group }), highlight(e.label, q),
        e.detail ? h('span', { style: { color: 'var(--st-faint)', marginLeft: '6px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }, text: e.detail }) : null,
        h('kbd', { text: e.kbd ?? '' }));
      row.addEventListener('pointerdown', (ev) => { ev.preventDefault(); pick(e); });
      return row;
    }));
    list.children[active]?.scrollIntoView({ block: 'nearest' });
  };
  const close = () => back.remove();
  const pick = (e) => {
    if (!e || e.disabled) return;
    close();
    e.run();
  };
  input.addEventListener('input', () => { active = 0; render(); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { active = Math.min(shown.length - 1, active + 1); render(); e.preventDefault(); }
    else if (e.key === 'ArrowUp') { active = Math.max(0, active - 1); render(); e.preventDefault(); }
    else if (e.key === 'Enter') { e.preventDefault(); pick(shown[active]); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
  });
  back.addEventListener('pointerdown', (e) => { if (e.target === back) close(); });
  document.body.appendChild(back);
  render();
  input.focus();
}

function highlight(text, q) {
  const span = h('span');
  if (!q) { span.textContent = text; return span; }
  const i = text.toLowerCase().indexOf(q.toLowerCase());
  if (i < 0) { span.textContent = text; return span; }
  span.append(text.slice(0, i), h('mark', { text: text.slice(i, i + q.length) }), text.slice(i + q.length));
  return span;
}
