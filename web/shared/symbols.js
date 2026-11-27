// Moviclone symbol library: original, parametric SVG artwork drawn in code.
//
// Every symbol draws itself into a w × h box at its native coordinates (no
// non-uniform scaling), so it stays crisp at any size and keeps pipe diameters
// consistent with neighbouring pipelines. The artwork is our own; nothing here
// is derived from Movicon/Progea symbol vectors.
//
//   drawSymbol(name, { w, h, params, state, value }) -> SVGGElement
//   SYMBOLS[name] = { title, category, size:[w,h], params:{…defaults}, states:[…],
//                     stateOf(value, quality, params), draw(ctx) }

import { s, uid, linearGradient, mix, lighten, darken, luminance } from './svg.js';
import { truthy, toNumber, formatNumber } from './format.js';

// ------------------------------------------------------------------ shared drawing

/**
 * A tube along any SVG path with cylindrical shading across its thickness
 * (dark edges, light centre). Built from concentric strokes so the shading
 * follows bends and diagonals, which a single linear gradient cannot.
 */
export function tube(d, thickness, { fill = '#EFEFF0', stroke = '#5E5E5E', cap = 'round', join = 'round', className } = {}) {
  const layers = [
    [1.0, stroke],
    [0.92, mix(fill, stroke, 0.72)],
    [0.83, mix(fill, stroke, 0.54)],
    [0.72, mix(fill, stroke, 0.38)],
    [0.59, mix(fill, stroke, 0.24)],
    [0.45, mix(fill, stroke, 0.11)],
    [0.3, fill],
    [0.14, lighten(fill, 0.6)],
  ];
  return s('g', { class: className, fill: 'none', 'stroke-linecap': cap, 'stroke-linejoin': join },
    layers.map(([k, c]) => s('path', { d, stroke: c, 'stroke-width': Math.max(0.5, thickness * k) })));
}

/** A short collar (coupling band) on a tube at point (x,y), along direction angle `deg`. */
export function collar(x, y, deg, thickness, len, colours = {}) {
  const r = (deg * Math.PI) / 180;
  const dx = (Math.cos(r) * len) / 2;
  const dy = (Math.sin(r) * len) / 2;
  return tube(`M${x - dx},${y - dy}L${x + dx},${y + dy}`, thickness, { cap: 'butt', ...colours });
}

const STEEL = { fill: '#F2F2F3', stroke: '#5E5E5E' };
const STATE_COLOURS = {
  open: '#35c46a', closed: '#e04848', running: '#35c46a', stopped: '#8b9096',
  fault: '#ffb000', on: '#35c46a', off: '#5b6066', unknown: '#8b9096',
};

function badgeLed(cx, cy, r, state) {
  if (state === 'static') return null;
  const colour = STATE_COLOURS[state] ?? STATE_COLOURS.unknown;
  return s('circle', {
    cx, cy, r, fill: colour, stroke: darken(colour, 0.45), 'stroke-width': Math.max(0.6, r * 0.25),
    class: state === 'fault' ? 'mc-blink' : null,
  });
}

/** Vertical-axis shading gradient (light band in the middle), for bodies seen side-on. */
function bodyGradient(defs, colour, horizontal = false) {
  const id = uid('g');
  defs.appendChild(linearGradient(id, [
    [0, darken(colour, 0.45)], [0.18, darken(colour, 0.12)], [0.42, lighten(colour, 0.35)],
    [0.6, colour], [1, darken(colour, 0.5)],
  ], horizontal ? { x1: 0, y1: 0, x2: 1, y2: 0 } : { x1: 0, y1: 0, x2: 0, y2: 1 }));
  return `url(#${id})`;
}

const defaultStateOf = (value, quality) => {
  if (quality === 'bad') return 'fault';
  if (value === null || value === undefined) return 'unknown';
  return truthy(value) ? 'on' : 'off';
};

// ------------------------------------------------------------------ pipe pieces

function drawPipeStraight({ w, h, p }) {
  const t = h * p.pipe;
  const y = h / 2;
  return [
    tube(`M0,${y}H${w}`, t, { cap: 'butt', ...STEEL }),
    collar(3, y, 0, t * 1.12, 6, STEEL),
    collar(w - 3, y, 0, t * 1.12, 6, STEEL),
  ];
}

