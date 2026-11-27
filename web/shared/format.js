// Value formatting shared by the HMI and the studio.
//
// Movicon projects carry .NET numeric format strings ("#0.000000", "#,##0.0", "0%",
// "F2"). formatNumber() implements the subset that appears in real projects:
// sections (pos;neg;zero), 0/# placeholders, grouping, percent/per-mille,
// scientific notation, quoted literals and standard formats (F, N, D, P, E, G, X).

const NO_VALUE = '----';

/** Separators for a culture name such as "en-us" or "it-IT". Cached. */
const separatorCache = new Map();
export function separators(culture = 'en-US') {
  const key = culture || 'en-US';
  if (separatorCache.has(key)) return separatorCache.get(key);
  let decimal = '.';
  let group = ',';
  try {
    const parts = new Intl.NumberFormat(key).formatToParts(12345.6);
    decimal = parts.find((p) => p.type === 'decimal')?.value ?? '.';
    group = parts.find((p) => p.type === 'group')?.value ?? ',';
  } catch {
    // Unknown culture: keep invariant separators.
  }
  const out = { decimal, group };
  separatorCache.set(key, out);
  return out;
}

/**
 * Formats a number with a .NET-style format string.
 * @param {number} value
 * @param {string} [fmt] e.g. "#0.00", "F3", "#,##0", "0.0%"
 * @param {string} [culture] BCP-47 name; decides decimal and group separators
 */
export function formatNumber(value, fmt, culture = 'en-US') {
  if (value === null || value === undefined || value === '') return NO_VALUE;
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n)) return Number.isNaN(n) ? 'NaN' : n > 0 ? '∞' : '-∞';
  const sep = separators(culture);
  const f = (fmt ?? '').trim();
  if (!f) return general(n, undefined, sep);
  const std = /^([CcDdEeFfGgNnPpXx])(\d{0,2})$/.exec(f);
  if (std) return standardFormat(n, std[1], std[2] === '' ? undefined : Number(std[2]), sep);
  return customFormat(n, f, sep);
}

function standardFormat(n, kind, prec, sep) {
  switch (kind.toUpperCase()) {
    case 'F': return fixed(n, prec ?? 2, prec ?? 2, false, sep);
    case 'N': return fixed(n, prec ?? 2, prec ?? 2, true, sep);
    case 'C': return fixed(n, prec ?? 2, prec ?? 2, true, sep);
    case 'P': return `${fixed(n * 100, prec ?? 2, prec ?? 2, true, sep)} %`;
    case 'D': {
      const r = Math.round(n);
      const digits = String(Math.abs(r)).padStart(prec ?? 1, '0');
      return (r < 0 ? '-' : '') + digits;
    }
    case 'E': return exponential(n, prec ?? 6, prec ?? 6, kind, '+', 3, sep);
    case 'X': {
      const r = Math.round(n);
      const hex = (r < 0 ? r >>> 0 : r).toString(16).padStart(prec ?? 1, '0');
      return kind === 'X' ? hex.toUpperCase() : hex;
    }
    default: return general(n, prec, sep);
  }
}

function general(n, prec, sep) {
  const p = prec && prec > 0 ? prec : 15;
  let str = Number(n.toPrecision(p)).toString();
  if (/e/i.test(str)) str = n.toExponential(Math.min(p, 14)).replace(/\.?0+e/, 'E').replace('e', 'E');
  return str.replace('.', sep.decimal);
}

/** Rounds to `maxDec` decimals, keeps at least `minDec`, optional grouping. */
function fixed(n, minDec, maxDec, group, sep, minInt = 1) {
  let str = Math.abs(n).toFixed(Math.min(100, maxDec));
  let [int, frac = ''] = str.split('.');
  while (frac.length > minDec && frac.endsWith('0')) frac = frac.slice(0, -1);
  int = int.replace(/^0+/, '');
  if (int.length < minInt) int = int.padStart(minInt, '0');
  if (group && int.length > 3) int = int.replace(/\B(?=(\d{3})+(?!\d))/g, sep.group);
  const isZero = /^[0.,\s]*$/.test(int + frac) || Number(str) === 0;
  const body = frac ? `${int}${sep.decimal}${frac}` : int;
  return n < 0 && !isZero ? `-${body}` : body;
}

