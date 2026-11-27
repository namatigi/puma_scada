// Moviclone Studio: boot, docking layout, documents, commands and saving.
// The studio edits a working copy of the project (store.js) and writes each
// part through the studio API (docs/PROTOCOL.md), which validates and answers
// with fresh diagnostics after every save.

import { loadSymbolMap } from '../shared/symbolmap.js';
import { toast } from '../shared/toast.js';
import { formatValue } from '../shared/format.js';
import { createStore } from './store.js';
import { createStudioApi } from './studio-api.js';
import { createCommands } from './commands.js';
import { createMenuBar, createToolbar, openPalette } from './chrome.js';
import { createExplorer } from './explorer.js';
import { createDocuments } from './docs.js';
import { createScreenEditor } from './screen-editor.js';
import { createPropertiesPanel } from './props.js';
import { screenTarget, animationOf } from './screen-props.js';
import { createToolbox, createSymbolLibrary, createTagBrowser, createRuntimeStatus } from './side-panels.js';
import { createTagsDoc } from './tags-doc.js';
import { createAlarmsDoc } from './alarms-doc.js';
import { createSettingsDoc, createDriverDoc, createDataDoc, createXrefDoc } from './forms-docs.js';
import { createBottomDock } from './bottom.js';
import { createLive } from './live.js';
import { h, formDialog, confirmDialog, initSplitters } from './ui.js';
import { icon } from './icons.js';
import { elementsToml, screenHeaderToml, tomlArrayOfTables, tomlTableBody } from './toml.js';
import { clone, isScreenName, localScreenChecks, parseItem, elementRefs, KIND_INFO } from './model.js';

const api = createStudioApi('');
const store = createStore();
const commands = createCommands();
const prefs = loadPrefs();
let symbolMap = null;
let docs;
let props;
let bottom;
let explorer;
let toolbar;
let live;
let tagBrowser;
let runtimeStatus;
let runtimeDiag = null;
let statusMsg = { text: 'Ready', kind: '' };
let saving = false;

// ---------------------------------------------------------------- boot

async function boot() {
  let res;
  try {
    [res, symbolMap] = await Promise.all([api.project(), loadSymbolMap()]);
  } catch (err) {
    bootError(`Could not load the project from ${location.origin}/api/studio/project`, err.message);
    return;
  }
  if (!res.project) {
    bootError('The project does not load. Fix the file below and reload.', (res.diagnostics ?? []).map((d) => `${d.file}: ${d.message}`).join('\n'));
    return;
  }
  buildUi();
  store.load(res.project, res.diagnostics ?? []);
  store.log(`Opened project "${res.project.meta.name}" (${res.project.screens.length} screens, ${res.project.tags.length} tags)`, 'ok');
  logDiagnostics(res.diagnostics ?? [], 'Validation');
  document.getElementById('boot').remove();
  restoreSession();
  live.start();
}

function bootError(title, detail) {
  const boot = document.getElementById('boot');
  boot.classList.add('error');
  boot.replaceChildren(h('div', { class: 'st-logo st-logo-big' }), h('div', { text: title }), h('pre', { text: detail ?? '' }),
    h('button', { class: 'st-btn primary', type: 'button', text: 'Retry', onclick: () => location.reload() }));
}

function buildUi() {
  registerCommands();
  createMenuBar(document.getElementById('menubar'), commands);
  toolbar = createToolbar(document.getElementById('toolbar'), commands);
  initSplitters();

  explorer = createExplorer(document.getElementById('explorer'), {
    store,
    open: (docId) => openDoc(docId),
    actions: { newScreen, renameScreen, duplicateScreen, deleteScreen, setStartup },
  });

  docs = createDocuments(document.getElementById('documents'), {
    onActivate: () => { updateContext(); refreshChrome(); updateTitle(); },
    isDirty: (d) => (d.api?.part ? store.isDirty(d.api.part) : false),
    startPage,
  });

  props = createPropertiesPanel(document.getElementById('properties'), { getTags: () => store.project.tags });

  bottom = createBottomDock(document.getElementById('bottom'), {
    store, commands, onProblem: goToProblem,
  });

  live = createLive({
    api,
    onValues: (changes) => {
      for (const d of docs.all()) if (d.api.kind === 'screen') d.api.setValues(changes);
      if (bottom.active === 'animations') updateAnimations();
      if (!tagBrowserEl.hidden) tagBrowser.render();
    },
    onStatus: ({ status, alive }) => {
      const pill = document.getElementById('runtime-pill');
      pill.textContent = alive ? (status === 'live' ? 'Runtime live' : `Runtime: ${status}`) : 'Runtime offline';
      pill.classList.toggle('on', status === 'live');
      pill.title = alive ? 'A runtime answers on this server. Live values are shown on the canvas.' : 'No runtime answers on this server; designing works offline.';
      if (status === 'live') store.log('Connected to the runtime: live values on the canvas', 'ok');
      runtimeStatus?.render();
      refreshStatus();
    },
    onReload: () => store.log('Runtime reloaded the project'),
    onDiagnostics: (d) => { runtimeDiag = d; runtimeStatus?.render(); },
  });

  // Edge tabs and side panels.
  const flyLeft = document.getElementById('flyout-left');
  const flyRight = document.getElementById('flyout-right');
  const tagBrowserEl = h('div', { class: 'st-dock', style: { flex: '1' }, hidden: true });
  const runtimeEl = h('div', { class: 'st-dock', style: { flex: '1' }, hidden: true });
  flyLeft.append(tagBrowserEl, runtimeEl);
  tagBrowser = createTagBrowser(tagBrowserEl, { store, onAdd: addToActiveScreen, onWhereUsed: (t) => whereUsedTag(t), getValues: () => live.values });
  runtimeStatus = createRuntimeStatus(runtimeEl, { getState: () => ({ alive: live.alive, status: live.status, values: live.values, diag: runtimeDiag }) });
  const toolboxEl = h('div', { class: 'st-dock', style: { flex: '1' } });
  const symbolsEl = h('div', { class: 'st-dock', style: { flex: '1' }, hidden: true });
  flyRight.append(toolboxEl, symbolsEl);
  createToolbox(toolboxEl, { onAdd: addToActiveScreen });
  createSymbolLibrary(symbolsEl, { onAdd: addToActiveScreen, symbolMap });

  edgeTabs(document.getElementById('edge-left'), flyLeft, [['Runtime Status', runtimeEl, 'runtime'], ['Tag Browser', tagBrowserEl, 'tag-browser']], prefs.leftPanel, (v) => { prefs.leftPanel = v; savePrefs(); });
  edgeTabs(document.getElementById('edge-right'), flyRight, [['ToolBox', toolboxEl, 'toolbox'], ['Symbol Library', symbolsEl, 'symbols']], prefs.rightPanel, (v) => { prefs.rightPanel = v; savePrefs(); });

  document.getElementById('quick-search').addEventListener('click', () => commands.run('tools.palette'));

  store.on('dirty', () => { docs.renderTabs(); refreshChrome(); updateTitle(); });
  store.on('changed', ({ part }) => {
    if (part.startsWith('screen:') || part === 'tags') runLocalChecks();
    updateContext({ keepProps: true });
  });
  store.on('structure', () => { runLocalChecks(); refreshChrome(); });
  store.on('diagnostics', () => { refreshStatus(); runLocalChecks(); });
  store.on('log', (e) => { if (e.kind === 'error' || e.kind === 'warn') setStatus(e.message, e.kind === 'error' ? 'error' : ''); });

  document.addEventListener('keydown', onKey);
  window.addEventListener('beforeunload', (e) => {
    const { dirty, deleted } = store.pending();
    if (dirty.length || deleted.length) { e.preventDefault(); e.returnValue = ''; }
    saveSession();
  });
  commands.onRun(() => setTimeout(refreshChrome, 0));
}

