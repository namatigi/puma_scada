// Launcher: recent projects, open / new / import. The Rust side starts the
// runtime and moves this window to the studio once a project is open.
'use strict';

const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
const status = $('status');
let busy = false;

function say(text, error = false) {
  status.textContent = text;
  status.classList.toggle('error', error);
}

async function run(label, cmd, args) {
  if (busy) return;
  busy = true;
  document.querySelectorAll('.action').forEach((b) => { b.disabled = true; });
  say(label);
  try {
    const opened = await invoke(cmd, args);
    say(opened ? 'Starting the runtime…' : '');
  } catch (err) {
    say(String(err), true);
    await refresh();
  } finally {
    busy = false;
    document.querySelectorAll('.action').forEach((b) => { b.disabled = false; });
  }
}

function el(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'text') e.textContent = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v);
  }
  e.append(...kids.filter(Boolean));
  return e;
}

async function refresh() {
  const state = await invoke('launcher_state');
  $('version').textContent = `Version ${state.version}.`;
  $('simulate').checked = state.simulate;
  const list = $('recent');
  list.replaceChildren();
  if (!state.recent.length) {
    list.append(el('li', { class: 'empty', text: 'No projects yet. Open, create or import one above.' }));
    return;
  }
  for (const p of state.recent) {
    const forget = el('button', { type: 'button', title: 'Remove from this list (the folder stays)', text: 'Remove',
      onclick: async (e) => { e.stopPropagation(); await invoke('forget_recent', { path: p.path }); refresh(); } });
    const li = el('li', { class: p.exists ? 'go' : 'missing', title: p.exists ? `Open ${p.path}` : 'This folder no longer holds a project' },
      el('div', { class: 'grow' }, el('span', { class: 'name', text: p.name }), el('span', { class: 'path', text: p.path })),
      forget);
    if (p.exists) li.addEventListener('click', () => run(`Opening ${p.name}…`, 'open_recent', { path: p.path }));
    list.append(li);
  }
}

$('open').addEventListener('click', () => run('Choose a project folder…', 'open_project_dialog'));
$('new').addEventListener('click', () => run('Choose an empty folder for the new project…', 'new_project'));
$('import').addEventListener('click', () => run('Choose the Movicon.NExT project…', 'import_movicon'));
$('simulate').addEventListener('change', (e) => invoke('set_simulate', { on: e.target.checked }));
refresh().catch((err) => say(String(err), true));
