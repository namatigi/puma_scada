// Project Explorer: Movicon NExT's project tree. Supported nodes open
// documents; nodes Moviclone does not implement yet are shown dimmed with a
// tooltip so the structure stays familiar.

import { h, iconButton, showMenu } from './ui.js';
import { icon } from './icons.js';

const SOON = 'Not supported by Moviclone yet';

export function createExplorer(root, { store, open, actions, isAdmin = () => false }) {
  const expanded = new Set(loadExpanded());
  let selectedId = null;
  let filter = '';
  const search = h('input', { class: 'st-search', placeholder: 'Search Project Explorer', spellcheck: 'false' });
  const tree = h('div', { class: 'st-tree', role: 'tree', tabindex: '0' });
  const body = h('div', { class: 'st-pane-body' }, tree);
  root.append(
    h('div', { class: 'st-pane-title' }, h('span', { class: 'grow', text: 'Project Explorer' }),
      iconButton('new', 'New screen', () => actions.newScreen()), iconButton('minus', 'Collapse all', () => { expanded.clear(); expanded.add('project'); save(); render(); })),
    h('div', { class: 'st-pane-tools' }, search), body);
  search.addEventListener('input', () => { filter = search.value.trim().toLowerCase(); render(); });

  function model() {
    const p = store.project;
    if (!p) return [];
    const diags = store.diagnostics;
    const errorsIn = (file) => diags.filter((d) => d.severity === 'error' && d.file === file).length;
    const screens = [...p.screens].sort((a, b) => a.name.localeCompare(b.name)).map((s) => ({
      id: `screen:${s.name}`, label: s.name, icon: 'screen', doc: `screen:${s.name}`, screen: s.name,
      dirty: store.isDirty(`screen:${s.name}`), main: p.meta.main_screen === s.name, badge: errorsIn(`screens/${s.name}.toml`),
      title: `${s.width} × ${s.height}, ${s.element?.length ?? 0} objects${p.meta.main_screen === s.name ? ' · startup screen' : ''}`,
    }));
    const drivers = p.drivers.map((d) => ({ id: `driver:${d.name}`, label: d.name, icon: 'driver', doc: `driver:${d.name}`, title: `${d.kind} ${d.host ?? ''}`, children: (d.station ?? []).map((st) => ({ id: `station:${d.name}/${st.name}`, label: st.name, icon: 'server', doc: `driver:${d.name}`, title: `unit id ${st.unit_id}` })) }));
    const historians = (p.data.historian ?? []).map((x) => ({ id: `historian:${x.name}`, label: x.name, icon: 'historian', doc: 'data', title: `database ${x.database}` }));
    const loggers = (p.data.logger ?? []).map((x) => ({ id: `logger:${x.name}`, label: x.name, icon: 'loggers', doc: 'data', title: `table ${x.table}` }));
    const dim = (id, label, ic, children) => ({ id, label, icon: ic, dim: true, title: SOON, children });
    return [{
      id: 'project', label: p.meta.name, icon: 'folder', title: p.meta.description ?? 'Project', doc: 'settings', children: [
        dim('client', 'Client Configuration', 'client'),
        dim('logic', 'Logic', 'logic', [dim('logic:basic', 'Basic Script', 'logic'), dim('logic:iec', 'IEC 61131-3', 'logic')]),
        dim('schedulers', 'Schedulers', 'scheduler'),
        { id: 'reports', label: 'Reports', icon: 'folder', count: (p.meta.reports ?? []).length, title: 'Report layouts listed in project.toml (layouts are not converted yet)', doc: 'settings',
          children: (p.meta.reports ?? []).map((r) => ({ id: `report:${r}`, label: r, icon: 'report', doc: 'settings', title: 'Layout not converted yet' })) },
        { id: 'screens', label: 'Screens', icon: 'folder', count: screens.length, children: screens, menu: 'screens' },
        dim('parameters', 'Parameters', 'params'),
        dim('texts', 'Texts', 'texts'),
        { id: 'xref', label: 'Cross Reference', icon: 'xref', doc: 'xref', title: 'Where every tag is used' },
        dim('events', 'Events', 'events'),
        dim('recipes', 'Recipes', 'recipes'),
        {
          id: 'io', label: 'I/O Data Server', icon: 'server', children: [
            dim('structure', 'Data Structure', 'structure'),
            { id: 'tagslist', label: 'Tags List', icon: 'tags', doc: 'tags', count: p.tags.length, dirty: store.isDirty('tags'), badge: errorsIn('tags.toml') },
            { id: 'settings', label: 'Settings', icon: 'settings', doc: 'settings', dirty: store.isDirty('meta'), badge: errorsIn('project.toml') },
            { id: 'drivers', label: 'Drivers', icon: 'folder', count: drivers.length, children: drivers },
            { id: 'alarms', label: 'Alarms', icon: 'alarm', doc: 'alarms', count: p.alarms.length, dirty: store.isDirty('alarms'), badge: errorsIn('alarms.toml') },
            { id: 'historians', label: 'Historians', icon: 'historian', doc: 'data', count: historians.length, children: historians },
            { id: 'loggers', label: 'Data Loggers', icon: 'loggers', doc: 'data', count: loggers.length, children: loggers },
            dim('units', 'Engineering Units', 'units'),
            dim('views', 'Views', 'views'),
            dim('redundancy', 'Redundancy', 'redundancy'),
          ],
        },
        isAdmin()
          ? { id: 'users', label: 'Users', icon: 'users', doc: 'users', title: 'Who may sign in to the studio, and their roles' }
          : { id: 'users', label: 'Users', icon: 'users', dim: true, title: 'Only administrators manage studio users' },
      ],
    }];
  }

  function matches(node) {
    if (!filter) return true;
    return node.label.toLowerCase().includes(filter) || (node.children ?? []).some(matches);
  }

  function render() {
    const rows = [];
    const walk = (nodes, depth) => {
      for (const n of nodes) {
        if (!matches(n)) continue;
        const hasKids = Boolean(n.children?.length);
        const isOpen = filter ? true : expanded.has(n.id);
        const row = h('div', {
          class: `st-node${n.dim ? ' dim' : ''}${n.dirty ? ' dirty' : ''}${n.main ? ' main' : ''}${selectedId === n.id ? ' selected' : ''}${isOpen && hasKids ? ' open' : ''}`,
          role: 'treeitem', 'aria-expanded': hasKids ? String(isOpen) : undefined, 'data-id': n.id, title: n.title ?? '',
          style: { paddingLeft: `${4 + depth * 14}px` },
        },
        h('span', { class: 'twisty' }, hasKids ? icon('chevron') : null),
        icon(n.icon === 'folder' && isOpen && hasKids ? 'folder-open' : n.icon),
        h('span', { class: 'lbl', text: n.label }),
        n.count !== undefined ? h('span', { class: 'count', text: `(${n.count})` }) : null,
        n.badge ? h('span', { class: 'badge', text: String(n.badge), title: `${n.badge} error(s)` }) : null);
        row.addEventListener('click', (e) => {
          selectedId = n.id;
          if (e.target.closest('.twisty') && hasKids) toggle(n.id);
          else render();
        });
        row.addEventListener('dblclick', () => {
          if (n.doc) open(n.doc, { node: n });
          else if (hasKids) toggle(n.id);
        });
        row.addEventListener('keydown', () => {});
        row.addEventListener('contextmenu', (e) => {
          e.preventDefault();
          selectedId = n.id;
          render();
          contextMenu(n, e.clientX, e.clientY);
        });
        rows.push(row);
        if (hasKids && isOpen) walk(n.children, depth + 1);
      }
    };
    walk(model(), 0);
    tree.replaceChildren(...rows);
  }

  function toggle(id) {
    if (expanded.has(id)) expanded.delete(id); else expanded.add(id);
    save();
    render();
  }

  function contextMenu(n, x, y) {
    if (n.screen) {
      showMenu([
        { label: 'Open', icon: 'open', run: () => open(n.doc) },
        { label: 'Rename…', kbd: 'F2', run: () => actions.renameScreen(n.screen) },
        { label: 'Duplicate', icon: 'duplicate', run: () => actions.duplicateScreen(n.screen) },
        { label: 'Set as Startup Screen', disabled: n.main, run: () => actions.setStartup(n.screen) },
        '-',
        { label: 'Delete', icon: 'delete', run: () => actions.deleteScreen(n.screen) },
      ], x, y);
    } else if (n.id === 'screens') {
      showMenu([{ label: 'New Screen…', icon: 'new', run: () => actions.newScreen() }], x, y);
    } else if (n.doc) {
      showMenu([{ label: 'Open', icon: 'open', run: () => open(n.doc) }], x, y);
    } else if (n.dim) {
      showMenu([{ label: SOON, disabled: true }], x, y);
    }
  }

  tree.addEventListener('keydown', (e) => {
    const rows = [...tree.querySelectorAll('.st-node')];
    const i = rows.findIndex((r) => r.dataset.id === selectedId);
    const node = findNode(model(), selectedId);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (next) { selectedId = next.dataset.id; render(); tree.querySelector('.st-node.selected')?.scrollIntoView({ block: 'nearest' }); }
    } else if (e.key === 'ArrowRight' && node?.children?.length && !expanded.has(node.id)) { toggle(node.id); }
    else if (e.key === 'ArrowLeft' && node && expanded.has(node.id)) { toggle(node.id); }
    else if (e.key === 'Enter' && node?.doc) { open(node.doc, { node }); }
    else if (e.key === 'F2' && node?.screen) { actions.renameScreen(node.screen); }
    else if (e.key === 'Delete' && node?.screen) { e.stopPropagation(); actions.deleteScreen(node.screen); }
  });

  function save() {
    try { localStorage.setItem('moviclone.studio.explorer', JSON.stringify([...expanded])); } catch { /* ignore */ }
  }

  store.on('loaded', render);
  store.on('dirty', render);
  store.on('structure', render);
  store.on('diagnostics', render);
  render();
  return {
    render,
    reveal(id) {
      selectedId = id;
      expanded.add('project');
      if (id.startsWith('screen:')) expanded.add('screens');
      render();
    },
  };
}

function findNode(nodes, id) {
  for (const n of nodes) {
    if (n.id === id) return n;
    const k = findNode(n.children ?? [], id);
    if (k) return k;
  }
  return null;
}

function loadExpanded() {
  try {
    return JSON.parse(localStorage.getItem('moviclone.studio.explorer') ?? '["project","screens","io"]');
  } catch {
    return ['project', 'screens', 'io'];
  }
}
