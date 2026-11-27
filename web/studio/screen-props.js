// Property rows for the screen editor: the screen itself when nothing is
// selected, otherwise every field of the selected element(s) with kind-specific
// editors. Multi-selection shows the fields all selected kinds share.

import { h, iconButton, tagInput } from './ui.js';
import { listSymbols } from '../shared/symbols.js';
import { setField, isIdentifier, KIND_INFO, clone } from './model.js';
import { tidy } from './geometry.js';

const FORMATS = ['#0', '#0.0', '#0.00', '#0.000', '#0.000000', '#,##0', '#,##0.00', '0.0%', 'F2', 'N0', 'N2', 'E3'];
const MODES = ['set', 'toggle', 'increase', 'decrease', 'pulse'];

/** Who has which field: [key, kinds or '*'] */
const FIELDS = {
  text: ['label', 'button'],
  symbol: ['symbol'],
  editable: ['display'],
  source: ['history-table'],
  area: ['alarm-list'],
  historian: ['trend'],
  span_s: ['trend'],
  tag: ['display', 'pipeline', 'symbol'],
  tags: ['trend'],
  unit: ['display'],
  format: ['display'],
  fill: ['pipeline'],
  stroke: ['pipeline'],
  thickness: ['pipeline'],
  points: ['pipeline'],
  flow_color: ['pipeline'],
  flow_min: ['pipeline'],
  flow_reverse: ['pipeline'],
};

const has = (kinds, key) => kinds.every((k) => FIELDS[key].includes(k));

/**
 * @param {object} ctx {store, editor, commands, openScreen}
 * @returns {{id, title, subtitle, rows}}
 */
