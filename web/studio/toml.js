// TOML text of model objects for the Source panel. Mirrors the layout the Rust
// side writes (key = value lines, inline arrays, [[table]] arrays, sub-tables
// last), so what engineers read here is what lands in the project files.

const BARE_KEY = /^[A-Za-z0-9_-]+$/;

export function tomlKey(k) {
  return BARE_KEY.test(k) ? k : tomlString(k);
}

export function tomlString(s) {
  const str = String(s);
  // Literal strings are friendlier for Windows paths, as the Rust writer does.
  if (str.includes('\\') && !str.includes("'") && !/[\n\r]/.test(str)) return `'${str}'`;
  return `"${str.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\r/g, '\\r').replace(/\t/g, '\\t')}"`;
}

export function tomlNumber(n) {
  if (!Number.isFinite(n)) return n > 0 ? 'inf' : n < 0 ? '-inf' : 'nan';
  return Number.isInteger(n) ? n.toFixed(1) : String(n);
}

const isTable = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const isTableArray = (v) => Array.isArray(v) && v.length > 0 && v.every(isTable);

/** Inline value. `ints` names keys whose numbers are integers in the model. */
export function tomlValue(v, key = '', ints = INT_KEYS) {
  if (typeof v === 'string') return tomlString(v);
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (typeof v === 'number') return ints.has(key) && Number.isInteger(v) ? String(v) : tomlNumber(v);
  if (Array.isArray(v)) return `[${v.map((x) => tomlValue(x, key, ints)).join(', ')}]`;
  if (isTable(v)) return `{ ${Object.entries(v).filter(([, x]) => x !== null && x !== undefined).map(([k, x]) => `${tomlKey(k)} = ${tomlValue(x, k, ints)}`).join(', ')} }`;
  return '""';
}

/** Keys that are integers in the Rust model (printed without ".0"). */
export const INT_KEYS = new Set([
  'register', 'unit_id', 'port', 'poll_ms', 'timeout_ms', 'max_retries', 'span_s', 'max_age_s',
  'fast_ms', 'slow_ms', 'publish_ms', 'every-s',
]);

/**
 * Body of one table: plain keys first, then sub-tables and table arrays.
 * @param {object} obj
 * @param {string} path dotted path of this table ("element")
 */
export function tomlTableBody(obj, path = '') {
  const plain = [];
  const nested = [];
  for (const [k, v] of Object.entries(obj)) {
    if (v === null || v === undefined) continue;
    if (isTableArray(v)) nested.push([k, v, 'array']);
    else if (isTable(v) && Object.keys(v).length) nested.push([k, v, 'table']);
    else plain.push(`${tomlKey(k)} = ${tomlValue(v, k)}`);
  }
  const out = [plain.join('\n')];
  for (const [k, v, how] of nested) {
    const p = path ? `${path}.${tomlKey(k)}` : tomlKey(k);
    if (how === 'table') out.push(`\n[${p}]\n${tomlTableBody(v, p)}`);
    else for (const item of v) out.push(`\n[[${p}]]\n${tomlTableBody(item, p)}`);
  }
  return out.filter((x) => x !== '').join('\n');
}

/** `[[name]]` blocks for a list of objects (e.g. "element", "tag", "alarm"). */
export function tomlArrayOfTables(name, items) {
  return items.map((it) => `[[${name}]]\n${tomlTableBody(it, name)}`).join('\n\n');
}

/** Element in the field order the Rust model writes: id, kind fields, x, y, w, h, rotation, on_click. */
export function orderElement(el) {
  const { id, kind, x, y, w, h, rotation, on_click: onClick, ...rest } = el;
  const out = { id, kind, ...rest, x, y, w, h };
  if (rotation) out.rotation = rotation;
  if (onClick?.length) out.on_click = onClick;
  return out;
}

/** Screen header (without elements). */
export function screenHeaderToml(screen) {
  const { element, ...head } = screen;
  return tomlTableBody(head);
}

export function elementsToml(els) {
  return tomlArrayOfTables('element', els.map(orderElement));
}
