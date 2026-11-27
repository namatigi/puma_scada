// Screen navigator (Movicon's tile view): screen thumbnails drawn by the shared
// renderer, grouped by name. Thumbnails render lazily as they scroll into view.

import { h } from '../shared/svg.js';
import { createScreenView, isPopupScreen } from '../shared/renderer.js';

/** Group key for a screen name: digits removed, so FM_01…FM_05 group as "FM". */
export function screenGroup(name) {
  const g = name.replace(/\d+/g, '').replace(/[_\-\s]+/g, ' ').trim();
  return g || name;
}

/**
 * @param {HTMLElement} host
 * @param {{project:object, viewOptions:() => object, onOpen:(screen:object) => void, current?:string}} opts
 * @returns {{destroy:() => void, update:(values:Map) => void}}
 */
export function createNavigator(host, opts) {
  const { project } = opts;
  const main = project.screens.find((sc) => sc.name === project.meta?.main_screen) ?? project.screens[0];
  const views = [];
  const groups = new Map();
  const sorted = [...project.screens].sort((a, b) => (a.name === main?.name ? -1 : b.name === main?.name ? 1 : a.name.localeCompare(b.name, undefined, { numeric: true })));
  for (const sc of sorted) {
    const g = sc.name === main?.name ? 'Main' : screenGroup(sc.name);
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(sc);
  }

  const filter = h('input', { type: 'search', class: 'mc-tinput hmi-nav-filter', placeholder: 'Filter screens', 'aria-label': 'Filter screens' });
  const page = h('div', { class: 'hmi-page hmi-nav' },
    h('div', { class: 'hmi-page-head' }, h('h1', { text: 'Screens' }), h('span', { class: 'hmi-page-sub', text: `${project.screens.length} screens in ${project.meta?.name ?? 'project'}` }), filter));
  const io = new IntersectionObserver((entries) => {
    for (const en of entries) {
      if (!en.isIntersecting) continue;
      io.unobserve(en.target);
      const sc = project.screens.find((x) => x.name === en.target.dataset.screen);
      if (!sc) continue;
      try {
        views.push(createScreenView(en.target, sc, { ...opts.viewOptions(), interactive: false, widgets: 'placeholder', zoomable: false }));
      } catch (err) {
        en.target.textContent = `Preview failed: ${err.message}`;
      }
    }
  }, { rootMargin: '200px' });

  const tiles = [];
  for (const [g, list] of groups) {
    const grid = h('div', { class: 'hmi-tiles' });
    for (const sc of list) {
      const popup = isPopupScreen(sc, main);
      const thumb = h('div', { class: 'hmi-thumb', 'data-screen': sc.name, style: { aspectRatio: `${sc.width} / ${sc.height}` } });
      const tile = h('button', {
        type: 'button', class: `hmi-tile${popup ? ' hmi-tile-popup' : ''}${sc.name === opts.current ? ' hmi-tile-current' : ''}`,
        'data-name': sc.name.toLowerCase(), onclick: () => opts.onOpen(sc),
        title: popup ? `${sc.name} opens as a floating panel` : `Open ${sc.name}`,
      },
      h('div', { class: 'hmi-thumb-frame' }, thumb),
      h('div', { class: 'hmi-tile-caption' },
        h('span', { class: 'hmi-tile-name', text: sc.name }),
        h('span', { class: 'hmi-tile-meta', text: `${sc.width}×${sc.height}${popup ? ' · panel' : ''}` })));
      grid.appendChild(tile);
      tiles.push(tile);
      io.observe(thumb);
    }
    page.append(h('section', { class: 'hmi-group' }, h('h2', { text: `${g}`, 'data-count': list.length }), grid));
  }
  filter.addEventListener('input', () => {
    const q = filter.value.trim().toLowerCase();
    for (const t of tiles) t.hidden = Boolean(q) && !t.dataset.name.includes(q);
  });
  host.replaceChildren(page);

  return {
    update(values) { views.forEach((v) => v.update(values)); },
    destroy() {
      io.disconnect();
      views.forEach((v) => v.destroy());
      host.replaceChildren();
    },
  };
}