export function screenTarget(ctx) {
  const { store, editor } = ctx;
  const els = editor.selectedElements();
  if (!els.length) return screenRows(ctx);
  const kinds = [...new Set(els.map((e) => e.kind))];
  const one = els.length === 1 ? els[0] : null;
  const val = (k) => {
    const vs = new Set(els.map((e) => JSON.stringify(e[k] ?? null)));
    return vs.size === 1 ? { value: els[0][k], mixed: false } : { value: undefined, mixed: true };
  };
  const set = (key, label, conv = (v) => v) => (v) => {
    editor.editElements(`Set ${label}`, (el) => setField(el, key, conv(v)));
  };
  const historians = store.project.data?.historian?.map((x) => x.name) ?? [];
  const loggers = store.project.data?.logger?.map((x) => x.name) ?? [];
  const rows = [];
  const push = (r) => rows.push(r);

  // General
  push({
    key: 'id', label: 'Name', cat: 'General', type: one ? 'text' : 'readonly', value: one ? one.id : `${els.length} objects`,
    desc: 'Unique name of the object on this screen. Problems and the Objects list refer to it.',
    onChange: (v) => renameElement(ctx, one, v),
  });
  push({ key: 'kind', label: 'Object type', cat: 'General', type: 'readonly', value: kinds.map((k) => KIND_INFO[k]?.title ?? k).join(', '), desc: 'Element kind in the project model.' });
  if (has(kinds, 'text')) push({ key: 'text', label: 'Text', cat: 'General', type: 'text', ...val('text'), desc: 'Text shown on the label or button.', onChange: set('text', 'text') });
  if (has(kinds, 'symbol')) {
    const lib = listSymbols();
    const options = ['', ...lib.map((s) => s.name)];
    push({
      key: 'symbol', label: 'Symbol', cat: 'General', type: 'text', ...val('symbol'), datalist: options.filter(Boolean),
      desc: `Library symbol (${lib.map((s) => s.name).join(', ')}), optionally with inline parameters such as coriolis-meter?product=%23FF4D4D. Imported names stay as movicon:<Name> until mapped.`,
      onChange: set('symbol', 'symbol', (v) => v.trim()),
    });
  }
  if (has(kinds, 'editable')) push({ key: 'editable', label: 'Operator input', cat: 'General', type: 'bool', ...val('editable'), desc: 'Operators can type a new value into the display (the tag must be writable).', onChange: set('editable', 'operator input', (v) => (v ? true : undefined)) });
  if (has(kinds, 'source')) push({ key: 'source', label: 'Data source', cat: 'General', type: 'enum', options: ['', ...new Set([...historians, ...loggers])], ...val('source'), desc: 'Historian or data logger whose recorded rows the table shows.', onChange: set('source', 'data source') });
  if (has(kinds, 'area')) push({ key: 'area', label: 'Alarm area', cat: 'General', type: 'text', ...val('area'), placeholder: '(all areas)', datalist: [...new Set(store.project.alarms.map((a) => a.area).filter(Boolean))], desc: 'Show only alarms of this area; blank shows all.', onChange: set('area', 'alarm area', (v) => v.trim()) });
  if (has(kinds, 'historian')) push({ key: 'historian', label: 'Historian', cat: 'General', type: 'enum', options: ['', ...historians], ...val('historian'), desc: 'Historian the trend reads past values from.', onChange: set('historian', 'historian') });
  if (has(kinds, 'span_s')) push({ key: 'span_s', label: 'Time span (s)', cat: 'General', type: 'number', integer: true, min: 1, ...val('span_s'), desc: 'Visible time window in seconds.', onChange: set('span_s', 'time span') });

  // Tag
  if (has(kinds, 'tag')) {
    push({
      key: 'tag', label: 'Tag', cat: 'Tag', type: 'tag', ...val('tag'),
      desc: kinds.includes('display') ? 'Tag whose value the display shows (and writes, when operator input is on).'
        : 'Tag that animates the object (pipeline flow, symbol state).',
      onChange: set('tag', 'tag', (v) => v.trim()),
    });
  }
  if (has(kinds, 'flow_color')) {
    push({ key: 'flow_color', label: 'Flow colour', cat: 'Animations', type: 'colour', ...val('flow_color'), placeholder: '#7FD0FF', desc: 'Colour of the moving dashes while the pipe flows: the product colour (Gasoline #FF4D4D, Jet A-1 #F4F4F4, Gasoil #555555, HFO #151515).', onChange: set('flow_color', 'flow colour') });
    push({ key: 'flow_min', label: 'Flowing above', cat: 'Animations', type: 'number', ...val('flow_min'), placeholder: '0', desc: 'The pipe animates while the tag is above this value (for example 1 mTon/hr, so a meter at standstill does not animate). Blank: any non-zero value.', onChange: set('flow_min', 'flow threshold', (v) => (v === '' || v === null || Number.isNaN(v) ? undefined : v)) });
    push({ key: 'flow_reverse', label: 'Reverse direction', cat: 'Animations', type: 'bool', ...val('flow_reverse'), desc: 'Product moves against the drawing direction of the points (first point to last).', onChange: set('flow_reverse', 'flow direction', (v) => (v ? true : undefined)) });
  }
  if (has(kinds, 'tags') && one) {
    push({ key: 'tags', label: 'Pens', cat: 'Tag', type: 'custom', wide: true, desc: 'Tags drawn as pens, in legend order.', render: () => pensEditor(ctx, one) });
  }

  // Style
  if (has(kinds, 'unit')) push({ key: 'unit', label: 'Unit', cat: 'Style', type: 'text', ...val('unit'), datalist: unitsInProject(store.project), desc: 'Engineering unit shown right of the value.', onChange: set('unit', 'unit', (v) => v.trim()) });
  if (has(kinds, 'format')) push({ key: 'format', label: 'Format', cat: 'Style', type: 'text', ...val('format'), datalist: FORMATS, desc: '.NET numeric format: # and 0 placeholders, decimals, grouping, %, E+0, or standard F2 / N0.', onChange: set('format', 'format', (v) => v.trim()) });
  if (has(kinds, 'fill')) push({ key: 'fill', label: 'Fill colour', cat: 'Style', type: 'colour', ...val('fill'), placeholder: '#EFEFF0', desc: 'Pipe body colour.', onChange: set('fill', 'fill colour') });
  if (has(kinds, 'stroke')) push({ key: 'stroke', label: 'Edge colour', cat: 'Style', type: 'colour', ...val('stroke'), placeholder: '#5E5E5E', desc: 'Pipe outline colour.', onChange: set('stroke', 'edge colour') });
  if (has(kinds, 'thickness')) push({ key: 'thickness', label: 'Pipe size', cat: 'Style', type: 'number', min: 1, ...val('thickness'), desc: 'Pipe diameter in screen units (Petrogas uses about 26).', onChange: set('thickness', 'pipe size') });

  // Fonts (not in the model yet)
  for (const [k, l] of [['font', 'Font family'], ['font_size', 'Font size'], ['bold', 'Bold'], ['italic', 'Italic'], ['text_colour', 'Text colour']]) {
    if (kinds.some((k2) => ['label', 'button', 'display'].includes(k2))) push({ key: k, label: l, cat: 'Fonts', dim: true, value: 'automatic', desc: 'Not in the project model yet: the renderer sizes text from the object height.' });
  }

  // Layout
  const geo = (key, label, desc, extra = {}) => push({
    key, label, cat: 'Layout', type: 'number', ...val(key), desc, ...extra,
    onChange: (v) => editor.editElements(`Set ${label}`, (el) => {
      if (key === 'rotation') { el.rotation = ((v % 360) + 360) % 360; if (!el.rotation) delete el.rotation; return; }
      if ((key === 'w' || key === 'h') && el.kind === 'pipeline' && el.points) {
        const nw = key === 'w' ? v : el.w;
        const nh = key === 'h' ? v : el.h;
        el.points = el.points.map(([x, y]) => [tidy(x * (el.w ? nw / el.w : 1)), tidy(y * (el.h ? nh / el.h : 1))]);
      }
      el[key] = v;
    }),
  });
  geo('x', 'Left', 'Distance from the left edge of the screen.');
  geo('y', 'Top', 'Distance from the top edge of the screen.');
  geo('w', 'Width', 'Object width.', { min: 1 });
  geo('h', 'Height', 'Object height.', { min: 1 });
  geo('rotation', 'Rotation (°)', 'Clockwise rotation about the object centre, in degrees. Drag the yellow handle on the canvas; 15° steps unless Shift is held.');
  if (has(kinds, 'points') && one) {
    push({
      key: 'points', label: 'Points', cat: 'Layout', type: 'textarea', value: (one.points ?? []).map((p) => p.join(', ')).join('\n'),
      desc: 'Polyline points relative to the object, one "x, y" pair per line.',
      onChange: (text) => {
        const pts = parsePoints(text);
        if (!pts) return false;
        editor.editElements('Set points', (el) => { el.points = pts; });
        return true;
      },
    });
  }

  // Visibility / User Access (not in the model yet)
  push({ key: 'visible', label: 'Visible', cat: 'Visibility', dim: true, value: 'always', desc: 'Visibility animations are not in the project model yet.' });
  push({ key: 'visible_tag', label: 'Visibility tag', cat: 'Visibility', dim: true, value: '', desc: 'Visibility animations are not in the project model yet.' });
  push({ key: 'access_level', label: 'Access level', cat: 'User Access', dim: true, value: 'everyone', desc: 'Users and access levels are not supported yet.' });

  // Animations: what the tag binding does
  for (const el of els.slice(0, 1)) {
    const anim = animationOf(el);
    push({ key: 'anim', label: 'Animation', cat: 'Animations', type: 'readonly', value: els.length > 1 ? '(per object)' : anim, desc: 'Animations come from the tag binding: value for displays, flow for pipelines, state for symbols. See the Animations panel.' });
  }

  // Commands
  if (one) push({ key: 'on_click', label: 'On click', cat: 'Commands', type: 'custom', wide: true, desc: 'Commands run in order when an operator clicks the object at runtime.', render: () => commandsEditor(ctx, one) });
  else push({ key: 'on_click', label: 'On click', cat: 'Commands', type: 'readonly', value: '(select one object)', desc: 'Commands are edited per object.' });

  return {
    id: `${editor.part}|${els.map((e) => e.id).join(',')}`,
    title: one ? one.id : `${els.length} objects`,
    subtitle: kinds.map((k) => KIND_INFO[k]?.title ?? k).join(', '),
    rows,
  };
}

