// Alarm list widget (ISA-18.2 states from docs/PROTOCOL.md): severity colours,
// acknowledge, acknowledge all, shelve/unshelve, filters. Data comes in through
// setAlarms() (the WebSocket "alarms" message); actions go through the api.
//
//   const list = createAlarmList(host, { api, area });
//   list.setAlarms(alarms); list.destroy();

import { h } from './svg.js';
import { formatAge, formatDateTime, formatValue } from './format.js';
import { toast } from './toast.js';

export const SEVERITIES = ['critical', 'high', 'medium', 'low'];
const SEVERITY_RANK = { critical: 4, high: 3, medium: 2, low: 1 };
const STATE_LABEL = {
  'active-unacked': 'Active · unacked', 'active-acked': 'Active', 'cleared-unacked': 'Cleared · unacked',
  shelved: 'Shelved', disabled: 'Disabled', normal: 'Normal',
};

/** Alarms that need an operator's attention (shown in lists and banners). */
export function isVisibleAlarm(a) {
  return a.state && a.state !== 'normal';
}
export const isActive = (a) => a.state === 'active-unacked' || a.state === 'active-acked';
export const isUnacked = (a) => a.state === 'active-unacked' || a.state === 'cleared-unacked';

/** Unacked first, then active, then severity, then newest. */
export function sortAlarms(list) {
  const rank = (a) => (a.state === 'active-unacked' ? 4 : a.state === 'cleared-unacked' ? 3 : a.state === 'active-acked' ? 2 : 1);
  return [...list].sort((a, b) => rank(b) - rank(a)
    || (SEVERITY_RANK[b.severity] ?? 0) - (SEVERITY_RANK[a.severity] ?? 0)
    || (b.since ?? 0) - (a.since ?? 0));
}

/** Active alarm counts per severity plus total unacked, for the top-bar banner. */
export function alarmSummary(list) {
  const counts = Object.fromEntries(SEVERITIES.map((s) => [s, 0]));
  let unacked = 0;
  let newest = null;
  for (const a of list ?? []) {
    if (!isVisibleAlarm(a) || a.state === 'shelved' || a.state === 'disabled') continue;
    if (isActive(a) && counts[a.severity] !== undefined) counts[a.severity] += 1;
    if (isUnacked(a)) unacked += 1;
    if (isActive(a) && (!newest || (a.since ?? 0) > (newest.since ?? 0))) newest = a;
  }
  return { counts, unacked, newest, total: SEVERITIES.reduce((n, s) => n + counts[s], 0) };
}

const FILTERS = [
  ['all', 'All', isVisibleAlarm],
  ['unacked', 'Unacked', isUnacked],
  ['active', 'Active', isActive],
  ['shelved', 'Shelved', (a) => a.state === 'shelved'],
];

/** The alarms a list shows for an area, a filter key and an optional severity, sorted. */
export function shownAlarms(alarms, { area = null, filter = 'all', severity = null } = {}) {
  const pred = (FILTERS.find(([k]) => k === filter) ?? FILTERS[0])[2];
  return sortAlarms((alarms ?? []).filter((a) => (!area || a.area === area) && pred(a) && (!severity || a.severity === severity)));
}

/**
 * What "Ack" in the toolbar acknowledges: everything (one ack-all request) when
 * the list shows the whole plant, otherwise only the unacknowledged alarms it
 * shows, by name. An operator only acknowledges what they can see (ISA-18.2).
 */
export function ackPlan(alarms, view = {}) {
  const scoped = Boolean(view.area || (view.filter && view.filter !== 'all') || view.severity);
  const names = shownAlarms(alarms, view).filter(isUnacked).map((a) => a.name);
  return { all: !scoped, names, label: scoped ? 'Ack shown' : 'Ack all' };
}

/**
 * @param {HTMLElement} host
 * @param {{api:object, area?:string, compact?:boolean, user?:string}} opts
 */
