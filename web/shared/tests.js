// Browser unit checks for format.js, the symbol library/map and small pure
// helpers. Open tests.html; the page title and #summary report the result
// (also exposed as window.__MC_TESTS__ for automation).

import {
  formatNumber, formatValue, parseInputValue, formatAge, niceScale, truthy, toNumber, formatDateTime,
} from './format.js';
import { SYMBOLS, drawSymbol, parseSymbolRef, symbolState, listSymbols } from './symbols.js';
import { resolveSymbol, normaliseSymbolMap, loadSymbolMap } from './symbolmap.js';
import { cssBackgroundToSvg, parseColor, mix } from './svg.js';
import { backoffDelay, createWsClient } from './client.js';
import { isPopupScreen, screenTags, createScreenView, pipeFlow } from './renderer.js';
import { sortRows, toCsv, rowsToObjects } from './historytable.js';
import { alarmSummary, sortAlarms } from './alarmlist.js';
import { timeStep, compactPoints } from './trend.js';
import { ackPlan, shownAlarms } from './alarmlist.js';
import { writeTimeoutFor } from './client.js';
import { screenFromHash } from '../hmi/hash.js';

const results = [];
let group = '';

function check(name, fn) {
  try {
    fn();
    results.push({ group, name, ok: true });
  } catch (err) {
    results.push({ group, name, ok: false, err: err.message });
  }
}
async function checkAsync(name, fn) {
  try {
    await fn();
    results.push({ group, name, ok: true });
  } catch (err) {
    results.push({ group, name, ok: false, err: err.message });
  }
}
function eq(actual, expected, msg = '') {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${msg}expected ${e}, got ${a}`);
}
function ok(cond, msg) {
  if (!cond) throw new Error(msg);
}
function throws(fn, re) {
  try {
    fn();
  } catch (err) {
    if (re && !re.test(err.message)) throw new Error(`threw "${err.message}", expected ${re}`);
    return;
  }
  throw new Error('expected an error');
}

// ---------------------------------------------------------------- format.js
group = 'format.js';
check('Movicon "#0.000000"', () => eq(formatNumber(961.06883, '#0.000000'), '961.068830'));
check('"#0.0" rounds half away', () => eq(formatNumber(12.25, '#0.0'), '12.3'));
check('"#0.0" on zero', () => eq(formatNumber(0, '#0.0'), '0.0'));
check('"#0.00" negative', () => eq(formatNumber(-3.14159, '#0.00'), '-3.14'));
check('negative that rounds to zero has no sign', () => eq(formatNumber(-0.001, '#0.00'), '0.00'));
check('"#.##" drops leading zero and trailing zeros', () => eq(formatNumber(0.5, '#.##'), '.5'));
check('"0.###" optional decimals', () => eq(formatNumber(2.1, '0.###'), '2.1'));
check('"#,##0.00" grouping', () => eq(formatNumber(1234567.891, '#,##0.00'), '1,234,567.89'));
check('"000" zero padding', () => eq(formatNumber(7, '000'), '007'));
check('"0%" percent', () => eq(formatNumber(0.256, '0%'), '26%'));
check('"0.0 \'bar\'" literal suffix', () => eq(formatNumber(3.25, "0.0 'bar'"), '3.3 bar'));
check('sections pos;neg;zero', () => {
  eq(formatNumber(5, '0.0;(0.0);zero'), '5.0');
  eq(formatNumber(-5, '0.0;(0.0);zero'), '(5.0)');
  eq(formatNumber(0, '0.0;(0.0);zero'), 'zero');
});
check('"#,##0," scales by 1000', () => eq(formatNumber(1234567, '#,##0,'), '1,235'));
check('scientific "0.00E+00"', () => eq(formatNumber(12345, '0.00E+00'), '1.23E+04'));
check('standard F2 / N0 / D4 / X', () => {
  eq(formatNumber(3.14159, 'F2'), '3.14');
  eq(formatNumber(1234.5, 'N0'), '1,235');
  eq(formatNumber(42, 'D4'), '0042');
  eq(formatNumber(255, 'X'), 'FF');
});
check('no format: general', () => {
  eq(formatNumber(1.5), '1.5');
  eq(formatNumber(100), '100');
});
check('culture with comma decimal', () => eq(formatNumber(1234.5, '#,##0.00', 'it-IT'), '1.234,50'));
check('null and NaN', () => {
  eq(formatNumber(null, '#0.0'), '----');
  eq(formatNumber(NaN, '#0.0'), 'NaN');
});
check('formatValue booleans and strings', () => {
  eq(formatValue(true), 'ON');
  eq(formatValue(0, { dataType: 'bool' }), 'OFF');
  eq(formatValue('abc'), 'abc');
  eq(formatValue(null), '----');
  eq(formatValue(2.5, { format: '#0.00' }), '2.50');
});
check('parseInputValue by type', () => {
  eq(parseInputValue('1', 'bool'), true);
  eq(parseInputValue('off', 'bool'), false);
  eq(parseInputValue(' 12.5 ', 'f32'), 12.5);
  eq(parseInputValue('12,5', 'f32', 'it-IT'), 12.5);
  eq(parseInputValue('-7', 'i16'), -7);
  eq(parseInputValue('hello', 'string'), 'hello');
});
check('parseInputValue rejects bad input', () => {
  throws(() => parseInputValue('abc', 'f32'), /not a number/);
  throws(() => parseInputValue('1.5', 'u16'), /whole number/);
  throws(() => parseInputValue('70000', 'u16'), /outside/);
  throws(() => parseInputValue('maybe', 'bool'), /ON\/OFF/);
  throws(() => parseInputValue('', 'f64'), /enter a value/);
});
check('formatAge', () => {
  eq(formatAge(4200), '4 s');
  eq(formatAge(192000), '3 min 12 s');
  eq(formatAge(2 * 3600e3 + 5 * 60e3), '2 h 5 min');
  eq(formatAge(3 * 86400e3 + 4 * 3600e3), '3 d 4 h');
});
check('formatDateTime pads', () => {
  const t = new Date(2026, 0, 2, 3, 4, 5, 6).getTime();
  eq(formatDateTime(t), '2026-01-02 03:04:05');
  eq(formatDateTime(t, { ms: true }), '2026-01-02 03:04:05.006');
});
check('niceScale', () => {
  const sc = niceScale(3.2, 97.5, 6);
  eq([sc.min, sc.max, sc.step], [0, 100, 20]);
  const flat = niceScale(5, 5);
  ok(flat.min < 5 && flat.max > 5, 'flat range is widened');
});
check('truthy / toNumber', () => {
  eq([truthy(1), truthy(0), truthy('on'), truthy('0'), truthy(null)], [true, false, true, false, false]);
  eq([toNumber('2.5'), toNumber(true), toNumber('x'), toNumber(null)], [2.5, 1, null, null]);
});

// ---------------------------------------------------------------- symbols + map
group = 'symbols';
check('every library symbol draws without error', () => {
  for (const name of Object.keys(SYMBOLS)) {
    const [w, hh] = SYMBOLS[name].size;
    for (const state of ['static', 'open', 'closed', 'on', 'off', 'fault']) {
      const g = drawSymbol(name, { w, h: hh, state, value: 42 });
      ok(g.childElementCount > 0, `${name}/${state} drew nothing`);
    }
  }
});
check('unknown symbol draws a placeholder', () => {
  const g = drawSymbol('nope', { w: 40, h: 40 });
  ok(g.classList.contains('mc-sym-unknown'), 'placeholder class');
});
check('parseSymbolRef inline params', () => {
  eq(parseSymbolRef('coriolis-meter?product=%23FF4D4D&head=white'), { name: 'coriolis-meter', params: { product: '#FF4D4D', head: 'white' } });
  eq(parseSymbolRef('ball-valve'), { name: 'ball-valve', params: {} });
});
check('symbolState', () => {
  eq(symbolState('ball-valve', { bound: false }), 'static');
  eq(symbolState('ball-valve', { bound: true, value: 1, quality: 'good' }), 'open');
  eq(symbolState('ball-valve', { bound: true, value: 0, quality: 'good' }), 'closed');
  eq(symbolState('ball-valve', { bound: true, value: 1, quality: 'bad' }), 'fault');
  eq(symbolState('pump', { bound: true, value: true }), 'on');
});
check('listSymbols has the required set', () => {
  const names = listSymbols().map((x) => x.name);
  for (const n of ['pipe-straight', 'pipe-elbow', 'pipe-tee', 'ball-valve', 'coriolis-meter', 'pump', 'tank', 'motor', 'gauge']) ok(names.includes(n), `missing ${n}`);
});

const PETROGAS_SYMBOLS = {
  'movicon:Symbol952': 'ball-valve', 'movicon:PipesPipe15': 'pipe-coupling', 'movicon:Symbol739': 'pipe-tee',
  'movicon:Symbol734': 'pipe-elbow', 'movicon:Symbol408': 'coriolis-meter', 'movicon:Symbol733': 'pipe-flange',
  'movicon:Symbol732': 'pipe-straight',
};
await checkAsync('symbol-map.json maps every Petrogas symbol to a library symbol', async () => {
  const map = await loadSymbolMap();
  for (const [from, to] of Object.entries(PETROGAS_SYMBOLS)) {
    const r = resolveSymbol(from, map);
    eq(r.name, to, `${from}: `);
    ok(r.known, `${from} resolves to an unknown symbol`);
    eq(r.source, 'map', `${from} source: `);
  }
  for (const [key, entry] of Object.entries(map.instances)) {
    ok(SYMBOLS[entry.symbol ?? 'coriolis-meter'], `instance ${key} names an unknown symbol`);
  }
});
check('pipe flow: threshold, direction, quality', () => {
  eq(pipeFlow({ v: 50, q: 'good' }, 1), { active: true, reverse: false });
  eq(pipeFlow({ v: 0.4, q: 'good' }, 1), { active: false, reverse: false }, 'standstill noise below the threshold');
  eq(pipeFlow({ v: 50, q: 'good' }, 1, true), { active: true, reverse: true });
  eq(pipeFlow({ v: -50, q: 'good' }, 1), { active: true, reverse: true }, 'negative flow runs backwards');
  eq(pipeFlow({ v: -50, q: 'good' }, 1, true), { active: true, reverse: false });
  eq(pipeFlow({ v: 50, q: 'bad' }, 1), { active: false, reverse: false }, 'bad quality never animates');
  eq(pipeFlow({ v: true, q: 'good' }), { active: true, reverse: false });
  eq(pipeFlow({ v: false, q: 'good' }), { active: false, reverse: false });
  eq(pipeFlow(undefined), { active: false, reverse: false });
});
await checkAsync('meter product colours per instance', async () => {
  const map = await loadSymbolMap();
  const colours = Object.fromEntries(['ContentControl33', 'ContentControl17', 'ContentControl21', 'ContentControl24']
    .map((id) => [resolveSymbol('movicon:Symbol408', map, { screen: 'Manifold', id }).label, resolveSymbol('movicon:Symbol408', map, { screen: 'Manifold', id }).params.product]));
  eq(colours, { Gasoline: '#FF4D4D', 'JET A-1': '#F4F4F4', Gasoil: '#555555', HFO: '#151515' });
  eq(resolveSymbol('movicon:Symbol408', map, { screen: 'Manifold', id: 'ContentControl24' }).params.head, '#FFFFFF');
});
check('inline params override the map', () => {
  const map = normaliseSymbolMap({ symbols: { 'movicon:X': { symbol: 'tank', params: { fluid: '#111' } } } });
  eq(resolveSymbol('movicon:X?fluid=%23222', map).params.fluid, '#222');
  eq(resolveSymbol('movicon:Unknown', map).source, 'unmapped');
  eq(resolveSymbol('movicon:Unknown', map).known, false);
});
await checkAsync('every symbol in the demo bundle resolves', async () => {
  const [map, bundle] = await Promise.all([loadSymbolMap(), fetch('../hmi/mock/bundle.json').then((r) => r.json())]);
  const counts = {};
  for (const sc of bundle.screens) {
    for (const el of sc.element) {
      if (el.kind !== 'symbol') continue;
      const r = resolveSymbol(el.symbol, map, { screen: sc.name, id: el.id });
      ok(r.known, `${sc.name}/${el.id} ${el.symbol} is unmapped`);
      counts[el.symbol] = (counts[el.symbol] ?? 0) + 1;
    }
  }
  eq(counts['movicon:Symbol952'], 17, 'Symbol952 count: ');
  eq(counts['movicon:Symbol408'], 4, 'Symbol408 count: ');
});

// ---------------------------------------------------------------- helpers
group = 'helpers';
check('CSS gradient to SVG', () => {
  const r = cssBackgroundToSvg('linear-gradient(#6E7072, #282526)');
  ok(r.fill.startsWith('url(#'), 'fill references a gradient');
  eq(r.defs.querySelectorAll('stop').length, 2);
  eq(cssBackgroundToSvg('#3F3F3F').fill, '#3F3F3F');
});
check('colour parsing and mixing', () => {
  eq(parseColor('#FF4D4D'), { r: 255, g: 77, b: 77, a: 1 });
  eq(parseColor('#80FF0000').r, 255);
  eq(mix('#000000', '#ffffff', 0.5), '#808080');
});
check('reconnect backoff grows and caps', () => {
  eq(backoffDelay(0, () => 0.5), 500);
  eq(backoffDelay(3, () => 0.5), 4000);
  eq(backoffDelay(20, () => 0.5), 10000);
  ok(backoffDelay(0, () => 0) === 375 && backoffDelay(0, () => 1) === 625, 'jitter ±25 %');
});
check('popup detection', () => {
  const main = { name: 'Manifold', width: 1920, height: 1080 };
  eq(isPopupScreen({ name: 'FM_01', width: 320, height: 680 }, main), true);
  eq(isPopupScreen({ name: 'FM01_Trend', width: 1920, height: 1080 }, main), false);
  eq(isPopupScreen(main, main), false);
});
check('screenTags', () => {
  const tags = screenTags({ element: [{ kind: 'display', tag: 'A' }, { kind: 'trend', tags: ['B', 'C'] }, { kind: 'label', text: 'x' }, { kind: 'pipeline', tag: 'A' }] });
  eq([...tags].sort(), ['A', 'B', 'C']);
});
check('history rows sort and CSV', () => {
  const rows = rowsToObjects({ columns: ['time', 'tag', 'value'], rows: [[2, 'b', 1.5], [1, 'a', 10], [3, 'c', 2]] });
  eq(sortRows(rows, 'value', 'asc').map((r) => r.tag), ['b', 'c', 'a']);
  eq(sortRows(rows, 'tag', 'desc').map((r) => r.tag), ['c', 'b', 'a']);
  eq(toCsv([{ a: 'x,y', b: 'q"z' }], [{ key: 'a', title: 'A' }, { key: 'b', title: 'B' }]), 'A,B\r\n"x,y","q""z"');
});
check('alarm summary and order', () => {
  const list = [
    { name: 'a', severity: 'low', state: 'active-acked', since: 1 },
    { name: 'b', severity: 'critical', state: 'active-unacked', since: 2 },
    { name: 'c', severity: 'high', state: 'cleared-unacked', since: 3 },
    { name: 'd', severity: 'high', state: 'shelved', since: 4 },
    { name: 'e', severity: 'medium', state: 'normal', since: 5 },
  ];
  const s = alarmSummary(list);
  eq(s.counts, { critical: 1, high: 0, medium: 0, low: 1 });
  eq(s.unacked, 2);
  eq(s.newest.name, 'b');
  eq(sortAlarms(list.slice(0, 3)).map((x) => x.name), ['b', 'c', 'a']);
});
check('trend time step', () => {
  eq(timeStep(3600e3, 6), 600e3);
  eq(timeStep(60e3, 6), 10e3);
});


// ---------------------------------------------------------------- client.js against a fake socket
group = 'client.js';
function fakeSocketFactory() {
  const sockets = [];
  class FakeWs {
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.sent = [];
      sockets.push(this);
    }
    send(text) { this.sent.push(JSON.parse(text)); }
    close() { this.readyState = 3; this.onclose?.(); }
    open() { this.readyState = 1; this.onopen?.(); }
    receive(msg) { this.onmessage?.({ data: JSON.stringify(msg) }); }
  }
  return { FakeWs, sockets };
}
await checkAsync('subscribes only the difference and resubscribes after reconnect', async () => {
  const { FakeWs, sockets } = fakeSocketFactory();
  const c = createWsClient({ url: 'ws://test/ws', WebSocketImpl: FakeWs, pingMs: 60000 });
  const statuses = [];
  c.on('status', (st) => statuses.push(st));
  c.setTags(['A', 'B']);
  c.connect();
  sockets[0].open();
  eq(sockets[0].sent, [{ op: 'subscribe', tags: ['A', 'B'] }]);
  c.setTags(['B', 'C']);
  eq(sockets[0].sent.slice(1), [{ op: 'subscribe', tags: ['C'] }, { op: 'unsubscribe', tags: ['A'] }]);
  const got = [];
  c.on('values', (v) => got.push(v));
  sockets[0].receive({ type: 'values', values: { B: { v: 1, q: 'good', ts: 5 } } });
  eq(got, [{ B: { v: 1, q: 'good', ts: 5 } }]);
  eq(c.values.get('B').v, 1);
  sockets[0].close();
  eq(c.status, 'closed');
  await new Promise((r) => setTimeout(r, 700));
  ok(sockets.length === 2, 'reconnected after backoff');
  sockets[1].open();
  eq(sockets[1].sent, [{ op: 'subscribe', tags: ['B', 'C'] }]);
  eq(statuses.slice(0, 4), ['connecting', 'open', 'closed', 'connecting']);
  c.close();
});
await checkAsync('write resolves from write-result and fails when disconnected', async () => {
  const { FakeWs, sockets } = fakeSocketFactory();
  const c = createWsClient({ url: 'ws://test/ws', WebSocketImpl: FakeWs, pingMs: 60000 });
  eq(await c.write('X', 1), { ok: false, error: 'not connected' });
  c.connect();
  sockets[0].open();
  const p1 = c.write('X', 1, 'pulse');
  const p2 = c.write('Y', 2);
  const [w1, w2] = sockets[0].sent.filter((m) => m.op === 'write');
  eq([w1.tag, w1.mode, w2.mode], ['X', 'pulse', 'set']);
  sockets[0].receive({ type: 'write-result', id: w2.id, ok: false, error: 'read-only' });
  sockets[0].receive({ type: 'write-result', id: w1.id, ok: true });
  eq(await p1, { ok: true });
  eq(await p2, { ok: false, error: 'read-only' });
  const p3 = c.write('Z', 3);
  sockets[0].close();
  eq(await p3, { ok: false, error: 'connection lost' });
  eq(await c.write('Z', 3, 'bogus'), { ok: false, error: 'unknown write mode "bogus"' });
  c.close();
});
await checkAsync('alarms and reload messages are forwarded', async () => {
  const { FakeWs, sockets } = fakeSocketFactory();
  const c = createWsClient({ url: 'ws://test/ws', WebSocketImpl: FakeWs, pingMs: 60000 });
  const seen = [];
  c.on('alarms', (a) => seen.push(['alarms', a.length]));
  c.on('reload', () => seen.push(['reload']));
  c.on('hello', (m) => seen.push(['hello', m.main_screen]));
  c.connect();
  sockets[0].open();
  sockets[0].receive({ type: 'hello', project: 'P', main_screen: 'Manifold' });
  sockets[0].receive({ type: 'alarms', alarms: [{ name: 'a' }] });
  sockets[0].receive({ type: 'reload' });
  eq(seen, [['hello', 'Manifold'], ['alarms', 1], ['reload']]);
  c.close();
});

// ---------------------------------------------------------------- renderer smoke test
group = 'renderer';
await checkAsync('renders Manifold with every element and live values', async () => {
  const [map, bundle] = await Promise.all([loadSymbolMap(), fetch('../hmi/mock/bundle.json').then((r) => r.json())]);
  const host = document.getElementById('stage');
  const sc = bundle.screens.find((x) => x.name === 'Manifold');
  const cmds = [];
  const view = createScreenView(host, sc, { project: bundle, symbolMap: map, onCommand: (c) => cmds.push(c), widgets: 'placeholder' });
  eq(host.querySelectorAll('.mc-el').length, sc.element.length, 'element count: ');
  view.update({ FM_02_MassFlow: { v: 12.345, q: 'good', ts: Date.now() }, FM_04_START: { v: true, q: 'good', ts: Date.now() } });
  eq(view.elementNode('EditDisplay3').querySelector('.mc-display-value').textContent, '12.3');
  ok(view.elementNode('Pipeline').classList.contains('mc-flowing'), 'pipe animates when its tag is on');
  view.update({ FM_02_MassFlow: { v: 1, q: 'bad', ts: Date.now() } });
  ok(view.elementNode('EditDisplay3').classList.contains('mc-q-bad'), 'bad quality styles the display');
  view.elementNode('ButtonControl').dispatchEvent(new MouseEvent('click', { bubbles: true }));
  eq(cmds, [{ action: 'open-screen', screen: 'FM_01' }]);
  eq(view.elementNode('ContentControl3').dataset.symbol, 'ball-valve');
  ok(view.tagsInUse().has('FM_01_Volume_Rate'), 'tagsInUse');
  view.destroy();
  eq(host.childElementCount, 0, 'destroy cleans up: ');
});

// ---------------------------------------------------------------- review fixes
group = 'review fixes';
check('compactPoints bounds a long live series and keeps extremes and gaps', () => {
  const pts = [];
  for (let i = 0; i < 100000; i += 1) pts.push([i * 250, i === 5000 ? null : Math.sin(i / 50) + (i === 777 ? 10 : 0)]);
  const out = compactPoints(pts, 4000);
  ok(out.length <= 2500, `kept ${out.length} points`);
  ok(out.every((p, i) => i === 0 || p[0] >= out[i - 1][0]), 'still sorted');
  ok(out.some((p) => p[1] === null), 'the gap survives');
  ok(out.some((p) => p[1] > 10), 'the spike survives');
  eq(out.slice(-1000), pts.slice(-1000), 'newest points exact: ');
  eq(compactPoints(pts.slice(0, 10), 4000).length, 10, 'small series untouched: ');
  // Repeated compaction as points keep arriving stays bounded.
  let live = [];
  for (let i = 0; i < 60000; i += 1) { live.push([i, i % 7]); live = compactPoints(live, 2000); }
  ok(live.length <= 2000, `live series ${live.length}`);
});
check('Ack in a scoped alarm list acknowledges only the alarms shown', () => {
  const alarms = [
    { name: 'A1', area: 'A', severity: 'high', state: 'active-unacked', since: 1 },
    { name: 'A2', area: 'A', severity: 'low', state: 'cleared-unacked', since: 2 },
    { name: 'A3', area: 'A', severity: 'low', state: 'active-acked', since: 3 },
    { name: 'B1', area: 'B', severity: 'high', state: 'active-unacked', since: 4 },
  ];
  eq(ackPlan(alarms, {}), { all: true, names: ['B1', 'A1', 'A2'], label: 'Ack all' });
  eq(ackPlan(alarms, { area: 'A' }), { all: false, names: ['A1', 'A2'], label: 'Ack shown' });
  eq(ackPlan(alarms, { severity: 'high' }).names, ['B1', 'A1']);
  eq(ackPlan(alarms, { area: 'A', filter: 'active' }).names, ['A1']);
  eq(shownAlarms(alarms, { area: 'B' }).map((a) => a.name), ['B1']);
});
check('client write timeouts exceed the server bounds', () => {
  ok(writeTimeoutFor('set') > 10000, 'set');
  ok(writeTimeoutFor('pulse') > 20500, 'pulse');
});
check('malformed URL hashes do not throw', () => {
  eq(screenFromHash('#Manifold'), 'Manifold');
  eq(screenFromHash('#FM%2001'), 'FM 01');
  eq(screenFromHash('#100%'), '100%');
  eq(screenFromHash('#%E0'), '%E0');
  eq(screenFromHash(''), '');
});

// ---------------------------------------------------------------- report
const failed = results.filter((r) => !r.ok);
const summary = document.getElementById('summary');
summary.textContent = `${results.length - failed.length} / ${results.length} passed`;
summary.className = failed.length ? 'fail' : 'pass';
document.title = `${failed.length ? 'FAIL' : 'PASS'} ${results.length - failed.length}/${results.length} · Moviclone shared tests`;
const list = document.getElementById('results');
let current = '';
for (const r of results) {
  if (r.group !== current) {
    current = r.group;
    const h2 = document.createElement('h2');
    h2.textContent = current;
    list.appendChild(h2);
  }
  const li = document.createElement('div');
  li.className = r.ok ? 'ok' : 'bad';
  li.textContent = `${r.ok ? '✓' : '✗'} ${r.name}${r.ok ? '' : ` — ${r.err}`}`;
  list.appendChild(li);
}
window.__MC_TESTS__ = { passed: results.length - failed.length, failed: failed.length, results };