function edgeTabs(edge, flyout, items, initial, remember) {
  const splitter = flyout.previousElementSibling?.classList.contains('st-splitter') ? flyout.previousElementSibling : null;
  const buttons = items.map(([label, panel, key]) => {
    const b = h('button', { class: 'st-edge-tab', type: 'button', text: label, title: label, 'data-key': key });
    b.addEventListener('click', () => select(panel.hidden || flyout.hidden ? key : null));
    edge.appendChild(b);
    return [key, b, panel];
  });
  function select(key) {
    for (const [k, b, panel] of buttons) {
      b.classList.toggle('on', k === key);
      panel.hidden = k !== key;
    }
    flyout.hidden = !key;
    if (splitter) splitter.hidden = !key;
    remember(key);
    window.dispatchEvent(new Event('resize'));
  }
  select(initial ?? null);
  // Exposed for View menu commands.
  edge.selectPanel = select;
  edge.isOpen = (key) => buttons.some(([k, b]) => k === key && b.classList.contains('on'));
}

function startPage() {
  const p = store.project;
  const link = (text, fn) => { const a = h('a', { text }); a.addEventListener('click', fn); return a; };
  if (!p) return h('div');
  return h('div', { class: 'st-start' },
    h('h1', { text: p.meta.name }),
    h('div', { text: `${p.screens.length} screens · ${p.tags.length} tags · ${p.alarms.length} alarms · ${p.drivers.length} driver(s)` }),
    p.meta.main_screen ? h('div', {}, link(`Open startup screen ${p.meta.main_screen}`, () => openDoc(`screen:${p.meta.main_screen}`))) : null,
    h('div', {}, link('Tags List', () => openDoc('tags')), ' · ', link('Alarms', () => openDoc('alarms')), ' · ', link('Project Settings', () => openDoc('settings'))),
    h('div', { style: { marginTop: '14px', color: 'var(--st-faint)' } }, 'Ctrl+K: Quick Search Commands · F5: save and run · Ctrl+S: save'));
}

// ---------------------------------------------------------------- documents

const DOC_TITLES = {
  tags: ['Tags List', 'tags'], alarms: ['Alarms', 'alarm'], settings: ['Project Settings', 'settings'],
  data: ['Historians & Data Loggers', 'historian'], xref: ['Cross Reference', 'xref'],
};

function openDoc(docId) {
  const ctx = { store, reveal, onSelection: () => updateContext() };
  if (docId.startsWith('screen:')) {
    const name = docId.slice(7);
    if (!store.getPart(docId)) { toast(`Screen ${name} does not exist`, { kind: 'warn' }); return null; }
    explorer.reveal(docId);
    return docs.open(docId, {
      title: name, icon: 'screen', tooltip: `screens/${name}.toml`,
      create: (holder) => createScreenEditor(holder, {
        ...ctx, name, prefs, symbolMap, commands,
        onSelection: () => updateContext(),
        onZoom: () => refreshStatus(),
        onActivate: () => props.focusSearch(),
        log: (m) => store.log(m),
      }),
    });
  }
  if (docId.startsWith('driver:')) {
    const name = docId.slice(7);
    return docs.open(docId, { title: name, icon: 'driver', tooltip: `drivers/${name}.toml`, create: (holder) => createDriverDoc(holder, ctx, name) });
  }
  const [title, ic] = DOC_TITLES[docId] ?? [];
  if (!title) return null;
  const factory = { tags: createTagsDoc, alarms: createAlarmsDoc, settings: createSettingsDoc, data: createDataDoc, xref: createXrefDoc }[docId];
  return docs.open(docId, { title, icon: ic, create: (holder) => factory(holder, ctx) });
}

const activeApi = () => docs?.active()?.api ?? null;
const activeScreen = () => { const a = activeApi(); return a?.kind === 'screen' ? a : null; };
const activePart = () => activeApi()?.part ?? null;

function addToActiveScreen(payload) {
  let ed = activeScreen();
  if (!ed) {
    const main = store.project.meta.main_screen ?? store.project.screens[0]?.name;
    if (!main) return;
    ed = openDoc(`screen:${main}`);
  }
  const scr = ed.screen();
  ed.createAt(payload, scr.width / 2, scr.height / 2);
}

/** Navigates to a usage: screen element, alarm or data logger. */
function reveal(u) {
  if (u.kind === 'element') {
    const ed = openDoc(`screen:${u.screen}`);
    ed?.select([u.id], { scroll: true });
  } else if (u.kind === 'alarm') {
    openDoc('alarms')?.revealAlarm(u.id);
  } else if (u.kind === 'logger') {
    openDoc('data');
  } else if (u.kind === 'tag') {
    openDoc('tags')?.revealTag(u.id);
  }
}

