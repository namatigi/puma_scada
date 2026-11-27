// Moviclone operator runtime (HMI).
//
// Routes (hash):  #<Screen>   a screen (small screens open as floating panels)
//                 #!screens   screen navigator      #!alarms   alarm page
// index.html?mock runs entirely in the browser from mock/bundle.json.
//
// Sign-in (docs/PROTOCOL.md): viewers watch, operators also write values and
// act on alarms. With the runtime's --hmi-guest the HMI can be watched without
// signing in; operators sign in from the top bar.

import { createHttpApi } from '../shared/api.js';
import { createWsClient } from '../shared/client.js';
import { createScreenView, isPopupScreen } from '../shared/renderer.js';
import { loadSymbolMap } from '../shared/symbolmap.js';
import { parseInputValue } from '../shared/format.js';
import { toast } from '../shared/toast.js';
import { h } from '../shared/svg.js';
import { createTopBar } from './topbar.js';
import { createPanels } from './panels.js';
import { createNavigator } from './navigator.js';
import { createAlarmPage } from './alarmpage.js';
import { screenFromHash } from './hash.js';
import { createAuthApi, signIn, roleAtLeast, changePasswordDialog } from '../shared/signin.js';
import { setAccess, canOperate, mayOperate } from '../shared/access.js';

const params = new URLSearchParams(window.location.search);
const MOCK = params.has('mock');

const state = {
  api: null,
  client: null,
  project: null,
  symbolMap: null,
  alarms: [],
  /** current page: {kind:'screen', name, view} | {kind:'screens'|'alarms', page} */
  current: null,
  depth: 0,
  /** Signed-in user {name, role}, or null (guest viewing, or ?mock). */
  me: null,
  /** The runtime lets people watch without signing in (--hmi-guest). */
  guestView: false,
};
const auth = MOCK ? null : createAuthApi('');

const mainEl = document.getElementById('main');
let topbar;
let panels;

// ---------------------------------------------------------------- boot

async function boot() {
  try {
    if (MOCK) {
      const { createMockBackend } = await import('./mock.js');
      const res = await fetch(params.get('bundle') ?? 'mock/bundle.json', { cache: 'no-cache' });
      if (!res.ok) throw new Error(`mock bundle: HTTP ${res.status}`);
      const backend = createMockBackend(await res.json());
      state.api = backend.api;
      state.client = backend.client;
    } else {
      const session = await auth.session();
      state.guestView = Boolean(session.guest_view);
      state.me = session.user ?? (await signIn(auth, session, { product: 'Moviclone HMI', allowGuest: state.guestView }));
      state.api = createHttpApi('', { onSignedOut: signInAgain });
      state.client = createWsClient();
    }
    applyAccess();
    [state.project, state.symbolMap] = await Promise.all([state.api.bundle(), loadSymbolMap()]);
    validateBundle(state.project);
  } catch (err) {
    showFatal(err);
    return;
  }

  topbar = createTopBar(document.getElementById('topbar'), {
    mock: MOCK,
    onBack: back,
    onHome: () => navigate(mainScreen()?.name),
    onScreens: () => navigate('!screens'),
    onAlarms: () => navigate('!alarms'),
  });
  topbar.setProject(state.project.meta?.name ?? 'Project', state.project.meta?.site);
  if (!MOCK) topbar.setUser(state.me, { onSignIn: () => signInAgain({ fresh: true }), onPassword: changePassword, onSignOut: signOut });
  panels = createPanels(document.getElementById('panels'), { viewOptions, onChange: syncSubscriptions });

  const { client } = state;
  client.on('status', (st) => {
    topbar.setConnection(st, MOCK);
    const offline = st !== 'open';
    currentViews().forEach((v) => v.setOffline(offline));
  });
  client.on('values', (changes) => {
    for (const v of currentViews()) v.update(changes);
    state.current?.nav?.update(changes);
  });
  client.on('alarms', setAlarms);
  client.on('reload', reloadProject);
  client.on('signed-out', sessionEnded);
  // A runtime restart or a session that ended while offline is noticed when the connection drops or returns.
  client.on('status', (st) => { if ((st === 'closed' || st === 'open') && !MOCK) checkSessionSoon(); });
  topbar.setConnection(client.status, MOCK);
  topbar.setAlarms([]);
  client.connect();
  // Alarms also come over the socket, but fetch once so the banner is right at once.
  state.api.alarms().then((r) => { if (r?.alarms && !state.alarms.length) setAlarms(r.alarms); }).catch(() => {});

  window.addEventListener('hashchange', route);
  document.addEventListener('keydown', onKey);
  route();
}

// ---------------------------------------------------------------- sign-in

function applyAccess() {
  setAccess({
    canOperate: () => MOCK || roleAtLeast(state.me?.role, 'operator'),
    onDenied: (what) => {
      if (!state.me) {
        toast(`Sign in as an operator to ${what}`, { kind: 'warn' });
        signInAgain({ fresh: true });
      } else {
        toast(`${state.me.name} is a ${state.me.role}: to ${what} you need the operator role`, { kind: 'warn', timeout: 4500 });
      }
    },
  });
  document.body.classList.toggle('hmi-view-only', !canOperate());
  topbar?.setUser(state.me, {});
}