function drawPipeElbow({ w, h, p }) {
  // Arms leave through the left edge and the bottom edge; outer corner top-right.
  const t = Math.min(w, h) * p.pipe;
  const c = t / 2;
  const cx = w - c;
  const r = Math.max(0.5, Math.min(cx, h - c) * 0.55);
  const d = `M0,${c}H${cx - r}A${r},${r} 0 0 1 ${cx},${c + r}V${h}`;
  return [
    tube(d, t, { cap: 'butt', ...STEEL }),
    collar(2.5, c, 0, t * 1.12, 5, STEEL),
    collar(cx, h - 2.5, 90, t * 1.12, 5, STEEL),
  ];
}

function drawPipeTee({ w, h, p }) {
  // Run along the bottom, branch up through the top edge at the centre.
  const t = Math.min(h * p.pipe, w * 0.6);
  const ay = h - t / 2;
  const cx = w / 2;
  return [
    tube(`M0,${ay}H${w}`, t, { cap: 'butt', ...STEEL }),
    tube(`M${cx},0V${ay}`, t, { cap: 'butt', ...STEEL }),
    s('ellipse', { cx, cy: ay, rx: t * 0.5, ry: t * 0.2, fill: lighten(STEEL.fill, 0.3), opacity: 0.35 }),
    collar(2.5, ay, 0, t * 1.1, 5, STEEL),
    collar(w - 2.5, ay, 0, t * 1.1, 5, STEEL),
    collar(cx, 2.5, 90, t * 1.1, 5, STEEL),
  ];
}

function drawPipeCoupling({ w, h, p }) {
  const y = h / 2;
  const t = h * p.pipe;
  const sleeve = { fill: '#F7F7F8', stroke: '#575757' };
  return [
    tube(`M0,${y}H${w}`, t, { cap: 'butt', ...STEEL }),
    tube(`M${w * 0.3},${y}H${w * 0.7}`, h, { cap: 'butt', ...sleeve }),
    s('path', { d: `M${w * 0.42},${y - h / 2 + 1}V${y + h / 2 - 1}M${w * 0.58},${y - h / 2 + 1}V${y + h / 2 - 1}`, stroke: '#6d6d6d', 'stroke-width': 0.8, opacity: 0.8 }),
  ];
}

function drawPipeFlange({ w, h }) {
  // A flange seen edge-on: a short, wide cylinder across the pipe axis.
  return [tube(`M${w / 2},${w / 2}V${h - w / 2}`, w, { cap: 'round', fill: '#F4F4F5', stroke: '#555' })];
}

// ------------------------------------------------------------------ valves

