// Command registry: one place for every action, used by the menu bar, the
// toolbar, the Ctrl+K palette, the Commands panel and keyboard shortcuts.

/** Normalises a KeyboardEvent to "Ctrl+Shift+K"-style text. */
export function keyOf(e) {
  const parts = [];
  if (e.ctrlKey || e.metaKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  let k = e.key;
  if (k === ' ') k = 'Space';
  else if (k.length === 1) k = k.toUpperCase();
  else if (k === 'Del') k = 'Delete';
  if (!['Control', 'Shift', 'Alt', 'Meta'].includes(k)) parts.push(k);
  return parts.join('+');
}

export function createCommands() {
  const map = new Map();
  const listeners = new Set();
  const api = {
    /**
     * @param {{id:string,label:string,group?:string,icon?:string,kbd?:string|string[],
     *   run:Function, enabled?:()=>boolean, checked?:()=>boolean, title?:string, hidden?:boolean}} cmd
     */
    register(cmd) {
      map.set(cmd.id, cmd);
      return cmd;
    },
    get: (id) => map.get(id),
    list: () => [...map.values()].filter((c) => !c.hidden),
    enabled(id) {
      const c = map.get(id);
      return Boolean(c) && (c.enabled ? c.enabled() : true);
    },
    checked(id) {
      const c = map.get(id);
      return Boolean(c?.checked?.());
    },
    run(id, ...args) {
      const c = map.get(id);
      if (!c || !api.enabled(id)) return false;
      Promise.resolve()
        .then(() => c.run(...args))
        .catch((err) => console.error(`[studio] command ${id} failed`, err));
      listeners.forEach((fn) => fn(id));
      return true;
    },
    /** First shortcut text of a command, for menus. */
    kbd(id) {
      const k = map.get(id)?.kbd;
      return Array.isArray(k) ? k[0] : k ?? '';
    },
    /** Finds the command bound to a key event. `inText` skips plain keys while typing. */
    forKey(e, { inText = false } = {}) {
      const key = keyOf(e);
      for (const c of map.values()) {
        const keys = Array.isArray(c.kbd) ? c.kbd : c.kbd ? [c.kbd] : [];
        if (!keys.includes(key)) continue;
        if (inText && !c.global && !key.startsWith('Ctrl+') && !/^F\d+$/.test(key)) continue;
        if (inText && c.textSafe === false) continue;
        return c;
      }
      return null;
    },
    onRun(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
  return api;
}

/** Fuzzy match: every query character in order. Returns a score (lower is better) or -1. */
export function fuzzyScore(text, query) {
  if (!query) return 0;
  const t = text.toLowerCase();
  const q = query.toLowerCase();
  const direct = t.indexOf(q);
  if (direct >= 0) return direct === 0 ? 0 : 1 + direct / 100;
  let ti = 0;
  let gaps = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return -1;
    gaps += found - ti;
    ti = found + 1;
  }
  return 10 + gaps;
}
