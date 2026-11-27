// Trend widget: canvas chart with several pens, one Y axis per pen on the left
// (Movicon style), legend, time presets, pan/zoom (mouse, wheel, touch), hover
// read-out and a live "follow" mode fed by pushValues().
//
//   const t = createTrend(host, { api, project, tags, historian, spanS });
//   t.pushValues(new Map([[tag, {v,q,ts}]]));  t.setRange(from, to);  t.destroy();

import { h } from './svg.js';
import { niceScale, formatTick, formatDateTime, formatValue, toNumber } from './format.js';

export const PEN_COLOURS = ['#e040fb', '#448aff', '#4caf50', '#ffb300', '#ef5350', '#26c6da', '#f06292', '#c0ca33', '#ff7043', '#9575cd', '#80cbc4', '#bdbdbd'];

export const PRESETS = [
  ['Minute', 60e3], ['Hour', 3600e3], ['Day', 86400e3], ['Week', 7 * 86400e3],
  ['Month', 30 * 86400e3], ['Year', 365 * 86400e3], ['All', null],
];

const TIME_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400, 172800, 604800, 1209600, 2592000, 7776000, 15552000, 31536000].map((x) => x * 1000);

/** Picks a time-axis step so that about `targetTicks` labels fit. */
export function timeStep(spanMs, targetTicks) {
  const ideal = spanMs / Math.max(1, targetTicks);
  return TIME_STEPS.find((st) => st >= ideal) ?? TIME_STEPS[TIME_STEPS.length - 1];
}

/** Unit of a tag, taken from any display that shows it (tags carry no unit). */
export function unitForTag(project, tag) {
  for (const sc of project?.screens ?? []) {
    for (const el of sc.element ?? []) if (el.kind === 'display' && el.tag === tag && el.unit) return el.unit;
  }
  return '';
}