function drawBallValve({ w, h, p, state, defs }) {
  const ay = h * p.axis;
  const pt = h * p.pipe;
  const cx = w / 2;
  const nut = { fill: '#F3F3F4', stroke: '#585858' };
  const bodyColour = state === 'fault' ? '#8a6a1a' : p.body;
  const body = { fill: lighten(bodyColour, 0.3), stroke: darken(bodyColour, 0.45) };
  const handleColour = state === 'closed' ? p.closedHandle : state === 'fault' ? '#ffb000' : p.handle;
  const hy = h * 0.2;
  const parts = [
    tube(`M0,${ay}H${w}`, pt, { cap: 'butt', ...STEEL }),
    tube(`M${w * 0.07},${ay}H${w * 0.27}`, pt * 1.22, { cap: 'butt', ...nut }),
    tube(`M${w * 0.73},${ay}H${w * 0.93}`, pt * 1.22, { cap: 'butt', ...nut }),
    s('path', { d: `M${w * 0.17},${ay - pt * 0.61}V${ay + pt * 0.61}M${w * 0.83},${ay - pt * 0.61}V${ay + pt * 0.61}`, stroke: '#7a7a7a', 'stroke-width': 0.7, opacity: 0.7 }),
    tube(`M${w * 0.29},${ay}H${w * 0.71}`, pt * 1.42, { cap: 'butt', ...body }),
  ];
  // Stem and gland nut.
  const stemFill = bodyGradient(defs, '#c9ccd1', true);
  parts.push(s('rect', { x: cx - w * 0.035, y: hy, width: w * 0.07, height: ay - pt * 0.7 - hy + 1, fill: stemFill, stroke: '#4d4d4d', 'stroke-width': 0.6 }));
  parts.push(s('rect', { x: cx - w * 0.07, y: ay - pt * 0.71 - h * 0.07, width: w * 0.14, height: h * 0.07, rx: 1, fill: bodyGradient(defs, '#b9bcc2', true), stroke: '#4d4d4d', 'stroke-width': 0.6 }));
  // Butterfly handle: two swept wings. Closed = turned across the pipe (seen end-on, narrow).
  const k = state === 'closed' ? 0.38 : 1;
  const wing = (dir) => {
    const x = (f) => cx + dir * f * w * k;
    return `M${cx},${hy - h * 0.035}L${x(0.17)},${hy - h * 0.13}Q${x(0.27)},${hy - h * 0.17} ${x(0.255)},${hy - h * 0.02}`
      + `Q${x(0.245)},${hy + h * 0.09} ${x(0.16)},${hy + h * 0.055}L${cx},${hy + h * 0.045}Z`;
  };
  const hg = bodyGradient(defs, handleColour);
  parts.push(s('path', { d: wing(1) + wing(-1), fill: hg, stroke: darken(handleColour, 0.5), 'stroke-width': 0.8, 'stroke-linejoin': 'round' }));
  parts.push(s('circle', { cx, cy: hy, r: w * 0.04, fill: lighten(handleColour, 0.4), stroke: darken(handleColour, 0.5), 'stroke-width': 0.7 }));
  if (state !== 'static') {
    parts.push(s('rect', {
      x: cx - w * 0.06, y: ay - pt * 0.71, width: w * 0.12, height: pt * 1.42,
      fill: STATE_COLOURS[state] ?? STATE_COLOURS.unknown, opacity: 0.85,
      class: state === 'fault' ? 'mc-blink' : null,
    }));
  }
  return parts;
}

// ------------------------------------------------------------------ instruments

