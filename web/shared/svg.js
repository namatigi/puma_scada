// Tiny DOM helpers shared by the renderer, the symbol library and the widgets.
// Kept dependency-free so every module can run offline as a plain ES module.

export const SVG_NS = 'http://www.w3.org/2000/svg';

/** Creates an SVG element with attributes and children (elements or strings). */
export function s(tag, attrs = {}, ...children) {
  const el = document.createElementNS(SVG_NS, tag);
  setAttrs(el, attrs);
  append(el, children);
  return el;
}

/** Creates an HTML element. `class` and `text` are accepted as attributes. */
export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'text') el.textContent = v;
    else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
    else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
    else el.setAttribute(k, v === true ? '' : String(v));
  }
  append(el, children);
  return el;
}

export function setAttrs(el, attrs) {
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    el.setAttribute(k, String(v));
  }
  return el;
}

function append(el, children) {
  for (const c of children.flat(Infinity)) {
    if (c === null || c === undefined || c === false) continue;
    el.appendChild(typeof c === 'string' || typeof c === 'number' ? document.createTextNode(String(c)) : c);
  }
}

let uidCounter = 0;
/** Document-unique id for gradients/clip paths (several SVG roots share one id space). */
export function uid(prefix = 'mc') {
  uidCounter += 1;
  return `${prefix}-${uidCounter.toString(36)}`;
}

/** Linear gradient element. `stops` is [[offset 0..1, colour, opacity?], …]. */
export function linearGradient(id, stops, { x1 = 0, y1 = 0, x2 = 0, y2 = 1, units } = {}) {
  return s('linearGradient', { id, x1, y1, x2, y2, gradientUnits: units },
    stops.map(([o, c, op]) => s('stop', { offset: o, 'stop-color': c, 'stop-opacity': op })));
}

/** Parses `#rgb`, `#rrggbb`, `#aarrggbb` (XAML order) or `rgb()` into {r,g,b,a}. */
export function parseColor(c) {
  if (!c || typeof c !== 'string') return null;
  const t = c.trim();
  let m = /^#([0-9a-f]{3})$/i.exec(t);
  if (m) {
    const [r, g, b] = m[1].split('').map((x) => parseInt(x + x, 16));
    return { r, g, b, a: 1 };
  }
  m = /^#([0-9a-f]{6})$/i.exec(t);
  if (m) {
    const n = parseInt(m[1], 16);
    return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, a: 1 };
  }
  m = /^#([0-9a-f]{2})([0-9a-f]{6})$/i.exec(t);
  if (m) {
    const n = parseInt(m[2], 16);
    return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, a: parseInt(m[1], 16) / 255 };
  }
  m = /^rgba?\(([^)]+)\)$/i.exec(t);
  if (m) {
    const p = m[1].split(',').map((x) => parseFloat(x));
    return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
  }
  const named = { white: '#ffffff', black: '#000000', red: '#ff0000', grey: '#808080', gray: '#808080' };
  return named[t.toLowerCase()] ? parseColor(named[t.toLowerCase()]) : null;
}

export function toHex({ r, g, b }) {
  const x = (n) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, '0');
  return `#${x(r)}${x(g)}${x(b)}`;
}

/** Mixes colour `a` towards `b` by `t` (0 = a, 1 = b). Unknown colours fall back to grey. */
export function mix(a, b, t) {
  const ca = parseColor(a) || { r: 128, g: 128, b: 128 };
  const cb = parseColor(b) || { r: 128, g: 128, b: 128 };
  return toHex({ r: ca.r + (cb.r - ca.r) * t, g: ca.g + (cb.g - ca.g) * t, b: ca.b + (cb.b - ca.b) * t });
}

export const lighten = (c, t) => mix(c, '#ffffff', t);
export const darken = (c, t) => mix(c, '#000000', t);

/** Relative luminance 0..1, used to pick readable text colours on product-coloured bodies. */
export function luminance(c) {
  const p = parseColor(c);
  if (!p) return 0.5;
  return (0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b) / 255;
}

/**
 * Converts a CSS background from the bundle into something SVG can paint.
 * Supports plain colours and `linear-gradient([angle|to dir,] c1 [p%], c2 [p%], …)`.
 * Returns {fill, defs} where defs is an element to append (or null).
 */
export function cssBackgroundToSvg(bg) {
  if (!bg) return { fill: null, defs: null };
  const m = /^\s*linear-gradient\((.*)\)\s*$/i.exec(bg);
  if (!m) return { fill: bg, defs: null };
  const parts = splitTopLevel(m[1]);
  let dir = { x1: 0, y1: 0, x2: 0, y2: 1 };
  const first = parts[0].trim();
  const angle = /^(-?[\d.]+)deg$/i.exec(first);
  if (angle || /^to\s/i.test(first)) {
    parts.shift();
    if (angle) {
      // CSS: 0deg points up, 90deg points right.
      const a = (parseFloat(angle[1]) * Math.PI) / 180;
      const dx = Math.sin(a) / 2;
      const dy = -Math.cos(a) / 2;
      dir = { x1: 0.5 - dx, y1: 0.5 - dy, x2: 0.5 + dx, y2: 0.5 + dy };
    } else {
      const to = first.toLowerCase();
      dir = {
        x1: to.includes('left') ? 1 : 0, x2: to.includes('right') ? 1 : 0,
        y1: to.includes('top') ? 1 : 0, y2: to.includes('bottom') ? 1 : 0,
      };
      if (!to.includes('left') && !to.includes('right')) { dir.x1 = 0; dir.x2 = 0; }
      if (!to.includes('top') && !to.includes('bottom')) { dir.y1 = 0; dir.y2 = 0; }
    }
  }
  const stops = parts.map((p, i) => {
    const mm = /^(.*?)\s+([\d.]+)%$/.exec(p.trim());
    const colour = mm ? mm[1] : p.trim();
    const off = mm ? parseFloat(mm[2]) / 100 : parts.length === 1 ? 0 : i / (parts.length - 1);
    return [off, colour];
  });
  const id = uid('bg');
  return { fill: `url(#${id})`, defs: linearGradient(id, stops, dir) };
}

function splitTopLevel(str) {
  const out = [];
  let depth = 0;
  let cur = '';
  for (const ch of str) {
    if (ch === '(') depth += 1;
    if (ch === ')') depth -= 1;
    if (ch === ',' && depth === 0) {
      out.push(cur);
      cur = '';
    } else cur += ch;
  }
  if (cur.trim()) out.push(cur);
  return out;
}
