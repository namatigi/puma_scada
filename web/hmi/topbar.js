// Operator top bar: project, current screen, back/home/screens, alarm banner,
// connection state and clock. Auto-hides over full-window screens and comes back
// when the pointer reaches the top edge or the top edge is tapped.

import { h } from '../shared/svg.js';
import { alarmSummary, SEVERITIES } from '../shared/alarmlist.js';

const HIDE_AFTER_MS = 3500;

const ICONS = {
  back: 'M15 5l-7 7 7 7',
  home: 'M4 11l8-7 8 7M6 10v9h4v-5h4v5h4v-9',
  grid: 'M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z',
  bell: 'M6 16V11a6 6 0 0 1 12 0v5l2 2H4zM10 20a2 2 0 0 0 4 0',
  pin: 'M9 4h6l-1 6 3 3H7l3-3zM12 13v7',
  full: 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5',
  user: 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 20c0-3.5 3.6-6 8-6s8 2.5 8 6',
};

function icon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', 'hmi-icon');
  svg.setAttribute('aria-hidden', 'true');
  const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  p.setAttribute('d', ICONS[name]);
  svg.appendChild(p);
  return svg;
}

/**
 * @param {HTMLElement} root the <header>
 * @param {{onBack:Function, onHome:Function, onScreens:Function, onAlarms:Function, mock:boolean}} actions
 */
