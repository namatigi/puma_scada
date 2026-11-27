# web/shared — Moviclone front-end modules

Plain ES modules, CSS and JSON used by the operator runtime (`web/hmi`) and the
studio (`web/studio`). No build step, no CDN: serve the folder as static files
(the runtime serves it at `/shared/`). Everything below is a stable API for the
studio; the studio may add its own files here only as `studio-*`.

| File | What it gives you |
|---|---|
| `theme.css` | Dark industrial theme. All colours are CSS variables on `:root` (`--mc-*`). Also styles every class the renderer and widgets emit. |
| `renderer.js` | `createScreenView()` — draws a bundle Screen as SVG and keeps it live. |
| `symbols.js` | Original parametric SVG symbol library (`SYMBOLS`, `drawSymbol`, `tube`). |
| `symbol-map.json` + `symbolmap.js` | Maps imported `movicon:<Name>` symbols to library symbols (+ per-instance params). |
| `format.js` | .NET-style number formats, value parsing, ages, dates, axis scales. |
| `client.js` | WebSocket client for `/ws` (subscriptions, writes, alarms, reload, reconnect). |
| `api.js` | REST client for `/api/*` (`createHttpApi`). |
| `trend.js` | Canvas trend widget (multi-pen, own Y axis per pen, presets, pan/zoom, live follow). |
| `historytable.js` | History table widget (`/api/history/table`, sortable, search, CSV, Refresh bar). |
| `alarmlist.js` | Alarm list widget (severity colours, ack, ack all, shelve) and alarm helpers. |
| `toast.js` | `toast(message, {kind})` notifications. |
| `svg.js` | Tiny DOM helpers (`s()` for SVG, `h()` for HTML, colours, CSS gradient → SVG). |
| `tests.html` | Browser unit checks for all of the above. Open it; the title says PASS/FAIL. |

## Rendering a screen

```js
import { createScreenView } from '../shared/renderer.js';
import { loadSymbolMap } from '../shared/symbolmap.js';
import { createHttpApi } from '../shared/api.js';

const api = createHttpApi('');
const project = await api.bundle();
const view = createScreenView(document.getElementById('host'), project.screens[0], {
  project,                       // tag types/access, popup detection, units for trends
  symbolMap: await loadSymbolMap(),
  api,                           // trend / history-table / alarm-list widgets read through it
  mode: 'runtime',               // or 'design' (no commands, displays show their format string)
  zoomable: true,                // wheel / pinch zoom and pan, double-click resets
  onCommand(cmd, { element }) {  // open-screen, write-tag {tag, value: string, mode}, report
    console.log(cmd);
  },
});
view.update({ FM_01_MassFlow: { v: 12.5, q: 'good', ts: Date.now() } }); // object or Map, partial
view.setAlarms(alarmStates);     // feeds alarm-list elements
view.tagsInUse();                // Set of tags to subscribe
view.elementNode('EditDisplay3');// the element's SVG <g> (data-id, data-kind attributes)
view.setOffline(true);           // greys values while the connection is down
view.destroy();
```

Other options: `interactive: false` (thumbnails), `widgets: 'live' | 'placeholder'`,
`staleMs` (default 5 min), `values` (initial snapshot), `culture`.

The host element needs a size (it is `position: relative`, the screen is centred
and scaled to fit, aspect ratio kept). Each element is a `<g class="mc-el mc-<kind>"
data-id data-kind>` translated to x/y and rotated about its centre. Trends,
history tables and alarm lists are HTML in an overlay above the SVG, positioned in
real pixels so they stay sharp; in design mode they are drawn as placeholders.

What the renderer does per element kind:

- **label** — centred text, size from the element height, shrunk to fit the width.
- **display** — dark panel, SemiBold value formatted with `format`, unit right.
  Quality: bad = red border + warning triangle, uncertain = dashed amber border,
  stale (older than `staleMs`) = dimmed italic. Hover shows tag, quality and age.
  `editable` displays bound to a writable tag open an inline editor, validate the
  input for the tag's type and emit `{action:'write-tag', tag, value, mode:'set'}`.
- **button** — glossy light-grey button; buttons that open a faceplate-sized
  screen are drawn as the dark name tags used on Movicon screens. Click / Enter
  emit each `on_click` command.
- **pipeline** — cylindrical steel tube along `points` (relative to x/y), round
  caps, couplings at straight joints; when `tag` is truthy an animated flow runs
  through it (reversed for negative values).
- **symbol** — resolved through the symbol map and drawn by `symbols.js`; state
  from the bound tag (`static` when no tag).
- **trend**, **history-table**, **alarm-list** — `trend.js`, `historytable.js`, `alarmlist.js`.