function screenRows(ctx) {
  const { store, editor } = ctx;
  const scr = editor.screen();
  const edit = (label, fn) => store.edit(editor.part, label, fn);
  const isMain = store.project.meta.main_screen === scr.name;
  return {
    id: `${editor.part}|screen`,
    title: scr.name,
    subtitle: 'Screen',
    rows: [
      { key: 'name', label: 'Name', cat: 'General', type: 'readonly', value: scr.name, desc: 'Rename the screen from the Project Explorer (F2); references are updated.' },
      { key: 'main', label: 'Startup screen', cat: 'General', type: 'bool', value: isMain, desc: 'Screen the runtime shows first (project settings).', onChange: (v) => store.edit('meta', 'Startup screen', (m) => { if (v) m.main_screen = scr.name; else if (m.main_screen === scr.name) delete m.main_screen; }) },
      { key: 'count', label: 'Objects', cat: 'General', type: 'readonly', value: String(scr.element.length) },
      { key: 'legacy_id', label: 'Imported id', cat: 'General', type: 'readonly', value: scr.legacy_id ?? '', desc: 'Identifier in the Movicon project this screen was imported from.' },
      { key: 'width', label: 'Width', cat: 'Layout', type: 'number', min: 1, value: scr.width, desc: 'Screen width in screen units.', onChange: (v) => (v > 0 ? edit('Screen width', (s) => { s.width = v; }) : false) },
      { key: 'height', label: 'Height', cat: 'Layout', type: 'number', min: 1, value: scr.height, desc: 'Screen height in screen units.', onChange: (v) => (v > 0 ? edit('Screen height', (s) => { s.height = v; }) : false) },
      { key: 'background', label: 'Background', cat: 'Style', type: 'colour', value: scr.background ?? '', placeholder: '#3F3F3F', desc: 'CSS colour or linear-gradient(top, bottom), e.g. linear-gradient(#6E7072, #282526).', onChange: (v) => edit('Background', (s) => setField(s, 'background', v || undefined)) },
      { key: 'popup', label: 'Opens as', cat: 'Style', type: 'readonly', value: popupHint(store.project, scr), desc: 'Screens under half the startup screen open as floating panels in the HMI.' },
    ],
  };
}

