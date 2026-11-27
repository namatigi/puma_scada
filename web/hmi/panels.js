// Floating panels for small screens (faceplates such as FM_01): opened over the
// current screen instead of replacing it, draggable by the title bar, several
// at once, Esc closes the top one.

import { h } from '../shared/svg.js';
import { createScreenView } from '../shared/renderer.js';

const MAX_PANELS = 4;

/**
 * @param {HTMLElement} layer container for panels
 * @param {{viewOptions:() => object, onChange:() => void}} deps
 */
export function createPanels(layer, deps) {
  /** @type {{name:string, el:HTMLElement, view:object}[]} */
  const open = [];
  let z = 10;

  function focus(p) {
    z += 1;
    p.el.style.zIndex = z;
    open.splice(open.indexOf(p), 1);
    open.push(p);
  }

  function close(p) {
    const i = open.indexOf(p);
    if (i < 0) return;
    open.splice(i, 1);
    p.view.destroy();
    p.el.remove();
    deps.onChange();
  }

  function size(screen) {
    const maxH = Math.max(200, window.innerHeight - 90);
    const maxW = Math.max(200, window.innerWidth - 24);
    let hgt = Math.min(screen.height, maxH);
    let wid = (hgt * screen.width) / screen.height;
    if (wid > maxW) {
      wid = maxW;
      hgt = (wid * screen.height) / screen.width;
    }
    // Small faceplates are enlarged a little on big displays for readability.
    const grow = Math.min(1.35, maxH / hgt, maxW / wid);
    if (screen.height < maxH * 0.7 && grow > 1) {
      wid *= grow;
      hgt *= grow;
    }
    return { wid: Math.round(wid), hgt: Math.round(hgt) };
  }

  /**
   * @param {object} screen bundle screen
   * @param {{x:number,y:number}} [near] preferred position (client px)
   */
  function openPanel(screen, near) {
    const existing = open.find((p) => p.name === screen.name);
    if (existing) {
      focus(existing);
      existing.el.classList.remove('hmi-panel-flash');
      void existing.el.offsetWidth;
      existing.el.classList.add('hmi-panel-flash');
      return existing;
    }
    if (open.length >= MAX_PANELS) close(open[0]);
    const { wid, hgt } = size(screen);
    const body = h('div', { class: 'hmi-panel-body', style: { width: `${wid}px`, height: `${hgt}px` } });
    const closeBtn = h('button', { type: 'button', class: 'hmi-panel-close', 'aria-label': `Close ${screen.name}`, text: '✕' });
    const title = h('div', { class: 'hmi-panel-title' }, h('span', { class: 'hmi-panel-grip', 'aria-hidden': 'true' }), h('span', { text: screen.name }), closeBtn);
    const el = h('section', { class: 'hmi-panel', role: 'dialog', 'aria-label': screen.name }, title, body);
    layer.appendChild(el);
    // Position: next to where it was opened from, kept on screen, cascaded.
    const pw = wid + 2;
    const ph = hgt + 40;
    let x = near ? near.x + 24 : window.innerWidth - pw - 24 - open.length * 28;
    let y = near ? near.y - ph / 2 : 60 + open.length * 28;
    if (near && x + pw > window.innerWidth - 8) x = near.x - pw - 24;
    x = Math.max(8, Math.min(window.innerWidth - pw - 8, x));
    y = Math.max(8, Math.min(window.innerHeight - ph - 8, y));
    el.style.left = `${x}px`;
    el.style.top = `${y}px`;
    const view = createScreenView(body, screen, { ...deps.viewOptions(), zoomable: false });
    const p = { name: screen.name, el, view };
    open.push(p);
    focus(p);
    closeBtn.addEventListener('click', () => close(p));
    el.addEventListener('pointerdown', () => focus(p));
    makeDraggable(el, title);
    deps.onChange();
    return p;
  }

  function makeDraggable(el, handle) {
    let start = null;
    handle.addEventListener('pointerdown', (e) => {
      if (e.target.closest('button')) return;
      start = { x: e.clientX, y: e.clientY, left: el.offsetLeft, top: el.offsetTop };
      handle.setPointerCapture(e.pointerId);
      el.classList.add('hmi-dragging');
    });
    handle.addEventListener('pointermove', (e) => {
      if (!start) return;
      const nx = Math.max(-el.offsetWidth + 80, Math.min(window.innerWidth - 80, start.left + e.clientX - start.x));
      const ny = Math.max(0, Math.min(window.innerHeight - 40, start.top + e.clientY - start.y));
      el.style.left = `${nx}px`;
      el.style.top = `${ny}px`;
    });
    const end = () => {
      start = null;
      el.classList.remove('hmi-dragging');
    };
    handle.addEventListener('pointerup', end);
    handle.addEventListener('pointercancel', end);
  }

  return {
    open: openPanel,
    closeTop() {
      const p = open[open.length - 1];
      if (p) close(p);
      return Boolean(p);
    },
    closeAll() {
      [...open].forEach(close);
    },
    views: () => open.map((p) => p.view),
    names: () => open.map((p) => p.name),
  };
}
