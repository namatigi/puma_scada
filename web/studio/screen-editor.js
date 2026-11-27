// Screen document: the shared renderer in design mode under an SVG overlay that
// draws the grid, selection, handles, marquee and smart guides. Every edit goes
// through store.edit(), so it is undoable and marks the screen dirty.

import { createScreenView } from '../shared/renderer.js';
import { s } from '../shared/svg.js';
import { h, iconButton, showMenu } from './ui.js';
import { icon } from './icons.js';
import {
  snap, tidy, bounds, unionBounds, rectFrom, contains, intersects, corners, HANDLES, handleOffset,
  rotatePoint, resizeRect, angleFrom, align, distribute, sameSize, centreOnScreen, scalePoints,
} from './geometry.js';
import { newElement, nextId, prefixFor, clone, KIND_INFO, elementRefs } from './model.js';

export const ZOOM_STEPS = [0.1, 0.125, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8];
export const DRAG_ELEMENT = 'application/x-moviclone-element';
export const DRAG_TAG = 'application/x-moviclone-tag';

/** Clipboard shared by every screen document (copy on one screen, paste on another). */
const clipboard = { elements: [], pasteCount: 0 };
const GRID_MINOR = 'rgba(255,255,255,0.09)';
const GRID_MAJOR = 'rgba(255,255,255,0.18)';

