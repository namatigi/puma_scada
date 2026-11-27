// Bottom dock: Animations, Source (TOML of the selection), Problems (server
// diagnostics + instant local checks), System Log and Commands.

import { h, iconButton } from './ui.js';
import { icon } from './icons.js';
import { fuzzyScore } from './commands.js';
import { formatDateTime } from '../shared/format.js';

const TABS = [
  { id: 'animations', label: 'Animations', icon: 'animation' },
  { id: 'source', label: 'Source', icon: 'source' },
  { id: 'problems', label: 'Problems', icon: 'warning' },
  { id: 'log', label: 'System Log', icon: 'log' },
  { id: 'commands', label: 'Commands', icon: 'command' },
];

const MAX_LOG = 500;

export function createBottomDock(root, { store, commands, onProblem }) {
  let active = loadTab();
  const tabsEl = h('div', { class: 'st-bottom-tabs', role: 'tablist' });
  const body = h('div', { class: 'st-bottom-body', tabindex: '-1' });
  root.append(tabsEl, body);
  const counters = {};
  for (const t of TABS) {
    const n = h('span', { class: 'n', hidden: true });
    counters[t.id] = n;
    const b = h('button', { class: 'st-bottom-tab', type: 'button', role: 'tab', 'data-tab': t.id }, icon(t.icon), h('span', { text: t.label }), n);
    b.addEventListener('click', () => show(t.id));
    tabsEl.appendChild(b);
  }

  // ---- state fed by the app
  let sourceText = '';
  let animations = [];
  let localDiags = [];
  const log = [];
  let probFilter = { error: true, warning: true, note: true };
  let cmdFilter = '';

  function show(id) {
    active = id;
    try { localStorage.setItem('moviclone.studio.bottom', id); } catch { /* ignore */ }
    tabsEl.querySelectorAll('.st-bottom-tab').forEach((b) => b.classList.toggle('on', b.dataset.tab === id));
    render();
  }

  function render() {
    if (active === 'source') body.replaceChildren(h('pre', { class: 'st-source' }, ...highlightToml(sourceText || '# Select an object, a tag or an alarm to see its project file text.')));
    else if (active === 'problems') renderProblems();
    else if (active === 'log') renderLog();
    else if (active === 'commands') renderCommands();
    else renderAnimations();
  }

  function allProblems() {
    const server = store.diagnostics.map((d) => ({ ...d, local: false }));
    return [...localDiags.map((d) => ({ ...d, local: true })), ...server];
  }

  function updateCounters() {
    const list = allProblems();
    const errors = list.filter((d) => d.severity === 'error').length;
    const n = counters.problems;
    n.hidden = list.length === 0;
    n.textContent = String(errors || list.length);
    n.classList.toggle('err', errors > 0);
  }

  function renderProblems() {
    const list = allProblems();
    const by = (s) => list.filter((d) => d.severity === s).length;
    const toggle = (sev, label) => {
      const b = iconButton(sev, `Show ${label}`, () => { probFilter[sev] = !probFilter[sev]; renderProblems(); }, { label: `${by(sev)} ${label}` });
      b.classList.toggle('on', probFilter[sev]);
      return b;
    };
    const rows = list.filter((d) => probFilter[d.severity]).map((d) => {
      const tr = h('tr', { class: d.local ? 'local' : '', title: d.local ? 'Found while editing (not saved yet)' : 'Reported by the project check after the last save' },
        h('td', { class: 'sev' }, icon(d.severity)), h('td', { text: d.code }), h('td', { text: d.message }),
        h('td', { text: d.file }), h('td', { text: d.item }));
      tr.addEventListener('click', () => onProblem(d));
      return tr;
    });
    body.replaceChildren(
      h('div', { class: 'st-filterbar' }, toggle('error', 'Errors'), toggle('warning', 'Warnings'), toggle('note', 'Notes'),
        h('span', { style: { color: 'var(--st-faint)', marginLeft: '8px' }, text: 'Click a problem to go to it. Italic rows come from unsaved edits.' })),
      h('table', { class: 'st-table st-problems' },
        h('thead', {}, h('tr', {}, ['', 'Code', 'Description', 'File', 'Item'].map((t) => h('th', { text: t })))),
        h('tbody', {}, rows)));
  }

  function renderLog() {
    body.replaceChildren(h('div', { class: 'st-log' }, log.map((e) => h('div', { class: e.kind }, h('span', { class: 't', text: formatDateTime(e.ts) }), document.createTextNode(e.message)))));
    body.scrollTop = body.scrollHeight;
  }

  function renderCommands() {
    const search = h('input', { class: 'st-search', placeholder: 'Filter commands', value: cmdFilter, spellcheck: 'false', style: { maxWidth: '300px' } });
    const list = commands.list()
      .map((c) => ({ c, score: fuzzyScore(`${c.group ?? ''} ${c.label}`, cmdFilter) }))
      .filter((x) => x.score >= 0)
      .sort((a, b) => (a.c.group ?? '').localeCompare(b.c.group ?? '') || a.c.label.localeCompare(b.c.label));
    const rows = list.map(({ c }) => {
      const enabled = commands.enabled(c.id);
      const tr = h('tr', { class: enabled ? '' : 'disabled', title: c.title ?? '' },
        h('td', { class: 'sev' }, c.icon ? icon(c.icon) : null), h('td', { text: c.group ?? '' }), h('td', { text: c.label }),
        h('td', { text: Array.isArray(c.kbd) ? c.kbd.join(', ') : c.kbd ?? '' }), h('td', { class: 'ro', text: enabled ? '' : 'not available now' }));
      tr.addEventListener('dblclick', () => commands.run(c.id));
      return tr;
    });
    search.addEventListener('input', () => { cmdFilter = search.value; const pos = search.selectionStart; renderCommands(); const s2 = body.querySelector('input'); s2.focus(); s2.setSelectionRange(pos, pos); });
    body.replaceChildren(h('div', { class: 'st-filterbar' }, search, h('span', { style: { color: 'var(--st-faint)' }, text: 'Double-click to run. Ctrl+K opens the same list as a palette.' })),
      h('table', { class: 'st-table st-problems' }, h('thead', {}, h('tr', {}, ['', 'Menu', 'Command', 'Shortcut', ''].map((t) => h('th', { text: t })))), h('tbody', {}, rows)));
  }

  function renderAnimations() {
    if (!animations.length) {
      body.replaceChildren(h('div', { class: 'st-empty', text: 'Select objects on a screen to see their animations (tag bindings). Drag a tag from the Tag Browser onto an object to bind it.' }));
      return;
    }
    body.replaceChildren(h('div', { class: 'st-anim' }, animations.map((a) => h('div', { class: 'st-anim-row' },
      icon(a.icon ?? 'animation'), h('span', { text: a.object }), h('span', { text: a.text }), h('span', { class: 'muted', text: a.value ?? '' })))));
  }

  store.on('diagnostics', () => { updateCounters(); if (active === 'problems') render(); });
  store.on('log', (e) => {
    log.push(e);
    if (log.length > MAX_LOG) log.splice(0, log.length - MAX_LOG);
    if (active === 'log') render();
  });

  show(active);
  updateCounters();

  return {
    show,
    get active() { return active; },
    setSource(text) { sourceText = text; if (active === 'source') render(); },
    setAnimations(list) { animations = list; if (active === 'animations') render(); },
    setLocalProblems(list) { localDiags = list; updateCounters(); if (active === 'problems') render(); },
    problems: allProblems,
    refreshCommands() { if (active === 'commands') render(); },
  };
}

/** Minimal TOML syntax colouring (tables, keys, strings, numbers, booleans, comments). */
export function highlightToml(text) {
  const out = [];
  for (const line of text.split('\n')) {
    if (/^\s*#/.test(line)) out.push(h('span', { class: 'c', text: line }));
    else if (/^\s*\[\[?[^\]]+\]\]?\s*$/.test(line)) out.push(h('span', { class: 'h', text: line }));
    else {
      const m = /^(\s*)([A-Za-z0-9_."-]+)(\s*=\s*)(.*)$/.exec(line);
      if (!m) out.push(document.createTextNode(line));
      else {
        out.push(document.createTextNode(m[1]), h('span', { class: 'k', text: m[2] }), document.createTextNode(m[3]));
        const v = m[4];
        const cls = /^["']/.test(v) ? 's' : /^(true|false)$/.test(v) ? 'b' : /^-?[\d.]/.test(v) ? 'n' : '';
        out.push(cls ? h('span', { class: cls, text: v }) : document.createTextNode(v));
      }
    }
    out.push(document.createTextNode('\n'));
  }
  return out;
}

function loadTab() {
  try { return localStorage.getItem('moviclone.studio.bottom') ?? 'problems'; } catch { return 'problems'; }
}