export function createTopBar(root, actions) {
  const hotzone = document.getElementById('topbar-hotzone');
  const project = h('span', { class: 'hmi-project' });
  const screenName = h('span', { class: 'hmi-screen-name' });
  const btn = (name, label, fn) => h('button', { type: 'button', class: 'hmi-tb-btn', title: label, 'aria-label': label, onclick: fn }, icon(name));
  const sevEls = Object.fromEntries(SEVERITIES.map((sv) => [sv, h('span', { class: `hmi-sev mc-sev-${sv}`, title: `${sv} alarms` }, h('b', { text: '0' }))]));
  const newest = h('span', { class: 'hmi-alarm-newest', text: 'No active alarms' });
  const banner = h('button', { type: 'button', class: 'hmi-alarm-banner', onclick: actions.onAlarms, title: 'Open the alarm page' },
    icon('bell'), h('span', { class: 'hmi-sevs' }, Object.values(sevEls)), newest);
  const connDot = h('span', { class: 'hmi-conn-dot' });
  const connText = h('span', { class: 'hmi-conn-text', text: 'Connecting' });
  const conn = h('span', { class: 'hmi-conn', role: 'status' }, connDot, connText);
  const clock = h('time', { class: 'hmi-clock' });
  const userName = h('span', { class: 'hmi-user-name' });
  const userRole = h('span', { class: 'hmi-user-role' });
  const userBtn = h('button', { type: 'button', class: 'hmi-user', hidden: true, 'aria-haspopup': 'menu' }, icon('user'), userName, userRole);
  let userActions = {};
  let user = null;
  const pinBtn = btn('pin', 'Keep the top bar visible', () => setPinned(!pinned));
  const fullBtn = btn('full', 'Full screen', () => {
    if (document.fullscreenElement) document.exitFullscreen?.();
    else document.documentElement.requestFullscreen?.().catch(() => {});
  });
  root.append(
    h('div', { class: 'hmi-tb-left' },
      h('span', { class: 'hmi-logo', 'aria-hidden': 'true' }),
      h('span', { class: 'hmi-brand' }, project, actions.mock ? h('span', { class: 'hmi-sim', text: 'SIMULATION', title: 'Values are simulated in the browser (?mock)' }) : null),
      btn('back', 'Back', actions.onBack),
      btn('home', 'Main screen', actions.onHome),
      btn('grid', 'Screens', actions.onScreens),
      screenName),
    banner,
    h('div', { class: 'hmi-tb-right' }, userBtn, conn, clock, pinBtn, fullBtn),
  );

  let pinned = false;
  let autoHide = true;
  let hideTimer = null;
  const body = document.body;

  function show() {
    body.classList.remove('hmi-topbar-hidden');
    clearTimeout(hideTimer);
    if (autoHide && !pinned) hideTimer = setTimeout(hide, HIDE_AFTER_MS);
  }
  function hide() {
    if (pinned || !autoHide || root.matches(':hover') || root.contains(document.activeElement)) {
      if (autoHide && !pinned) hideTimer = setTimeout(hide, HIDE_AFTER_MS);
      return;
    }
    body.classList.add('hmi-topbar-hidden');
  }
  function setPinned(p) {
    pinned = p;
    pinBtn.classList.toggle('hmi-on', p);
    body.classList.toggle('hmi-topbar-pinned', p);
    try {
      localStorage.setItem('moviclone.hmi.pinTopbar', p ? '1' : '0');
    } catch {
      // Storage unavailable (private mode): the choice lasts for this page only.
    }
    show();
  }
  try {
    if (localStorage.getItem('moviclone.hmi.pinTopbar') === '1') setPinned(true);
  } catch {
    // ignore
  }

  document.addEventListener('pointermove', (e) => { if (e.clientY < 56 && e.pointerType === 'mouse') show(); });
  hotzone.addEventListener('pointerdown', show);
  root.addEventListener('pointerenter', show);
  root.addEventListener('focusin', show);

  const tick = () => {
    const d = new Date();
    const p = (n) => String(n).padStart(2, '0');
    clock.textContent = `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
    clock.title = d.toDateString();
    clock.dateTime = d.toISOString();
  };
  tick();
  setInterval(tick, 1000);

  function closeUserMenu() {
    document.querySelector('.hmi-user-menu')?.remove();
  }
  userBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (!user) { userActions.onSignIn?.(); return; }
    if (document.querySelector('.hmi-user-menu')) { closeUserMenu(); return; }
    const item = (text, fn) => h('button', { type: 'button', role: 'menuitem', text, onclick: () => { closeUserMenu(); fn?.(); } });
    const r = userBtn.getBoundingClientRect();
    const menu = h('div', { class: 'hmi-user-menu', role: 'menu', style: { top: `${r.bottom + 6}px`, right: `${Math.max(8, window.innerWidth - r.right)}px` } },
      h('div', { class: 'hmi-user-menu-head' }, h('b', { text: user.name }), h('span', { text: user.role })),
      item('Sign in as someone else…', userActions.onSignIn),
      item('Change password…', userActions.onPassword),
      item('Sign out', userActions.onSignOut));
    document.body.appendChild(menu);
    const away = (ev) => { if (!menu.contains(ev.target)) { closeUserMenu(); document.removeEventListener('pointerdown', away, true); } };
    setTimeout(() => document.addEventListener('pointerdown', away, true), 0);
    show();
  });

  return {
    show,
    /** Signed-in user ({name, role}) or null (watching without signing in). */
    setUser(me, actions = {}) {
      user = me;
      userActions = { ...userActions, ...Object.fromEntries(Object.entries(actions).filter(([, v]) => v)) };
      userBtn.hidden = false;
      userBtn.classList.toggle('hmi-guest', !me);
      userName.textContent = me ? me.name : 'Sign in';
      userRole.textContent = me ? me.role : 'watching';
      userRole.dataset.role = me?.role ?? 'guest';
      userBtn.title = me ? `Signed in as ${me.name} (${me.role})` : 'Watching without signing in. Sign in to write values or acknowledge alarms.';
    },
    setProject(name) {
      project.textContent = name;
      document.title = `${name} · Moviclone HMI`;
    },
    setScreen(name) { screenName.textContent = name; },
    /** Auto-hide over full-window screens; always visible on pages. */
    setAutoHide(on) {
      autoHide = on;
      body.classList.toggle('hmi-topbar-static', !on);
      show();
    },
    setConnection(state, mock) {
      const label = mock ? (state === 'open' ? 'Simulation' : 'Starting') : { open: 'Online', connecting: 'Connecting', closed: 'Offline' }[state] ?? state;
      connText.textContent = label;
      conn.dataset.state = state;
      if (state !== 'open') show();
    },
    setAlarms(list) {
      const sum = alarmSummary(list);
      for (const sv of SEVERITIES) {
        sevEls[sv].querySelector('b').textContent = sum.counts[sv];
        sevEls[sv].classList.toggle('hmi-zero', sum.counts[sv] === 0);
      }
      banner.classList.toggle('hmi-unacked', sum.unacked > 0);
      banner.dataset.severity = sum.newest?.severity ?? '';
      newest.textContent = sum.newest ? sum.newest.message || sum.newest.name : sum.unacked ? `${sum.unacked} cleared alarm(s) to acknowledge` : 'No active alarms';
      banner.title = `${sum.total} active, ${sum.unacked} unacknowledged — open the alarm page`;
    },
  };
}