export function createAlarmList(host, opts) {
  const { api, area } = opts;
  let alarms = [];
  let filterKey = 'all';
  let severity = null;

  host.classList.add('mc-alarmlist');
  if (opts.compact) host.classList.add('mc-compact');
  const filterBtns = FILTERS.map(([key, label]) => h('button', {
    type: 'button', class: `mc-tbtn${key === filterKey ? ' mc-on' : ''}`, 'data-key': key, text: label,
    onclick: () => {
      filterKey = key;
      filterBtns.forEach((b) => b.classList.toggle('mc-on', b.dataset.key === key));
      render();
    },
  }));
  const sevBtns = SEVERITIES.map((sv) => h('button', {
    type: 'button', class: `mc-sev-chip mc-sev-${sv}`, 'data-sev': sv, title: `Only ${sv}`,
    onclick: () => {
      severity = severity === sv ? null : sv;
      sevBtns.forEach((b) => b.classList.toggle('mc-on', b.dataset.sev === severity));
      render();
    },
  }, h('span', { class: 'mc-sev-n', text: '0' }), sv));
  const view = () => ({ area, filter: filterKey, severity });
  const ackAll = h('button', { type: 'button', class: 'mc-btn mc-btn-primary', text: 'Ack all', onclick: () => ackShown() });
  const list = h('div', { class: 'mc-alarm-rows', role: 'list' });
  host.append(
    h('div', { class: 'mc-alarm-toolbar' },
      h('div', { class: 'mc-tgroup' }, filterBtns),
      h('div', { class: 'mc-tgroup mc-sevs' }, sevBtns),
      h('span', { class: 'mc-spacer' }),
      area ? h('span', { class: 'mc-alarm-area', text: `Area: ${area}` }) : null,
      ackAll),
    list);

  async function act(fn, okMsg) {
    if (!api) return;
    try {
      await fn();
      if (okMsg) toast(okMsg, { kind: 'ok', timeout: 2000 });
    } catch (err) {
      toast(`Alarm action failed: ${err.message}`, { kind: 'error' });
    }
  }

  function ackShown() {
    const plan = ackPlan(alarms, view());
    if (plan.all) return act(() => api.ackAll(), 'All alarms acknowledged');
    if (!plan.names.length) return undefined;
    return act(async () => {
      const results = await Promise.allSettled(plan.names.map((n) => api.ack(n)));
      const failed = results.filter((r) => r.status === 'rejected');
      if (failed.length) throw new Error(`${failed.length} of ${plan.names.length} not acknowledged: ${failed[0].reason?.message ?? 'error'}`);
    }, `${plan.names.length} alarm(s) acknowledged`);
  }

  function shelveMenu(a, anchor) {
    host.querySelector('.mc-shelve-menu')?.remove();
    const menu = h('div', { class: 'mc-shelve-menu', role: 'menu' },
      [15, 30, 60, 240, 480].map((m) => h('button', {
        type: 'button', role: 'menuitem', text: m < 60 ? `${m} min` : `${m / 60} h`,
        onclick: () => {
          menu.remove();
          act(() => api.shelve(a.name, m), `${a.name} shelved for ${m < 60 ? `${m} min` : `${m / 60} h`}`);
        },
      })));
    anchor.parentElement.appendChild(menu);
    const close = (e) => {
      if (!menu.contains(e.target)) {
        menu.remove();
        document.removeEventListener('pointerdown', close, true);
      }
    };
    setTimeout(() => document.addEventListener('pointerdown', close, true), 0);
  }

  function render() {
    const scoped = alarms.filter((a) => !area || a.area === area);
    const counts = Object.fromEntries(SEVERITIES.map((sv) => [sv, scoped.filter((a) => a.severity === sv && isActive(a)).length]));
    sevBtns.forEach((b) => { b.querySelector('.mc-sev-n').textContent = counts[b.dataset.sev]; });
    const shown = shownAlarms(alarms, view());
    const plan = ackPlan(alarms, view());
    ackAll.textContent = plan.label;
    ackAll.title = plan.all ? 'Acknowledge every unacknowledged alarm' : 'Acknowledge the unacknowledged alarms in this list';
    ackAll.disabled = !plan.names.length;
    const now = Date.now();
    list.replaceChildren(...shown.map((a) => {
      const actions = [];
      if (isUnacked(a)) actions.push(h('button', { type: 'button', class: 'mc-btn mc-btn-ack', text: 'Ack', onclick: () => act(() => api.ack(a.name)) }));
      if (a.state === 'shelved') actions.push(h('button', { type: 'button', class: 'mc-btn', text: 'Unshelve', onclick: () => act(() => api.unshelve(a.name), `${a.name} unshelved`) }));
      else if (a.state !== 'disabled') {
        const btn = h('button', { type: 'button', class: 'mc-btn', text: 'Shelve…', 'aria-haspopup': 'menu' });
        btn.addEventListener('click', () => shelveMenu(a, btn));
        actions.push(btn);
      }
      const since = a.since ? `${formatAge(now - a.since)} ago` : '';
      return h('div', {
        role: 'listitem',
        class: `mc-alarm mc-sev-${a.severity} mc-state-${a.state}${a.state === 'active-unacked' ? ' mc-blink-soft' : ''}`,
        title: `${a.name}\nTag ${a.tag}\nSince ${formatDateTime(a.since)}${a.acked_by ? `\nAcknowledged by ${a.acked_by}` : ''}${a.shelved_until ? `\nShelved until ${formatDateTime(a.shelved_until)}` : ''}`,
      },
      h('span', { class: 'mc-alarm-sev', text: a.severity }),
      h('div', { class: 'mc-alarm-main' },
        h('div', { class: 'mc-alarm-msg', text: a.message || a.name }),
        h('div', { class: 'mc-alarm-meta' },
          h('span', { text: a.name }),
          h('span', { text: `${a.tag} = ${formatValue(a.value, { format: '#0.###' })}` }),
          a.area ? h('span', { text: a.area }) : null,
          h('span', { text: formatDateTime(a.since) }),
          h('span', { class: 'mc-alarm-age', text: since }))),
      h('span', { class: 'mc-alarm-state', text: STATE_LABEL[a.state] ?? a.state }),
      h('div', { class: 'mc-alarm-actions' }, actions));
    }));
    if (!shown.length) list.appendChild(h('div', { class: 'mc-empty-state' }, h('div', { class: 'mc-empty-icon', text: '✓' }), scoped.some(isVisibleAlarm) ? 'No alarms match this filter' : 'No alarms'));
  }

  const timer = setInterval(() => { if (alarms.length) render(); }, 10000);
  render();

  return {
    setAlarms(list2) {
      alarms = Array.isArray(list2) ? list2 : [];
      render();
    },
    destroy() {
      clearInterval(timer);
      host.replaceChildren();
      host.classList.remove('mc-alarmlist', 'mc-compact');
    },
  };
}
