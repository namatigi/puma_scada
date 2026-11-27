// Browser unit checks for the studio's pure modules. Open tests.html; the page
// title says PASS/FAIL and window.__STUDIO_TESTS__ holds the counts.

import * as g from './geometry.js';
import { tomlValue, tomlTableBody, elementsToml, tomlArrayOfTables, tomlString } from './toml.js';
import { newElement, nextId, setField, whereUsed, usageCounts, renameTagRefs, renameScreenRefs, localScreenChecks, parseItem, isScreenName, ELEMENT_KINDS } from './model.js';
import { createStore } from './store.js';
import { keyOf, fuzzyScore, createCommands } from './commands.js';
import { parsePoints } from './screen-props.js';
import { setCondition } from './alarms-doc.js';
import { createStudioApi } from './studio-api.js';

const results = [];
let group = '';
async function check(name, fn) {
  try { await fn(); results.push({ group, name, ok: true }); } catch (err) { results.push({ group, name, ok: false, err: err.message }); }
}
function eq(a, e, msg = '') {
  const x = JSON.stringify(a);
  const y = JSON.stringify(e);
  if (x !== y) throw new Error(`${msg}expected ${y}, got ${x}`);
}
const near = (a, b, msg = '') => { if (Math.abs(a - b) > 1e-6) throw new Error(`${msg}expected ${b}, got ${a}`); };
const ok = (v, msg = 'expected truthy') => { if (!v) throw new Error(msg); };

const project = () => ({
  meta: { name: 'T', culture: 'en-us', main_screen: 'Main', sampling: { fast_ms: 500, slow_ms: 5000, publish_ms: 250 } },
  tags: [{ name: 'Flow', type: 'f32' }, { name: 'Pump', type: 'bool', access: 'read-write' }],
  alarms: [{ name: 'Hi', tag: 'Flow', when: 'above', limit: 5, message: 'hi' }],
  drivers: [],
  data: { historian: [{ name: 'H1', database: 'db', max_age_s: 10 }], logger: [{ name: 'L', database: 'db', table: 't', record: 'on-change', max_age_s: 1, column: [{ name: 'c', tag: 'Flow' }] }] },
  screens: [
    { name: 'Main', width: 800, height: 600, element: [
      { id: 'D1', kind: 'display', tag: 'Flow', x: 0, y: 0, w: 100, h: 40 },
      { id: 'B1', kind: 'button', text: 'Go', x: 0, y: 50, w: 100, h: 40, on_click: [{ action: 'open-screen', screen: 'Pop' }, { action: 'write-tag', tag: 'Pump', value: '1', mode: 'toggle' }] },
      { id: 'T1', kind: 'trend', tags: ['Flow', 'Pump'], span_s: 60, x: 0, y: 100, w: 300, h: 200 },
    ] },
    { name: 'Pop', width: 300, height: 200, element: [] },
  ],
});