function popupHint(project, scr) {
  const main = project.screens.find((s) => s.name === project.meta.main_screen);
  if (!main || main.name === scr.name) return 'full screen';
  const smaller = scr.width < main.width / 2 || scr.height < main.height / 2;
  return smaller && scr.width * scr.height < (main.width * main.height) / 2 ? 'floating panel' : 'full screen';
}

export function animationOf(el) {
  if (el.kind === 'display') return el.tag ? `Value ← ${el.tag}` : 'none (no tag)';
  if (el.kind === 'pipeline') return el.tag ? `Flow ← ${el.tag}${Number.isFinite(el.flow_min) ? ` > ${el.flow_min}` : ''}${el.flow_color ? ` (${el.flow_color})` : ''}` : 'none';
  if (el.kind === 'symbol') return el.tag ? `State ← ${el.tag}` : 'static';
  if (el.kind === 'trend') return `${(el.tags ?? []).length} pen(s)`;
  return 'none';
}

function renameElement(ctx, el, v) {
  const id = String(v).trim();
  if (!el || id === el.id) return true;
  const scr = ctx.editor.screen();
  if (!id || !/^[A-Za-z0-9_.-]+$/.test(id)) { ctx.store.log(`Invalid object name "${id}": use letters, digits, '_', '-' or '.'`, 'warn'); return false; }
  if (scr.element.some((e) => e.id === id)) { ctx.store.log(`Object name "${id}" is already used on ${scr.name}`, 'warn'); return false; }
  ctx.store.edit(ctx.editor.part, `Rename ${el.id}`, (s) => {
    const target = s.element.find((e) => e.id === el.id);
    if (target) target.id = id;
  });
  ctx.editor.select([id]);
  return true;
}

