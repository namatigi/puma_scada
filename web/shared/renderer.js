// Screen renderer: draws a bundle Screen as SVG at its native size, scaled to
// fit its container (aspect ratio kept), and keeps it live.
//
//   const view = createScreenView(container, screen, {
//     project, symbolMap, api, mode: 'runtime' | 'design', onCommand,
//   });
//   view.update({ TAG: { v, q, ts } });   view.setAlarms(list);
//   view.tagsInUse();  view.elementNode(id);  view.destroy();
//
// HTML widgets (trend, history table, alarm list) live in an overlay above the
// SVG, positioned in real pixels, so canvas and table text stay sharp at any scale.

import { s, h, uid, linearGradient, cssBackgroundToSvg } from './svg.js';
import { mayOperate } from './access.js';
import { formatValue, formatAge, truthy, toNumber, parseInputValue, formatDateTime, NO_VALUE } from './format.js';
import { drawSymbol, symbolState, tube, collar } from './symbols.js';
import { resolveSymbol, EMPTY_SYMBOL_MAP } from './symbolmap.js';
import { createTrend } from './trend.js';
import { createHistoryTable } from './historytable.js';
import { createAlarmList } from './alarmlist.js';
import { toast } from './toast.js';

const WIDGET_KINDS = new Set(['trend', 'history-table', 'alarm-list']);
const FONT = "'Segoe UI', 'Segoe UI Variable', system-ui, -apple-system, 'Helvetica Neue', Arial, sans-serif";

/**
 * True when `screen` should open as a floating panel over `main` rather than
 * replace it: narrower or shorter than half the main screen and under half its area.
 */
export function isPopupScreen(screen, main) {
  if (!screen || !main || screen.name === main.name) return false;
  const smaller = screen.width < main.width / 2 || screen.height < main.height / 2;
  return smaller && screen.width * screen.height < (main.width * main.height) / 2;
}

/** Every tag an element reads (for subscriptions and "where used"). */
export function elementTags(el) {
  switch (el.kind) {
    case 'display':
    case 'pipeline':
    case 'symbol':
      return el.tag ? [el.tag] : [];
    case 'trend':
      return el.tags ?? [];
    default:
      return [];
  }
}

/** Tags read by a whole screen. */
export function screenTags(screen) {
  const out = new Set();
  for (const el of screen.element ?? []) elementTags(el).forEach((t) => out.add(t));
  return out;
}