const pad = (n) => String(n).padStart(2, '0');
function timeLabel(ts, step, span) {
  const d = new Date(ts);
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  if (span > 60 * 86400e3) return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  if (span > 2 * 86400e3) return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}`;
  if (step < 60e3) return `${hm}:${pad(d.getSeconds())}`;
  return hm;
}

/** Aligns tick times to local midnight for day-sized steps, to whole units otherwise. */
function firstTick(from, step) {
  if (step >= 86400e3) {
    const d = new Date(from);
    d.setHours(0, 0, 0, 0);
    let t = d.getTime();
    while (t < from) t += step;
    return t;
  }
  const off = new Date(from).getTimezoneOffset() * 60e3;
  return Math.ceil((from - off) / step) * step + off;
}

/**
 * @param {HTMLElement} host
 * @param {{api:object, project?:object, tags:string[], historian?:string, spanS?:number, culture?:string}} opts
 */
/** Live points kept per pen at most, whatever the plot width. */
export const MAX_LIVE_POINTS = 20000;

/**
 * Bounds a pen's point list. Live mode appends a point per value change (4 per
 * second for a busy tag), so a Week or Year view left open would otherwise grow
 * without limit and redraw millions of points every second. Above `limit`, the
 * older three quarters are reduced to the min and max of each time bucket (as
 * the server's history query does), keeping a null where the bucket had a gap;
 * the newest quarter stays exact. The result has at most 5/8 of `limit` points,
 * so this runs once per ~3/8 `limit` new points.
 *
 * @param {Array<[number, number|null]>} points sorted by time
 * @param {number} limit
 */
export function compactPoints(points, limit) {
  if (points.length <= limit) return points;
  const keepRaw = Math.floor(limit / 4);
  const old = points.slice(0, points.length - keepRaw);
  const recent = points.slice(points.length - keepRaw);
  const buckets = Math.max(1, Math.floor(limit / 8));
  const t0 = old[0][0];
  const width = Math.max(1, (old[old.length - 1][0] - t0) / buckets);
  const bucketOf = (p) => Math.min(buckets - 1, Math.floor((p[0] - t0) / width));
  const out = [];
  for (let i = 0; i < old.length;) {
    const b = bucketOf(old[i]);
    let min = null;
    let max = null;
    let gap = null;
    for (; i < old.length && bucketOf(old[i]) === b; i += 1) {
      const p = old[i];
      if (p[1] === null || p[1] === undefined) gap ??= p;
      else {
        if (!min || p[1] < min[1]) min = p;
        if (!max || p[1] > max[1]) max = p;
      }
    }
    out.push(...[...new Set([min, max, gap])].filter(Boolean).sort((a, c) => a[0] - c[0]));
  }
  return out.concat(recent);
}

export function createTrend(host, opts) {
  const api = opts.api;
  const project = opts.project;
  const pens = (opts.tags ?? []).map((tag, i) => {
    const def = project?.tags?.find((t) => t.name === tag);
    return {
      tag, colour: PEN_COLOURS[i % PEN_COLOURS.length], visible: true,
      label: tag, description: def?.description ?? '', unit: unitForTag(project, tag),
      points: [], last: null, scale: null,
    };
  });
  let span = (opts.spanS ?? 3600) * 1000;
  let to = Date.now();
  let from = to - span;
  let follow = true;
  let maxPoints = 600;
  let loading = false;
  let error = '';
  let hoverX = null;
  let fetchSeq = 0;
  let destroyed = false;

  // ---------------------------------------------------------------- DOM
  host.classList.add('mc-trend');
  const presetBtns = PRESETS.map(([name, ms]) => h('button', {
    type: 'button', class: 'mc-tbtn', text: name, title: ms ? `Show the last ${name.toLowerCase()}` : 'Show all recorded data',
    onclick: () => applyPreset(name, ms),
  }));
  const liveBtn = h('button', { type: 'button', class: 'mc-tbtn mc-live', title: 'Follow live values', onclick: () => setFollow(!follow) }, h('span', { class: 'mc-live-dot' }), 'Live');
  const zoomIn = h('button', { type: 'button', class: 'mc-tbtn mc-icon', title: 'Zoom in', 'aria-label': 'Zoom in', text: '+', onclick: () => zoomBy(0.5) });
  const zoomOut = h('button', { type: 'button', class: 'mc-tbtn mc-icon', title: 'Zoom out', 'aria-label': 'Zoom out', text: '−', onclick: () => zoomBy(2) });
  const back = h('button', { type: 'button', class: 'mc-tbtn mc-icon', title: 'Earlier', 'aria-label': 'Earlier', text: '‹', onclick: () => shift(-0.5) });
  const fwd = h('button', { type: 'button', class: 'mc-tbtn mc-icon', title: 'Later', 'aria-label': 'Later', text: '›', onclick: () => shift(0.5) });
  const maxInput = h('input', { type: 'number', class: 'mc-tinput', min: 50, max: 10000, step: 50, value: maxPoints, title: 'Maximum points per pen', 'aria-label': 'Max points' });
  maxInput.addEventListener('change', () => {
    const n = Number(maxInput.value);
    maxPoints = Number.isFinite(n) ? Math.max(50, Math.min(10000, Math.round(n))) : 600;
    maxInput.value = maxPoints;
    refetch();
  });
  const rangeLabel = h('span', { class: 'mc-trend-range' });
  const toolbar = h('div', { class: 'mc-trend-toolbar' },
    h('div', { class: 'mc-tgroup' }, presetBtns),
    h('div', { class: 'mc-tgroup' }, back, zoomOut, zoomIn, fwd, liveBtn),
    h('label', { class: 'mc-tgroup mc-tlabel' }, 'Max points', maxInput),
    rangeLabel);
  const canvas = h('canvas', { class: 'mc-trend-canvas', 'aria-label': `Trend of ${pens.length} tags` });
  const tip = h('div', { class: 'mc-trend-tip', hidden: true });
  const body = h('div', { class: 'mc-trend-body' }, canvas, tip);
  const legend = h('div', { class: 'mc-trend-legend' });
  host.append(toolbar, body, legend);

  const legendItems = pens.map((pen) => {
    const val = h('span', { class: 'mc-pen-value', text: '----' });
    const item = h('button', {
      type: 'button', class: 'mc-pen', title: `${pen.tag}${pen.description ? ` — ${pen.description}` : ''}\nClick to show/hide`,
      style: { '--pen': pen.colour },
      onclick: () => {
        pen.visible = !pen.visible;
        item.classList.toggle('mc-pen-off', !pen.visible);
        draw();
      },
    }, h('span', { class: 'mc-pen-swatch' }), h('span', { class: 'mc-pen-name', text: pen.label }), val);
    legend.appendChild(item);
    return { pen, val };
  });

  // ---------------------------------------------------------------- data
  async function refetch() {
    if (!api || !pens.length) return;
    const seq = ++fetchSeq;
    loading = true;
    error = '';
    draw();
    const all = from <= 0;
    const qFrom = all ? 0 : from;
    const qTo = follow ? Date.now() : to;
    try {
      const res = await api.history({ tags: pens.map((p) => p.tag), from: qFrom, to: qTo, points: Math.min(maxPoints, Math.max(50, Math.round(plotWidth() * 1.5))) });
      if (seq !== fetchSeq || destroyed) return;
      const byTag = new Map((res?.series ?? []).map((sr) => [sr.tag, sr.points ?? []]));
      for (const pen of pens) {
        const pts = (byTag.get(pen.tag) ?? []).filter((p) => Array.isArray(p) && Number.isFinite(p[0])).map(([t, v]) => [t, toNumber(v)]);
        pts.sort((a, b) => a[0] - b[0]);
        // Keep live samples that arrived after the query's end.
        const tail = pen.points.filter((p) => p[0] > qTo);
        pen.points = pts.concat(tail);
      }
      if (all) {
        const first = Math.min(...pens.map((p) => (p.points[0] ? p.points[0][0] : Infinity)));
        from = Number.isFinite(first) ? first : Date.now() - span;
        to = Date.now();
        span = to - from;
      }
    } catch (err) {
      if (seq !== fetchSeq) return;
      error = `History not available: ${err.message}`;
    } finally {
      if (seq === fetchSeq) {
        loading = false;
        draw();
      }
    }
  }
  let fetchTimer = null;
  const refetchSoon = () => {
    clearTimeout(fetchTimer);
    fetchTimer = setTimeout(refetch, 250);
  };

  function pushValues(delta) {
    let changed = false;
    for (const pen of pens) {
      if (!delta.has(pen.tag)) continue;
      const sm = delta.get(pen.tag);
      pen.last = sm;
      const ts = sm?.ts ?? Date.now();
      const v = sm?.q === 'bad' ? null : toNumber(sm?.v);
      const lastPt = pen.points[pen.points.length - 1];
      if (!lastPt || ts > lastPt[0]) pen.points.push([ts, v]);
      changed = true;
    }
    if (changed) updateLegend();
  }

  function trim() {
    // Bound memory in follow mode: keep a little more than the visible window,
    // at a few points per pixel at most (long presets would grow without limit).
    const keepFrom = from - span;
    const limit = Math.min(MAX_LIVE_POINTS, Math.max(2000, Math.round(plotWidth() * 4)));
    for (const pen of pens) {
      if (pen.points.length && pen.points[0][0] < keepFrom) {
        const i = pen.points.findIndex((p) => p[0] >= keepFrom);
        // A tag that has not changed for a while still draws its last value.
        pen.points.splice(0, i < 0 ? pen.points.length - 1 : i);
      }
      pen.points = compactPoints(pen.points, limit);
    }
  }

  function updateLegend() {
    for (const { pen, val } of legendItems) {
      const sm = pen.last;
      val.textContent = sm ? `${formatValue(sm.v, { format: '#0.###' })}${pen.unit ? ` ${pen.unit}` : ''}` : '----';
      val.classList.toggle('mc-bad', sm?.q === 'bad');
    }
  }

  // ---------------------------------------------------------------- range control
  function setFollow(on) {
    follow = on;
    liveBtn.classList.toggle('mc-on', follow);
    if (follow) {
      to = Date.now();
      from = to - span;
      refetchSoon();
    }
    draw();
  }

  function applyPreset(name, ms) {
    presetBtns.forEach((b) => b.classList.toggle('mc-on', b.textContent === name));
    if (ms === null) {
      follow = false;
      liveBtn.classList.remove('mc-on');
      from = 0;
      refetch();
      return;
    }
    span = ms;
    setFollow(true);
  }

  function setRange(f, t) {
    from = f;
    to = t;
    span = Math.max(1000, t - f);
    follow = false;
    liveBtn.classList.remove('mc-on');
    presetBtns.forEach((b) => b.classList.remove('mc-on'));
    draw();
    refetchSoon();
  }

  function zoomBy(k, centreFrac = follow ? 1 : 0.5) {
    const c = from + (to - from) * centreFrac;
    const ns = Math.max(10e3, Math.min(20 * 365 * 86400e3, (to - from) * k));
    if (follow && centreFrac === 1) {
      span = ns;
      from = to - span;
      presetBtns.forEach((b) => b.classList.remove('mc-on'));
      draw();
      refetchSoon();
      return;
    }
    setRange(c - ns * centreFrac, c + ns * (1 - centreFrac));
  }

  function shift(frac) {
    const d = (to - from) * frac;
    setRange(from + d, to + d);
  }

  // ---------------------------------------------------------------- drawing
  const AXIS_W = 54;
  const BOTTOM = 26;
  const TOP = 10;
  const RIGHT = 14;
  let geom = { left: 0, width: 0, height: 0, axes: [] };

  function plotWidth() {
    return Math.max(100, body.clientWidth - geom.axes.length * AXIS_W - RIGHT);
  }

  function computeScales() {
    for (const pen of pens) {
      let lo = Infinity;
      let hi = -Infinity;
      for (const [t, v] of pen.points) {
        if (v === null || t < from || t > to) continue;
        if (v < lo) lo = v;
        if (v > hi) hi = v;
      }
      if (lo === Infinity) {
        const lv = toNumber(pen.last?.v);
        lo = lv ?? 0;
        hi = lv ?? 1;
      }
      pen.scale = niceScale(lo, hi, 6);
    }
  }

  function draw() {
    if (destroyed) return;
    const cssW = body.clientWidth;
    const cssH = body.clientHeight;
    if (!cssW || !cssH) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(cssW * dpr) || canvas.height !== Math.round(cssH * dpr)) {
      canvas.width = Math.round(cssW * dpr);
      canvas.height = Math.round(cssH * dpr);
      canvas.style.width = `${cssW}px`;
      canvas.style.height = `${cssH}px`;
    }
    if (follow) {
      to = Date.now();
      from = to - span;
      trim();
    }
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const css = getComputedStyle(host);
    const cText = css.getPropertyValue('--mc-text-dim').trim() || '#aab';
    const cGrid = css.getPropertyValue('--mc-grid').trim() || 'rgba(255,255,255,0.08)';
    const cPlot = css.getPropertyValue('--mc-plot-bg').trim() || '#2b2b2b';
    ctx.clearRect(0, 0, cssW, cssH);

    const visible = pens.filter((p) => p.visible);
    const maxAxes = Math.max(1, Math.floor((cssW * 0.4) / AXIS_W));
    const axes = visible.slice(0, maxAxes);
    const left = axes.length * AXIS_W;
    const width = Math.max(10, cssW - left - RIGHT);
    const height = Math.max(10, cssH - TOP - BOTTOM);
    geom = { left, width, height, axes };
    computeScales();

    ctx.fillStyle = cPlot;
    ctx.fillRect(left, TOP, width, height);
    ctx.font = '11px "Segoe UI", system-ui, sans-serif';

    // Horizontal grid from the first pen's scale; others draw ticks on their own axis.
    const yOf = (pen, v) => TOP + height - ((v - pen.scale.min) / (pen.scale.max - pen.scale.min || 1)) * height;
    ctx.strokeStyle = cGrid;
    ctx.lineWidth = 1;
    if (axes[0]) {
      for (const tv of axes[0].scale.ticks) {
        const y = Math.round(yOf(axes[0], tv)) + 0.5;
        ctx.beginPath(); ctx.moveTo(left, y); ctx.lineTo(left + width, y); ctx.stroke();
      }
    }
    // Time grid and labels.
    const spanNow = to - from;
    const step = timeStep(spanNow, Math.max(2, Math.floor(width / 130)));
    ctx.fillStyle = cText;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    const xOf = (t) => left + ((t - from) / (spanNow || 1)) * width;
    for (let t = firstTick(from, step); t <= to; t += step) {
      const x = Math.round(xOf(t)) + 0.5;
      ctx.strokeStyle = cGrid;
      ctx.beginPath(); ctx.moveTo(x, TOP); ctx.lineTo(x, TOP + height); ctx.stroke();
      ctx.fillText(timeLabel(t, step, spanNow), x, TOP + height + 6);
    }

    // Y axes, one per pen, in pen colour.
    axes.forEach((pen, i) => {
      const ax = left - i * AXIS_W - 6;
      ctx.strokeStyle = pen.colour;
      ctx.lineWidth = 2;
      ctx.beginPath(); ctx.moveTo(ax + 0.5, TOP); ctx.lineTo(ax + 0.5, TOP + height); ctx.stroke();
      ctx.fillStyle = pen.colour;
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';
      ctx.lineWidth = 1;
      for (const tv of pen.scale.ticks) {
        const y = yOf(pen, tv);
        ctx.beginPath(); ctx.moveTo(ax - 4, y); ctx.lineTo(ax, y); ctx.stroke();
        if (y > TOP + 6 && y < TOP + height - 4) ctx.fillText(formatTick(tv, pen.scale.step), ax - 6, y);
      }
    });

    // Pens.
    ctx.save();
    ctx.beginPath();
    ctx.rect(left, TOP, width, height);
    ctx.clip();
    for (const pen of visible) {
      ctx.strokeStyle = pen.colour;
      ctx.lineWidth = 1.6;
      ctx.lineJoin = 'round';
      ctx.beginPath();
      let pen_down = false;
      for (const [t, v] of pen.points) {
        if (t < from - spanNow * 0.05 || t > to + spanNow * 0.05) continue;
        if (v === null) { pen_down = false; continue; }
        const x = xOf(t);
        const y = yOf(pen, v);
        if (pen_down) ctx.lineTo(x, y);
        else ctx.moveTo(x, y);
        pen_down = true;
      }
      ctx.stroke();
    }
    ctx.restore();

    // Hover read-out.
    if (hoverX !== null && hoverX >= left && hoverX <= left + width) {
      const t = from + ((hoverX - left) / width) * spanNow;
      ctx.strokeStyle = 'rgba(255,255,255,0.55)';
      ctx.setLineDash([4, 3]);
      ctx.beginPath(); ctx.moveTo(hoverX + 0.5, TOP); ctx.lineTo(hoverX + 0.5, TOP + height); ctx.stroke();
      ctx.setLineDash([]);
      const rows = visible.map((pen) => {
        const p = nearest(pen.points, t);
        if (p && p[1] !== null) {
          ctx.fillStyle = pen.colour;
          ctx.beginPath(); ctx.arc(xOf(p[0]), yOf(pen, p[1]), 3.2, 0, Math.PI * 2); ctx.fill();
        }
        return { pen, v: p ? p[1] : null };
      });
      tip.hidden = false;
      tip.replaceChildren(h('div', { class: 'mc-tip-time', text: formatDateTime(t) }),
        ...rows.map(({ pen, v }) => h('div', { class: 'mc-tip-row' },
          h('span', { class: 'mc-pen-swatch', style: { background: pen.colour } }),
          h('span', { text: pen.label }),
          h('b', { text: v === null ? '----' : `${formatValue(v, { format: '#0.###' })}${pen.unit ? ` ${pen.unit}` : ''}` }))));
      const tipW = tip.offsetWidth || 220;
      tip.style.left = `${hoverX + 14 + tipW > cssW ? hoverX - tipW - 14 : hoverX + 14}px`;
      tip.style.top = `${TOP + 6}px`;
    } else tip.hidden = true;

    if (loading || error || !pens.length) {
      ctx.fillStyle = error ? '#ff8a80' : cText;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.font = '13px "Segoe UI", system-ui, sans-serif';
      ctx.fillText(!pens.length ? 'No pens configured' : error || 'Loading history…', left + width / 2, TOP + height / 2);
    }
    rangeLabel.textContent = `${formatDateTime(from)}  →  ${follow ? 'now' : formatDateTime(to)}`;
  }

  function nearest(points, t) {
    let lo = 0;
    let hi = points.length - 1;
    if (hi < 0) return null;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (points[mid][0] < t) lo = mid + 1;
      else hi = mid;
    }
    const a = points[Math.max(0, lo - 1)];
    const b = points[lo];
    return Math.abs(a[0] - t) < Math.abs(b[0] - t) ? a : b;
  }

  // ---------------------------------------------------------------- interaction
  const pointers = new Map();
  let drag = null;
  let pinch = null;
  const localX = (e) => e.clientX - canvas.getBoundingClientRect().left;

  canvas.addEventListener('pointerdown', (e) => {
    canvas.setPointerCapture?.(e.pointerId);
    pointers.set(e.pointerId, localX(e));
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      pinch = { dist: Math.abs(a - b) || 1, from, to, mid: (a + b) / 2 };
      drag = null;
    } else drag = { x: localX(e), from, to, moved: false };
  });
  canvas.addEventListener('pointermove', (e) => {
    const x = localX(e);
    if (e.pointerType === 'mouse') hoverX = x;
    if (pointers.has(e.pointerId)) pointers.set(e.pointerId, x);
    if (pinch && pointers.size >= 2) {
      const [a, b] = [...pointers.values()];
      const k = pinch.dist / (Math.abs(a - b) || 1);
      const span0 = pinch.to - pinch.from;
      const frac = (pinch.mid - geom.left) / (geom.width || 1);
      const c = pinch.from + span0 * frac;
      const ns = Math.max(10e3, span0 * k);
      setRange(c - ns * frac, c + ns * (1 - frac));
    } else if (drag) {
      const dx = x - drag.x;
      if (!drag.moved && Math.abs(dx) < 4) { draw(); return; }
      drag.moved = true;
      const dt = (-dx / (geom.width || 1)) * (drag.to - drag.from);
      setRange(drag.from + dt, drag.to + dt);
    } else draw();
  });
  const endPointer = (e) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (!pointers.size) {
      if (drag && !drag.moved && e.pointerType !== 'mouse') {
        hoverX = localX(e);
        draw();
      }
      drag = null;
    }
  };
  canvas.addEventListener('pointerup', endPointer);
  canvas.addEventListener('pointercancel', endPointer);
  canvas.addEventListener('pointerleave', () => { hoverX = null; draw(); });
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const frac = Math.max(0, Math.min(1, (localX(e) - geom.left) / (geom.width || 1)));
    zoomBy(Math.exp(e.deltaY * 0.0015), follow ? 1 : frac);
  }, { passive: false });
  canvas.addEventListener('dblclick', () => applyPreset('Hour', 3600e3));

  const ro = new ResizeObserver(() => draw());
  ro.observe(body);
  const timer = setInterval(() => { if (follow) draw(); }, 1000);

  // Initial state: the preset matching span_s, live.
  const match = PRESETS.find(([, ms]) => ms === span);
  presetBtns.forEach((b) => b.classList.toggle('mc-on', Boolean(match) && b.textContent === match[0]));
  liveBtn.classList.add('mc-on');
  refetch();

  return {
    pushValues,
    setRange,
    refetch,
    get range() { return { from, to, follow }; },
    destroy() {
      destroyed = true;
      ro.disconnect();
      clearInterval(timer);
      clearTimeout(fetchTimer);
      host.replaceChildren();
      host.classList.remove('mc-trend');
    },
  };
}