function exponential(n, minDec, maxDec, eChar, signMode, expDigits, sep) {
  if (n === 0) {
    return `${fixed(0, minDec, maxDec, false, sep)}${eChar}${signMode === '+' ? '+' : ''}${'0'.repeat(expDigits)}`;
  }
  const [mant, expStr] = Math.abs(n).toExponential(maxDec).split('e');
  const exp = Number(expStr);
  let m = fixed(Number(mant), minDec, maxDec, false, sep);
  if (n < 0) m = `-${m}`;
  const expSign = exp < 0 ? '-' : signMode === '+' ? '+' : '';
  return `${m}${eChar}${expSign}${String(Math.abs(exp)).padStart(expDigits, '0')}`;
}

/** Splits a custom format on ';' outside quotes. */
function splitSections(fmt) {
  const out = [];
  let cur = '';
  let quote = null;
  for (let i = 0; i < fmt.length; i += 1) {
    const ch = fmt[i];
    if (quote) {
      if (ch === quote) quote = null;
      cur += ch;
    } else if (ch === '\\' && i + 1 < fmt.length) {
      cur += ch + fmt[i + 1];
      i += 1;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      cur += ch;
    } else if (ch === ';') {
      out.push(cur);
      cur = '';
    } else cur += ch;
  }
  out.push(cur);
  return out;
}

/** Tokenises one section into literals and pattern characters. */
function tokenize(section) {
  const toks = [];
  for (let i = 0; i < section.length; i += 1) {
    const ch = section[i];
    if (ch === '"' || ch === "'") {
      const end = section.indexOf(ch, i + 1);
      const lit = end < 0 ? section.slice(i + 1) : section.slice(i + 1, end);
      toks.push({ lit });
      i = end < 0 ? section.length : end;
    } else if (ch === '\\' && i + 1 < section.length) {
      toks.push({ lit: section[i + 1] });
      i += 1;
    } else if ((ch === 'E' || ch === 'e') && /^[+-]?0/.test(section.slice(i + 1))) {
      const m = /^([+-]?)(0+)/.exec(section.slice(i + 1));
      toks.push({ exp: { ch, sign: m[1], digits: m[2].length } });
      i += m[0].length;
    } else if ('0#.,%‰'.includes(ch)) toks.push({ pat: ch });
    else toks.push({ lit: ch });
  }
  return toks;
}

