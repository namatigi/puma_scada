// Pure geometry for the screen editor: snapping, bounds, hit tests, resize in
// a rotated frame, align / distribute / same size. No DOM, unit-tested in tests.js.

export const MIN_SIZE = 2;

export const snap = (v, grid, on = true) => (on && grid > 0 ? Math.round(v / grid) * grid : v);

/** Rounds to 3 decimals so saved files do not fill up with float noise. */
export const tidy = (v) => Math.round(v * 1000) / 1000;

const rad = (deg) => (deg * Math.PI) / 180;

/** Rotates point (x,y) about (cx,cy) clockwise by `deg` (SVG y-down convention). */
export function rotatePoint(x, y, cx, cy, deg) {
  if (!deg) return [x, y];
  const a = rad(deg);
  const c = Math.cos(a);
  const s = Math.sin(a);
  const dx = x - cx;
  const dy = y - cy;
  return [cx + dx * c - dy * s, cy + dx * s + dy * c];
}

/** The four corners of an element in screen units, rotation applied. */
export function corners(el) {
  const cx = el.x + el.w / 2;
  const cy = el.y + el.h / 2;
  const r = el.rotation || 0;
  return [
    rotatePoint(el.x, el.y, cx, cy, r),
    rotatePoint(el.x + el.w, el.y, cx, cy, r),
    rotatePoint(el.x + el.w, el.y + el.h, cx, cy, r),
    rotatePoint(el.x, el.y + el.h, cx, cy, r),
  ];
}