let reauth = null;
/**
 * Signs in over the open HMI (screens keep running underneath). Used when a
 * request is refused because the session ended, and by the top bar's Sign in.
 * Resolves true once someone is signed in.
 */
function signInAgain({ fresh = false } = {}) {
  reauth ??= (async () => {
    try {
      const session = await auth.session().catch(() => ({}));
      let user = fresh ? null : session.user;
      if (!user) {
        user = await signIn(auth, session, {
          modal: true,
          product: 'Moviclone HMI',
          name: state.me?.name ?? '',
          allowGuest: state.guestView,
          title: fresh ? 'Sign in' : '',
          reason: fresh ? '' : 'Your session ended. Sign in again to go on.',
        });
      }
      const changed = (user?.name ?? '') !== (state.me?.name ?? '');
      state.me = user;
      applyAccess();
      // The WebSocket carries the session it was opened with.
      if (changed) state.client.reconnect?.();
      if (user) toast(`Signed in as ${user.name} (${user.role})`, { kind: 'ok', timeout: 2500 });
      return Boolean(user);
    } finally {
      reauth = null;
    }
  })();
  return reauth;
}

/** The server closed the socket because the session ended. */
function sessionEnded() {
  if (state.guestView) {
    const who = state.me?.name;
    state.me = null;
    applyAccess();
    if (who) toast(`${who} was signed out (the session ended); still watching`, { kind: 'warn', timeout: 5000 });
    return;
  }
  signInAgain();
}

let sessionCheck = null;
/** Brings the page in line with the server's view of the session (ended, or a new role). */
function checkSessionSoon() {
  clearTimeout(sessionCheck);
  sessionCheck = setTimeout(async () => {
    if (reauth) return;
    try {
      const s = await auth.session();
      if (!s.user && state.me) sessionEnded();
      else if (s.user && (s.user.name !== state.me?.name || s.user.role !== state.me?.role)) {
        state.me = s.user;
        applyAccess();
      }
    } catch { /* runtime offline: the connection indicator says so */ }
  }, 1500);
}

async function changePassword() {
  if (await changePasswordDialog(auth, { product: 'Moviclone HMI' })) toast('Password changed', { kind: 'ok' });
}

async function signOut() {
  try { await auth.signOut(); } catch { /* signed out anyway */ }
  window.location.reload();
}

function setAlarms(list) {
  state.alarms = list;
  topbar.setAlarms(list);
  for (const v of currentViews()) v.setAlarms(list);
  state.current?.page?.setAlarms?.(list);
}

function validateBundle(p) {
  if (!p || !Array.isArray(p.screens)) throw new Error('the project bundle has no screens');
  if (!p.screens.length) throw new Error('the project has no screens yet — create one in the studio');
}

function showFatal(err) {
  console.error('[moviclone] HMI start failed', err);
  mainEl.replaceChildren(h('div', { class: 'hmi-fatal' },
    h('div', { class: 'hmi-boot-mark', 'aria-hidden': 'true' }),
    h('h1', { text: 'The HMI could not start' }),
    h('p', { text: err.message }),
    h('p', { class: 'hmi-page-sub', text: MOCK ? 'Check that mock/bundle.json exists next to index.html.' : 'Check that the Moviclone runtime is running, or open index.html?mock for the simulation.' }),
    h('button', { type: 'button', class: 'mc-btn mc-btn-primary', text: 'Retry', onclick: () => window.location.reload() })));
}

function mainScreen() {
  const p = state.project;
  return p.screens.find((sc) => sc.name === p.meta?.main_screen) ?? p.screens[0];
}

function findScreen(name) {
  return state.project.screens.find((sc) => sc.name === name);
}

function viewOptions() {
  return {
    project: state.project,
    symbolMap: state.symbolMap,
    api: state.api,
    mode: 'runtime',
    onCommand: runCommand,
    values: state.client.values,
  };
}

function currentViews() {
  const out = [];
  if (state.current?.view) out.push(state.current.view);
  if (panels) out.push(...panels.views());
  return out;
}

function syncSubscriptions() {
  const tags = new Set();
  for (const v of currentViews()) v.tagsInUse().forEach((t) => tags.add(t));
  state.client.setTags(tags);
}

// ---------------------------------------------------------------- routing

function navigate(target) {
  if (!target) return;
  const hash = `#${target.startsWith('!') ? target : encodeURIComponent(target)}`;
  if (window.location.hash === hash) route();
  else {
    state.depth += 1;
    window.location.hash = hash;
  }
}

function back() {
  if (state.depth > 0) {
    state.depth -= 1;
    window.history.back();
  } else navigate(mainScreen()?.name);
}

function teardown() {
  const c = state.current;
  c?.view?.destroy();
  c?.page?.destroy();
  c?.nav?.destroy();
  state.current = null;
  mainEl.replaceChildren();
  mainEl.className = 'hmi-main';
}