function customFormat(n, fmt, sep) {
  const sections = splitSections(fmt);
  let section = sections[0];
  let value = n;
  let forceNoSign = false;
  if (n < 0 && sections.length > 1 && sections[1] !== '') {
    section = sections[1];
    value = -n;
    forceNoSign = true;
  } else if (n === 0 && sections.length > 2 && sections[2] !== '') {
    section = sections[2];
  }
  const toks = tokenize(section);
  const firstPat = toks.findIndex((t) => t.pat && '0#.'.includes(t.pat));
  let lastPat = -1;
  toks.forEach((t, i) => { if ((t.pat && '0#.,'.includes(t.pat)) || t.exp) lastPat = i; });
  if (firstPat < 0) {
    // No digit placeholders: the format is all literal (e.g. "ON").
    return toks.map((t) => t.lit ?? (t.pat === '%' || t.pat === '‰' ? t.pat : '')).join('');
  }
  const pats = toks.slice(firstPat, lastPat + 1);
  const prefix = toks.slice(0, firstPat);
  const suffix = toks.slice(lastPat + 1);
  const all = [...prefix, ...suffix];
  if (all.some((t) => t.pat === '%') || pats.some((t) => t.pat === '%')) value *= 100;
  if (all.some((t) => t.pat === '‰') || pats.some((t) => t.pat === '‰')) value *= 1000;

  const dot = pats.findIndex((t) => t.pat === '.');
  const exp = pats.find((t) => t.exp)?.exp;
  const intPats = (dot < 0 ? pats : pats.slice(0, dot)).filter((t) => t.pat);
  const fracPats = dot < 0 ? [] : pats.slice(dot + 1).filter((t) => t.pat === '0' || t.pat === '#');
  // Commas right before the decimal point (or at the end) scale by 1000 each.
  let scale = 0;
  for (let i = intPats.length - 1; i >= 0 && intPats[i].pat === ','; i -= 1) scale += 1;
  value /= 1000 ** scale;
  const digitInts = intPats.slice(0, intPats.length - scale);
  const grouping = digitInts.some((t) => t.pat === ',');
  const minInt = digitInts.filter((t) => t.pat === '0').length;
  const minDec = fracPats.filter((t) => t.pat === '0').length;
  const maxDec = fracPats.length;

  let body;
  if (exp) {
    body = exponential(value, minDec, maxDec, exp.ch, exp.sign, exp.digits, sep);
  } else {
    body = fixed(value, minDec, maxDec, grouping, sep, minInt);
    if (minInt === 0 && body.replace('-', '').startsWith('0' + sep.decimal)) body = body.replace('0' + sep.decimal, sep.decimal);
    if (minInt === 0 && /^-?0$/.test(body)) body = body.replace('0', '');
  }
  if (forceNoSign) body = body.replace(/^-/, '');
  const lit = (ts) => ts.map((t) => t.lit ?? (t.pat === '%' || t.pat === '‰' ? t.pat : '')).join('');
  // Literals inside the digit run (e.g. "0' 'kg") are kept after the number.
  const inner = pats.filter((t) => t.lit !== undefined).map((t) => t.lit).join('');
  return `${lit(prefix)}${body}${inner}${lit(suffix)}`;
}

/**
 * Formats any tag value for display.
 * @param {*} v tag value (null, bool, number, string)
 * @param {{format?:string, culture?:string, dataType?:string}} [opts]
 */
export function formatValue(v, opts = {}) {
  if (v === null || v === undefined) return NO_VALUE;
  if (typeof v === 'boolean') return v ? 'ON' : 'OFF';
  if (typeof v === 'number') {
    if (opts.dataType === 'bool') return v ? 'ON' : 'OFF';
    return formatNumber(v, opts.format, opts.culture);
  }
  if (typeof v === 'string') return v;
  return JSON.stringify(v);
}

/** Interprets a tag value as on/off the way the runtime does (non-zero numbers are on). */
export function truthy(v) {
  if (typeof v === 'boolean') return v;
  if (typeof v === 'number') return v !== 0 && !Number.isNaN(v);
  if (typeof v === 'string') {
    const t = v.trim().toLowerCase();
    if (['true', 'on', 'yes'].includes(t)) return true;
    const f = Number(t);
    return Number.isFinite(f) && f !== 0;
  }
  return false;
}

/** Numeric view of a value, or null. Booleans are 0/1. */
export function toNumber(v) {
  if (typeof v === 'number') return Number.isFinite(v) ? v : null;
  if (typeof v === 'boolean') return v ? 1 : 0;
  if (typeof v === 'string' && v.trim() !== '') {
    const f = Number(v);
    return Number.isFinite(f) ? f : null;
  }
  return null;
}

const INT_RANGES = {
  i8: [-128, 127], u8: [0, 255], i16: [-32768, 32767], u16: [0, 65535],
  i32: [-2147483648, 2147483647], u32: [0, 4294967295],
  i64: [Number.MIN_SAFE_INTEGER, Number.MAX_SAFE_INTEGER], u64: [0, Number.MAX_SAFE_INTEGER],
};

/**
 * Parses operator input (or a command's string value) into a JSON value of the tag's type.
 * Throws an Error with an operator-readable message on bad input.
 */
