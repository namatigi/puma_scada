// Project-model helpers shared by the studio documents: element kinds and
// templates, id generation, optional-field handling, "where used" and quick
// local checks that run on every edit (the server re-validates on save).

import { listSymbols } from '../shared/symbols.js';

export const clone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)));

/** Every ElementKind of the Rust model, in ToolBox order. */
export const ELEMENT_KINDS = [
  { kind: 'label', title: 'Text', group: 'Basic Shapes', prefix: 'TextBox', icon: 'label' },
  { kind: 'display', title: 'Display / Edit', group: 'Controls', prefix: 'EditDisplay', icon: 'display' },
  { kind: 'button', title: 'Button', group: 'Controls', prefix: 'Button', icon: 'button' },
  { kind: 'pipeline', title: 'Pipeline', group: 'Basic Shapes', prefix: 'Pipeline', icon: 'pipeline' },
  { kind: 'symbol', title: 'Symbol', group: 'Basic Shapes', prefix: 'Symbol', icon: 'symbol' },
  { kind: 'trend', title: 'Trend / Data Analysis', group: 'Advanced', prefix: 'Trend', icon: 'trend' },
  { kind: 'history-table', title: 'Historical Log Viewer', group: 'Advanced', prefix: 'HistoricalViewer', icon: 'table' },
  { kind: 'alarm-list', title: 'Alarm Window', group: 'Advanced', prefix: 'AlarmWindow', icon: 'alarm' },
];

export const KIND_INFO = Object.fromEntries(ELEMENT_KINDS.map((k) => [k.kind, k]));

/** Optional fields per kind: removed instead of saved as "" or null. */
const OPTIONAL = new Set(['tag', 'unit', 'format', 'fill', 'stroke', 'symbol', 'historian', 'area', 'output_dir']);

/** A new element of `kind`, centred on (cx, cy) in screen units. */
export function newElement(kind, project, variant = null) {
  const historian = project?.data?.historian?.[0]?.name ?? project?.data?.logger?.[0]?.name ?? '';
  const t = 26;
  const templates = {
    label: { text: 'Text', w: 120, h: 30 },
    display: { format: '#0.00', w: 175, h: 50 },
    button: { text: 'Button', w: 160, h: 44 },
    pipeline: variant === 'elbow'
      ? { points: [[t / 2, 0], [t / 2, 120 - t / 2], [160, 120 - t / 2]], thickness: t, fill: '#EFEFF0', stroke: '#5E5E5E', w: 160, h: 120 }
      : variant === 'vertical'
        ? { points: [[t / 2, 0], [t / 2, 200]], thickness: t, fill: '#EFEFF0', stroke: '#5E5E5E', w: t, h: 200 }
        : { points: [[0, t / 2], [200, t / 2]], thickness: t, fill: '#EFEFF0', stroke: '#5E5E5E', w: 200, h: t },
    symbol: { symbol: variant ?? 'ball-valve', w: 90, h: 60 },
    trend: { tags: [], ...(historian ? { historian } : {}), span_s: 3600, w: 800, h: 400 },
    'history-table': { source: historian, w: 800, h: 300 },
    'alarm-list': { w: 900, h: 300 },
  };
  const base = templates[kind];
  if (!base) throw new Error(`unknown element kind ${kind}`);
  const el = { id: '', kind, ...clone(base), x: 0, y: 0 };
  if (kind === 'symbol') {
    const def = listSymbols().find((s) => s.name === el.symbol);
    if (def) [el.w, el.h] = def.size;
  }
  return el;
}

/** Next free id like "EditDisplay12" on a screen. */
export function nextId(screen, prefix, taken = new Set()) {
  const used = new Set([...(screen.element ?? []).map((e) => e.id), ...taken]);
  let max = 0;
  const re = new RegExp(`^${prefix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(\\d+)$`);
  for (const id of used) {
    const m = re.exec(id);
    if (m) max = Math.max(max, Number(m[1]));
  }
  let n = max + 1;
  while (used.has(`${prefix}${n}`)) n += 1;
  return `${prefix}${n}`;
}

export function prefixFor(el) {
  return KIND_INFO[el.kind]?.prefix ?? 'Element';
}

/** Sets a field, deleting optional ones when blank so the TOML stays clean. */
export function setField(obj, key, value) {
  if ((value === '' || value === null || value === undefined) && OPTIONAL.has(key)) delete obj[key];
  else if (value === undefined) delete obj[key];
  else obj[key] = value;
}

/** Tags an element refers to, with the role of each reference. */
export function elementRefs(el) {
  const out = [];
  if (el.tag && ['display', 'pipeline', 'symbol'].includes(el.kind)) {
    out.push({ tag: el.tag, role: el.kind === 'display' ? 'value' : el.kind === 'pipeline' ? 'flow animation' : 'symbol state' });
  }
  if (el.kind === 'trend') for (const t of el.tags ?? []) out.push({ tag: t, role: 'trend pen' });
  for (const c of el.on_click ?? []) {
    if (c.action === 'write-tag' && c.tag) out.push({ tag: c.tag, role: `on click: write (${c.mode ?? 'set'})` });
    if (c.action === 'report') for (const t of c.tags ?? []) out.push({ tag: t, role: `on click: report ${c.report}` });
  }
  return out;
}