function drawCoriolis({ w, h, p, state, defs }) {
  const ay = h * p.axis;
  const pt = h * p.pipe;
  const product = p.product;
  const dark = luminance(product) < 0.18;
  const outline = dark ? '#8d8d8d' : darken(product, 0.55);
  const flange = { fill: '#EEEFF0', stroke: '#565656' };
  const bodyTube = { fill: lighten(product, dark ? 0.32 : 0.35), stroke: dark ? '#000' : darken(product, 0.5) };
  const parts = [
    tube(`M0,${ay}H${w}`, pt, { cap: 'butt', ...STEEL }),
    tube(`M${w * 0.02},${ay}H${w * 0.1}`, pt * 1.6, { cap: 'butt', ...flange }),
    tube(`M${w * 0.9},${ay}H${w * 0.98}`, pt * 1.6, { cap: 'butt', ...flange }),
    tube(`M${w * 0.1},${ay}H${w * 0.9}`, pt * 1.38, { cap: 'butt', ...bodyTube }),
  ];
  // Sensor case: a rounded housing sitting on the measuring tubes.
  const caseTop = ay - h * 0.3;
  parts.push(s('rect', {
    x: w * 0.2, y: caseTop, width: w * 0.6, height: ay - caseTop + pt * 0.55, rx: w * 0.07,
    fill: bodyGradient(defs, product, true), stroke: outline, 'stroke-width': 1,
  }));
  parts.push(s('path', { d: `M${w * 0.26},${caseTop + h * 0.05}H${w * 0.74}`, stroke: lighten(product, 0.6), 'stroke-width': 1.2, opacity: 0.55, 'stroke-linecap': 'round' }));
  // Neck between case and transmitter.
  const headBottom = h * 0.43;
  parts.push(s('rect', {
    x: w * 0.42, y: headBottom - 1, width: w * 0.16, height: caseTop - headBottom + 2,
    fill: bodyGradient(defs, dark ? '#2a2a2a' : darken(product, 0.25), true), stroke: outline, 'stroke-width': 0.8,
  }));
  parts.push(s('rect', { x: w * 0.37, y: caseTop - h * 0.035, width: w * 0.26, height: h * 0.04, rx: 1.5, fill: bodyGradient(defs, '#3a3a3a', true), stroke: '#222', 'stroke-width': 0.6 }));
  // Transmitter head.
  const head = p.head;
  const hx = w * 0.18;
  const hw = w * 0.64;
  const hTop = h * 0.03;
  parts.push(s('rect', {
    x: hx, y: hTop, width: hw, height: headBottom - hTop, rx: w * 0.045,
    fill: bodyGradient(defs, head), stroke: '#7d7f82', 'stroke-width': 1,
  }));
  parts.push(s('rect', { x: hx + 2, y: hTop + 2, width: hw - 4, height: (headBottom - hTop) * 0.18, rx: w * 0.03, fill: '#ffffff', opacity: 0.35 }));
  // Display window with a lit LCD segment row.
  const dx = w * 0.29;
  const dw = w * 0.42;
  const dy = h * 0.11;
  const dh = h * 0.15;
  parts.push(s('rect', { x: dx, y: dy, width: dw, height: dh, rx: 2, fill: '#1c2226', stroke: '#5b5f63', 'stroke-width': 0.8 }));
  parts.push(s('rect', { x: dx + dw * 0.16, y: dy + dh * 0.38, width: dw * 0.68, height: dh * 0.26, rx: 1, fill: state === 'fault' ? '#ff8a3d' : '#9ad8b0', opacity: state === 'off' || state === 'unknown' ? 0.25 : 0.75 }));
  // Cable glands either side of the head.
  for (const gx of [hx - w * 0.05, hx + hw]) {
    parts.push(s('rect', { x: gx, y: h * 0.22, width: w * 0.05, height: h * 0.07, rx: 1, fill: bodyGradient(defs, '#b8bbc0'), stroke: '#555', 'stroke-width': 0.6 }));
  }
  const led = badgeLed(w * 0.74, h * 0.35, Math.max(1.5, w * 0.025), state === 'on' ? 'running' : state);
  if (led) parts.push(led);
  return parts;
}

function drawGauge({ w, h, p, value, quality, defs }) {
  const r = Math.min(w, h) / 2 - 1;
  const cx = w / 2;
  const cy = h / 2;
  const min = Number(p.min);
  const max = Number(p.max);
  const v = toNumber(value);
  const frac = v === null ? 0 : Math.max(0, Math.min(1, (v - min) / (max - min || 1)));
  const a0 = 135;
  const sweep = 270;
  const pol = (deg, rr) => [cx + rr * Math.cos((deg * Math.PI) / 180), cy + rr * Math.sin((deg * Math.PI) / 180)];
  const parts = [
    s('circle', { cx, cy, r, fill: bodyGradient(defs, '#c9ccd1'), stroke: '#4a4a4a', 'stroke-width': 1 }),
    s('circle', { cx, cy, r: r * 0.86, fill: '#f6f6f4', stroke: '#8a8a8a', 'stroke-width': 0.8 }),
  ];
  for (let i = 0; i <= 10; i += 1) {
    const deg = a0 + (sweep * i) / 10;
    const [x1, y1] = pol(deg, r * 0.8);
    const [x2, y2] = pol(deg, r * (i % 5 === 0 ? 0.64 : 0.71));
    parts.push(s('line', { x1, y1, x2, y2, stroke: '#333', 'stroke-width': i % 5 === 0 ? 1.4 : 0.8 }));
  }
  // Red zone over the last 20 %.
  const [rx1, ry1] = pol(a0 + sweep * 0.8, r * 0.76);
  const [rx2, ry2] = pol(a0 + sweep, r * 0.76);
  parts.push(s('path', { d: `M${rx1},${ry1}A${r * 0.76},${r * 0.76} 0 0 1 ${rx2},${ry2}`, stroke: '#d84040', 'stroke-width': r * 0.07, fill: 'none' }));
  const [nx, ny] = pol(a0 + sweep * frac, r * 0.7);
  parts.push(s('line', { x1: cx, y1: cy, x2: nx, y2: ny, stroke: quality === 'bad' ? '#999' : '#c62828', 'stroke-width': Math.max(1.2, r * 0.05), 'stroke-linecap': 'round' }));
  parts.push(s('circle', { cx, cy, r: r * 0.08, fill: '#333' }));
  if (r > 18) {
    parts.push(s('text', { x: cx, y: cy + r * 0.45, 'text-anchor': 'middle', 'font-size': r * 0.22, fill: '#222', 'font-weight': 600 },
      v === null ? '----' : formatNumber(v, p.format)));
  }
  return parts;
}