/** Rough text width for layout before the text is in the DOM (Segoe UI metrics). */
export function estimateTextWidth(text, size, bold = false) {
  let w = 0;
  for (const ch of String(text)) {
    if ('il.,:;|!\'`'.includes(ch)) w += 0.28;
    else if ('mwMW'.includes(ch)) w += 0.86;
    else if (ch === ' ') w += 0.28;
    else if (/[0-9#]/.test(ch)) w += 0.54;
    else if (/[A-Z]/.test(ch)) w += 0.62;
    else w += 0.52;
  }
  return w * size * (bold ? 1.06 : 1);
}

/**
 * @param {HTMLElement} container
 * @param {object} screen bundle Screen ({name,width,height,background,element:[…]})
 * @param {object} [opts]
 * @param {object} [opts.project] the bundle; used for tag types/access and popup detection
 * @param {object} [opts.symbolMap] see symbolmap.js
 * @param {object} [opts.api] REST api (api.js) for trend/history widgets
 * @param {'runtime'|'design'} [opts.mode]
 * @param {(cmd:object, ctx:{element:object, view:object}) => void} [opts.onCommand]
 * @param {boolean} [opts.interactive] false for thumbnails
 * @param {boolean} [opts.zoomable] pinch / wheel zoom and pan (runtime)
 * @param {'live'|'placeholder'} [opts.widgets]
 * @param {number} [opts.staleMs] age after which a value is shown as stale
 * @param {object|Map} [opts.values] initial values
 */
/**
 * Whether a pipe animates for `sample` and which way. Numbers flow when their
 * size is above `min` (negative values flow backwards); bools when true.
 * Bad quality never animates: a stale value must not look like product moving.
 */
export function pipeFlow(sample, min = 0, reversed = false) {
  if (!sample || sample.q === 'bad') return { active: false, reverse: false };
  const n = toNumber(sample.v);
  const active = typeof sample.v === 'number' ? Math.abs(n) > min : truthy(sample.v);
  return { active, reverse: active && (reversed !== (n !== null && n < 0)) };
}

export function createScreenView(container, screen, opts = {}) {
  const mode = opts.mode ?? 'runtime';
  const runtime = mode === 'runtime';
  const interactive = opts.interactive ?? true;
  const project = opts.project ?? null;
  const symbolMap = opts.symbolMap ?? EMPTY_SYMBOL_MAP;
  const widgetsMode = opts.widgets ?? (runtime && interactive ? 'live' : 'placeholder');
  const staleMs = opts.staleMs ?? 5 * 60 * 1000;
  const culture = opts.culture ?? project?.meta?.culture ?? 'en-US';
  const onCommand = opts.onCommand ?? defaultCommandHandler;
  const W = Math.max(1, screen.width);
  const H = Math.max(1, screen.height);
  const tagsByName = new Map((project?.tags ?? []).map((t) => [t.name, t]));
  const mainScreen = project?.screens?.find((x) => x.name === project?.meta?.main_screen);

  const values = new Map();
  const nodes = new Map();
  /** Per-element live updaters: (values) => void */
  const updaters = [];
  const widgets = [];
  const cleanups = [];

  // ---------------------------------------------------------------- skeleton
  container.classList.add('mc-screen-host');
  if (!runtime) container.classList.add('mc-design');
  const bg = cssBackgroundToSvg(screen.background);
  const svg = s('svg', {
    class: 'mc-screen-svg', viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: 'none',
    'font-family': FONT, role: 'img', 'aria-label': `Screen ${screen.name}`,
  });
  const defs = s('defs');
  if (bg.defs) defs.appendChild(bg.defs);
  const btnGrad = uid('btn');
  const tagGrad = uid('tag');
  defs.append(
    linearGradient(btnGrad, [[0, '#fbfbfb'], [0.48, '#dcdddf'], [0.52, '#cfd0d2'], [1, '#aeb0b3']]),
    linearGradient(tagGrad, [[0, '#444547'], [1, '#232324']]),
  );
  svg.appendChild(defs);
  svg.appendChild(s('rect', { class: 'mc-bg', x: 0, y: 0, width: W, height: H, fill: bg.fill ?? 'var(--mc-screen-bg, #3F3F3F)' }));
  const layer = s('g', { class: 'mc-layer' });
  svg.appendChild(layer);
  const overlay = h('div', { class: 'mc-overlay' });
  const stage = h('div', { class: 'mc-stage' }, svg, overlay);
  container.appendChild(stage);

  const view = {
    screen,
    container,
    get scale() { return scale; },
    update,
    setAlarms,
    tagsInUse: () => screenTags(screen),
    elementNode: (id) => nodes.get(id) ?? null,
    setOffline: (off) => container.classList.toggle('mc-offline', Boolean(off)),
    resetZoom,
    resize: layout,
    destroy,
  };

  // ---------------------------------------------------------------- elements
  for (const el of screen.element ?? []) {
    try {
      const node = renderElement(el);
      if (node) {
        nodes.set(el.id, node);
        layer.appendChild(node);
      }
    } catch (err) {
      console.error(`[moviclone] element ${screen.name}/${el.id} (${el.kind}) failed to render`, err);
    }
  }

  function placeTransform(el) {
    const rot = el.rotation ? ` rotate(${el.rotation} ${el.w / 2} ${el.h / 2})` : '';
    return `translate(${el.x} ${el.y})${rot}`;
  }

  function baseGroup(el, extraClass = '') {
    return s('g', {
      class: `mc-el mc-${el.kind} ${extraClass}`.trim(),
      'data-id': el.id, 'data-kind': el.kind, transform: placeTransform(el),
    });
  }

  function renderElement(el) {
    switch (el.kind) {
      case 'label': return renderLabel(el);
      case 'display': return renderDisplay(el);
      case 'button': return renderButton(el);
      case 'pipeline': return renderPipeline(el);
      case 'symbol': return renderSymbol(el);
      case 'trend':
      case 'history-table':
      case 'alarm-list': return renderWidget(el);
      default: {
        const g = baseGroup(el, 'mc-unknown');
        g.appendChild(s('rect', { width: el.w, height: el.h, fill: 'none', stroke: '#ff4fb8', 'stroke-dasharray': '4 3' }));
        return g;
      }
    }
  }

  // ---- label
  function renderLabel(el) {
    const g = baseGroup(el);
    const text = el.text ?? '';
    const bold = el.h >= 36;
    let size = Math.max(9, Math.min(el.h * (bold ? 0.5 : 0.47), 28));
    const fit = (el.w - 4) / Math.max(1, estimateTextWidth(text, 1, bold));
    size = Math.max(8, Math.min(size, fit));
    g.appendChild(s('rect', { class: 'mc-hit', width: el.w, height: el.h, fill: 'transparent' }));
    g.appendChild(s('text', {
      class: 'mc-label-text', x: el.w / 2, y: el.h / 2, 'text-anchor': 'middle', 'dominant-baseline': 'central',
      'font-size': size.toFixed(1), 'font-weight': bold ? 700 : 400,
    }, text));
    return g;
  }

  // ---- display
  function renderDisplay(el) {
    const tagDef = el.tag ? tagsByName.get(el.tag) : null;
    const writable = Boolean(tagDef && tagDef.access && tagDef.access !== 'read');
    const editable = runtime && interactive && el.editable && writable;
    const g = baseGroup(el, editable ? 'mc-editable' : '');
    const unit = el.unit ?? '';
    const valueSize = Math.max(9, Math.min(20, el.h * 0.42));
    const unitSize = Math.max(8, Math.min(14, el.h * 0.29));
    const unitW = unit ? estimateTextWidth(unit, unitSize) + 8 : 0;
    const title = s('title');
    const frame = s('rect', { class: 'mc-display-bg', width: el.w, height: el.h, rx: Math.min(3, el.h * 0.06) });
    const border = s('rect', { class: 'mc-display-border', x: 1, y: 1, width: Math.max(0, el.w - 2), height: Math.max(0, el.h - 2), rx: Math.min(3, el.h * 0.06), fill: 'none' });
    const valueText = s('text', {
      class: 'mc-display-value', x: el.w - unitW - 8, y: el.h / 2, 'text-anchor': 'end', 'dominant-baseline': 'central',
      'font-size': valueSize.toFixed(1),
    });
    const tri = el.h * 0.36;
    const warn = s('g', { class: 'mc-quality-icon', transform: `translate(${6} ${(el.h - tri) / 2})` },
      s('path', { d: `M${tri / 2},0L${tri},${tri}H0Z`, 'stroke-linejoin': 'round' }),
      s('text', { x: tri / 2, y: tri * 0.84, 'text-anchor': 'middle', 'font-size': (tri * 0.72).toFixed(1), 'font-weight': 800 }, '!'));
    g.append(title, frame, valueText, border, warn);
    if (unit) {
      g.appendChild(s('text', {
        class: 'mc-display-unit', x: el.w - 6, y: el.h / 2, 'text-anchor': 'end', 'dominant-baseline': 'central',
        'font-size': unitSize.toFixed(1),
      }, unit));
    }
    const maxValueW = el.w - unitW - 10 - tri * 0.6;
    const tagLabel = el.tag ?? 'no tag';
    let lastSample = null;

    const paint = (sample) => {
      lastSample = sample;
      let text;
      if (!sample) text = runtime ? NO_VALUE : (el.format ?? NO_VALUE);
      else text = formatValue(sample.v, { format: el.format, culture, dataType: tagDef?.type });
      valueText.textContent = text;
      // Shrink long values instead of letting them run into the unit.
      const est = estimateTextWidth(text, valueSize, true);
      valueText.setAttribute('font-size', (est > maxValueW && est > 0 ? Math.max(8, valueSize * (maxValueW / est)) : valueSize).toFixed(1));
      const q = sample?.q ?? (runtime && el.tag ? 'none' : 'good');
      g.dataset.quality = q;
      g.classList.toggle('mc-q-bad', q === 'bad');
      g.classList.toggle('mc-q-uncertain', q === 'uncertain');
      g.classList.toggle('mc-q-none', q === 'none');
      refreshStale();
    };
    const refreshStale = () => {
      const ts = lastSample?.ts;
      g.classList.toggle('mc-stale', Boolean(runtime && ts && Date.now() - ts > staleMs));
    };
    const describe = () => {
      if (!el.tag) return 'No tag bound';
      if (!lastSample) return `${tagLabel}\nNo value yet`;
      const age = lastSample.ts ? formatAge(Date.now() - lastSample.ts) : 'unknown';
      const lines = [
        `${tagLabel}${tagDef?.description ? ` (${tagDef.description})` : ''}`,
        `Quality: ${lastSample.q ?? 'unknown'}`,
        `Updated ${age} ago${lastSample.ts ? ` (${formatDateTime(lastSample.ts)})` : ''}`,
      ];
      if (editable) lines.push('Click to enter a new value');
      else if (el.editable && !writable && runtime) lines.push('Read-only tag');
      return lines.join('\n');
    };
    g.addEventListener('pointerenter', () => { title.textContent = describe(); });
    paint(null);
    if (el.tag) {
      updaters.push((vals) => {
        if (vals.has(el.tag)) paint(vals.get(el.tag));
      });
      updaters.push({ tick: refreshStale });
    }
    if (editable) {
      g.setAttribute('tabindex', '0');
      g.setAttribute('role', 'button');
      g.setAttribute('aria-label', `Edit ${el.tag}`);
      const openEd = (e) => {
        e.stopPropagation();
        if (!mayOperate(`change ${el.tag}`)) return;
        openEditor(el, tagDef, lastSample);
      };
      g.addEventListener('click', openEd);
      g.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') openEd(e); });
    }
    return g;
  }

  function openEditor(el, tagDef, sample) {
    overlay.querySelector('.mc-editor')?.remove();
    const input = h('input', {
      class: 'mc-editor-input', type: 'text', inputmode: tagDef.type === 'string' ? 'text' : 'decimal',
      value: sample && sample.v !== null && sample.v !== undefined ? String(sample.v) : '', 'aria-label': `New value for ${el.tag}`,
    });
    const err = h('div', { class: 'mc-editor-error' });
    const ok = h('button', { class: 'mc-editor-ok', type: 'button', text: 'Write' });
    const cancel = h('button', { class: 'mc-editor-cancel', type: 'button', text: 'Cancel' });
    const box = h('form', { class: 'mc-editor' },
      h('div', { class: 'mc-editor-title', text: `${el.tag}${el.unit ? ` [${el.unit}]` : ''}` }), input, err,
      h('div', { class: 'mc-editor-actions' }, cancel, ok));
    const place = () => {
      const left = Math.max(4, Math.min(el.x * scale, overlay.clientWidth - 230));
      const top = Math.max(4, Math.min((el.y + el.h) * scale + 4, overlay.clientHeight - 130));
      box.style.left = `${left}px`;
      box.style.top = `${top}px`;
    };
    place();
    const close = () => box.remove();
    box.addEventListener('submit', (e) => {
      e.preventDefault();
      try {
        parseInputValue(input.value, tagDef.type, culture);
      } catch (ex) {
        err.textContent = ex.message;
        input.focus();
        return;
      }
      close();
      onCommand({ action: 'write-tag', tag: el.tag, value: input.value, mode: 'set' }, { element: el, view });
    });
    cancel.addEventListener('click', close);
    box.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });
    box.addEventListener('pointerdown', (e) => e.stopPropagation());
    overlay.appendChild(box);
    input.focus();
    input.select();
  }

  // ---- button
  function renderButton(el) {
    const cmds = el.on_click ?? [];
    const target = cmds.find((c) => c.action === 'open-screen');
    const targetScreen = target && project?.screens?.find((x) => x.name === target.screen);
    // Buttons that open a faceplate read as dark name tags on Movicon screens.
    const pill = Boolean(targetScreen && mainScreen && isPopupScreen(targetScreen, mainScreen));
    const g = baseGroup(el, pill ? 'mc-button-tag' : 'mc-button-glossy');
    const r = Math.min(el.h * 0.16, 6);
    const text = el.text ?? '';
    let size = Math.min(el.h * (pill ? 0.42 : 0.4), pill ? 15 : 22);
    const fit = (el.w - 10) / Math.max(1, estimateTextWidth(text, 1, true));
    size = Math.max(8, Math.min(size, fit));
    g.append(
      s('rect', { class: 'mc-btn-shadow', x: 1, y: 2, width: el.w - 1, height: el.h - 1, rx: r }),
      s('rect', { class: 'mc-btn-face', width: el.w, height: el.h - 1, rx: r, fill: `url(#${pill ? tagGrad : btnGrad})` }),
      s('rect', { class: 'mc-btn-gloss', x: 2, y: 1.5, width: Math.max(0, el.w - 4), height: Math.max(0, (el.h - 1) * 0.46), rx: Math.max(0, r - 1) }),
      s('text', {
        class: 'mc-btn-text', x: el.w / 2, y: (el.h - 1) / 2, 'text-anchor': 'middle', 'dominant-baseline': 'central',
        'font-size': size.toFixed(1), 'font-weight': 700,
      }, text),
    );
    if (runtime && interactive) {
      g.setAttribute('tabindex', '0');
      g.setAttribute('role', 'button');
      g.setAttribute('aria-label', text || el.id);
      const fire = (e) => {
        e.stopPropagation();
        if (!cmds.length) {
          toast(`"${text || el.id}" has no command`, { kind: 'warn' });
          return;
        }
        for (const cmd of cmds) onCommand(cmd, { element: el, view });
      };
      g.addEventListener('click', fire);
      g.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); fire(e); } });
      g.addEventListener('pointerdown', () => g.classList.add('mc-pressed'));
      const up = () => g.classList.remove('mc-pressed');
      g.addEventListener('pointerup', up);
      g.addEventListener('pointerleave', up);
    }
    return g;
  }

  // ---- pipeline
  function renderPipeline(el) {
    const g = baseGroup(el);
    const pts = (el.points ?? []).filter((p) => Array.isArray(p) && p.length >= 2);
    if (pts.length < 2) return g;
    const d = pts.map(([x, y], i) => `${i ? 'L' : 'M'}${x},${y}`).join('');
    const t = el.thickness || 20;
    const colours = { fill: el.fill ?? '#EFEFF0', stroke: el.stroke ?? '#5E5E5E' };
    g.appendChild(tube(d, t, colours));
    // Couplings at straight-through joints; bends get their own fittings (elbow symbols).
    for (let i = 1; i < pts.length - 1; i += 1) {
      const a1 = Math.atan2(pts[i][1] - pts[i - 1][1], pts[i][0] - pts[i - 1][0]);
      const a2 = Math.atan2(pts[i + 1][1] - pts[i][1], pts[i + 1][0] - pts[i][0]);
      let diff = Math.abs(a1 - a2);
      if (diff > Math.PI) diff = 2 * Math.PI - diff;
      const segLen = Math.min(Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]), Math.hypot(pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]));
      if (diff < 0.12 && segLen > t * 0.6) g.appendChild(collar(pts[i][0], pts[i][1], (a1 * 180) / Math.PI, t * 1.1, Math.max(3, t * 0.16), colours));
    }
    if (el.tag) {
      // Moving dashes in the product's colour over a darker edge, so even a
      // white product (Jet A-1) shows on the light steel pipe.
      const dash = { d, fill: 'none', 'stroke-linecap': 'round', 'stroke-dasharray': `${(t * 0.55).toFixed(1)} ${(t * 1.1).toFixed(1)}`, style: `--mc-flow-len:${(t * 1.65).toFixed(1)}px` };
      const core = Math.max(2, t * 0.3);
      g.appendChild(s('path', { ...dash, class: 'mc-flow mc-flow-edge', 'stroke-width': core + Math.max(2, t * 0.12) }));
      const flow = s('path', { ...dash, class: 'mc-flow', 'stroke-width': core });
      if (el.flow_color) flow.style.stroke = el.flow_color;
      g.appendChild(flow);
      const title = s('title', {}, el.tag);
      g.appendChild(title);
      const min = Number.isFinite(el.flow_min) ? el.flow_min : 0;
      updaters.push((vals) => {
        if (!vals.has(el.tag)) return;
        const sample = vals.get(el.tag);
        const { active, reverse } = pipeFlow(sample, min, el.flow_reverse === true);
        g.classList.toggle('mc-flowing', active);
        g.classList.toggle('mc-flow-reverse', active && reverse);
        g.classList.toggle('mc-q-bad', sample?.q === 'bad');
        title.textContent = `${el.tag}: ${formatValue(sample?.v)}${active ? ' (flowing)' : ''}`;
      });
    }
    return g;
  }

  // ---- symbol
  function renderSymbol(el) {
    const g = baseGroup(el);
    const resolved = resolveSymbol(el.symbol, symbolMap, { screen: screen.name, id: el.id });
    g.dataset.symbol = resolved.name;
    const title = s('title');
    g.appendChild(title);
    let body = null;
    let lastKey = '';
    const draw = (sample) => {
      const state = symbolState(resolved.name, { bound: Boolean(el.tag), value: sample?.v, quality: sample?.q, params: resolved.params });
      const valueKey = typeof sample?.v === 'number' ? sample.v.toFixed(2) : String(sample?.v);
      const key = `${state}|${valueKey}|${sample?.q}`;
      if (key === lastKey) return;
      lastKey = key;
      const next = drawSymbol(resolved.name, { w: el.w, h: el.h, params: resolved.params, state, value: sample?.v ?? null, quality: sample?.q ?? 'good' });
      if (body) body.replaceWith(next);
      else g.appendChild(next);
      body = next;
      const name = resolved.label ? `${resolved.label} — ` : '';
      title.textContent = el.tag
        ? `${name}${el.tag}: ${formatValue(sample?.v)} (${state})`
        : `${name}${resolved.known ? resolved.name : `unmapped symbol ${el.symbol ?? ''}`}`;
    };
    draw(null);
    if (el.tag) updaters.push((vals) => { if (vals.has(el.tag)) draw(vals.get(el.tag)); });
    return g;
  }

  // ---- widgets (trend, history table, alarm list)
  function renderWidget(el) {
    const g = baseGroup(el, 'mc-widget-frame');
    g.appendChild(s('rect', { class: 'mc-widget-bg', width: el.w, height: el.h, rx: 2 }));
    if (widgetsMode !== 'live') {
      drawWidgetPlaceholder(g, el);
      return g;
    }
    const host = h('div', { class: `mc-widget mc-widget-${el.kind}`, 'data-id': el.id });
    overlay.appendChild(host);
    let widget;
    const common = { api: opts.api, project, culture };
    if (el.kind === 'trend') widget = createTrend(host, { ...common, tags: el.tags ?? [], historian: el.historian, spanS: el.span_s ?? 3600 });
    else if (el.kind === 'history-table') widget = createHistoryTable(host, { ...common, source: el.source });
    else widget = createAlarmList(host, { ...common, area: el.area, compact: el.h < 260 });
    widgets.push({ el, host, widget });
    return g;
  }

  function drawWidgetPlaceholder(g, el) {
    const label = el.kind === 'trend' ? `Trend · ${(el.tags ?? []).length} pens${el.historian ? ` · ${el.historian}` : ''}`
      : el.kind === 'history-table' ? `History table · ${el.source}` : `Alarm list${el.area ? ` · ${el.area}` : ''}`;
    if (el.kind === 'trend') {
      const left = Math.min(240, el.w * 0.15);
      for (let i = 1; i < 5; i += 1) {
        const y = (el.h * i) / 5;
        g.appendChild(s('line', { x1: left, y1: y, x2: el.w - 10, y2: y, class: 'mc-ph-grid' }));
      }
      const colours = ['#e040fb', '#448aff', '#43a047', '#ffb300', '#ef5350'];
      colours.forEach((c, i) => {
        let d = '';
        for (let k = 0; k <= 24; k += 1) {
          const x = left + ((el.w - left - 10) * k) / 24;
          const y = el.h * (0.25 + 0.12 * i) + Math.sin(k / 2.2 + i) * el.h * 0.06;
          d += `${k ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`;
        }
        g.appendChild(s('path', { d, stroke: c, fill: 'none', 'stroke-width': Math.max(1.5, el.h / 160), opacity: 0.85 }));
        g.appendChild(s('line', { x1: 14 + i * (left / 6), y1: 10, x2: 14 + i * (left / 6), y2: el.h - 20, stroke: c, 'stroke-width': 2 }));
      });
    } else {
      const rows = Math.max(2, Math.floor(el.h / 40));
      for (let i = 0; i < rows; i += 1) {
        g.appendChild(s('rect', { x: 8, y: 10 + i * (el.h - 20) / rows, width: el.w - 16, height: (el.h - 20) / rows - 6, class: i === 0 ? 'mc-ph-head' : 'mc-ph-row' }));
      }
    }
    g.appendChild(s('text', { class: 'mc-ph-label', x: el.w / 2, y: el.h / 2, 'text-anchor': 'middle', 'dominant-baseline': 'central', 'font-size': Math.max(12, Math.min(40, el.h / 8)) }, label));
  }

  // ---------------------------------------------------------------- live data
  function update(changes) {
    if (!changes) return;
    const entries = changes instanceof Map ? [...changes] : Object.entries(changes);
    if (!entries.length) return;
    const delta = new Map();
    for (const [tag, sample] of entries) {
      values.set(tag, sample);
      delta.set(tag, sample);
    }
    for (const u of updaters) if (typeof u === 'function') u(delta);
    for (const { widget } of widgets) widget.pushValues?.(delta);
  }

  function setAlarms(list) {
    for (const { widget } of widgets) widget.setAlarms?.(list);
  }

  const tickTimer = runtime ? setInterval(() => {
    for (const u of updaters) if (typeof u === 'object' && u.tick) u.tick();
  }, 5000) : null;

  // ---------------------------------------------------------------- layout, zoom, pan
  let scale = 1;
  let zoom = 1;
  let panX = 0;
  let panY = 0;

  function layout() {
    const cw = container.clientWidth;
    const ch = container.clientHeight;
    if (!cw || !ch) return;
    const fit = Math.min(cw / W, ch / H);
    scale = fit * zoom;
    const sw = W * scale;
    const sh = H * scale;
    // Keep at least part of the screen in view while panning.
    const maxX = Math.max(0, (sw - cw) / 2);
    const maxY = Math.max(0, (sh - ch) / 2);
    panX = Math.max(-maxX, Math.min(maxX, panX));
    panY = Math.max(-maxY, Math.min(maxY, panY));
    stage.style.width = `${sw}px`;
    stage.style.height = `${sh}px`;
    stage.style.left = `${(cw - sw) / 2 + panX}px`;
    stage.style.top = `${(ch - sh) / 2 + panY}px`;
    for (const { el, host } of widgets) {
      host.style.left = `${el.x * scale}px`;
      host.style.top = `${el.y * scale}px`;
      host.style.width = `${el.w * scale}px`;
      host.style.height = `${el.h * scale}px`;
      host.style.transform = el.rotation ? `rotate(${el.rotation}deg)` : '';
    }
    container.classList.toggle('mc-zoomed', zoom > 1.001);
  }

  function zoomAt(newZoom, cx, cy) {
    const z = Math.max(1, Math.min(8, newZoom));
    const rect = container.getBoundingClientRect();
    // Keep the point under the cursor fixed.
    const px = cx - rect.left - rect.width / 2 - panX;
    const py = cy - rect.top - rect.height / 2 - panY;
    const k = z / zoom;
    panX -= px * (k - 1);
    panY -= py * (k - 1);
    zoom = z;
    if (zoom === 1) { panX = 0; panY = 0; }
    layout();
  }

  function resetZoom() {
    zoom = 1;
    panX = 0;
    panY = 0;
    layout();
  }

  if (opts.zoomable && interactive) {
    container.classList.add('mc-zoomable');
    const pointers = new Map();
    let pinch = null;
    let drag = null;
    let suppressClick = false;
    const inWidget = (t) => t instanceof Element && t.closest('.mc-widget, .mc-editor');
    const onWheel = (e) => {
      if (inWidget(e.target)) return;
      e.preventDefault();
      zoomAt(zoom * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)), e.clientX, e.clientY);
    };
    const onDown = (e) => {
      if (inWidget(e.target)) return;
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        pinch = { dist: Math.hypot(a.x - b.x, a.y - b.y), zoom };
        drag = null;
      } else if (pointers.size === 1) {
        drag = { x: e.clientX, y: e.clientY, panX, panY, moved: false };
      }
    };
    const onMove = (e) => {
      if (!pointers.has(e.pointerId)) return;
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pinch && pointers.size >= 2) {
        const [a, b] = [...pointers.values()];
        zoomAt(pinch.zoom * (Math.hypot(a.x - b.x, a.y - b.y) / (pinch.dist || 1)), (a.x + b.x) / 2, (a.y + b.y) / 2);
        suppressClick = true;
      } else if (drag && zoom > 1.001) {
        const dx = e.clientX - drag.x;
        const dy = e.clientY - drag.y;
        if (!drag.moved && Math.hypot(dx, dy) < 8) return;
        drag.moved = true;
        suppressClick = true;
        panX = drag.panX + dx;
        panY = drag.panY + dy;
        layout();
      }
    };
    const onUp = (e) => {
      pointers.delete(e.pointerId);
      if (pointers.size < 2) pinch = null;
      if (pointers.size === 0) {
        drag = null;
        setTimeout(() => { suppressClick = false; }, 0);
      }
    };
    const onClickCapture = (e) => {
      if (suppressClick) {
        e.stopPropagation();
        e.preventDefault();
        suppressClick = false;
      }
    };
    const onDbl = (e) => { if (!inWidget(e.target) && !e.target.closest?.('.mc-editable, .mc-button-glossy, .mc-button-tag')) resetZoom(); };
    container.addEventListener('wheel', onWheel, { passive: false });
    container.addEventListener('pointerdown', onDown);
    container.addEventListener('pointermove', onMove);
    container.addEventListener('pointerup', onUp);
    container.addEventListener('pointercancel', onUp);
    container.addEventListener('click', onClickCapture, true);
    container.addEventListener('dblclick', onDbl);
    cleanups.push(() => {
      container.removeEventListener('wheel', onWheel);
      container.removeEventListener('pointerdown', onDown);
      container.removeEventListener('pointermove', onMove);
      container.removeEventListener('pointerup', onUp);
      container.removeEventListener('pointercancel', onUp);
      container.removeEventListener('click', onClickCapture, true);
      container.removeEventListener('dblclick', onDbl);
    });
  }

  const ro = new ResizeObserver(() => layout());
  ro.observe(container);
  layout();

  if (opts.values) update(opts.values);

  function destroy() {
    ro.disconnect();
    if (tickTimer) clearInterval(tickTimer);
    cleanups.forEach((fn) => fn());
    for (const { widget } of widgets) widget.destroy?.();
    stage.remove();
    container.classList.remove('mc-screen-host', 'mc-design', 'mc-zoomable', 'mc-zoomed', 'mc-offline');
  }

  return view;
}

function defaultCommandHandler(cmd) {
  if (cmd.action === 'report') toast(`Report "${cmd.report}" is not available yet`, { kind: 'warn' });
  else toast(`Command "${cmd.action}" is not handled here`, { kind: 'warn' });
}