export function parsePoints(text) {
  const pts = [];
  for (const line of text.split(/[\n;]+/)) {
    const t = line.trim();
    if (!t) continue;
    const m = /^\[?\s*(-?[\d.]+(?:e-?\d+)?)\s*[, ]\s*(-?[\d.]+(?:e-?\d+)?)\s*\]?,?$/i.exec(t);
    if (!m) return null;
    pts.push([Number(m[1]), Number(m[2])]);
  }
  return pts.length >= 2 && pts.every((p) => p.every(Number.isFinite)) ? pts : null;
}

function unitsInProject(project) {
  const set = new Set();
  for (const s of project.screens) for (const el of s.element ?? []) if (el.unit) set.add(el.unit);
  return [...set].sort();
}

function pensEditor(ctx, el) {
  const { store, editor } = ctx;
  const tags = () => store.project.tags;
  const wrap = h('div', { class: 'st-cmds' });
  const list = el.tags ?? [];
  list.forEach((t, i) => {
    const input = tagInput({
      value: t, tags, allowNone: false,
      onChange: (v) => editor.editElements('Set pen', (e) => { e.tags = (e.tags ?? []).map((x, j) => (j === i ? v : x)).filter(Boolean); }),
    });
    const up = iconButton('chevron-down', 'Move up', () => editor.editElements('Move pen', (e) => { if (i > 0) [e.tags[i - 1], e.tags[i]] = [e.tags[i], e.tags[i - 1]]; }));
    up.querySelector('svg').style.transform = 'rotate(180deg)';
    const del = iconButton('delete', 'Remove pen', () => editor.editElements('Remove pen', (e) => { e.tags.splice(i, 1); }));
    wrap.appendChild(h('div', { class: 'st-cmd', style: { gridTemplateColumns: '1fr auto auto' } }, input, up, del));
  });
  const add = h('button', { class: 'st-btn add', type: 'button', text: '+ Add pen' });
  add.addEventListener('click', async () => {
    const { pickTag } = await import('./ui.js');
    const t = await pickTag(add, tags(), { allowNone: false, filter: (x) => x.type !== 'string' && x.type !== 'bool' });
    if (t) editor.editElements('Add pen', (e) => { e.tags = [...(e.tags ?? []), t]; });
  });
  wrap.appendChild(add);
  return wrap;
}