async function run() {
  group = 'geometry';
  await check('snap and tidy', () => { eq(g.snap(14, 10), 10); eq(g.snap(15, 10), 20); eq(g.snap(14, 10, false), 14); eq(g.tidy(1.23456), 1.235); });
  await check('bounds of a rotated element', () => {
    const b = g.bounds({ x: 0, y: 0, w: 100, h: 20, rotation: 90 });
    near(b.x, 40); near(b.y, -40); near(b.w, 20); near(b.h, 100);
  });
  await check('hit test respects rotation', () => {
    const el = { x: 0, y: 0, w: 100, h: 20, rotation: 90 };
    ok(g.hitElement(el, 50, -30), 'inside after rotation');
    ok(!g.hitElement(el, 5, 10), 'outside after rotation');
  });
  await check('resize from the south-east corner', () => eq(g.resizeRect({ x: 10, y: 10, w: 100, h: 50 }, 'se', 20, 10), { x: 10, y: 10, w: 120, h: 60 }));
  await check('resize from the north-west keeps the far corner', () => eq(g.resizeRect({ x: 10, y: 10, w: 100, h: 50 }, 'nw', 10, 5), { x: 20, y: 15, w: 90, h: 45 }));
  await check('resize clamps to the minimum size', () => { const r = g.resizeRect({ x: 0, y: 0, w: 10, h: 10 }, 'e', -50, 0); eq(r.w, g.MIN_SIZE); eq(r.x, 0); });
  await check('resize with aspect lock', () => { const r = g.resizeRect({ x: 0, y: 0, w: 100, h: 50 }, 'se', 100, 0, { keepAspect: true }); eq([r.w, r.h], [200, 100]); });
  await check('resize of a rotated element grows along its own axis and keeps the anchor', () => {
    const el = { x: 0, y: 0, w: 100, h: 20, rotation: 90 };
    // East handle of a 90° element points down on screen.
    const r = g.resizeRect(el, 'e', 0, 30);
    near(r.w, 130); near(r.h, 20);
    const before = g.corners(el)[0];
    const after = g.corners({ ...r, rotation: 90 })[0];
    near(after[0], before[0]); near(after[1], before[1]);
  });
  await check('angleFrom: 0 up, 90 right', () => { eq(g.angleFrom(0, 0, 0, -10), 0); eq(g.angleFrom(0, 0, 10, 0), 90); eq(g.angleFrom(0, 0, -10, 0), 270); });
  const els = [{ id: 'a', x: 10, y: 10, w: 50, h: 20 }, { id: 'b', x: 100, y: 40, w: 30, h: 30 }, { id: 'c', x: 300, y: 5, w: 10, h: 10 }];
  await check('align left / bottom / center to the primary', () => {
    eq(g.align(els, 'left'), [{ id: 'b', x: 10, y: 40 }, { id: 'c', x: 10, y: 5 }]);
    eq(g.align(els, 'bottom').map((p) => p.y), [0, 20]);
    eq(g.align(els, 'center').map((p) => p.x), [20, 30]);
  });
  await check('distribute horizontally gives equal gaps', () => {
    const p = g.distribute(els, 'h');
    eq(p.length, 1);
    // total span 10..310 = 300, sizes 90 -> gap 105; b starts at 10+50+105 = 165
    eq(p[0], { id: 'b', x: 165, y: 40 });
  });
  await check('same size keeps centres', () => eq(g.sameSize(els.slice(0, 2), 'both'), [{ id: 'b', x: 90, y: 40, w: 50, h: 20 }].map((x) => ({ ...x, y: 45 }))));
  await check('centre on screen', () => eq(g.centreOnScreen([{ id: 'a', x: 0, y: 0, w: 100, h: 100 }], { width: 1000, height: 500 }), [{ id: 'a', x: 450, y: 200 }]));
  await check('scale pipeline points', () => eq(g.scalePoints([[0, 13], [200, 13]], 200, 26, 100, 26), [[0, 13], [100, 13]]));
  await check('marquee containment and touch', () => {
    const r = g.rectFrom(50, 50, 0, 0);
    eq(r, { x: 0, y: 0, w: 50, h: 50 });
    ok(g.contains(r, { x: 10, y: 10, w: 5, h: 5 }));
    ok(!g.contains(r, { x: 40, y: 40, w: 20, h: 5 }));
    ok(g.intersects(r, { x: 40, y: 40, w: 20, h: 5 }));
  });

  group = 'toml';
  await check('values like the Rust writer', () => {
    eq(tomlValue(1), '1.0'); eq(tomlValue(1.5), '1.5'); eq(tomlValue(502, 'port'), '502'); eq(tomlValue(true), 'true');
    eq(tomlValue([[3, 0.5]]), '[[3.0, 0.5]]'); eq(tomlString('a"b'), '"a\\"b"'); eq(tomlString('C:\\x'), "'C:\\x'");
  });
  await check('element block with on_click array of tables', () => {
    const txt = elementsToml([{ id: 'B', kind: 'button', text: 'Go', x: 1, y: 2, w: 3, h: 4, on_click: [{ action: 'open-screen', screen: 'S' }] }]);
    eq(txt, '[[element]]\nid = "B"\nkind = "button"\ntext = "Go"\nx = 1.0\ny = 2.0\nw = 3.0\nh = 4.0\n\n[[element.on_click]]\naction = "open-screen"\nscreen = "S"');
  });
  await check('tag with address sub-table', () => {
    const txt = tomlArrayOfTables('tag', [{ name: 'A', type: 'u16', address: { kind: 'modbus', driver: 'd', station: 's', area: 'holding-register', register: 7 } }]);
    ok(txt.includes('[tag.address]\nkind = "modbus"'), txt);
    ok(txt.includes('register = 7\n') || txt.endsWith('register = 7'), txt);
  });
  await check('nulls are skipped', () => eq(tomlTableBody({ a: 1, b: null, c: undefined }), 'a = 1.0'));

  group = 'model';
  await check('every element kind has a template', () => {
    for (const k of ELEMENT_KINDS) { const el = newElement(k.kind, project()); ok(el.w > 0 && el.h > 0, k.kind); eq(el.kind, k.kind); }
    eq(newElement('history-table', project()).source, 'H1');
    eq(newElement('symbol', project(), 'tank').symbol, 'tank');
  });
  await check('next id skips used numbers', () => {
    const s = { element: [{ id: 'EditDisplay1' }, { id: 'EditDisplay7' }] };
    eq(nextId(s, 'EditDisplay'), 'EditDisplay8');
    eq(nextId(s, 'EditDisplay', new Set(['EditDisplay8'])), 'EditDisplay9');
    eq(nextId({ element: [] }, 'Button'), 'Button1');
  });
  await check('optional fields are removed when blank', () => {
    const el = { tag: 'X', text: 'a' };
    setField(el, 'tag', ''); setField(el, 'text', '');
    eq(el, { text: '' });
  });
  await check('where used covers screens, commands, trends, alarms and loggers', () => {
    const u = whereUsed(project(), 'Flow').map((x) => `${x.kind}:${x.id}`);
    eq(u, ['element:D1', 'element:T1', 'alarm:Hi', 'logger:L']);
    eq(whereUsed(project(), 'Pump').map((x) => x.role), ['on click: write (toggle)', 'trend pen']);
    eq(usageCounts(project()).get('Flow'), 4);
  });
  await check('tag rename follows references', () => {
    const p = project();
    const r = renameTagRefs(p, 'Pump', 'Pump1');
    eq(r, { screens: ['Main'], alarms: false });
    eq(p.screens[0].element[1].on_click[1].tag, 'Pump1');
    eq(p.screens[0].element[2].tags, ['Flow', 'Pump1']);
  });
  await check('screen rename retargets buttons', () => {
    const p = project();
    eq(renameScreenRefs(p, 'Pop', 'Popup'), ['Main']);
    eq(p.screens[0].element[1].on_click[0].screen, 'Popup');
  });
  await check('local checks find missing tags, screens and duplicate ids', () => {
    const p = project();
    p.screens[0].element.push({ id: 'D1', kind: 'display', tag: 'Nope', x: 0, y: 0, w: 1, h: 1 });
    p.screens[0].element[1].on_click[0].screen = 'Gone';
    const codes = localScreenChecks(p, p.screens[0]).map((d) => d.code).sort();
    eq(codes, ['L032', 'L034', 'L040']);
  });
  await check('problem item parsing', () => {
    eq(parseItem('element "EditDisplay3"'), { type: 'element', name: 'EditDisplay3' });
    eq(parseItem('tag "A" column'), { type: 'tag', name: 'A' });
    eq(parseItem('main_screen'), { type: 'main_screen', name: '' });
  });
  await check('screen names match the server rules', () => {
    for (const s of ['Manifold', 'FM_01', 'a-b.c']) ok(isScreenName(s), s);
    for (const s of ['', '..', 'a b', 'x/y', '.x', 'x.', 'a..b']) ok(!isScreenName(s), s);
  });
  await check('pipeline points parser', () => {
    eq(parsePoints('0, 13\n200, 13'), [[0, 13], [200, 13]]);
    eq(parsePoints('[3.0, 1.5]; [4, -2]'), [[3, 1.5], [4, -2]]);
    eq(parsePoints('1, 2'), null);
    eq(parsePoints('a, b\n1,2'), null);
  });
  await check('alarm condition switch keeps the limit', () => {
    eq(setCondition({ when: 'above', limit: 5 }, 'equals'), { when: 'equals', value: 5 });
    eq(setCondition({ when: 'equals', value: 5 }, 'is-true'), { when: 'is-true' });
  });

  group = 'store';
  await check('edit, dirty, undo, redo', () => {
    const st = createStore();
    st.load(project());
    ok(!st.isDirty('screen:Main'));
    st.edit('screen:Main', 'Move', (s) => { s.element[0].x = 5; });
    ok(st.isDirty('screen:Main'));
    eq(st.undoLabel('screen:Main'), 'Move');
    st.undo('screen:Main');
    eq(st.getPart('screen:Main').element[0].x, 0);
    ok(!st.isDirty('screen:Main'), 'undo back to saved is clean');
    st.redo('screen:Main');
    eq(st.getPart('screen:Main').element[0].x, 5);
    st.markSaved('screen:Main');
    ok(!st.isDirty('screen:Main'));
  });
  await check('no-op edits are not recorded', () => {
    const st = createStore();
    st.load(project());
    eq(st.edit('tags', 'Nothing', () => {}), false);
    ok(!st.canUndo('tags'));
  });
  await check('coalesced nudges are one undo step', () => {
    const st = createStore();
    st.load(project());
    for (let i = 0; i < 5; i += 1) st.edit('screen:Main', 'Nudge', (s) => { s.element[0].x += 1; }, { coalesce: 'nudge' });
    eq(st.getPart('screen:Main').element[0].x, 5);
    st.undo('screen:Main');
    eq(st.getPart('screen:Main').element[0].x, 0);
    ok(!st.canUndo('screen:Main'));
  });
  await check('added, renamed and removed screens show up as pending saves and deletes', () => {
    const st = createStore();
    st.load(project());
    st.addScreen({ name: 'New', width: 1, height: 1, element: [] });
    st.renameScreen('Pop', 'Popup');
    const pend = st.pending();
    eq({ dirty: pend.dirty.sort(), deleted: pend.deleted }, { dirty: ['screen:New', 'screen:Popup'], deleted: ['screen:Pop'] });
    st.removeScreen('New');
    eq(st.pending().dirty, ['screen:Popup']);
  });
  await check('edits notify listeners', () => {
    const st = createStore();
    st.load(project());
    const seen = [];
    st.on('changed', (e) => seen.push(e.part));
    st.edit('alarms', 'x', (a) => { a[0].limit = 9; });
    eq(seen, ['alarms']);
  });

  group = 'commands';
  await check('key normalisation', () => {
    eq(keyOf({ key: 's', ctrlKey: true }), 'Ctrl+S');
    eq(keyOf({ key: 'Z', ctrlKey: true, shiftKey: true }), 'Ctrl+Shift+Z');
    eq(keyOf({ key: 'F5' }), 'F5');
    eq(keyOf({ key: 'Delete' }), 'Delete');
  });
  await check('shortcut lookup skips plain keys while typing', () => {
    const c = createCommands();
    c.register({ id: 'del', label: 'Delete', kbd: 'Delete', run() {} });
    c.register({ id: 'save', label: 'Save', kbd: 'Ctrl+S', run() {} });
    eq(c.forKey({ key: 'Delete' })?.id, 'del');
    eq(c.forKey({ key: 'Delete' }, { inText: true }), null);
    eq(c.forKey({ key: 's', ctrlKey: true }, { inText: true })?.id, 'save');
  });
  await check('disabled commands do not run', async () => {
    const c = createCommands();
    let n = 0;
    c.register({ id: 'x', label: 'X', run: () => { n += 1; }, enabled: () => false });
    eq(c.run('x'), false);
    await Promise.resolve();
    eq(n, 0);
  });
  await check('fuzzy score ranks prefix over scattered', () => {
    ok(fuzzyScore('Save All', 'save') < fuzzyScore('Select All Elements', 'sae'));
    eq(fuzzyScore('Align Left', 'xyz'), -1);
  });

  group = 'studio api';
  await check('save sends the screen to its URL and keeps refusal details', async () => {
    const calls = [];
    const fake = async (url, init) => {
      calls.push([init.method, url, init.body && JSON.parse(init.body).name]);
      if (url.includes('Bad')) return { ok: false, status: 400, text: async () => JSON.stringify({ ok: false, error: 'invalid screen' }) };
      return { ok: true, status: 200, text: async () => JSON.stringify({ ok: true, file: 'screens/A.toml', diagnostics: [] }) };
    };
    const api = createStudioApi('', { fetchImpl: fake });
    const res = await api.saveScreen({ name: 'A b', width: 1, height: 1 });
    eq(res.file, 'screens/A.toml');
    eq(calls[0], ['PUT', '/api/studio/screens/A%20b', 'A b']);
    let err = null;
    try { await api.saveScreen({ name: 'Bad', width: 1, height: 1 }); } catch (e) { err = e; }
    eq([err?.status, err?.message], [400, 'invalid screen']);
  });

  await check('saves send the loaded file version as If-Match; 409 conflicts keep the current version', async () => {
    const headers = [];
    const fake = async (url, init) => {
      headers.push(init.headers['if-match'] ?? null);
      if (init.headers['if-match'] === '"old"') return { ok: false, status: 409, text: async () => JSON.stringify({ ok: false, conflict: true, version: 'new', error: 'changed on disk' }) };
      return { ok: true, status: 200, text: async () => JSON.stringify({ ok: true, file: 'tags.toml', version: 'v2', diagnostics: [] }) };
    };
    const api = createStudioApi('', { fetchImpl: fake });
    eq((await api.saveTags([], 'v1')).version, 'v2');
    await api.saveTags([]);
    let err = null;
    try { await api.deleteScreen('Main', 'old'); } catch (e) { err = e; }
    eq(headers, ['"v1"', null, '"old"']);
    eq([err?.status, err?.body?.conflict, err?.body?.version], [409, true, 'new']);
  });

  group = 'store versions';
  await check('versions per part, absent for new parts, changes detected', () => {
    const st = createStore();
    st.load(project(), [], { meta: 'm1', tags: 't1', alarms: 'absent', 'screen:Main': 's1' });
    eq(st.version('screen:Main'), 's1');
    eq(st.version('screen:New'), 'absent', 'a screen with no file yet');
    eq(st.changedOnDisk({ meta: 'm1', tags: 't1', alarms: 'absent', 'screen:Main': 's1' }), []);
    eq(st.changedOnDisk({ meta: 'm1', tags: 't2', alarms: 'absent', 'screen:Main': 's1', 'screen:Other': 'o1' }).sort(), ['screen:Other', 'tags']);
    st.setVersion('tags', 't2');
    eq(st.changedOnDisk({ meta: 'm1', tags: 't2', alarms: 'absent', 'screen:Main': 's1' }), []);
    st.setVersion('screen:Main', null);
    eq(st.changedOnDisk({ meta: 'm1', tags: 't2', alarms: 'absent' }), [], 'a deleted screen is absent on both sides');
    const old = createStore();
    old.load(project());
    eq(old.version('tags'), undefined, 'no versions from the server: saves are not checked');
    eq(old.changedOnDisk({ tags: 'x' }), []);
  });

  const failed = results.filter((r) => !r.ok);
  const summary = document.getElementById('summary');
  summary.textContent = `${failed.length ? 'FAIL' : 'PASS'} ${results.length - failed.length}/${results.length}`;
  summary.className = failed.length ? 'fail' : 'pass';
  document.title = `${failed.length ? 'FAIL' : 'PASS'} studio tests`;
  const out = document.getElementById('results');
  let last = '';
  for (const r of results) {
    if (r.group !== last) { out.insertAdjacentHTML('beforeend', `<h2>${r.group}</h2>`); last = r.group; }
    const div = document.createElement('div');
    div.className = r.ok ? 'ok' : 'bad';
    div.textContent = `${r.ok ? '✓' : '✗'} ${r.name}${r.ok ? '' : ` — ${r.err}`}`;
    out.appendChild(div);
  }
  window.__STUDIO_TESTS__ = { passed: results.length - failed.length, failed: failed.length, failures: failed };
}

run();