export function createScreenEditor(root, ctx) {
  const { store, prefs, symbolMap } = ctx;
  let name = ctx.name;
  const part = () => `screen:${name}`;
  const screen = () => store.getPart(part());

  let zoom = 1;
  let selection = [];
  let view = null;
  let values = new Map();
  let preview = null; // {id: {x,y,w,h,rotation}} while dragging
  let hoverId = null;
  let drawer = null; // 'objects' | 'tags' | null

  // ---------------------------------------------------------------- DOM
  const host = h('div', { class: 'st-canvas-host' });
  const layer = s('svg', { class: 'st-sel-layer' });
  const frame = h('div', { class: 'st-canvas-frame' }, host, layer);
  const pad = h('div', { class: 'st-canvas-pad' }, frame);
  const scroller = h('div', { class: 'st-canvas-scroll', tabindex: '0', 'aria-label': 'Screen canvas' }, pad);
  const sizeTip = h('div', { class: 'st-size-tip', hidden: true });
  scroller.appendChild(sizeTip);
  const drawerEl = h('div', { class: 'st-drawer', hidden: true });

  const num = (label, key, title) => {
    const input = h('input', { class: 'num', type: 'number', step: 'any', title, 'aria-label': title });
    input.addEventListener('change', () => applyBox(key, Number(input.value)));
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { input.blur(); scroller.focus(); } });
    return { el: h('label', {}, h('span', { text: label }), input), input };
  };
  const fx = num('X', 'x', 'Left (screen units)');
  const fy = num('Y', 'y', 'Top (screen units)');
  const fw = num('W', 'w', 'Width');
  const fh = num('H', 'h', 'Height');
  const fr = num('R', 'rotation', 'Rotation in degrees, clockwise');
  const zoomSel = h('select', { title: 'Zoom', 'aria-label': 'Zoom' },
    h('option', { value: 'fit', text: 'Fit' }), ...ZOOM_STEPS.map((z) => h('option', { value: String(z), text: `${Math.round(z * 100)}%` })));
  zoomSel.addEventListener('change', () => (zoomSel.value === 'fit' ? zoomFit() : setZoom(Number(zoomSel.value))));
  const langSel = h('select', { title: 'Language (texts are not translated yet)', disabled: true }, h('option', { text: 'Default' }));
  const tabObjects = h('button', { class: 'st-subtab', type: 'button', text: 'Objects' });
  const tabTags = h('button', { class: 'st-subtab', type: 'button', text: 'Tags Explorer' });
  tabObjects.addEventListener('click', () => toggleDrawer('objects'));
  tabTags.addEventListener('click', () => toggleDrawer('tags'));
  const bar = h('div', { class: 'st-canvas-bar' },
    h('label', { class: 'lang' }, h('span', { text: 'Language' }), langSel),
    h('label', {}, h('span', { text: 'Zoom' }), zoomSel),
    iconButton('minus', 'Zoom out', () => stepZoom(-1)), iconButton('plus', 'Zoom in', () => stepZoom(1)),
    h('span', { class: 'st-tb-sep' }), fx.el, fy.el, fw.el, fh.el, fr.el,
    h('span', { class: 'spacer' }), h('div', { class: 'st-subtabs' }, tabObjects, tabTags));
  fr.el.classList.add('rot');
  const doc = h('div', { class: 'st-doc st-screen-doc' }, scroller, drawerEl, h('div', { class: 'st-canvas-wrap' }, bar));
  root.appendChild(doc);

  // ---------------------------------------------------------------- rendering
  function render() {
    const scr = screen();
    if (!scr) return;
    const shown = preview ? withPreview(scr) : scr;
    view?.destroy();
    host.replaceChildren();
    view = createScreenView(host, shown, {
      project: store.project, symbolMap, mode: 'design', widgets: 'placeholder', values,
    });
    sizeFrame();
    drawOverlay();
  }

  function withPreview(scr) {
    return { ...scr, element: scr.element.map((el) => (preview[el.id] ? { ...el, ...preview[el.id] } : el)) };
  }

  function sizeFrame() {
    const scr = screen();
    if (!scr) return;
    frame.style.width = `${scr.width * zoom}px`;
    frame.style.height = `${scr.height * zoom}px`;
    layer.setAttribute('viewBox', `0 0 ${scr.width} ${scr.height}`);
    view?.resize?.();
  }

  const elementById = (id) => {
    const el = screen()?.element.find((e) => e.id === id);
    return el && preview?.[id] ? { ...el, ...preview[id] } : el;
  };
  const selected = () => selection.map(elementById).filter(Boolean);

  function drawOverlay() {
    const scr = screen();
    if (!scr) return;
    layer.replaceChildren();
    const px = 1 / zoom; // one screen pixel in screen units
    if (prefs.grid && prefs.gridSize * zoom >= 4) {
      const gs = prefs.gridSize;
      const id = `grid-${Math.random().toString(36).slice(2, 8)}`;
      const major = gs * 5;
      layer.appendChild(s('defs', {},
        s('pattern', { id, width: major, height: major, patternUnits: 'userSpaceOnUse' },
          ...Array.from({ length: 5 }, (_, i) => [
            s('line', { x1: i * gs, y1: 0, x2: i * gs, y2: major, stroke: i ? GRID_MINOR : GRID_MAJOR, 'stroke-width': px }),
            s('line', { x1: 0, y1: i * gs, x2: major, y2: i * gs, stroke: i ? GRID_MINOR : GRID_MAJOR, 'stroke-width': px }),
          ]).flat())));
      layer.appendChild(s('rect', { class: 'st-grid', x: 0, y: 0, width: scr.width, height: scr.height, fill: `url(#${id})` }));
    }
    if (hoverId && !selection.includes(hoverId)) {
      const el = elementById(hoverId);
      if (el) layer.appendChild(polygon(el, 'st-hover-box'));
    }
    const els = selected();
    els.forEach((el, i) => layer.appendChild(polygon(el, `st-sel-box${i === 0 ? ' primary' : ''}`)));
    if (els.length > 1) {
      const u = unionBounds(els);
      layer.appendChild(s('rect', { class: 'st-group-box', x: u.x - 3 * px, y: u.y - 3 * px, width: u.w + 6 * px, height: u.h + 6 * px }));
    }
    if (els.length === 1) drawHandles(els[0], px);
    renderBarValues();
  }

  function polygon(el, cls) {
    return s('polygon', { class: cls, points: corners(el).map((p) => p.join(',')).join(' ') });
  }

  function drawHandles(el, px) {
    const size = 7 * px;
    const cx = el.x + el.w / 2;
    const cy = el.y + el.h / 2;
    for (const hd of HANDLES) {
      const [ox, oy] = handleOffset(hd, el.w, el.h);
      const [x, y] = rotatePoint(el.x + ox, el.y + oy, cx, cy, el.rotation || 0);
      const r = s('rect', { class: 'st-handle primary', x: x - size / 2, y: y - size / 2, width: size, height: size, 'data-handle': hd });
      r.style.cursor = cursorFor(hd, el.rotation || 0);
      layer.appendChild(r);
    }
    const [tx, ty] = rotatePoint(cx, el.y, cx, cy, el.rotation || 0);
    const [rx, ry] = rotatePoint(cx, el.y - 22 * px, cx, cy, el.rotation || 0);
    layer.appendChild(s('line', { class: 'st-rot-line', x1: tx, y1: ty, x2: rx, y2: ry }));
    layer.appendChild(s('circle', { class: 'st-rot-handle', cx: rx, cy: ry, r: 4.5 * px, 'data-rotate': '1' }));
  }

  function cursorFor(handle, rot) {
    const dirs = ['n', 'ne', 'e', 'se', 's', 'sw', 'w', 'nw'];
    const base = dirs.indexOf(handle);
    const k = (base + Math.round((((rot % 360) + 360) % 360) / 45)) % 8;
    return ['ns-resize', 'nesw-resize', 'ew-resize', 'nwse-resize', 'ns-resize', 'nesw-resize', 'ew-resize', 'nwse-resize'][k];
  }

  function renderBarValues() {
    const els = selected();
    const fields = [fx, fy, fw, fh, fr];
    if (!els.length) {
      const scr = screen();
      fields.forEach((f) => { f.input.value = ''; f.input.disabled = true; });
      fw.input.value = scr?.width ?? '';
      fh.input.value = scr?.height ?? '';
      return;
    }
    fields.forEach((f) => { f.input.disabled = false; });
    const one = (k) => {
      const vs = new Set(els.map((e) => tidy(e[k] ?? 0)));
      return vs.size === 1 ? String([...vs][0]) : '';
    };
    if (document.activeElement !== fx.input) fx.input.value = one('x');
    if (document.activeElement !== fy.input) fy.input.value = one('y');
    if (document.activeElement !== fw.input) fw.input.value = one('w');
    if (document.activeElement !== fh.input) fh.input.value = one('h');
    if (document.activeElement !== fr.input) fr.input.value = one('rotation');
  }

  function applyBox(key, v) {
    if (!Number.isFinite(v)) { renderBarValues(); return; }
    if (!selection.length) {
      if (key === 'w' || key === 'h') {
        if (v <= 0) { renderBarValues(); return; }
        store.edit(part(), key === 'w' ? 'Screen width' : 'Screen height', (scr) => { scr[key === 'w' ? 'width' : 'height'] = v; });
      }
      return;
    }
    if ((key === 'w' || key === 'h') && v < 1) { renderBarValues(); return; }
    editElements(`Set ${key.toUpperCase()}`, (el) => {
      if (key === 'rotation') el.rotation = ((v % 360) + 360) % 360;
      else if (key === 'w' || key === 'h') resizeElementTo(el, key === 'w' ? v : el.w, key === 'h' ? v : el.h);
      else el[key] = v;
    });
  }

  function resizeElementTo(el, w, h) {
    if (el.kind === 'pipeline' && Array.isArray(el.points)) el.points = scalePoints(el.points, el.w, el.h, w, h);
    el.w = tidy(w);
    el.h = tidy(h);
  }

  // ---------------------------------------------------------------- selection
  function setSelection(ids, { scroll = false } = {}) {
    const scr = screen();
    const valid = new Set(scr?.element.map((e) => e.id));
    const next = [...new Set(ids)].filter((id) => valid.has(id));
    const changed = next.join('\n') !== selection.join('\n');
    selection = next;
    drawOverlay();
    if (scroll && next.length) scrollIntoView(next[0]);
    if (changed) notifySelection();
    if (drawer) renderDrawer();
  }

  function notifySelection() {
    ctx.onSelection?.({ editor: api, ids: [...selection] });
  }

  function scrollIntoView(id) {
    const el = elementById(id);
    if (!el) return;
    const b = bounds(el);
    const fr0 = frame.offsetLeft;
    const ft0 = frame.offsetTop;
    const cx = fr0 + (b.x + b.w / 2) * zoom;
    const cy = ft0 + (b.y + b.h / 2) * zoom;
    scroller.scrollTo({ left: cx - scroller.clientWidth / 2, top: cy - scroller.clientHeight / 2, behavior: 'smooth' });
  }

  // ---------------------------------------------------------------- edits
  /** Applies fn to each selected element as one undo step. */
  function editElements(label, fn, opts = {}) {
    const ids = new Set(selection);
    return store.edit(part(), label, (scr) => {
      for (const el of scr.element) if (ids.has(el.id)) fn(el);
    }, opts);
  }

  function applyPatches(label, patches) {
    if (!patches.length) return;
    const byId = new Map(patches.map((p) => [p.id, p]));
    store.edit(part(), label, (scr) => {
      for (const el of scr.element) {
        const p = byId.get(el.id);
        if (!p) continue;
        if (p.w !== undefined || p.h !== undefined) resizeElementTo(el, p.w ?? el.w, p.h ?? el.h);
        el.x = tidy(p.x ?? el.x);
        el.y = tidy(p.y ?? el.y);
      }
    });
  }

  function nudge(dx, dy) {
    if (!selection.length) return;
    editElements('Nudge', (el) => { el.x = tidy(el.x + dx); el.y = tidy(el.y + dy); }, { coalesce: 'nudge' });
  }

  function deleteSelection() {
    if (!selection.length) return;
    const ids = new Set(selection);
    const n = ids.size;
    store.edit(part(), n === 1 ? `Delete ${selection[0]}` : `Delete ${n} objects`, (scr) => {
      scr.element = scr.element.filter((el) => !ids.has(el.id));
    });
    setSelection([]);
  }

  function copySelection() {
    const els = selected();
    if (!els.length) return false;
    clipboard.elements = clone(els);
    clipboard.pasteCount = 0;
    try {
      navigator.clipboard?.writeText(JSON.stringify({ moviclone: 'elements', elements: els }, null, 2)).catch(() => {});
    } catch { /* clipboard API unavailable */ }
    return true;
  }

  function insertElements(els, label, { offset = 0, at = null } = {}) {
    if (!els.length) return [];
    const ids = [];
    store.edit(part(), label, (scr) => {
      const taken = new Set();
      const u = unionBounds(els);
      for (const src of els) {
        const el = clone(src);
        el.id = nextId(scr, prefixFor(el), taken);
        taken.add(el.id);
        if (at) {
          el.x = tidy(src.x - u.x + at.x);
          el.y = tidy(src.y - u.y + at.y);
        } else {
          el.x = tidy(src.x + offset);
          el.y = tidy(src.y + offset);
        }
        scr.element.push(el);
        ids.push(el.id);
      }
    });
    setSelection(ids);
    return ids;
  }

  function paste() {
    if (!clipboard.elements.length) return;
    clipboard.pasteCount += 1;
    const step = Math.max(prefs.gridSize, 10);
    insertElements(clipboard.elements, clipboard.elements.length === 1 ? 'Paste' : `Paste ${clipboard.elements.length} objects`, { offset: step * clipboard.pasteCount });
  }

  function duplicate() {
    const els = selected();
    if (!els.length) return;
    insertElements(els, 'Duplicate', { offset: Math.max(prefs.gridSize, 10) });
  }

  function zorder(how) {
    if (!selection.length) return;
    const ids = new Set(selection);
    store.edit(part(), { front: 'Bring to front', back: 'Send to back', forward: 'Bring forward', backward: 'Send backward' }[how], (scr) => {
      const list = scr.element;
      if (how === 'front') scr.element = [...list.filter((e) => !ids.has(e.id)), ...list.filter((e) => ids.has(e.id))];
      else if (how === 'back') scr.element = [...list.filter((e) => ids.has(e.id)), ...list.filter((e) => !ids.has(e.id))];
      else if (how === 'forward') {
        for (let i = list.length - 2; i >= 0; i -= 1) {
          if (ids.has(list[i].id) && !ids.has(list[i + 1].id)) [list[i], list[i + 1]] = [list[i + 1], list[i]];
        }
      } else {
        for (let i = 1; i < list.length; i += 1) {
          if (ids.has(list[i].id) && !ids.has(list[i - 1].id)) [list[i], list[i - 1]] = [list[i - 1], list[i]];
        }
      }
    });
  }

  // ---------------------------------------------------------------- zoom
  function setZoom(z, anchor = null) {
    const nz = Math.max(0.05, Math.min(8, z));
    const prev = zoom;
    let keep = null;
    if (anchor) {
      const r = scroller.getBoundingClientRect();
      const sx = (scroller.scrollLeft + anchor.x - r.left - frame.offsetLeft) / prev;
      const sy = (scroller.scrollTop + anchor.y - r.top - frame.offsetTop) / prev;
      keep = { sx, sy, ox: anchor.x - r.left, oy: anchor.y - r.top };
    }
    zoom = nz;
    sizeFrame();
    drawOverlay();
    const opt = [...zoomSel.options].find((o) => Number(o.value) === nz);
    if (opt) zoomSel.value = opt.value;
    else {
      zoomSel.querySelector('.custom')?.remove();
      const o = h('option', { class: 'custom', value: String(nz), text: `${Math.round(nz * 100)}%` });
      zoomSel.appendChild(o);
      zoomSel.value = o.value;
    }
    if (keep) {
      scroller.scrollLeft = frame.offsetLeft + keep.sx * nz - keep.ox;
      scroller.scrollTop = frame.offsetTop + keep.sy * nz - keep.oy;
    }
    ctx.onZoom?.(zoom);
  }

  function stepZoom(dir) {
    const i = ZOOM_STEPS.findIndex((z) => z >= zoom - 1e-6);
    const idx = dir > 0 ? (ZOOM_STEPS[i] > zoom + 1e-6 ? i : i + 1) : (i <= 0 ? 0 : i - 1);
    setZoom(ZOOM_STEPS[Math.max(0, Math.min(ZOOM_STEPS.length - 1, idx))]);
  }

  function zoomFit() {
    const scr = screen();
    if (!scr) return;
    const w = scroller.clientWidth - 80;
    const hgt = scroller.clientHeight - 80;
    if (w <= 0 || hgt <= 0) return;
    setZoom(Math.min(1, Math.floor(Math.min(w / scr.width, hgt / scr.height) * 100) / 100));
    zoomSel.value = 'fit';
  }

  // ---------------------------------------------------------------- pointer
  const toScreen = (e) => {
    const r = frame.getBoundingClientRect();
    return [(e.clientX - r.left) / zoom, (e.clientY - r.top) / zoom];
  };

  function elementAt(target) {
    const g = target instanceof Element ? target.closest('.mc-el') : null;
    return g && host.contains(g) ? g.dataset.id : null;
  }

  let gesture = null;

  scroller.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    // Clicks on the scroll bars are not marquee starts.
    if (e.target === scroller && (e.offsetX >= scroller.clientWidth || e.offsetY >= scroller.clientHeight)) return;
    scroller.focus({ preventScroll: true });
    const [sx, sy] = toScreen(e);
    const handle = e.target.dataset?.handle;
    if (handle && selection.length === 1) {
      const el = elementById(selection[0]);
      gesture = { type: 'resize', handle, start: [sx, sy], orig: clone(el) };
    } else if (e.target.dataset?.rotate && selection.length === 1) {
      const el = elementById(selection[0]);
      gesture = { type: 'rotate', orig: clone(el), centre: [el.x + el.w / 2, el.y + el.h / 2] };
    } else {
      const id = elementAt(e.target);
      if (id) {
        const additive = e.shiftKey || e.ctrlKey || e.metaKey;
        if (additive) {
          setSelection(selection.includes(id) ? selection.filter((x) => x !== id) : [...selection, id]);
          if (!selection.includes(id)) return;
        } else if (!selection.includes(id)) setSelection([id]);
        else if (selection[0] !== id && !additive) {
          // Clicking an already selected element makes it the primary (alignment reference).
          setSelection([id, ...selection.filter((x) => x !== id)]);
        }
        gesture = { type: 'move', start: [sx, sy], client: [e.clientX, e.clientY], orig: selected().map(clone), moved: false, copy: e.altKey };
      } else {
        gesture = { type: 'marquee', start: [sx, sy], client: [e.clientX, e.clientY], additive: e.shiftKey || e.ctrlKey, base: [...selection] };
      }
    }
    scroller.setPointerCapture(e.pointerId);
    e.preventDefault();
  });

  scroller.addEventListener('pointermove', (e) => {
    const [sx, sy] = toScreen(e);
    if (!gesture) {
      const id = elementAt(e.target);
      if (id !== hoverId) { hoverId = id; drawOverlay(); }
      return;
    }
    if (gesture.type === 'move') moveGesture(e, sx, sy);
    else if (gesture.type === 'resize') resizeGesture(e, sx, sy);
    else if (gesture.type === 'rotate') rotateGesture(e, sx, sy);
    else if (gesture.type === 'marquee') marqueeGesture(e, sx, sy);
  });

  const endGesture = (e) => {
    if (!gesture) return;
    const g = gesture;
    gesture = null;
    sizeTip.hidden = true;
    try { scroller.releasePointerCapture(e.pointerId); } catch { /* not captured */ }
    if (g.type === 'marquee') { drawOverlay(); return; }
    const p = preview;
    preview = null;
    if (!p || !g.changed) {
      if (p) render();
      else drawOverlay();
      return;
    }
    if (g.type === 'move' && g.copy) {
      const els = g.orig.map((o) => ({ ...o, ...p[o.id] }));
      insertElements(els, 'Copy by dragging');
      return;
    }
    const label = g.type === 'move' ? (g.orig.length > 1 ? `Move ${g.orig.length} objects` : `Move ${g.orig[0].id}`)
      : g.type === 'resize' ? `Resize ${g.orig.id}` : `Rotate ${g.orig.id}`;
    const changed = store.edit(part(), label, (scr) => {
      for (const el of scr.element) {
        const q = p[el.id];
        if (!q) continue;
        if (q.w !== undefined && (q.w !== el.w || q.h !== el.h)) resizeElementTo(el, q.w, q.h);
        if (q.x !== undefined) el.x = tidy(q.x);
        if (q.y !== undefined) el.y = tidy(q.y);
        if (q.rotation !== undefined) el.rotation = q.rotation;
        if (!el.rotation) delete el.rotation;
      }
    });
    if (!changed) render();
  };
  scroller.addEventListener('pointerup', endGesture);
  scroller.addEventListener('pointercancel', endGesture);
  scroller.addEventListener('pointerleave', () => { if (!gesture && hoverId) { hoverId = null; drawOverlay(); } });

  function moveGesture(e, sx, sy) {
    const g = gesture;
    if (!g.moved && Math.hypot(e.clientX - g.client[0], e.clientY - g.client[1]) < 3) return;
    g.moved = true;
    let dx = sx - g.start[0];
    let dy = sy - g.start[1];
    const prim = g.orig[0];
    if (prefs.snap && !e.altKey) {
      dx = snap(prim.x + dx, prefs.gridSize) - prim.x;
      dy = snap(prim.y + dy, prefs.gridSize) - prim.y;
    }
    const guides = [];
    if (prefs.guides && !e.altKey) {
      const moved = g.orig.map((o) => ({ ...o, x: o.x + dx, y: o.y + dy }));
      const adj = smartGuides(moved, guides);
      dx += adj[0];
      dy += adj[1];
    }
    preview = {};
    for (const o of g.orig) {
      const x = tidy(o.x + dx);
      const y = tidy(o.y + dy);
      preview[o.id] = { x, y };
      const node = view?.elementNode(o.id);
      if (node && !g.copy) node.setAttribute('transform', placeTransform({ ...o, x, y }));
    }
    g.changed = dx !== 0 || dy !== 0;
    drawOverlay();
    drawGuides(guides);
    if (g.copy) for (const o of g.orig) layer.appendChild(polygon({ ...o, ...preview[o.id] }, 'st-drop-ghost'));
    showTip(e, `X ${tidy(prim.x + dx)}  Y ${tidy(prim.y + dy)}${g.copy ? '  (copy)' : ''}`);
  }

  /** Snaps the moving selection's edges / centres to other elements' within 5 px. */
  function smartGuides(moving, out) {
    const u = unionBounds(moving);
    const ids = new Set(moving.map((m) => m.id));
    const scr = screen();
    const others = scr.element.filter((el) => !ids.has(el.id)).map(bounds);
    others.push({ x: 0, y: 0, w: scr.width, h: scr.height });
    const tol = 5 / zoom;
    const xs = [u.x, u.x + u.w / 2, u.x + u.w];
    const ys = [u.y, u.y + u.h / 2, u.y + u.h];
    let bestX = null;
    let bestY = null;
    for (const b of others) {
      for (const tx of [b.x, b.x + b.w / 2, b.x + b.w]) {
        for (const mx of xs) {
          const d = tx - mx;
          if (Math.abs(d) <= tol && (!bestX || Math.abs(d) < Math.abs(bestX.d))) bestX = { d, at: tx, b };
        }
      }
      for (const ty of [b.y, b.y + b.h / 2, b.y + b.h]) {
        for (const my of ys) {
          const d = ty - my;
          if (Math.abs(d) <= tol && (!bestY || Math.abs(d) < Math.abs(bestY.d))) bestY = { d, at: ty, b };
        }
      }
    }
    if (bestX) out.push({ x1: bestX.at, x2: bestX.at, y1: Math.min(u.y, bestX.b.y), y2: Math.max(u.y + u.h, bestX.b.y + bestX.b.h) });
    if (bestY) out.push({ y1: bestY.at, y2: bestY.at, x1: Math.min(u.x, bestY.b.x), x2: Math.max(u.x + u.w, bestY.b.x + bestY.b.w) });
    return [bestX?.d ?? 0, bestY?.d ?? 0];
  }

  function drawGuides(list) {
    for (const gl of list) layer.appendChild(s('line', { class: 'st-guide', ...gl }));
  }

  let rafPending = false;
  function previewRender() {
    if (rafPending) return;
    rafPending = true;
    requestAnimationFrame(() => {
      rafPending = false;
      if (preview) render();
    });
  }

  function resizeGesture(e, sx, sy) {
    const g = gesture;
    const o = g.orig;
    const r = resizeRect(o, g.handle, sx - g.start[0], sy - g.start[1], {
      keepAspect: e.shiftKey,
      grid: prefs.snap && !e.altKey && !o.rotation ? prefs.gridSize : 0,
    });
    if (prefs.snap && !e.altKey && !o.rotation) {
      // Snap the moving edges' positions too, not just the size.
      if (g.handle.includes('w')) { const nx = snap(r.x, prefs.gridSize); r.w += r.x - nx; r.x = nx; }
      if (g.handle.includes('n')) { const ny = snap(r.y, prefs.gridSize); r.h += r.y - ny; r.y = ny; }
    }
    const next = { x: tidy(r.x), y: tidy(r.y), w: tidy(Math.max(1, r.w)), h: tidy(Math.max(1, r.h)) };
    preview = { [o.id]: next };
    if (o.kind === 'pipeline') preview[o.id].points = scalePoints(o.points ?? [], o.w, o.h, next.w, next.h);
    g.changed = true;
    previewRender();
    showTip(e, `W ${next.w}  H ${next.h}`);
  }

  function rotateGesture(e, sx, sy) {
    const g = gesture;
    let a = angleFrom(g.centre[0], g.centre[1], sx, sy);
    a = e.shiftKey ? a : Math.round(a / 15) * 15 % 360;
    if (e.altKey) a = Math.round(a);
    preview = { [g.orig.id]: { rotation: a } };
    const node = view?.elementNode(g.orig.id);
    if (node) node.setAttribute('transform', placeTransform({ ...g.orig, rotation: a }));
    g.changed = a !== (g.orig.rotation || 0);
    drawOverlay();
    showTip(e, `${a}°${e.shiftKey ? '' : '  (Shift: free)'}`);
  }

  function marqueeGesture(e, sx, sy) {
    const g = gesture;
    if (Math.hypot(e.clientX - g.client[0], e.clientY - g.client[1]) < 3) return;
    const r = rectFrom(g.start[0], g.start[1], sx, sy);
    // Left-to-right selects what is fully inside; right-to-left what it touches (CAD convention).
    const touch = sx < g.start[0];
    const hits = screen().element.filter((el) => (touch ? intersects(r, bounds(el)) : contains(r, bounds(el)))).map((el) => el.id);
    selection = g.additive ? [...new Set([...g.base, ...hits])] : hits;
    drawOverlay();
    layer.appendChild(s('rect', { class: 'st-marquee', x: r.x, y: r.y, width: r.w, height: r.h, ...(touch ? { 'stroke-dasharray': '1 2' } : {}) }));
    notifySelection();
  }

  function showTip(e, text) {
    const r = scroller.getBoundingClientRect();
    sizeTip.hidden = false;
    sizeTip.textContent = text;
    sizeTip.style.left = `${e.clientX - r.left + scroller.scrollLeft + 14}px`;
    sizeTip.style.top = `${e.clientY - r.top + scroller.scrollTop + 16}px`;
  }

  function placeTransform(el) {
    const rot = el.rotation ? ` rotate(${el.rotation} ${el.w / 2} ${el.h / 2})` : '';
    return `translate(${el.x} ${el.y})${rot}`;
  }

  scroller.addEventListener('wheel', (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    setZoom(zoom * Math.exp(-e.deltaY * 0.0015), { x: e.clientX, y: e.clientY });
  }, { passive: false });

  scroller.addEventListener('dblclick', (e) => {
    const id = elementAt(e.target);
    if (id) ctx.onActivate?.({ editor: api, id });
  });

  scroller.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    const id = elementAt(e.target);
    if (id && !selection.includes(id)) setSelection([id]);
    const c = ctx.commands;
    const item = (cid, label) => ({ label: label ?? c.get(cid)?.label, icon: c.get(cid)?.icon, kbd: c.kbd(cid), disabled: !c.enabled(cid), run: () => c.run(cid) });
    showMenu([
      item('edit.cut'), item('edit.copy'), item('edit.paste'), item('edit.duplicate'), item('edit.delete'), '-',
      item('format.front'), item('format.back'), item('format.forward'), item('format.backward'), '-',
      item('format.align-left'), item('format.align-center'), item('format.align-right'),
      item('format.align-top'), item('format.align-middle'), item('format.align-bottom'), '-',
      item('edit.select-all'), item('view.properties', 'Properties'),
    ], e.clientX, e.clientY);
  });

  // ---------------------------------------------------------------- drag and drop (ToolBox, Symbol Library, Tag Browser)
  let ghost = null;
  scroller.addEventListener('dragover', (e) => {
    const types = [...e.dataTransfer.types];
    if (!types.includes(DRAG_ELEMENT) && !types.includes(DRAG_TAG)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
    const [sx, sy] = toScreen(e);
    ghost?.remove();
    const target = types.includes(DRAG_TAG) ? bindTarget(e.target) : null;
    if (target) {
      ghost = polygon(elementById(target), 'st-drop-ghost');
    } else {
      const sz = 60;
      ghost = s('rect', { class: 'st-drop-ghost', x: snap(sx - sz / 2, prefs.gridSize, prefs.snap), y: snap(sy - sz / 4, prefs.gridSize, prefs.snap), width: sz, height: sz / 2 });
    }
    layer.appendChild(ghost);
  });
  scroller.addEventListener('dragleave', () => { ghost?.remove(); ghost = null; });
  scroller.addEventListener('drop', (e) => {
    ghost?.remove();
    ghost = null;
    const [sx, sy] = toScreen(e);
    const elData = e.dataTransfer.getData(DRAG_ELEMENT);
    const tagData = e.dataTransfer.getData(DRAG_TAG);
    if (!elData && !tagData) return;
    e.preventDefault();
    scroller.focus({ preventScroll: true });
    if (tagData) {
      const target = bindTarget(e.target);
      if (target) {
        store.edit(part(), `Bind ${tagData} to ${target}`, (scr) => {
          const el = scr.element.find((x) => x.id === target);
          if (el.kind === 'trend') el.tags = [...new Set([...(el.tags ?? []), tagData])];
          else el.tag = tagData;
        });
        setSelection([target]);
        return;
      }
      createAt({ kind: 'display', tag: tagData }, sx, sy);
      return;
    }
    try {
      createAt(JSON.parse(elData), sx, sy);
    } catch (err) {
      console.error('[studio] bad drop payload', err);
    }
  });

  /** Element under the pointer that can take a tag binding. */
  function bindTarget(target) {
    const id = elementAt(target);
    const el = id && elementById(id);
    return el && ['display', 'pipeline', 'symbol', 'trend'].includes(el.kind) ? id : null;
  }

  /** Creates an element from a ToolBox / Symbol Library / Tag Browser payload centred at (sx, sy). */
  function createAt(payload, sx, sy) {
    const el = newElement(payload.kind, store.project, payload.variant ?? payload.symbol ?? null);
    if (payload.symbol) el.symbol = payload.symbol;
    if (payload.size) [el.w, el.h] = payload.size;
    if (payload.tag) {
      el.tag = payload.tag;
      const unit = guessUnit(payload.tag);
      if (unit) el.unit = unit;
    }
    el.x = tidy(snap(sx - el.w / 2, prefs.gridSize, prefs.snap));
    el.y = tidy(snap(sy - el.h / 2, prefs.gridSize, prefs.snap));
    let id = null;
    store.edit(part(), `Add ${KIND_INFO[el.kind]?.title ?? el.kind}`, (scr) => {
      el.id = nextId(scr, prefixFor(el));
      id = el.id;
      scr.element.push(el);
    });
    setSelection([id]);
    ctx.log?.(`Added ${el.kind} ${id} to ${name}`);
  }

  /** Unit of another display bound to the same tag anywhere in the project. */
  function guessUnit(tag) {
    for (const scr of store.project.screens) {
      for (const el of scr.element ?? []) if (el.kind === 'display' && el.tag === tag && el.unit) return el.unit;
    }
    return '';
  }

  // ---------------------------------------------------------------- keyboard (canvas-local; app handles global shortcuts)
  scroller.addEventListener('keydown', (e) => {
    const arrows = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    if (arrows[e.key] && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      const step = e.shiftKey ? prefs.gridSize : 1;
      nudge(arrows[e.key][0] * step, arrows[e.key][1] * step);
    } else if (e.key === 'Escape') {
      setSelection([]);
    } else if (e.key === 'Tab') {
      e.preventDefault();
      const list = screen().element;
      if (!list.length) return;
      const i = list.findIndex((el) => el.id === selection[0]);
      const next = list[(i + (e.shiftKey ? -1 : 1) + list.length) % list.length];
      setSelection([next.id], { scroll: true });
    }
  });

  // ---------------------------------------------------------------- drawers (Objects / Tags Explorer)
  function toggleDrawer(which) {
    drawer = drawer === which ? null : which;
    tabObjects.classList.toggle('on', drawer === 'objects');
    tabTags.classList.toggle('on', drawer === 'tags');
    drawerEl.hidden = !drawer;
    renderDrawer();
  }

  function renderDrawer() {
    if (!drawer) return;
    const scr = screen();
    if (!scr) return;
    const sel = new Set(selection);
    if (drawer === 'objects') {
      const rows = [...scr.element].reverse().map((el) => {
        const refs = elementRefs(el).map((r) => r.tag);
        const tr = h('tr', { class: sel.has(el.id) ? 'selected' : '' },
          h('td', { class: 'rowhead' }, icon(KIND_INFO[el.kind]?.icon ?? 'symbol')),
          h('td', { class: 'ro', text: el.id }), h('td', { class: 'ro', text: el.kind }),
          h('td', { class: 'ro', text: el.text ?? el.symbol ?? el.source ?? '' }),
          h('td', { class: 'ro', text: refs.join(', ') }),
          h('td', { class: 'ro num-cell', text: `${tidy(el.x)}, ${tidy(el.y)}` }),
          h('td', { class: 'ro num-cell', text: `${tidy(el.w)} × ${tidy(el.h)}` }));
        tr.addEventListener('click', (e) => {
          setSelection(e.shiftKey || e.ctrlKey ? [...selection, el.id] : [el.id], { scroll: true });
        });
        return tr;
      });
      drawerEl.replaceChildren(h('table', { class: 'st-table' },
        h('thead', {}, h('tr', {}, ...['', 'Name', 'Type', 'Text / Symbol', 'Tags', 'Position', 'Size'].map((t) => h('th', { text: t })))),
        h('tbody', {}, rows)));
    } else {
      const uses = new Map();
      for (const el of scr.element) for (const r of elementRefs(el)) {
        if (!uses.has(r.tag)) uses.set(r.tag, []);
        uses.get(r.tag).push(el.id);
      }
      const tags = new Map(store.project.tags.map((t) => [t.name, t]));
      const rows = [...uses.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([tag, ids]) => {
        const def = tags.get(tag);
        const tr = h('tr', { class: ids.some((i) => sel.has(i)) ? 'selected' : '' },
          h('td', { class: 'rowhead' }, icon(def ? 'tag' : 'error')),
          h('td', { class: 'ro', text: tag }), h('td', { class: 'ro', text: def?.type ?? 'missing' }),
          h('td', { class: 'ro', text: def?.access ?? (def ? 'read' : '') }),
          h('td', { class: 'ro', text: def?.description ?? '' }),
          h('td', { class: 'ro', text: `${ids.length} × ${ids.slice(0, 6).join(', ')}${ids.length > 6 ? '…' : ''}` }));
        tr.addEventListener('click', () => setSelection(ids, { scroll: true }));
        return tr;
      });
      drawerEl.replaceChildren(rows.length ? h('table', { class: 'st-table' },
        h('thead', {}, h('tr', {}, ...['', 'Tag', 'Type', 'Access', 'Description', 'Used by'].map((t) => h('th', { text: t })))),
        h('tbody', {}, rows)) : h('div', { class: 'st-empty', text: 'No tags are used on this screen.' }));
    }
  }

  // ---------------------------------------------------------------- store events
  const offChanged = store.on('changed', ({ part: p }) => {
    if (p !== part()) {
      // Tag renames or new screens change how buttons and displays draw.
      if (p === 'tags') render();
      return;
    }
    preview = null;
    const scr = screen();
    if (!scr) return;
    const valid = new Set(scr.element.map((e) => e.id));
    const before = selection.join('\n');
    selection = selection.filter((id) => valid.has(id));
    render();
    if (before !== selection.join('\n')) notifySelection();
    else ctx.onSelection?.({ editor: api, ids: [...selection], edited: true });
    if (drawer) renderDrawer();
  });

  const ro = new ResizeObserver(() => { if (zoomSel.value === 'fit') zoomFit(); });
  ro.observe(scroller);

  // ---------------------------------------------------------------- public API
  const api = {
    kind: 'screen',
    get part() { return part(); },
    get name() { return name; },
    get zoom() { return zoom; },
    el: doc,
    screen,
    selection: () => [...selection],
    selectedElements: () => selected().map(clone),
    select: (ids, opts) => setSelection(ids, opts),
    selectAll: () => setSelection(screen().element.map((e) => e.id)),
    focus: () => scroller.focus({ preventScroll: true }),
    refresh: render,
    redrawOverlay: drawOverlay,
    rename(next) { name = next; render(); },
    setValues(changes) {
      for (const [k, v] of changes instanceof Map ? changes : Object.entries(changes)) values.set(k, v);
      view?.update(changes);
    },
    clearValues() { values = new Map(); render(); },
    tagsInUse: () => view?.tagsInUse() ?? new Set(),
    editElements,
    hasSelection: () => selection.length > 0,
    canPaste: () => clipboard.elements.length > 0,
    copy: copySelection,
    cut() { if (copySelection()) deleteSelection(); },
    paste,
    duplicate,
    deleteSelection,
    nudge,
    zorder,
    align: (how) => applyPatches(`Align ${how}`, align(selected(), how)),
    distribute: (axis) => applyPatches(axis === 'h' ? 'Distribute horizontally' : 'Distribute vertically', distribute(selected(), axis)),
    sameSize: (which) => applyPatches(`Make same ${which}`, sameSize(selected(), which)),
    centre: (axis) => applyPatches('Centre on screen', centreOnScreen(selected(), screen(), axis)),
    zoomIn: () => stepZoom(1),
    zoomOut: () => stepZoom(-1),
    zoomFit,
    zoom100: () => setZoom(1),
    setZoom,
    createAt,
    insertElements,
    destroy() {
      offChanged();
      ro.disconnect();
      view?.destroy();
      doc.remove();
    },
  };

  render();
  requestAnimationFrame(() => zoomFit());
  return api;
}
