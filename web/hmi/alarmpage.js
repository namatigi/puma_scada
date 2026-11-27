// Alarm page: live alarm list (ack, ack all, shelve), the alarm journal and
// driver diagnostics, as tabs.

import { h } from '../shared/svg.js';
import { createAlarmList } from '../shared/alarmlist.js';
import { formatDateTime, formatValue, formatAge } from '../shared/format.js';

const EVENT_LABEL = { activated: 'Activated', cleared: 'Cleared', acked: 'Acknowledged', shelved: 'Shelved', unshelved: 'Unshelved' };

/**
 * @param {HTMLElement} host
 * @param {{api:object, alarms:object[]}} opts
 */
export function createAlarmPage(host, opts) {
  const { api } = opts;
  const listHost = h('div', { class: 'hmi-alarm-list-host' });
  const journalBody = h('tbody');
  const journal = h('div', { class: 'hmi-journal', hidden: true },
    h('div', { class: 'mc-table-scroll' }, h('table', { class: 'mc-table' },
      h('thead', {}, h('tr', {}, ['Time', 'Alarm', 'Event', 'Value', 'User'].map((t) => h('th', { text: t })))), journalBody)),
    h('div', { class: 'mc-refresh-bar' }, h('span', { class: 'mc-htable-status', text: 'Newest first' }), h('button', { type: 'button', class: 'mc-refresh', text: '⟳ Refresh', onclick: () => loadJournal() })));
  const diag = h('div', { class: 'hmi-diag', hidden: true });
  const tabs = [['Alarms', listHost], ['Journal', journal], ['Diagnostics', diag]];
  const tabBtns = tabs.map(([name, panel]) => h('button', {
    type: 'button', role: 'tab', class: 'hmi-tab', text: name,
    onclick: () => select(name),
  }));
  function select(name) {
    tabs.forEach(([n, panel], i) => {
      panel.hidden = n !== name;
      tabBtns[i].classList.toggle('hmi-on', n === name);
      tabBtns[i].setAttribute('aria-selected', String(n === name));
    });
    if (name === 'Journal') loadJournal();
    if (name === 'Diagnostics') loadDiag();
  }

  async function loadJournal() {
    try {
      const res = await api.journal(500);
      const events = res?.events ?? [];
      journalBody.replaceChildren(...events.map((e) => h('tr', { class: `hmi-ev-${e.event}` },
        h('td', { class: 'mc-num', text: formatDateTime(e.ts) }),
        h('td', { text: e.name }),
        h('td', { text: EVENT_LABEL[e.event] ?? e.event }),
        h('td', { class: 'mc-num', text: formatValue(e.value, { format: '#0.###' }) }),
        h('td', { text: e.user ?? '' }))));
      if (!events.length) journalBody.replaceChildren(h('tr', { class: 'mc-empty' }, h('td', { colspan: 5, text: 'No alarm events yet' })));
    } catch (err) {
      journalBody.replaceChildren(h('tr', { class: 'mc-empty' }, h('td', { colspan: 5, text: `Journal not available: ${err.message}` })));
    }
  }

  async function loadDiag() {
    try {
      const d = await api.diagnostics();
      diag.replaceChildren(
        h('p', { class: 'hmi-page-sub', text: `Runtime up for ${formatAge((d?.uptime_s ?? 0) * 1000)}` }),
        h('div', { class: 'hmi-diag-cards' }, (d?.drivers ?? []).map((dr) => h('div', { class: `hmi-diag-card ${dr.connected ? 'hmi-ok' : 'hmi-bad'}` },
          h('div', { class: 'hmi-diag-name' }, h('span', { class: 'hmi-conn-dot' }), dr.name),
          h('div', { text: dr.connected ? 'Connected' : 'Not connected' }),
          h('div', { class: 'hmi-diag-num', text: `${dr.requests ?? 0} requests · ${dr.errors ?? 0} errors` }),
          dr.last_error ? h('div', { class: 'hmi-diag-err', text: dr.last_error }) : null))));
    } catch (err) {
      diag.replaceChildren(h('p', { class: 'hmi-page-sub', text: `Diagnostics not available: ${err.message}` }));
    }
  }

  const page = h('div', { class: 'hmi-page hmi-alarm-page' },
    h('div', { class: 'hmi-page-head' }, h('h1', { text: 'Alarms' }), h('div', { class: 'hmi-tabs', role: 'tablist' }, tabBtns)),
    listHost, journal, diag);
  host.replaceChildren(page);
  const list = createAlarmList(listHost, { api });
  list.setAlarms(opts.alarms ?? []);
  select('Alarms');

  return {
    setAlarms: (a) => list.setAlarms(a),
    destroy() {
      list.destroy();
      host.replaceChildren();
    },
  };
}
