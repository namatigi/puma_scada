// The screen name in the page's URL hash ("#Manifold", "#FM%2001").
//
// A bookmark or a typed URL may hold a malformed escape ("#100%", "#%E0");
// decodeURIComponent throws on those, which would leave the HMI without a
// screen. Such a hash is taken literally instead (it then names no screen and
// the navigator is shown).

/** @param {string} hash e.g. window.location.hash */
export function screenFromHash(hash) {
  const h = String(hash ?? '').replace(/^#/, '');
  try {
    return decodeURIComponent(h);
  } catch {
    return h;
  }
}