function goToProblem(d) {
  const { type, name } = parseItem(d.item);
  const file = d.file ?? '';
  if (file.startsWith('screens/')) {
    const stem = file.replace(/^screens\//, '').replace(/\.toml$/, '');
    const scr = store.project.screens.find((s) => s.name === stem) ?? store.project.screens.find((s) => s.name.replace(/[^A-Za-z0-9_.-]/g, '_') === stem);
    if (!scr) { toast(`Screen file ${file} is not in the project any more`, { kind: 'warn' }); return; }
    const ed = openDoc(`screen:${scr.name}`);
    if (type === 'element' && name) ed?.select([name], { scroll: true });
    else if (/display\(s\)/.test(d.message)) {
      const ids = (d.message.split(': ').pop() ?? '').split(',').map((x) => x.trim()).filter(Boolean);
      ed?.select(ids, { scroll: true });
    } else ed?.select([]);
  } else if (file === 'tags.toml') {
    const doc = openDoc('tags');
    if (type === 'tag') doc?.revealTag(name);
  } else if (file === 'alarms.toml') {
    const doc = openDoc('alarms');
    if (type === 'alarm') doc?.revealAlarm(name);
  } else if (file === 'project.toml') openDoc('settings');
  else if (file === 'data.toml') openDoc('data');
  else if (file.startsWith('drivers/')) openDoc(`driver:${file.replace(/^drivers\//, '').replace(/\.toml$/, '')}`);
}

function whereUsedTag(tag) {
  const x = openDoc('xref');
  x?.setFilter(tag);
}

// ---------------------------------------------------------------- context panels

function updateContext() {
  const a = activeApi();
  if (!props) return;
  if (a?.kind === 'screen') {
    props.show(screenTarget({ store, editor: a, openScreen: (n) => openDoc(`screen:${n}`) }));
    const els = a.selectedElements();
    bottom.setSource(els.length ? elementsToml(els) : `${screenHeaderToml({ ...a.screen() })}\n\n# ${a.screen().element.length} [[element]] tables follow; select objects to see theirs.`);
    updateAnimations();
    live?.setTags(allScreenTags());
  } else if (a?.kind === 'tags') {
    const sel = a.selectedTags();
    props.show(sel.length === 1 ? tagTarget(sel[0]) : null);
    bottom.setSource(sel.length ? tomlArrayOfTables('tag', sel) : '# Select tags to see their tags.toml entries.');
    bottom.setAnimations([]);
  } else if (a?.kind === 'alarms') {
    const sel = a.selectedAlarms();
    props.show(sel.length === 1 ? alarmTarget(sel[0]) : null);
    bottom.setSource(sel.length ? tomlArrayOfTables('alarm', sel) : '# Select alarms to see their alarms.toml entries.');
    bottom.setAnimations([]);
  } else if (a?.kind === 'settings') {
    props.show(null);
    bottom.setSource(tomlTableBody(store.project.meta));
  } else {
    props.show(null);
    bottom.setSource('');
    bottom.setAnimations([]);
  }
  refreshStatus();
  refreshChrome();
}

function allScreenTags() {
  const out = new Set();
  for (const d of docs.all()) if (d.api.kind === 'screen') d.api.tagsInUse().forEach((t) => out.add(t));
  return out;
}

function updateAnimations() {
  const a = activeScreen();
  if (!a) return;
  const list = [];
  for (const el of a.selectedElements()) {
    const refs = elementRefs(el);
    if (!refs.length) { list.push({ icon: KIND_INFO[el.kind]?.icon, object: el.id, text: animationOf(el) }); continue; }
    for (const r of refs) {
      const v = live?.values.get(r.tag);
      list.push({ icon: KIND_INFO[el.kind]?.icon, object: el.id, text: `${r.role} ← ${r.tag}`, value: v ? `${formatValue(v.v)} (${v.q})` : 'no live value' });
    }
  }
  bottom.setAnimations(list);
}

function tagTarget(t) {
  const ro = (key, label, value, cat = 'General', desc = '') => ({ key, label, cat, type: 'readonly', value: value ?? '', desc });
  const a = t.address ?? {};
  return {
    id: `tag|${t.name}`, title: t.name, subtitle: `Tag · ${t.type}`,
    rows: [
      ro('name', 'Name', t.name), ro('type', 'Data type', t.type), ro('access', 'Access', t.access ?? 'read'),
      ro('description', 'Description', t.description), ro('historian', 'Historian', t.historian),
      ro('driver', 'Driver', a.driver, 'Tag'), ro('station', 'Station', a.station, 'Tag'), ro('area', 'Area', a.area, 'Tag'),
      ro('register', 'Register', a.register, 'Tag'), ro('write', 'Write function', a.write ?? (a.area ? 'multiple' : ''), 'Tag'),
      ro('legacy', 'Imported id', t.legacy_id, 'General', 'Movicon NodeId GUID of the imported tag.'),
      ro('live', 'Live value', live?.values.get(t.name) ? formatValue(live.values.get(t.name).v) : '(runtime offline)', 'Animations'),
    ],
  };
}

function alarmTarget(al) {
  const ro = (key, label, value, cat = 'General') => ({ key, label, cat, type: 'readonly', value: value ?? '' });
  return {
    id: `alarm|${al.name}`, title: al.name, subtitle: 'Alarm',
    rows: [ro('tag', 'Tag', al.tag, 'Tag'), ro('when', 'Condition', al.when), ro('limit', 'Limit / value', al.limit ?? al.value), ro('severity', 'Severity', al.severity ?? 'medium'), ro('message', 'Message', al.message), ro('area', 'Area', al.area)],
  };
}

function runLocalChecks() {
  if (!store.project) return;
  const list = [];
  for (const part of store.pending().dirty) {
    if (part.startsWith('screen:')) list.push(...localScreenChecks(store.project, store.getPart(part)));
  }
  bottom.setLocalProblems(list);
}

// ---------------------------------------------------------------- chrome refresh

function refreshChrome() {
  toolbar?.refresh();
  bottom?.refreshCommands();
}

function setStatus(text, kind = '') {
  statusMsg = { text, kind };
  refreshStatus();
}

function refreshStatus() {
  const bar = document.getElementById('statusbar');
  if (!bar || !store.project) return;
  const diags = [...store.diagnostics, ...(bottom?.problems().filter((d) => d.local) ?? [])];
  const errors = diags.filter((d) => d.severity === 'error').length;
  const warnings = diags.filter((d) => d.severity === 'warning').length;
  const { dirty, deleted } = store.pending();
  const a = activeApi();
  const parts = [];
  if (a?.kind === 'screen') {
    const scr = a.screen();
    const sel = a.selection();
    parts.push(`${scr.name}  ${scr.width} × ${scr.height}`, `${sel.length ? `${sel.length} selected` : `${scr.element.length} objects`}`, `${Math.round(a.zoom * 100)}%`);
  }
  const item = (ic, text, title, onClick) => {
    const el = h('span', { class: 'item', title }, icon(ic), h('span', { text }));
    if (onClick) { el.style.cursor = 'pointer'; el.addEventListener('click', onClick); }
    return el;
  };
  bar.className = `st-statusbar${saving ? ' busy' : statusMsg.kind === 'error' ? ' error' : ''}`;
  bar.replaceChildren(
    h('span', { class: 'grow', text: statusMsg.text }),
    ...parts.map((p) => h('span', { class: 'item', text: p })),
    item('error', String(errors), 'Errors (click for Problems)', () => bottom.show('problems')),
    item('warning', String(warnings), 'Warnings (click for Problems)', () => bottom.show('problems')),
    item('save', dirty.length + deleted.length ? `${dirty.length + deleted.length} unsaved` : 'all saved', 'Unsaved documents (Ctrl+Shift+S saves all)'),
    item('live', live?.status ?? 'offline', 'Runtime connection'),
  );
}

function updateTitle() {
  const p = store.project;
  const a = docs?.active();
  const { dirty, deleted } = store.pending();
  const mark = dirty.length || deleted.length ? ' *' : '';
  const t = `${a ? `${a.title} - ` : ''}${p.meta.name}${mark} - Moviclone Studio`;
  document.title = t;
  document.getElementById('window-title').textContent = t;
}

// ---------------------------------------------------------------- saving

async function savePart(part) {
  const value = store.getPart(part);
  let res;
  if (part.startsWith('screen:')) res = value ? await api.saveScreen(value) : await api.deleteScreen(part.slice(7));
  else if (part === 'tags') res = await api.saveTags(value);
  else if (part === 'alarms') res = await api.saveAlarms(value);
  else if (part === 'meta') res = await api.saveMeta(value);
  else return null;
  if (value === undefined) store.forgetSaved(part);
  else store.markSaved(part);
  store.setDiagnostics(res.diagnostics ?? []);
  store.log(value === undefined ? `Deleted ${part.slice(7)} (${res.file})` : `Saved ${res.file}`, 'ok');
  return res;
}

/**
 * A rename that only changes letter case ("Manifold" → "manifold"): on Windows
 * both names are the same file, so saving the new name and then deleting the old
 * one would delete the screen. Goes through a temporary name instead; if a step
 * fails, the content is still on disk under one of the names.
 */
async function saveCaseRename(from, to) {
  const value = store.getPart(`screen:${to}`);
  const taken = new Set(store.project.screens.map((s) => s.name.toLowerCase()));
  let tmp = `${to.slice(0, 80)}.renaming`;
  for (let n = 2; taken.has(tmp.toLowerCase()); n += 1) tmp = `${to.slice(0, 80)}.renaming${n}`;
  await api.saveScreen({ ...value, name: tmp });
  await api.deleteScreen(from);
  store.forgetSaved(`screen:${from}`);
  await api.saveScreen(value);
  store.markSaved(`screen:${to}`);
  const res = await api.deleteScreen(tmp);
  store.setDiagnostics(res.diagnostics ?? []);
  store.log(`Renamed ${from} → ${to} (${res.file})`, 'ok');
  return res;
}

/** Pairs of [deleted, saved] screen parts whose names differ only in case. */
function caseRenames(parts) {
  const saved = parts.filter((p) => p.startsWith('screen:') && store.getPart(p));
  const pairs = [];
  for (const del of parts.filter((p) => p.startsWith('screen:') && !store.getPart(p))) {
    const to = saved.find((p) => p !== del && p.toLowerCase() === del.toLowerCase() && !pairs.some(([, t]) => t === p));
    if (to) pairs.push([del, to]);
  }
  return pairs;
}

/** Saves the given parts in dependency order: tags, alarms, screens, deletions, project settings. */
async function saveParts(parts) {
  if (saving) return false;
  if (!parts.length) { setStatus('Nothing to save'); return true; }
  const renames = caseRenames(parts);
  const inRename = new Set(renames.flat());
  const rename = (p) => renames.find(([, to]) => to === p);
  const rank = (p) => (p === 'tags' ? 0 : p === 'alarms' ? 1 : p.startsWith('screen:') ? (store.getPart(p) ? 2 : 3) : 4);
  // The deleted half of a case-only rename is handled together with its saved half.
  const ordered = parts.filter((p) => !inRename.has(p) || rename(p)).sort((a, b) => rank(a) - rank(b));
  saving = true;
  setStatus(`Saving ${ordered.length} item(s)…`);
  let failed = 0;
  let last = null;
  for (const part of ordered) {
    try {
      const pair = rename(part);
      last = pair ? await saveCaseRename(pair[0].slice(7), pair[1].slice(7)) : await savePart(part);
    } catch (err) {
      failed += 1;
      store.log(`Save of ${part} failed: ${err.message}`, 'error');
      toast(`Save failed: ${err.message}`, { kind: 'error' });
    }
  }
  saving = false;
  docs.renderTabs();
  if (last) {
    const errs = (last.diagnostics ?? []).filter((d) => d.severity === 'error').length;
    const warns = (last.diagnostics ?? []).filter((d) => d.severity === 'warning').length;
    setStatus(failed ? `${failed} save(s) failed` : `Saved · ${errs} error(s), ${warns} warning(s)`, failed ? 'error' : '');
    logDiagnostics(last.diagnostics ?? [], 'Validation after save');
    if (!failed) toast(`Saved${errs ? ` — ${errs} error(s) in Problems` : ''}`, { kind: errs ? 'warn' : 'ok' });
  } else if (failed) setStatus(`${failed} save(s) failed`, 'error');
  return failed === 0;
}

function saveActive() {
  const part = activePart();
  if (!part) return saveAll();
  const parts = store.isDirty(part) ? [part] : [];
  // A renamed screen also needs its old file removed.
  if (part.startsWith('screen:')) parts.push(...store.pending().deleted);
  if (part.startsWith('screen:') && store.isDirty('meta')) parts.push('meta');
  return saveParts([...new Set(parts)]);
}

function saveAll() {
  const { dirty, deleted } = store.pending();
  return saveParts([...dirty, ...deleted]);
}

function logDiagnostics(list, label) {
  const e = list.filter((d) => d.severity === 'error').length;
  const w = list.filter((d) => d.severity === 'warning').length;
  const n = list.length - e - w;
  store.log(`${label}: ${e} error(s), ${w} warning(s), ${n} note(s)`, e ? 'warn' : 'info');
}

async function reloadFromDisk() {
  const { dirty, deleted } = store.pending();
  if (dirty.length || deleted.length) {
    const ok = await confirmDialog({ title: 'Reload project', text: `Discard ${dirty.length + deleted.length} unsaved change(s) and reload the project from disk?`, okText: 'Discard and reload', danger: true });
    if (!ok) return;
  }
  try {
    const res = await api.project();
    if (!res.project) { store.setDiagnostics(res.diagnostics); toast('The project does not load; see Problems', { kind: 'error' }); return; }
    const open = docs.all().map((d) => d.id);
    for (const id of open) docs.close(id);
    store.load(res.project, res.diagnostics ?? []);
    for (const id of open) if (id.startsWith('screen:') ? store.getPart(id) : true) openDoc(id);
    store.log('Reloaded the project from disk', 'ok');
    logDiagnostics(res.diagnostics ?? [], 'Validation');
  } catch (err) {
    toast(`Reload failed: ${err.message}`, { kind: 'error' });
  }
}

async function run() {
  if (!(await saveAll())) return;
  const scr = activeScreen()?.name ?? store.project.meta.main_screen ?? '';
  const url = `/hmi/${scr ? `#${encodeURIComponent(scr)}` : ''}`;
  const win = window.open(url, 'moviclone-hmi');
  store.log(live.alive ? `Saved and opened the HMI (${url}); the runtime reloads the project` : `Saved and opened the HMI (${url}); no runtime answers on this server, so it has no live data`, 'ok');
  if (!win) toast('The browser blocked the HMI window; allow pop-ups for this site', { kind: 'warn' });
}

// ---------------------------------------------------------------- screen actions

const screenNameProblem = (name, except) => {
  if (!name) return 'Enter a name';
  if (!isScreenName(name)) return "Use letters, digits, '_', '-' or '.' (no spaces)";
  // Case-insensitive: Manifold and manifold would be the same file on Windows.
  const lower = name.toLowerCase();
  const clash = store.project.screens.find((s) => s.name.toLowerCase() === lower && s.name !== except);
  if (clash) return clash.name === name ? 'A screen with this name exists' : `A screen named ${clash.name} exists (names ignore letter case)`;
  return null;
};

async function newScreen() {
  const main = store.project.screens.find((s) => s.name === store.project.meta.main_screen);
  const v = await formDialog({
    title: 'New Screen', okText: 'Create',
    fields: [
      { name: 'name', label: 'Name', value: uniqueScreenName('Screen') },
      { name: 'width', label: 'Width', type: 'number', value: main?.width ?? 1920 },
      { name: 'height', label: 'Height', type: 'number', value: main?.height ?? 1080 },
      { name: 'background', label: 'Background', value: '#3F3F3F' },
    ],
    validate: (x) => screenNameProblem(x.name) ?? (x.width > 0 && x.height > 0 ? null : 'Width and height must be positive'),
  });
  if (!v) return;
  store.addScreen({ name: v.name, width: v.width, height: v.height, ...(v.background ? { background: v.background } : {}), element: [] });
  store.log(`Created screen ${v.name} (not saved yet)`);
  openDoc(`screen:${v.name}`);
}

function uniqueScreenName(base) {
  const names = new Set(store.project.screens.map((s) => s.name));
  let n = 1;
  while (names.has(`${base}${n}`)) n += 1;
  return `${base}${n}`;
}

async function renameScreen(from) {
  const v = await formDialog({
    title: `Rename ${from}`, okText: 'Rename', text: 'Buttons that open this screen and the startup screen setting are updated.',
    fields: [{ name: 'name', label: 'New name', value: from }],
    validate: (x) => screenNameProblem(x.name, from),
  });
  if (!v || v.name === from) return;
  const to = v.name;
  // Update open-screen commands in other screens as undoable edits first.
  for (const s of store.project.screens) {
    if (s.name === from) continue;
    const uses = (s.element ?? []).some((el) => (el.on_click ?? []).some((c) => c.action === 'open-screen' && c.screen === from));
    if (uses) {
      store.edit(`screen:${s.name}`, `Retarget ${from} → ${to}`, (scr) => {
        for (const el of scr.element) for (const c of el.on_click ?? []) if (c.action === 'open-screen' && c.screen === from) c.screen = to;
      });
    }
  }
  const self = store.getPart(`screen:${from}`);
  for (const el of self.element ?? []) for (const c of el.on_click ?? []) if (c.action === 'open-screen' && c.screen === from) c.screen = to;
  store.renameScreen(from, to);
  docs.rename(`screen:${from}`, `screen:${to}`, to);
  const ed = docs.get(`screen:${to}`);
  ed?.rename(to);
  store.log(`Renamed screen ${from} → ${to} (save to write the files)`);
  updateContext();
}

async function duplicateScreen(name) {
  const src = store.getPart(`screen:${name}`);
  if (!src) return;
  const v = await formDialog({ title: `Duplicate ${name}`, okText: 'Duplicate', fields: [{ name: 'name', label: 'Name', value: uniqueScreenName(`${name}_copy`) }], validate: (x) => screenNameProblem(x.name) });
  if (!v) return;
  const copy = clone(src);
  copy.name = v.name;
  delete copy.legacy_id;
  store.addScreen(copy);
  openDoc(`screen:${v.name}`);
  store.log(`Duplicated ${name} as ${v.name} (not saved yet)`);
}

async function deleteScreen(name) {
  const refs = store.project.screens.filter((s) => s.name !== name && (s.element ?? []).some((el) => (el.on_click ?? []).some((c) => c.screen === name))).map((s) => s.name);
  const main = store.project.meta.main_screen === name;
  const text = `Delete screen ${name}? The file is removed when you save.${refs.length ? ` Buttons on ${refs.join(', ')} open it.` : ''}${main ? ' It is the startup screen.' : ''}`;
  if (!(await confirmDialog({ title: 'Delete screen', text, okText: 'Delete', danger: true }))) return;
  docs.close(`screen:${name}`);
  store.removeScreen(name);
  store.log(`Deleted screen ${name} (save to remove the file)`, 'warn');
}

function setStartup(name) {
  store.edit('meta', 'Startup screen', (m) => { m.main_screen = name; });
  store.log(`Startup screen is now ${name} (save project settings)`);
}

// ---------------------------------------------------------------- commands

function registerCommands() {
  const c = (o) => commands.register(o);
  const scr = () => activeScreen();
  const hasSel = () => Boolean(scr()?.hasSelection());
  const multi = (n) => () => (scr()?.selection().length ?? 0) >= n;
  const undoPart = () => activePart();

  c({ id: 'file.new-screen', group: 'File', label: 'New Screen…', icon: 'new', kbd: 'Ctrl+Alt+N', run: newScreen });
  c({ id: 'file.save', group: 'File', label: 'Save', icon: 'save', kbd: 'Ctrl+S', global: true, run: saveActive, enabled: () => !saving && (activePart() ? store.isDirty(activePart()) || store.pending().deleted.length > 0 : store.pending().dirty.length > 0) });
  c({ id: 'file.save-all', group: 'File', label: 'Save All', icon: 'saveall', kbd: 'Ctrl+Shift+S', global: true, run: saveAll, enabled: () => !saving && (store.pending().dirty.length + store.pending().deleted.length) > 0 });
  c({ id: 'file.reload', group: 'File', label: 'Reload Project from Disk', icon: 'open', run: reloadFromDisk });
  c({ id: 'file.export', group: 'File', label: 'Export Project as JSON', icon: 'source', run: exportJson, title: 'Download the working copy (the same shape as GET /api/bundle)' });
  c({ id: 'run.start', group: 'Run', label: 'Save and Run', icon: 'run', kbd: 'F5', run, title: 'Saves all documents and opens the HMI in a new tab' });
  c({ id: 'run.stop', group: 'Run', label: 'Stop Live Values', icon: 'stop', kbd: 'Shift+F5', run: () => { live.setEnabled(false); store.log('Live values paused'); }, enabled: () => live?.enabled && live?.alive });
  c({ id: 'tools.hmi', group: 'Tools', label: 'Open HMI (without saving)', icon: 'screen', run: () => window.open('/hmi/', 'moviclone-hmi') });

  c({ id: 'edit.undo', group: 'Edit', label: 'Undo', icon: 'undo', kbd: 'Ctrl+Z', textSafe: false, run: () => { const l = store.undo(undoPart()); if (l) setStatus(`Undo: ${l}`); }, enabled: () => Boolean(undoPart()) && store.canUndo(undoPart()), dynamicTitle: () => (store.canUndo(undoPart() ?? '') ? `Undo ${store.undoLabel(undoPart())} (Ctrl+Z)` : 'Undo (Ctrl+Z)') });
  c({ id: 'edit.redo', group: 'Edit', label: 'Redo', icon: 'redo', kbd: ['Ctrl+Y', 'Ctrl+Shift+Z'], textSafe: false, run: () => { const l = store.redo(undoPart()); if (l) setStatus(`Redo: ${l}`); }, enabled: () => Boolean(undoPart()) && store.canRedo(undoPart()), dynamicTitle: () => (store.canRedo(undoPart() ?? '') ? `Redo ${store.redoLabel(undoPart())} (Ctrl+Y)` : 'Redo (Ctrl+Y)') });
  c({ id: 'edit.cut', group: 'Edit', label: 'Cut', icon: 'cut', kbd: 'Ctrl+X', textSafe: false, run: () => scr().cut(), enabled: hasSel });
  c({ id: 'edit.copy', group: 'Edit', label: 'Copy', icon: 'copy', kbd: 'Ctrl+C', textSafe: false, run: () => scr().copy(), enabled: hasSel });
  c({ id: 'edit.paste', group: 'Edit', label: 'Paste', icon: 'paste', kbd: 'Ctrl+V', textSafe: false, run: () => scr().paste(), enabled: () => Boolean(scr()?.canPaste()) });
  c({ id: 'edit.duplicate', group: 'Edit', label: 'Duplicate', icon: 'duplicate', kbd: 'Ctrl+D', textSafe: false, run: () => activeApi().duplicate(), enabled: () => Boolean(activeApi()?.duplicate && activeApi()?.hasSelection?.()) });
  c({ id: 'edit.delete', group: 'Edit', label: 'Delete', icon: 'delete', kbd: 'Delete', run: () => activeApi().deleteSelection(), enabled: () => Boolean(activeApi()?.deleteSelection && activeApi()?.hasSelection?.()) });
  c({ id: 'edit.select-all', group: 'Edit', label: 'Select All', kbd: 'Ctrl+A', textSafe: false, run: () => scr().selectAll(), enabled: () => Boolean(scr()) });
  c({ id: 'edit.find', group: 'Edit', label: 'Find…', icon: 'search', kbd: 'Ctrl+F', run: () => { const a = activeApi(); if (a?.focusSearch) a.focusSearch(); else commands.run('tools.palette'); } });

  const panel = (edgeId, key) => () => { const edge = document.getElementById(edgeId); edge.selectPanel(edge.isOpen(key) ? null : key); };
  const isPanel = (edgeId, key) => () => Boolean(document.getElementById(edgeId)?.isOpen?.(key));
  c({ id: 'view.explorer', group: 'View', label: 'Project Explorer', icon: 'folder', run: () => document.querySelector('#explorer .st-tree')?.focus() });
  c({ id: 'view.properties', group: 'View', label: 'Properties Window', icon: 'settings', kbd: 'F4', run: () => props.focusSearch() });
  c({ id: 'view.toolbox', group: 'View', label: 'ToolBox', icon: 'objects', run: panel('edge-right', 'toolbox'), checked: isPanel('edge-right', 'toolbox') });
  c({ id: 'view.symbols', group: 'View', label: 'Symbol Library', icon: 'symbol', run: panel('edge-right', 'symbols'), checked: isPanel('edge-right', 'symbols') });
  c({ id: 'view.tag-browser', group: 'View', label: 'Tag Browser', icon: 'tags', run: panel('edge-left', 'tag-browser'), checked: isPanel('edge-left', 'tag-browser') });
  c({ id: 'view.runtime', group: 'View', label: 'Runtime Status', icon: 'live', run: panel('edge-left', 'runtime'), checked: isPanel('edge-left', 'runtime') });
  for (const [id, label, ic] of [['problems', 'Problems', 'warning'], ['source', 'Source', 'source'], ['log', 'System Log', 'log'], ['commands', 'Commands', 'command'], ['animations', 'Animations', 'animation']]) {
    c({ id: `view.${id}`, group: 'View', label, icon: ic, run: () => bottom.show(id), checked: () => bottom?.active === id });
  }
  c({ id: 'view.zoom-in', group: 'View', label: 'Zoom In', icon: 'plus', kbd: ['Ctrl+=', 'Ctrl++'], run: () => scr().zoomIn(), enabled: () => Boolean(scr()) });
  c({ id: 'view.zoom-out', group: 'View', label: 'Zoom Out', icon: 'minus', kbd: 'Ctrl+-', run: () => scr().zoomOut(), enabled: () => Boolean(scr()) });
  c({ id: 'view.zoom-fit', group: 'View', label: 'Zoom to Fit', icon: 'fit', kbd: 'Ctrl+0', run: () => scr().zoomFit(), enabled: () => Boolean(scr()) });
  c({ id: 'view.zoom-100', group: 'View', label: 'Actual Size (100%)', kbd: 'Ctrl+1', run: () => scr().zoom100(), enabled: () => Boolean(scr()) });
  c({ id: 'view.live', group: 'View', label: 'Live Values on Canvas', icon: 'live', run: () => { live.setEnabled(!live.enabled); if (!live.enabled) for (const d of docs.all()) if (d.api.kind === 'screen') d.api.clearValues(); }, checked: () => Boolean(live?.enabled) });

  const pref = (key, label, ic) => c({ id: `options.${key}`, group: 'Options', label, icon: ic, run: () => { prefs[key] = !prefs[key]; savePrefs(); redrawCanvases(); }, checked: () => prefs[key] });
  pref('grid', 'Show Grid', 'grid');
  pref('snap', 'Snap to Grid', 'snap');
  pref('guides', 'Smart Guides', 'align-center');
  c({ id: 'options.grid-size', group: 'Options', label: 'Grid Size…', run: async () => {
    const v = await formDialog({ title: 'Grid', fields: [{ name: 'size', label: 'Grid size', type: 'number', value: prefs.gridSize }], validate: (x) => (x.size >= 1 && x.size <= 200 ? null : '1 … 200') });
    if (v) { prefs.gridSize = v.size; savePrefs(); redrawCanvases(); }
  } });

  c({ id: 'project.tags', group: 'Project', label: 'Tags List', icon: 'tags', run: () => openDoc('tags') });
  c({ id: 'project.alarms', group: 'Project', label: 'Alarms', icon: 'alarm', run: () => openDoc('alarms') });
  c({ id: 'project.settings', group: 'Project', label: 'Project Settings', icon: 'settings', run: () => openDoc('settings') });
  c({ id: 'project.data', group: 'Project', label: 'Historians & Data Loggers', icon: 'historian', run: () => openDoc('data') });
  c({ id: 'project.xref', group: 'Project', label: 'Cross Reference', icon: 'xref', run: () => openDoc('xref') });
  c({ id: 'project.validate', group: 'Project', label: 'Validate Saved Project', icon: 'ok', run: async () => {
    try {
      const res = await api.project();
      store.setDiagnostics(res.diagnostics ?? []);
      logDiagnostics(res.diagnostics ?? [], 'Validation');
      bottom.show('problems');
    } catch (err) { toast(err.message, { kind: 'error' }); }
  } });

  const fmt = (id, label, ic, fn, need = 2, kbd) => c({ id: `format.${id}`, group: 'Format', label, icon: ic, kbd, run: () => fn(scr()), enabled: multi(need) });
  fmt('align-left', 'Align Left', 'align-left', (e) => e.align('left'));
  fmt('align-center', 'Align Centers', 'align-center', (e) => e.align('center'));
  fmt('align-right', 'Align Right', 'align-right', (e) => e.align('right'));
  fmt('align-top', 'Align Top', 'align-top', (e) => e.align('top'));
  fmt('align-middle', 'Align Middles', 'align-middle', (e) => e.align('middle'));
  fmt('align-bottom', 'Align Bottom', 'align-bottom', (e) => e.align('bottom'));
  fmt('dist-h', 'Distribute Horizontally', 'dist-h', (e) => e.distribute('h'), 3);
  fmt('dist-v', 'Distribute Vertically', 'dist-v', (e) => e.distribute('v'), 3);
  fmt('same-w', 'Make Same Width', 'same-w', (e) => e.sameSize('width'));
  fmt('same-h', 'Make Same Height', 'same-h', (e) => e.sameSize('height'));
  fmt('same-size', 'Make Same Size', 'same-size', (e) => e.sameSize('both'));
  fmt('front', 'Bring to Front', 'front', (e) => e.zorder('front'), 1, 'Ctrl+Shift+]');
  fmt('back', 'Send to Back', 'back', (e) => e.zorder('back'), 1, 'Ctrl+Shift+[');
  fmt('forward', 'Bring Forward', 'front', (e) => e.zorder('forward'), 1, 'Ctrl+]');
  fmt('backward', 'Send Backward', 'back', (e) => e.zorder('backward'), 1, 'Ctrl+[');
  c({ id: 'design.centre-h', group: 'Design', label: 'Center Horizontally on Screen', run: () => scr().centre('h'), enabled: hasSel });
  c({ id: 'design.centre-v', group: 'Design', label: 'Center Vertically on Screen', run: () => scr().centre('v'), enabled: hasSel });
  c({ id: 'design.where-used', group: 'Design', label: 'Where Used (tag of selection)', icon: 'where', run: () => {
    const el = scr()?.selectedElements()[0];
    const tag = el && elementRefs(el)[0]?.tag;
    if (tag) whereUsedTag(tag); else toast('The selection has no tag', { kind: 'warn' });
  }, enabled: hasSel });

  c({ id: 'tools.palette', group: 'Tools', label: 'Quick Search Commands', icon: 'search', kbd: 'Ctrl+K', global: true, run: () => openPalette(commands, paletteExtras) });
  c({ id: 'doc.close', group: 'Window', label: 'Close Document', kbd: 'Ctrl+F4', run: () => { const a = docs.active(); if (a) docs.close(a.id); }, enabled: () => Boolean(docs?.active()) });
  c({ id: 'doc.next', group: 'Window', label: 'Next Document', kbd: 'Ctrl+PageDown', run: () => docs.cycle(1) });
  c({ id: 'doc.prev', group: 'Window', label: 'Previous Document', kbd: 'Ctrl+PageUp', run: () => docs.cycle(-1) });
  c({ id: 'help.shortcuts', group: 'Help', label: 'Keyboard Shortcuts', run: () => bottom.show('commands') });
  c({ id: 'help.about', group: 'Help', label: 'About Moviclone Studio', run: () => toast('Moviclone Studio — a web designer for Moviclone projects. All artwork is original.', { kind: 'info', timeout: 6000 }) });
}

function paletteExtras() {
  const p = store.project;
  return [
    ...p.screens.map((s) => ({ kind: 'screen', group: 'Screen', icon: 'screen', label: s.name, detail: `${s.width}×${s.height}`, run: () => openDoc(`screen:${s.name}`) })),
    ...p.tags.map((t) => ({ kind: 'tag', group: 'Tag', icon: 'tag', label: t.name, detail: t.description ?? t.type, run: () => openDoc('tags')?.revealTag(t.name) })),
    ...p.alarms.map((a) => ({ kind: 'alarm', group: 'Alarm', icon: 'alarm', label: a.name, detail: a.message, run: () => openDoc('alarms')?.revealAlarm(a.name) })),
    ...(activeScreen()?.screen().element ?? []).map((el) => ({ kind: 'object', group: 'Object', icon: KIND_INFO[el.kind]?.icon, label: el.id, detail: el.text ?? el.tag ?? el.kind, run: () => activeScreen()?.select([el.id], { scroll: true }) })),
  ];
}

function redrawCanvases() {
  for (const d of docs.all()) if (d.api.kind === 'screen') d.api.redrawOverlay();
  refreshChrome();
}

function exportJson() {
  const blob = new Blob([JSON.stringify(store.project, null, 2)], { type: 'application/json' });
  const a = h('a', { href: URL.createObjectURL(blob), download: `${store.project.meta.name.replace(/[^A-Za-z0-9_.-]+/g, '_')}.bundle.json` });
  document.body.appendChild(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
}

// ---------------------------------------------------------------- keyboard

function onKey(e) {
  if (e.defaultPrevented) return;
  if (document.querySelector('.st-modal-back')) return;
  const t = e.target;
  const inText = t instanceof HTMLElement && (t.matches('input, textarea, select') || t.isContentEditable);
  // Canvas-local keys (arrows, Esc, Tab) are handled by the editor itself.
  const cmd = commands.forKey(e, { inText });
  if (!cmd) return;
  if (inText && ['edit.cut', 'edit.copy', 'edit.paste', 'edit.select-all', 'edit.undo', 'edit.redo', 'edit.duplicate'].includes(cmd.id)) return;
  // Grid documents keep Delete for their own focused rows; screens use the command.
  if (cmd.id === 'edit.delete' && !(t === document.body || t.closest?.('.st-documents'))) return;
  e.preventDefault();
  commands.run(cmd.id);
}

// ---------------------------------------------------------------- prefs and session

function loadPrefs() {
  const d = { grid: true, snap: true, guides: true, gridSize: 10, rightPanel: 'toolbox', leftPanel: null };
  try { return { ...d, ...JSON.parse(localStorage.getItem('moviclone.studio.prefs') ?? '{}') }; } catch { return d; }
}

function savePrefs() {
  try { localStorage.setItem('moviclone.studio.prefs', JSON.stringify(prefs)); } catch { /* ignore */ }
}

function saveSession() {
  try {
    localStorage.setItem('moviclone.studio.session', JSON.stringify({ project: store.project?.meta?.name, open: docs.all().map((d) => d.id), active: docs.active()?.id }));
  } catch { /* ignore */ }
}

function restoreSession() {
  let s = null;
  try { s = JSON.parse(localStorage.getItem('moviclone.studio.session') ?? 'null'); } catch { s = null; }
  const hash = decodeURIComponent(location.hash.slice(1));
  if (hash) { openDoc(hash.includes(':') || DOC_TITLES[hash] ? hash : `screen:${hash}`); return; }
  if (s && s.project === store.project.meta.name && s.open?.length) {
    for (const id of s.open) { try { openDoc(id); } catch (err) { console.warn('[studio] could not reopen', id, err); } }
    if (s.active && docs.has(s.active)) docs.activate(s.active);
    return;
  }
  if (store.project.meta.main_screen) openDoc(`screen:${store.project.meta.main_screen}`);
}

window.addEventListener('hashchange', () => {
  const id = decodeURIComponent(location.hash.slice(1));
  if (id) openDoc(id.includes(':') || DOC_TITLES[id] ? id : `screen:${id}`);
});

boot();

// Exposed for automated checks (Playwright) and console debugging.
window.__studio = { store, commands, get docs() { return docs; }, openDoc, saveAll };