function drawTank({ w, h, p, value, defs }) {
  const v = toNumber(value);
  const frac = v === null ? 0 : Math.max(0, Math.min(1, (v - Number(p.min)) / ((Number(p.max) - Number(p.min)) || 1)));
  const dome = Math.min(h * 0.12, w * 0.25);
  const top = dome;
  const bottom = h - dome * 0.5;
  const shell = bodyGradient(defs, p.shell, true);
  const clip = uid('clip');
  const shape = `M0,${top}Q0,0 ${w / 2},0Q${w},0 ${w},${top}V${bottom}Q${w},${h} ${w / 2},${h}Q0,${h} 0,${bottom}Z`;
  defs.appendChild(s('clipPath', { id: clip }, s('path', { d: shape })));
  const levelY = bottom - (bottom - top) * frac;
  const parts = [
    s('path', { d: shape, fill: shell, stroke: '#4a4a4a', 'stroke-width': 1 }),
    s('g', { 'clip-path': `url(#${clip})` },
      s('rect', { x: 0, y: levelY, width: w, height: h - levelY, fill: bodyGradient(defs, p.fluid, true), opacity: 0.9 })),
    s('path', { d: shape, fill: 'none', stroke: '#3a3a3a', 'stroke-width': 1.2 }),
  ];
  for (let i = 1; i < 10; i += 1) {
    const y = bottom - ((bottom - top) * i) / 10;
    parts.push(s('line', { x1: w - Math.max(4, w * 0.08), y1: y, x2: w - 1, y2: y, stroke: '#222', 'stroke-width': i === 5 ? 1 : 0.6, opacity: 0.7 }));
  }
  if (w > 30) {
    parts.push(s('text', { x: w / 2, y: (top + bottom) / 2, 'text-anchor': 'middle', 'dominant-baseline': 'middle', 'font-size': Math.min(w * 0.18, 18), 'font-weight': 700, fill: '#fff', stroke: '#000', 'stroke-width': 0.6, 'paint-order': 'stroke' },
      v === null ? '----' : `${Math.round(frac * 100)} %`));
  }
  return parts;
}

function drawPump({ w, h, state, defs }) {
  const colour = STATE_COLOURS[state === 'on' ? 'running' : state === 'off' ? 'stopped' : state] ?? '#7d8aa0';
  const r = Math.min(w, h * 0.8) * 0.36;
  const cx = w * 0.46;
  const cy = h * 0.46;
  const casing = bodyGradient(defs, state === 'static' ? '#7d8aa0' : colour);
  return [
    s('path', { d: `M${w * 0.15},${h - 2}L${w * 0.3},${cy + r * 0.8}H${w * 0.62}L${w * 0.77},${h - 2}Z`, fill: bodyGradient(defs, '#555b62'), stroke: '#2b2b2b', 'stroke-width': 0.8 }),
    s('rect', { x: cx, y: cy - r, width: w * 0.5 - 1, height: r * 0.62, fill: casing, stroke: '#2b2b2b', 'stroke-width': 0.8 }),
    s('circle', { cx, cy, r, fill: casing, stroke: '#2b2b2b', 'stroke-width': 1 }),
    s('g', { class: state === 'on' || state === 'running' ? 'mc-spin' : null, style: `transform-origin:${cx}px ${cy}px` },
      [0, 120, 240].map((a) => s('path', {
        d: `M${cx},${cy}L${cx + r * 0.62 * Math.cos((a * Math.PI) / 180)},${cy + r * 0.62 * Math.sin((a * Math.PI) / 180)}`,
        stroke: '#1d1d1d', 'stroke-width': Math.max(1, r * 0.14), 'stroke-linecap': 'round',
      }))),
    s('circle', { cx, cy, r: r * 0.18, fill: '#ddd', stroke: '#333', 'stroke-width': 0.6 }),
  ];
}