/**
 * Every place a tag is used: screens, alarms, data loggers.
 * @returns {{kind:'element'|'alarm'|'logger', screen?:string, id:string, role:string}[]}
 */
export function whereUsed(project, tag) {
  const out = [];
  for (const s of project.screens ?? []) {
    for (const el of s.element ?? []) {
      for (const r of elementRefs(el)) if (r.tag === tag) out.push({ kind: 'element', screen: s.name, id: el.id, role: r.role });
    }
  }
  for (const a of project.alarms ?? []) if (a.tag === tag) out.push({ kind: 'alarm', id: a.name, role: `alarm (${a.when})` });
  for (const l of project.data?.logger ?? []) {
    for (const c of l.column ?? []) if (c.tag === tag) out.push({ kind: 'logger', id: l.name, role: `data logger column ${c.name}` });
  }
  return out;
}

/** Usage count per tag name, for the tag grid. */
export function usageCounts(project) {
  const counts = new Map();
  const bump = (t) => counts.set(t, (counts.get(t) ?? 0) + 1);
  for (const s of project.screens ?? []) for (const el of s.element ?? []) elementRefs(el).forEach((r) => bump(r.tag));
  for (const a of project.alarms ?? []) bump(a.tag);
  for (const l of project.data?.logger ?? []) for (const c of l.column ?? []) bump(c.tag);
  return counts;
}

/** Renames a tag everywhere in the working copy; returns the names of screens it touched. */
export function renameTagRefs(project, from, to) {
  const touched = new Set();
  for (const s of project.screens ?? []) {
    for (const el of s.element ?? []) {
      let hit = false;
      if (el.tag === from) { el.tag = to; hit = true; }
      if (Array.isArray(el.tags)) el.tags = el.tags.map((t) => { if (t === from) { hit = true; return to; } return t; });
      for (const c of el.on_click ?? []) {
        if (c.tag === from) { c.tag = to; hit = true; }
        if (Array.isArray(c.tags)) c.tags = c.tags.map((t) => { if (t === from) { hit = true; return to; } return t; });
      }
      if (hit) touched.add(s.name);
    }
  }
  let alarms = false;
  for (const a of project.alarms ?? []) if (a.tag === from) { a.tag = to; alarms = true; }
  return { screens: [...touched], alarms };
}

/** Renames open-screen targets; returns the screens it touched. */
export function renameScreenRefs(project, from, to) {
  const touched = new Set();
  for (const s of project.screens ?? []) {
    for (const el of s.element ?? []) {
      for (const c of el.on_click ?? []) {
        if (c.action === 'open-screen' && c.screen === from) { c.screen = to; touched.add(s.name); }
      }
    }
  }
  return [...touched];
}

const IDENT = /^[A-Za-z_][A-Za-z0-9_]*$/;
export const isIdentifier = (s) => IDENT.test(s);
export const isScreenName = (s) => /^[A-Za-z0-9_-][A-Za-z0-9_.-]*$/.test(s) && !s.includes('..') && !s.endsWith('.') && s.length <= 100;

/**
 * Checks that run on every edit of a screen, before the server sees it.
 * Codes start with "L" (local) so they are not confused with server codes.
 */
export function localScreenChecks(project, screen) {
  const out = [];
  const file = `screens/${screen.name}.toml`;
  const tags = new Set((project.tags ?? []).map((t) => t.name));
  const screens = new Set((project.screens ?? []).map((s) => s.name));
  const ids = new Set();
  for (const el of screen.element ?? []) {
    const item = `element "${el.id}"`;
    if (!el.id) out.push({ severity: 'error', code: 'L032', file, item, message: 'element has no id' });
    else if (ids.has(el.id)) out.push({ severity: 'error', code: 'L032', file, item, message: 'element id is used twice on this screen' });
    ids.add(el.id);
    for (const r of elementRefs(el)) {
      if (!tags.has(r.tag)) out.push({ severity: 'error', code: 'L034', file, item, message: `tag "${r.tag}" (${r.role}) does not exist` });
    }
    for (const c of el.on_click ?? []) {
      if (c.action === 'open-screen' && !screens.has(c.screen)) out.push({ severity: 'error', code: 'L040', file, item, message: `open-screen target "${c.screen}" does not exist` });
    }
    if (el.kind === 'display' && !el.tag) out.push({ severity: 'warning', code: 'L038', file, item, message: 'display has no tag' });
  }
  return out;
}

/** Parses a Problems "item" such as `element "X"` / `tag "Y"` into {type, name}. */
export function parseItem(item) {
  const m = /^(\w+)\s+"((?:[^"\\]|\\.)*)"/.exec(item ?? '');
  if (!m) return { type: item ?? '', name: '' };
  return { type: m[1], name: m[2].replace(/\\"/g, '"') };
}