Helpers: `isPopupScreen(screen, main)` (open as floating panel?), `screenTags(screen)`,
`elementTags(element)`, `estimateTextWidth(text, size, bold)`.

## Symbols

```js
import { SYMBOLS, listSymbols, drawSymbol, symbolState, tube } from '../shared/symbols.js';
const g = drawSymbol('ball-valve', { w: 90, h: 60, params: { handle: '#2e63c9' }, state: 'open' });
```

Library: `pipe-straight`, `pipe-elbow`, `pipe-tee`, `pipe-coupling`, `pipe-flange`,
`ball-valve` (open / closed / fault), `coriolis-meter` (`product`, `head` colours),
`gauge` (`min`, `max`, `format`), `tank` (`min`, `max`, `fluid`, `shell`), `pump`,
`motor`, `lamp`. `listSymbols()` returns names, titles, categories, default sizes,
params and states (for the studio's Symbol Library). Symbols draw at the element's
own w × h (no stretching), so pipe diameters stay consistent with pipelines.

A symbol reference can carry parameters inline, which needs no model change:
`symbol = "coriolis-meter?product=%23FF4D4D"`.

### symbol-map.json

```json
{ "symbols":   { "movicon:Symbol408": { "symbol": "coriolis-meter", "params": { "head": "#FFFFFF" } } },
  "instances": { "Manifold/ContentControl33": { "params": { "product": "#FF4D4D" }, "label": "Gasoline" } } }
```

`resolveSymbol(ref, map, {screen, id})` → `{name, params, known, source, label}`.
Order of precedence: library defaults < `symbols` entry < `instances` entry < inline params.
Petrogas mapping (identified from sizes, positions and rotations only):

| Movicon | Count | Moviclone |
|---|---|---|
| Symbol952 (90×60) | 19 | `ball-valve` (pipe axis at 72 % height, handle above) |
| PipesPipe15 (50×29) | 11 | `pipe-coupling` |
| Symbol739 (45.5×36.4) | 9 | `pipe-tee` (run along the bottom, branch up) |
| Symbol734 (42.9×42.9) | 9 | `pipe-elbow` (arms left and down) |
| Symbol408 (104×125) | 5 | `coriolis-meter`, product colour per instance |
| Symbol733 (7.7×27) | 2 | `pipe-flange` |
| Symbol732 (146×33) | 1 | `pipe-straight` (best guess; not visible in the original thumbnail) |

## Live data

```js
import { createWsClient } from '../shared/client.js';
const client = createWsClient();            // ws(s)://<host>/ws
client.on('status', (s) => …);              // 'connecting' | 'open' | 'closed'
client.on('values', (changes) => view.update(changes));
client.on('alarms', (list) => view.setAlarms(list));
client.on('reload', () => …);               // refetch /api/bundle
client.setTags(view.tagsInUse());           // desired set; only differences are sent
const res = await client.write('FM_01_Reset_Total', true, 'pulse'); // {ok} | {ok:false, error}
client.connect();
```

Reconnects with exponential backoff (0.5 s → 10 s, ±25 % jitter), re-subscribes,
pings every 15 s and drops a silent socket; pending writes resolve with
`{ok:false, error:'connection lost'}`. `client.values` keeps the latest sample per tag.

The HMI's mock backend (`web/hmi/mock.js`) implements the same `client` and `api`
interfaces, so widgets work unchanged without a server.

## Formatting

`formatNumber(v, fmt, culture)` supports `0`/`#` placeholders, `.`, grouping `,`,
scaling commas, `%`, `‰`, `E+00`, quoted literals, `pos;neg;zero` sections and the
standard formats `F`, `N`, `D`, `P`, `E`, `G`, `X`. Also `formatValue`,
`parseInputValue(text, dataType, culture)` (throws operator-readable errors),
`formatAge`, `formatDateTime`, `niceScale`, `truthy`, `toNumber`.

## Widgets

```js
createTrend(host, { api, project, tags, historian, spanS })    // → {pushValues(Map), setRange(from,to), refetch(), destroy()}
createHistoryTable(host, { api, project, source, limit })      // → {refresh(), destroy()}
createAlarmList(host, { api, area, compact })                  // → {setAlarms(list), destroy()}
```

Each widget fills its host (give it a size and `display:flex; flex-direction:column`,
which `.mc-widget` does).

## Tests

Serve `web/` (`python -m http.server` from `web/`) and open
`/shared/tests.html`. It checks format.js, the symbol library and map (including
every symbol in the Petrogas bundle), the WebSocket client against a fake socket,
table/alarm helpers and a renderer smoke test. `window.__MC_TESTS__` holds the
result for automation.