function drawMotor({ w, h, state, defs }) {
  const colour = state === 'static' ? '#4f78a8' : STATE_COLOURS[state === 'on' ? 'running' : state === 'off' ? 'stopped' : state] ?? '#4f78a8';
  const bodyH = h * 0.62;
  const by = h * 0.22;
  const parts = [
    s('rect', { x: w * 0.1, y: by, width: w * 0.68, height: bodyH, rx: h * 0.08, fill: bodyGradient(defs, colour), stroke: '#262626', 'stroke-width': 0.9 }),
    s('rect', { x: w * 0.78, y: by + bodyH * 0.12, width: w * 0.12, height: bodyH * 0.76, rx: 2, fill: bodyGradient(defs, darken(colour, 0.25)), stroke: '#262626', 'stroke-width': 0.8 }),
    s('rect', { x: 0, y: by + bodyH * 0.42, width: w * 0.1, height: bodyH * 0.16, fill: bodyGradient(defs, '#c9ccd1'), stroke: '#333', 'stroke-width': 0.6 }),
    s('rect', { x: w * 0.36, y: h * 0.05, width: w * 0.22, height: h * 0.18, rx: 1.5, fill: bodyGradient(defs, darken(colour, 0.15)), stroke: '#262626', 'stroke-width': 0.8 }),
    s('rect', { x: w * 0.18, y: h * 0.84, width: w * 0.55, height: h * 0.14, fill: '#3b3f44', stroke: '#222', 'stroke-width': 0.6 }),
  ];
  for (let i = 1; i < 7; i += 1) {
    const x = w * 0.1 + (w * 0.68 * i) / 7;
    parts.push(s('line', { x1: x, y1: by + 2, x2: x, y2: by + bodyH - 2, stroke: darken(colour, 0.45), 'stroke-width': 0.8, opacity: 0.8 }));
  }
  return parts;
}

function drawLamp({ w, h, state, defs }) {
  const r = Math.min(w, h) / 2 - 1;
  const colour = STATE_COLOURS[state] ?? '#5b6066';
  return [
    s('circle', { cx: w / 2, cy: h / 2, r, fill: bodyGradient(defs, '#9ea2a8'), stroke: '#333', 'stroke-width': 1 }),
    s('circle', { cx: w / 2, cy: h / 2, r: r * 0.74, fill: colour, class: state === 'fault' ? 'mc-blink' : null }),
    s('ellipse', { cx: w / 2 - r * 0.2, cy: h / 2 - r * 0.28, rx: r * 0.3, ry: r * 0.18, fill: '#fff', opacity: 0.45 }),
  ];
}

function drawUnknown({ w, h, name }) {
  return [
    s('rect', { x: 0.5, y: 0.5, width: Math.max(0, w - 1), height: Math.max(0, h - 1), fill: 'rgba(255,0,170,0.08)', stroke: '#ff4fb8', 'stroke-dasharray': '4 3', 'stroke-width': 1 }),
    s('text', { x: w / 2, y: h / 2, 'text-anchor': 'middle', 'dominant-baseline': 'middle', 'font-size': Math.max(7, Math.min(12, h / 3)), fill: '#ff9fd6' },
      s('title', {}, `Unmapped symbol ${name ?? ''}`), '?'),
  ];
}

// ------------------------------------------------------------------ registry

const valveStateOf = (value, quality) => {
  if (quality === 'bad') return 'fault';
  if (value === null || value === undefined) return 'unknown';
  return truthy(value) ? 'open' : 'closed';
};