/** On-click command list editor: add, remove, reorder, kind-specific fields. */
export function commandsEditor(ctx, el) {
  const { store, editor } = ctx;
  const cmds = clone(el.on_click ?? []);
  const wrap = h('div', { class: 'st-cmds' });
  const save = (label, next) => editor.editElements(label, (e) => {
    if (next.length) e.on_click = next; else delete e.on_click;
  });
  const field = (label, input) => [h('label', { text: label }), input];
  cmds.forEach((c, i) => {
    const update = (patch, label = 'Edit command') => {
      const next = clone(cmds);
      next[i] = { ...next[i], ...patch };
      for (const [k, v] of Object.entries(patch)) if (v === undefined || v === '') delete next[i][k];
      save(label, next);
    };
    const action = h('select', {}, ...['open-screen', 'write-tag', 'report'].map((a) => h('option', { value: a, text: a })));
    action.value = c.action;
    action.addEventListener('change', () => {
      const next = clone(cmds);
      next[i] = defaultCommand(action.value, store.project);
      save('Change command', next);
    });
    const rows = [...field('Action', action)];
    if (c.action === 'open-screen') {
      const scr = h('select', {}, ...store.project.screens.map((s) => h('option', { value: s.name, text: s.name })));
      if (!store.project.screens.some((s) => s.name === c.screen)) scr.appendChild(h('option', { value: c.screen ?? '', text: `${c.screen ?? ''} (missing)` }));
      scr.value = c.screen ?? '';
      scr.addEventListener('change', () => update({ screen: scr.value }));
      const go = iconButton('screen', 'Open this screen', () => ctx.openScreen?.(c.screen));
      rows.push(...field('Screen', h('div', { class: 'st-picker' }, scr, go)));
    } else if (c.action === 'write-tag') {
      rows.push(...field('Tag', tagInput({ value: c.tag ?? '', tags: () => store.project.tags, allowNone: false, onChange: (v) => update({ tag: v }) })));
      const value = h('input', { type: 'text', value: c.value ?? '', spellcheck: 'false' });
      value.addEventListener('change', () => update({ value: value.value }));
      rows.push(...field('Value', value));
      const mode = h('select', {}, ...MODES.map((m) => h('option', { value: m, text: m })));
      mode.value = c.mode ?? 'set';
      mode.addEventListener('change', () => update({ mode: mode.value === 'set' ? undefined : mode.value }));
      rows.push(...field('Mode', mode));
    } else if (c.action === 'report') {
      const rep = h('select', {}, ...(store.project.meta.reports ?? []).map((r) => h('option', { value: r, text: r })));
      if (!(store.project.meta.reports ?? []).includes(c.report)) rep.appendChild(h('option', { value: c.report ?? '', text: `${c.report ?? ''} (missing)` }));
      rep.value = c.report ?? '';
      rep.addEventListener('change', () => update({ report: rep.value }));
      rows.push(...field('Report', rep));
      const tags = h('input', { type: 'text', value: (c.tags ?? []).join(', '), placeholder: 'tag, tag, …', spellcheck: 'false' });
      tags.addEventListener('change', () => update({ tags: tags.value.split(/[\s,]+/).filter(Boolean) }));
      rows.push(...field('Tags', tags));
      const out = h('input', { type: 'text', value: c.output_dir ?? '', placeholder: '(default)', spellcheck: 'false' });
      out.addEventListener('change', () => update({ output_dir: out.value.trim() || undefined }));
      rows.push(...field('Output', out));
    }
    const move = (d) => {
      const j = i + d;
      if (j < 0 || j >= cmds.length) return;
      const next = clone(cmds);
      [next[i], next[j]] = [next[j], next[i]];
      save('Reorder commands', next);
    };
    const up = iconButton('chevron-down', 'Move up', () => move(-1));
    up.querySelector('svg').style.transform = 'rotate(180deg)';
    const down = iconButton('chevron-down', 'Move down', () => move(1));
    const del = iconButton('delete', 'Remove command', () => save('Remove command', cmds.filter((_, j) => j !== i)));
    rows.push(h('div', { class: 'bar' }, h('span', { text: `#${i + 1}`, style: { marginRight: 'auto', color: 'var(--st-faint)' } }), up, down, del));
    wrap.appendChild(h('div', { class: 'st-cmd' }, ...rows));
  });
  const add = h('button', { class: 'st-btn add', type: 'button', text: '+ Add command' });
  add.addEventListener('click', () => save('Add command', [...cmds, defaultCommand('open-screen', store.project)]));
  wrap.appendChild(add);
  return wrap;
}

function defaultCommand(action, project) {
  if (action === 'open-screen') return { action, screen: project.meta.main_screen ?? project.screens[0]?.name ?? '' };
  if (action === 'write-tag') return { action, tag: project.tags.find((t) => t.access && t.access !== 'read')?.name ?? project.tags[0]?.name ?? '', value: '1' };
  return { action: 'report', report: project.meta.reports?.[0] ?? '', tags: [] };
}

export { isIdentifier };