/** Axis-aligned bounds {x,y,w,h} of an element (rotation included). */
export function bounds(el) {
  if (!el.rotation) return { x: el.x, y: el.y, w: el.w, h: el.h };
  const pts = corners(el);
  const xs = pts.map((p) => p[0]);
  const ys = pts.map((p) => p[1]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}

/** Bounds of several elements, or null for none. */
export function unionBounds(els) {
  if (!els.length) return null;
  let x0 = Infinity; let y0 = Infinity; let x1 = -Infinity; let y1 = -Infinity;
  for (const el of els) {
    const b = bounds(el);
    x0 = Math.min(x0, b.x); y0 = Math.min(y0, b.y);
    x1 = Math.max(x1, b.x + b.w); y1 = Math.max(y1, b.y + b.h);
  }
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/** Rectangle from two corner points, any order. */
export function rectFrom(ax, ay, bx, by) {
  return { x: Math.min(ax, bx), y: Math.min(ay, by), w: Math.abs(bx - ax), h: Math.abs(by - ay) };
}

export const contains = (outer, inner) => inner.x >= outer.x && inner.y >= outer.y
  && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h;

export const intersects = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

/** Is the screen point inside the (rotated) element? */
export function hitElement(el, px, py) {
  const [lx, ly] = rotatePoint(px, py, el.x + el.w / 2, el.y + el.h / 2, -(el.rotation || 0));
  return lx >= el.x && lx <= el.x + el.w && ly >= el.y && ly <= el.y + el.h;
}

/** Handle names, clockwise from top-left. */
export const HANDLES = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

/** Local (unrotated) position of a handle relative to the element's top-left. */
export function handleOffset(handle, w, h) {
  const x = handle.includes('w') ? 0 : handle.includes('e') ? w : w / 2;
  const y = handle.includes('n') ? 0 : handle.includes('s') ? h : h / 2;
  return [x, y];
}

/**
 * Resizes `rect` (x,y,w,h,rotation) by dragging `handle` by (dx,dy) screen units.
 * The drag is converted into the element's own frame, so rotated elements grow
 * along their own axes, and the opposite edge stays fixed on screen.
 * `keepAspect` keeps w:h (corner handles), `grid` snaps the new size.
 */
export function resizeRect(rect, handle, dx, dy, { keepAspect = false, grid = 0, min = MIN_SIZE } = {}) {
  const rot = rect.rotation || 0;
  const [ldx, ldy] = rotatePoint(dx, dy, 0, 0, -rot);
  let l = 0; let t = 0; let r = 0; let b = 0;
  if (handle.includes('w')) l = ldx;
  if (handle.includes('e')) r = ldx;
  if (handle.includes('n')) t = ldy;
  if (handle.includes('s')) b = ldy;
  let w = rect.w - l + r;
  let h = rect.h - t + b;
  if (grid > 0) {
    const sw = Math.max(grid, snap(w, grid));
    const sh = Math.max(grid, snap(h, grid));
    if (handle.includes('w')) l += w - sw; else if (handle.includes('e')) r += sw - w;
    if (handle.includes('n')) t += h - sh; else if (handle.includes('s')) b += sh - h;
    if (handle.includes('w') || handle.includes('e')) w = sw;
    if (handle.includes('n') || handle.includes('s')) h = sh;
  }
  if (keepAspect && handle.length === 2 && rect.w > 0 && rect.h > 0) {
    const k = Math.max(w / rect.w, h / rect.h);
    const nw = rect.w * k;
    const nh = rect.h * k;
    if (handle.includes('w')) l = rect.w - nw; else r = nw - rect.w;
    if (handle.includes('n')) t = rect.h - nh; else b = nh - rect.h;
    w = nw;
    h = nh;
  }
  if (w < min) {
    if (handle.includes('w')) l = rect.w - min; else r = min - rect.w;
    w = min;
  }
  if (h < min) {
    if (handle.includes('n')) t = rect.h - min; else b = min - rect.h;
    h = min;
  }
  // New centre in the element frame, relative to the old centre, then on screen.
  const [cx, cy] = rotatePoint((l + r) / 2, (t + b) / 2, 0, 0, rot);
  const ocx = rect.x + rect.w / 2;
  const ocy = rect.y + rect.h / 2;
  return { x: ocx + cx - w / 2, y: ocy + cy - h / 2, w, h };
}

/** Angle in degrees (clockwise, 0 = up) from the centre to a point. */
export function angleFrom(cx, cy, px, py) {
  const a = (Math.atan2(px - cx, -(py - cy)) * 180) / Math.PI;
  return normaliseAngle(a);
}

export function normaliseAngle(a) {
  let x = a % 360;
  if (x < 0) x += 360;
  return tidy(x);
}

// ---------------------------------------------------------------- arrange
// Each returns [{id, x, y, w?, h?}] patches; the caller applies them.
// The reference is the primary (first) selected element, as in Visual Studio.

export function align(els, how) {
  if (els.length < 2) return [];
  const ref = bounds(els[0]);
  return els.slice(1).map((el) => {
    const b = bounds(el);
    let { x, y } = el;
    if (how === 'left') x += ref.x - b.x;
    if (how === 'right') x += ref.x + ref.w - (b.x + b.w);
    if (how === 'center') x += ref.x + ref.w / 2 - (b.x + b.w / 2);
    if (how === 'top') y += ref.y - b.y;
    if (how === 'bottom') y += ref.y + ref.h - (b.y + b.h);
    if (how === 'middle') y += ref.y + ref.h / 2 - (b.y + b.h / 2);
    return { id: el.id, x: tidy(x), y: tidy(y) };
  });
}

/** Equal gaps between neighbours, outermost elements stay put. */
export function distribute(els, axis) {
  if (els.length < 3) return [];
  const pos = axis === 'h' ? 'x' : 'y';
  const size = axis === 'h' ? 'w' : 'h';
  const items = els.map((el) => ({ el, b: bounds(el) })).sort((a, b) => a.b[pos] - b.b[pos]);
  const first = items[0].b;
  const last = items[items.length - 1].b;
  const total = items.reduce((s, it) => s + it.b[size], 0);
  const gap = (last[pos] + last[size] - first[pos] - total) / (items.length - 1);
  let cursor = first[pos];
  const out = [];
  for (const it of items) {
    const delta = cursor - it.b[pos];
    if (it !== items[0] && it !== items[items.length - 1]) {
      out.push({ id: it.el.id, x: tidy(it.el.x + (pos === 'x' ? delta : 0)), y: tidy(it.el.y + (pos === 'y' ? delta : 0)) });
    }
    cursor += it.b[size] + gap;
  }
  return out;
}

/** Same width / height / both as the primary element, keeping each centre. */
export function sameSize(els, which) {
  if (els.length < 2) return [];
  const ref = els[0];
  return els.slice(1).map((el) => {
    const w = which === 'height' ? el.w : ref.w;
    const h = which === 'width' ? el.h : ref.h;
    return { id: el.id, x: tidy(el.x + (el.w - w) / 2), y: tidy(el.y + (el.h - h) / 2), w, h };
  });
}

/** Centres the selection on the screen along one or both axes. */
export function centreOnScreen(els, screen, axis = 'both') {
  const u = unionBounds(els);
  if (!u) return [];
  const dx = axis === 'v' ? 0 : screen.width / 2 - (u.x + u.w / 2);
  const dy = axis === 'h' ? 0 : screen.height / 2 - (u.y + u.h / 2);
  return els.map((el) => ({ id: el.id, x: tidy(el.x + dx), y: tidy(el.y + dy) }));
}

/** Scales a pipeline's points when its box changes size. */
export function scalePoints(points, oldW, oldH, newW, newH) {
  const kx = oldW > 0 ? newW / oldW : 1;
  const ky = oldH > 0 ? newH / oldH : 1;
  return points.map(([x, y]) => [tidy(x * kx), tidy(y * ky)]);
}

/** Bounding box of a polyline, used to fit a pipeline's box to its points. */
export function pointsBounds(points, thickness = 0) {
  if (!points.length) return { x: 0, y: 0, w: thickness, h: thickness };
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(thickness, Math.max(...xs) - x), h: Math.max(thickness, Math.max(...ys) - y) };
}