export const SYMBOLS = {
  'pipe-straight': { title: 'Pipe segment', category: 'Pipes', size: [146, 33], params: { pipe: 0.8 }, draw: drawPipeStraight },
  'pipe-elbow': { title: 'Pipe elbow 90°', category: 'Pipes', size: [43, 43], params: { pipe: 0.616 }, draw: drawPipeElbow },
  'pipe-tee': { title: 'Pipe tee', category: 'Pipes', size: [45.5, 36.4], params: { pipe: 0.72 }, draw: drawPipeTee },
  'pipe-coupling': { title: 'Pipe coupling', category: 'Pipes', size: [50, 29], params: { pipe: 0.9 }, draw: drawPipeCoupling },
  'pipe-flange': { title: 'Blind flange', category: 'Pipes', size: [8, 27], params: {}, draw: drawPipeFlange },
  'ball-valve': {
    title: 'Ball valve (butterfly handle)', category: 'Valves', size: [90, 60],
    params: { axis: 0.72, pipe: 0.44, body: '#2f4f86', handle: '#2e63c9', closedHandle: '#4a4f57' },
    states: ['open', 'closed', 'fault'], stateOf: valveStateOf, draw: drawBallValve,
  },
  'coriolis-meter': {
    title: 'Coriolis flow meter', category: 'Instruments', size: [104, 125],
    params: { axis: 0.8, pipe: 0.21, product: '#9aa0a6', head: '#ffffff' },
    states: ['on', 'off', 'fault'], draw: drawCoriolis,
  },
  gauge: { title: 'Pressure gauge', category: 'Instruments', size: [80, 80], params: { min: 0, max: 100, format: '#0.0' }, valueDriven: true, draw: drawGauge },
  tank: { title: 'Vertical tank', category: 'Vessels', size: [90, 160], params: { min: 0, max: 100, fluid: '#2f86d8', shell: '#b9bec5' }, valueDriven: true, draw: drawTank },
  pump: { title: 'Centrifugal pump', category: 'Machines', size: [80, 70], params: {}, states: ['on', 'off', 'fault'], draw: drawPump },
  motor: { title: 'Electric motor', category: 'Machines', size: [100, 60], params: {}, states: ['on', 'off', 'fault'], draw: drawMotor },
  lamp: { title: 'Indicator lamp', category: 'Indicators', size: [30, 30], params: {}, states: ['on', 'off', 'fault'], draw: drawLamp },
};

export function listSymbols() {
  return Object.entries(SYMBOLS).map(([name, d]) => ({ name, title: d.title, category: d.category, size: d.size, params: { ...d.params }, states: d.states ?? [] }));
}

/**
 * State of a symbol for a tag value. Without a bound tag the state is 'static'
 * (drawn as designed, no status colours).
 */
export function symbolState(name, { bound, value, quality }) {
  if (!bound) return 'static';
  const def = SYMBOLS[name];
  return (def?.stateOf ?? defaultStateOf)(value, quality);
}

/**
 * Draws a library symbol into a new <g>. Unknown names draw a dashed placeholder.
 * @param {string} name library name (e.g. "ball-valve")
 * @param {{w:number,h:number,params?:object,state?:string,value?:*,quality?:string}} opts
 */
export function drawSymbol(name, { w, h, params = {}, state = 'static', value = null, quality = 'good' }) {
  const def = SYMBOLS[name];
  const defs = s('defs');
  const g = s('g', { class: `mc-symbol mc-sym-${def ? name : 'unknown'}`, 'data-state': state }, defs);
  const p = { ...(def?.params ?? {}), ...params };
  const parts = def
    ? def.draw({ w, h, p, state, value, quality, defs })
    : drawUnknown({ w, h, name });
  for (const part of parts.flat()) if (part) g.appendChild(part);
  if (!defs.firstChild) defs.remove();
  return g;
}

/** Parses "name?key=value&key2=value" (the inline-parameter syntax) into {name, params}. */
export function parseSymbolRef(ref) {
  if (!ref) return { name: '', params: {} };
  const q = ref.indexOf('?');
  if (q < 0) return { name: ref.trim(), params: {} };
  const params = {};
  for (const pair of ref.slice(q + 1).split('&')) {
    if (!pair) continue;
    const eq = pair.indexOf('=');
    const k = eq < 0 ? pair : pair.slice(0, eq);
    const v = eq < 0 ? 'true' : pair.slice(eq + 1);
    try {
      params[decodeURIComponent(k)] = decodeURIComponent(v);
    } catch {
      params[k] = v;
    }
  }
  return { name: ref.slice(0, q).trim(), params };
}