function route() {
  const raw = screenFromHash(window.location.hash);
  if (raw === '!screens') return showNavigator();
  if (raw === '!alarms') return showAlarmPage();
  const main = mainScreen();
  const sc = raw ? findScreen(raw) : main;
  if (!sc) {
    toast(`Screen "${raw}" does not exist`, { kind: 'warn' });
    return showNavigator();
  }
  // A faceplate opened by URL shows over the main screen.
  if (isPopupScreen(sc, main)) {
    if (state.current?.kind !== 'screen') showScreen(main);
    panels.open(sc);
    return undefined;
  }
  panels.closeAll();
  return showScreen(sc);
}

function showScreen(sc) {
  if (state.current?.kind === 'screen' && state.current.name === sc.name) return;
  // Faceplates belong to the screen they were opened from.
  panels.closeAll();
  teardown();
  mainEl.classList.add('hmi-main-screen');
  const host = h('div', { class: 'hmi-screen-host' });
  mainEl.appendChild(host);
  const view = createScreenView(host, sc, { ...viewOptions(), zoomable: true });
  view.setOffline(state.client.status !== 'open');
  view.setAlarms(state.alarms);
  state.current = { kind: 'screen', name: sc.name, view };
  topbar.setScreen(sc.name);
  topbar.setAutoHide(true);
  syncSubscriptions();
}

function showNavigator() {
  teardown();
  mainEl.classList.add('hmi-main-page');
  const nav = createNavigator(mainEl, {
    project: state.project,
    viewOptions,
    current: null,
    onOpen: (sc) => navigate(sc.name),
  });
  state.current = { kind: 'screens', nav };
  topbar.setScreen('Screens');
  topbar.setAutoHide(false);
  syncSubscriptions();
}

function showAlarmPage() {
  teardown();
  mainEl.classList.add('hmi-main-page');
  const page = createAlarmPage(mainEl, { api: state.api, alarms: state.alarms });
  state.current = { kind: 'alarms', page };
  topbar.setScreen('Alarms');
  topbar.setAutoHide(false);
  syncSubscriptions();
}

function onKey(e) {
  if (e.key === 'Escape' && !e.target.closest?.('.mc-editor')) {
    if (panels.closeTop()) e.preventDefault();
  } else if (e.altKey && e.key === 'ArrowLeft') {
    e.preventDefault();
    back();
  } else if (e.altKey && e.key === 'Home') {
    e.preventDefault();
    navigate(mainScreen()?.name);
  }
}

// ---------------------------------------------------------------- commands

async function runCommand(cmd, ctx = {}) {
  switch (cmd.action) {
    case 'open-screen': {
      const sc = findScreen(cmd.screen);
      if (!sc) {
        toast(`Screen "${cmd.screen}" does not exist`, { kind: 'error' });
        return;
      }
      if (isPopupScreen(sc, mainScreen())) {
        const near = ctx.element && ctx.view ? anchorOf(ctx.element, ctx.view) : undefined;
        panels.open(sc, near);
      } else navigate(sc.name);
      return;
    }
    case 'write-tag': {
      if (!mayWrite(cmd.tag)) return;
      const tag = state.project.tags?.find((t) => t.name === cmd.tag);
      if (!tag) {
        toast(`Tag "${cmd.tag}" does not exist`, { kind: 'error' });
        return;
      }
      const mode = cmd.mode ?? 'set';
      let value;
      try {
        value = parseInputValue(cmd.value, tag.type, state.project.meta?.culture);
      } catch (err) {
        toast(`${cmd.tag}: ${err.message}`, { kind: 'error' });
        return;
      }
      const res = await state.client.write(cmd.tag, value, mode);
      if (res.ok) toast(`${cmd.tag} ← ${cmd.value}${mode !== 'set' ? ` (${mode})` : ''}`, { kind: 'ok', timeout: 2200 });
      else toast(`Write to ${cmd.tag} failed: ${res.error}`, { kind: 'error' });
      return;
    }
    case 'report':
      toast(`Report "${cmd.report}" is not available yet`, { kind: 'warn' });
      return;
    default:
      toast(`Unknown command "${cmd.action}"`, { kind: 'warn' });
  }
}

function mayWrite(tag) {
  return mayOperate(`write ${tag}`);
}

/** Client position of an element's right edge, to open its panel beside it. */
function anchorOf(el, view) {
  const node = view.elementNode(el.id);
  const r = node?.getBoundingClientRect();
  return r ? { x: r.right, y: r.top + r.height / 2 } : undefined;
}

// ---------------------------------------------------------------- reload

async function reloadProject() {
  try {
    const p = await state.api.bundle();
    validateBundle(p);
    state.project = p;
    const openPanels = panels.names();
    panels.closeAll();
    teardown();
    route();
    openPanels.forEach((n) => { const sc = findScreen(n); if (sc) panels.open(sc); });
    topbar.setProject(p.meta?.name ?? 'Project', p.meta?.site);
    toast('Project updated', { kind: 'ok' });
  } catch (err) {
    toast(`Project reload failed: ${err.message}`, { kind: 'error' });
  }
}

boot();
