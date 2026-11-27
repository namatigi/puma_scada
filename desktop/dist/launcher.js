// Launcher: depot setup (first start), recent projects, open / new / import.
// The Rust side starts the runtime and moves this window to the studio once a
// project is open.
'use strict';

const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
let busy = false;

function say(el, text, error = false) {
  el.textContent = text;
  el.classList.toggle('error', error);
}

function el(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'text') e.textContent = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (v === true) e.setAttribute(k, '');
    else if (v !== false && v !== null && v !== undefined) e.setAttribute(k, v);
  }
  e.append(...kids.filter(Boolean));
  return e;
}

// ---------------------------------------------------------------- home

async function run(label, cmd, args) {
  if (busy) return;
  busy = true;
  document.querySelectorAll('.action').forEach((b) => { b.disabled = true; });
  say($('status'), label);
  try {
    const opened = await invoke(cmd, args);
    say($('status'), opened ? 'Starting the runtime…' : '');
  } catch (err) {
    say($('status'), String(err), true);
    await refresh();
  } finally {
    busy = false;
    document.querySelectorAll('.action').forEach((b) => { b.disabled = false; });
  }
}

async function refresh() {
  const state = await invoke('launcher_state');
  $('version').textContent = `Version ${state.version}.`;
  $('simulate').checked = state.simulate;
  const list = $('recent');
  list.replaceChildren();
  if (!state.recent.length) {
    list.append(el('li', { class: 'empty', text: 'No projects yet. Set up a depot, or open, create or import a project above.' }));
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
  return state;
}

$('depot').addEventListener('click', () => showSetup());
$('open').addEventListener('click', () => run('Choose a project folder…', 'open_project_dialog'));
$('new').addEventListener('click', () => run('Choose an empty folder for the new project…', 'new_project'));
$('import').addEventListener('click', () => run('Choose the Movicon.NExT project…', 'import_movicon'));
$('simulate').addEventListener('change', (e) => invoke('set_simulate', { on: e.target.checked }));

// ---------------------------------------------------------------- depot setup

const setup = { step: 1, meters: [], chosen: new Set(), parent: '', firstRun: false, template: null };

/** The folder name the depot gets (the Rust side uses the same rule). */
function folderName(depot) {
  const n = depot.trim().replace(/[<>:"/\\|?*\u0000-\u001f]/g, '-').replace(/^[. ]+|[. ]+$/g, '');
  return n || 'Depot';
}

function sep() {
  return setup.parent.includes('\\') ? '\\' : '/';
}

async function showSetup({ firstRun = false } = {}) {
  setup.firstRun = firstRun;
  try {
    setup.template ??= await invoke('site_template');
  } catch (err) {
    say($('status'), `The depot template is missing: ${err}`, true);
    return;
  }
  setup.parent ||= setup.template.projects_dir;
  setup.step = 1;
  setup.chosen = new Set(setup.template.meters.map((m) => m.key));
  renderMeters();
  $('setup-cancel').textContent = firstRun ? 'Skip for now' : 'Cancel';
  say($('setup-status'), '');
  $('home').hidden = true;
  $('setup').hidden = false;
  go(1);
}
window.showSetup = showSetup;

function hideSetup() {
  $('setup').hidden = true;
  $('home').hidden = false;
}

function renderMeters() {
  const box = $('meters');
  box.replaceChildren();
  for (const m of setup.template.meters) {
    const number = (m.tag_prefix.match(/\d+/) ?? [''])[0];
    const input = el('input', { type: 'checkbox', checked: setup.chosen.has(m.key) });
    const card = el('label', { class: `meter${setup.chosen.has(m.key) ? ' on' : ''}` },
      input,
      el('span', { class: 'swatch', style: `background:${m.color || '#888'}` }),
      el('span', {}, el('b', { text: m.product }), el('small', { text: number ? `Flow meter FM_${number}` : '' })));
    input.addEventListener('change', () => {
      if (input.checked) setup.chosen.add(m.key);
      else setup.chosen.delete(m.key);
      card.classList.toggle('on', input.checked);
      say($('setup-status'), '');
    });
    box.append(card);
  }
}

function go(step) {
  setup.step = step;
  document.querySelectorAll('#setup .step').forEach((s) => { s.hidden = Number(s.dataset.step) !== step; });
  document.querySelectorAll('#setup .steps li').forEach((li) => {
    const n = Number(li.dataset.step);
    li.classList.toggle('on', n === step);
    li.classList.toggle('done', n < step);
  });
  $('setup-back').hidden = step === 1;
  $('setup-next').textContent = step === 3 ? 'Build the SCADA' : 'Next';
  const depot = $('depot-name').value.trim();
  $('depot-echo').textContent = depot;
  if (step === 3) {
    $('where').textContent = `${setup.parent}${sep()}${folderName(depot)}`;
    const chosen = setup.template.meters.filter((m) => setup.chosen.has(m.key));
    const left = setup.template.meters.filter((m) => !setup.chosen.has(m.key));
    $('summary').replaceChildren(
      el('div', {}, 'Depot: ', el('b', { text: depot })),
      el('div', {}, 'Meters: ', el('b', { text: chosen.map((m) => m.product).join(', ') })),
      left.length ? el('div', { text: `Left out: ${left.map((m) => m.product).join(', ')}.` }) : null,
      el('div', { text: 'The project then opens in the studio; it asks you to create the administrator first.' }));
  }
  if (step === 1) $('depot-name').focus();
  if (step === 2) $('meters').querySelector('input')?.focus();
  if (step === 3) $('setup-next').focus();
}

$('setup-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  if (busy) return;
  const depot = $('depot-name').value.trim();
  if (setup.step === 1) {
    if (!depot) { say($('setup-status'), 'Enter the depot name.', true); $('depot-name').focus(); return; }
    say($('setup-status'), '');
    go(2);
    return;
  }
  if (setup.step === 2) {
    if (!setup.chosen.size) { say($('setup-status'), 'Tick at least one meter.', true); return; }
    say($('setup-status'), '');
    go(3);
    return;
  }
  busy = true;
  $('setup-next').disabled = true;
  say($('setup-status'), 'Building the SCADA…');
  try {
    const meters = setup.template.meters.filter((m) => setup.chosen.has(m.key)).map((m) => m.key);
    const opened = await invoke('create_depot', { depot, meters, parent: setup.parent });
    say($('setup-status'), opened ? 'Starting the runtime…' : '');
  } catch (err) {
    say($('setup-status'), String(err), true);
  } finally {
    busy = false;
    $('setup-next').disabled = false;
  }
});

$('setup-back').addEventListener('click', () => go(Math.max(1, setup.step - 1)));
$('setup-cancel').addEventListener('click', async () => {
  if (setup.firstRun) await invoke('skip_setup');
  hideSetup();
});
$('change-where').addEventListener('click', async () => {
  const dir = await invoke('choose_folder');
  if (dir) {
    setup.parent = dir;
    go(3);
  }
});

refresh()
  .then((state) => { if (state.setup) showSetup({ firstRun: true }); })
  .catch((err) => say($('status'), String(err), true));
