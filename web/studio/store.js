// The studio's working copy of the project, what was last saved, undo/redo per
// document part and the diagnostics. Documents never talk to the API directly;
// they edit through `store.edit(part, label, fn)` so every change is undoable,
// marks the part dirty and notifies the panels.
//
// Parts: "screen:<name>", "tags", "alarms", "meta".

import { clone } from './model.js';

const HISTORY_LIMIT = 300;

export function createEmitter() {
  const handlers = new Map();
  return {
    on(event, fn) {
      if (!handlers.has(event)) handlers.set(event, new Set());
      handlers.get(event).add(fn);
      return () => handlers.get(event)?.delete(fn);
    },
    emit(event, payload) {
      for (const fn of handlers.get(event) ?? []) {
        try {
          fn(payload);
        } catch (err) {
          console.error(`[studio] ${event} handler failed`, err);
        }
      }
    },
  };
}

export function createStore() {
  const bus = createEmitter();
  /** @type {object|null} */
  let project = null;
  let diagnostics = [];
  const saved = new Map(); // part -> JSON string as last loaded/saved
  // part -> version of its file on disk when last loaded/saved (null: the
  // server sent none, so saves are not checked).
  let versions = null;
  const history = new Map(); // part -> {undo: [], redo: []}
  let coalesce = null; // {part, key, until} for nudges and typing

  const screenIndex = (name) => (project?.screens ?? []).findIndex((s) => s.name === name);

  function getPart(part) {
    if (!project) return undefined;
    if (part === 'tags') return project.tags;
    if (part === 'alarms') return project.alarms;
    if (part === 'meta') return project.meta;
    if (part.startsWith('screen:')) return project.screens[screenIndex(part.slice(7))];
    return undefined;
  }

  function setPart(part, value) {
    if (part === 'tags') project.tags = value;
    else if (part === 'alarms') project.alarms = value;
    else if (part === 'meta') project.meta = value;
    else if (part.startsWith('screen:')) {
      const i = screenIndex(part.slice(7));
      if (i >= 0) project.screens[i] = value;
    }
  }

  const snapshot = (part) => JSON.stringify(getPart(part) ?? null);

  function parts() {
    const out = ['meta', 'tags', 'alarms'];
    for (const s of project?.screens ?? []) out.push(`screen:${s.name}`);
    return out;
  }

  function markSaved(part) {
    const v = getPart(part);
    if (v === undefined) saved.delete(part);
    else saved.set(part, JSON.stringify(v));
  }

  function hist(part) {
    if (!history.has(part)) history.set(part, { undo: [], redo: [] });
    return history.get(part);
  }

  const api = {
    on: bus.on,
    emit: bus.emit,
    get project() { return project; },
    get diagnostics() { return diagnostics; },

    load(p, diags = [], fileVersions = null) {
      project = p;
      versions = fileVersions ? new Map(Object.entries(fileVersions)) : null;
      project.tags ??= [];
      project.alarms ??= [];
      project.screens ??= [];
      project.drivers ??= [];
      project.data ??= {};
      saved.clear();
      history.clear();
      for (const part of parts()) markSaved(part);
      diagnostics = diags;
      bus.emit('loaded', project);
      bus.emit('diagnostics', diagnostics);
      bus.emit('dirty');
    },

    getPart,
    parts,

    /**
     * The version a save of `part` is based on: its file's version when loaded
     * or last saved, "absent" for a part that has no file yet, or undefined when
     * the server does not report versions.
     */
    version(part) {
      if (!versions) return undefined;
      return versions.get(part) ?? 'absent';
    },
    setVersion(part, v) {
      if (!versions) return;
      if (v) versions.set(part, v);
      else versions.delete(part);
    },
    /** Parts whose file version on disk differs from the one this window holds. */
    changedOnDisk(diskVersions) {
      if (!versions || !diskVersions) return [];
      const keys = new Set([...versions.keys(), ...Object.keys(diskVersions)]);
      return [...keys].filter((k) => (versions.get(k) ?? 'absent') !== (diskVersions[k] ?? 'absent'));
    },

    isDirty(part) {
      if (!saved.has(part)) return getPart(part) !== undefined;
      return saved.get(part) !== snapshot(part);
    },

    /** Parts that need saving, plus screens deleted since the last save. */
    pending() {
      const dirty = parts().filter((p) => api.isDirty(p));
      const live = new Set(parts());
      const deleted = [...saved.keys()].filter((p) => p.startsWith('screen:') && !live.has(p));
      return { dirty, deleted };
    },

    markSaved(part) {
      markSaved(part);
      bus.emit('dirty');
    },

    forgetSaved(part) {
      saved.delete(part);
      bus.emit('dirty');
    },

    /**
     * Applies `fn(value)` to a part as one undoable step. `fn` may mutate the
     * value in place or return a replacement. `opts.coalesce` merges repeated
     * edits with the same key (arrow nudges, typing) into one step.
     */
    edit(part, label, fn, opts = {}) {
      const current = getPart(part);
      if (current === undefined) return false;
      const before = JSON.stringify(current);
      const working = clone(current);
      const result = fn(working);
      const next = result === undefined ? working : result;
      const after = JSON.stringify(next);
      if (after === before) return false;
      const h = hist(part);
      const now = Date.now();
      const merge = opts.coalesce && coalesce && coalesce.part === part && coalesce.key === opts.coalesce && now < coalesce.until;
      if (!merge) {
        h.undo.push({ label, state: before });
        if (h.undo.length > HISTORY_LIMIT) h.undo.shift();
      }
      h.redo = [];
      coalesce = opts.coalesce ? { part, key: opts.coalesce, until: now + 800 } : null;
      setPart(part, next);
      bus.emit('changed', { part, label, source: opts.source });
      bus.emit('dirty');
      return true;
    },

    canUndo: (part) => (history.get(part)?.undo.length ?? 0) > 0,
    canRedo: (part) => (history.get(part)?.redo.length ?? 0) > 0,
    undoLabel: (part) => history.get(part)?.undo.at(-1)?.label ?? '',
    redoLabel: (part) => history.get(part)?.redo.at(-1)?.label ?? '',

    undo(part) {
      const h = history.get(part);
      const step = h?.undo.pop();
      if (!step) return null;
      h.redo.push({ label: step.label, state: snapshot(part) });
      setPart(part, JSON.parse(step.state));
      coalesce = null;
      bus.emit('changed', { part, label: `Undo ${step.label}`, undo: true });
      bus.emit('dirty');
      return step.label;
    },

    redo(part) {
      const h = history.get(part);
      const step = h?.redo.pop();
      if (!step) return null;
      h.undo.push({ label: step.label, state: snapshot(part) });
      setPart(part, JSON.parse(step.state));
      coalesce = null;
      bus.emit('changed', { part, label: `Redo ${step.label}`, redo: true });
      bus.emit('dirty');
      return step.label;
    },

    /** Structural changes (add / rename / delete screen) reset that part's history. */
    addScreen(screen) {
      project.screens.push(screen);
      bus.emit('structure', { added: screen.name });
      bus.emit('dirty');
    },

    removeScreen(name) {
      const i = screenIndex(name);
      if (i < 0) return;
      project.screens.splice(i, 1);
      history.delete(`screen:${name}`);
      bus.emit('structure', { removed: name });
      bus.emit('dirty');
    },

    renameScreen(from, to) {
      const s = getPart(`screen:${from}`);
      if (!s) return;
      s.name = to;
      history.delete(`screen:${from}`);
      if (project.meta.main_screen === from) project.meta.main_screen = to;
      bus.emit('structure', { renamed: { from, to } });
      bus.emit('dirty');
    },

    setDiagnostics(list) {
      diagnostics = list ?? [];
      bus.emit('diagnostics', diagnostics);
    },

    log(message, kind = 'info') {
      bus.emit('log', { ts: Date.now(), message, kind });
    },
  };
  return api;
}
