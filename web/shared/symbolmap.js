// Resolution of a Symbol element's `symbol` reference to a library entry.
//
//   "ball-valve"                         -> library symbol, default params
//   "coriolis-meter?product=%23FF4D4D"   -> inline params (URL-encoded)
//   "movicon:Symbol952"                  -> looked up in symbol-map.json
//
// Per-instance overrides in the map ("<screen>/<element id>") come after the
// symbol-level mapping; inline params win over both.

import { SYMBOLS, parseSymbolRef } from './symbols.js';

export const EMPTY_SYMBOL_MAP = Object.freeze({ version: 1, symbols: {}, instances: {} });

/** Fetches a symbol map; returns the empty map (and logs) if it cannot be loaded. */
export async function loadSymbolMap(url = new URL('./symbol-map.json', import.meta.url).href) {
  try {
    const res = await fetch(url, { cache: 'no-cache' });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return normaliseSymbolMap(await res.json());
  } catch (err) {
    console.warn(`[moviclone] symbol map not loaded from ${url}: ${err.message}`);
    return EMPTY_SYMBOL_MAP;
  }
}

export function normaliseSymbolMap(raw) {
  return {
    version: raw?.version ?? 1,
    symbols: raw?.symbols && typeof raw.symbols === 'object' ? raw.symbols : {},
    instances: raw?.instances && typeof raw.instances === 'object' ? raw.instances : {},
  };
}

/**
 * @param {string|null|undefined} ref the element's `symbol`
 * @param {object} map a normalised symbol map
 * @param {{screen?:string, id?:string}} [where] used for per-instance overrides
 * @returns {{name:string, params:object, known:boolean, source:string, label?:string}}
 */
export function resolveSymbol(ref, map = EMPTY_SYMBOL_MAP, where = {}) {
  const { name: rawName, params: inline } = parseSymbolRef(ref ?? '');
  let name = rawName;
  let params = {};
  let source = 'library';
  let label;
  if (rawName.startsWith('movicon:') || !SYMBOLS[rawName]) {
    const entry = map.symbols?.[rawName];
    if (entry?.symbol) {
      name = entry.symbol;
      params = { ...(entry.params ?? {}) };
      source = 'map';
    } else {
      source = 'unmapped';
    }
  }
  const inst = where.screen && where.id ? map.instances?.[`${where.screen}/${where.id}`] : undefined;
  if (inst) {
    if (inst.symbol) name = inst.symbol;
    params = { ...params, ...(inst.params ?? {}) };
    label = inst.label;
    if (source === 'unmapped' && inst.symbol) source = 'map';
  }
  params = { ...params, ...inline };
  return { name, params, known: Boolean(SYMBOLS[name]), source, label };
}