export function parseInputValue(text, dataType, culture = 'en-US') {
  const raw = String(text ?? '').trim();
  if (dataType === 'string') return String(text ?? '');
  if (dataType === 'bool') {
    const t = raw.toLowerCase();
    if (['1', 'true', 'on', 'yes'].includes(t)) return true;
    if (['0', 'false', 'off', 'no'].includes(t)) return false;
    throw new Error(`"${raw}" is not ON/OFF (use 1 or 0)`);
  }
  if (raw === '') throw new Error('enter a value');
  const sep = separators(culture);
  // Accept the culture's decimal separator as well as '.'; group separators are
  // not stripped because "1.5" vs "1,5" would then be ambiguous on mixed keyboards.
  const normalised = raw.replace(/\s/g, '').replace(sep.decimal, '.');
  const n = Number(normalised);
  if (!Number.isFinite(n)) throw new Error(`"${raw}" is not a number`);
  const range = INT_RANGES[dataType];
  if (range) {
    if (!Number.isInteger(n)) throw new Error(`${dataType} needs a whole number`);
    if (n < range[0] || n > range[1]) throw new Error(`${n} is outside ${range[0]}…${range[1]}`);
  }
  return n;
}

/** Human age of a timestamp difference in ms: "4 s", "3 min 12 s", "2 h 5 min", "3 d 4 h". */
export function formatAge(ms) {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return 'unknown';
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
  const hr = Math.floor(m / 60);
  if (hr < 24) return m % 60 ? `${hr} h ${m % 60} min` : `${hr} h`;
  const d = Math.floor(hr / 24);
  return hr % 24 ? `${d} d ${hr % 24} h` : `${d} d`;
}

const pad = (n, w = 2) => String(n).padStart(w, '0');

/** Local date-time "YYYY-MM-DD HH:mm:ss" (unambiguous on multilingual plants). */
export function formatDateTime(ts, { ms = false } = {}) {
  if (ts === null || ts === undefined || !Number.isFinite(ts)) return '';
  const d = new Date(ts);
  const base = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  return ms ? `${base}.${pad(d.getMilliseconds(), 3)}` : base;
}

export function formatTime(ts, { seconds = true } = {}) {
  if (!Number.isFinite(ts)) return '';
  const d = new Date(ts);
  return seconds ? `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` : `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/**
 * "Nice" axis scale: rounds [min,max] outward to tidy step sizes.
 * @returns {{min:number,max:number,step:number,ticks:number[]}}
 */
export function niceScale(min, max, maxTicks = 6) {
  if (!Number.isFinite(min) || !Number.isFinite(max)) return { min: 0, max: 1, step: 0.2, ticks: [0, 0.2, 0.4, 0.6, 0.8, 1] };
  if (min > max) [min, max] = [max, min];
  if (min === max) {
    const pad0 = Math.abs(min) * 0.1 || 1;
    min -= pad0;
    max += pad0;
  }
  const range = niceNum(max - min, false);
  const step = niceNum(range / Math.max(1, maxTicks - 1), true);
  const lo = Math.floor(min / step) * step;
  const hi = Math.ceil(max / step) * step;
  const ticks = [];
  const decimals = Math.max(0, -Math.floor(Math.log10(step)) + (niceNum(step, true) % 1 ? 1 : 0));
  for (let v = lo; v <= hi + step / 2; v += step) ticks.push(Number(v.toFixed(Math.min(12, decimals + 2))));
  return { min: lo, max: hi, step, ticks };
}

function niceNum(x, round) {
  const exp = Math.floor(Math.log10(x));
  const f = x / 10 ** exp;
  let nf;
  if (round) nf = f < 1.5 ? 1 : f < 3 ? 2 : f < 7 ? 5 : 10;
  else nf = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10;
  return nf * 10 ** exp;
}

/** Short tick label for an axis value given the step. */
export function formatTick(v, step) {
  const decimals = step >= 1 ? 0 : Math.min(6, Math.ceil(-Math.log10(step)));
  if (Math.abs(v) >= 1e6 || (Math.abs(v) > 0 && Math.abs(v) < 1e-4)) return v.toExponential(1);
  return v.toFixed(decimals);
}

export { NO_VALUE };
