// What the signed-in user may do in the page, for widgets that act on the
// plant (value entry, alarm actions). The server checks every action anyway;
// this only keeps the controls honest. Without sign-in (studio canvas,
// ?mock) everything is allowed.
//
//   setAccess({ canOperate: () => role >= operator, onDenied: (what) => … });
//   if (!mayOperate('change a value')) return;

let canOperateFn = () => true;
let onDeniedFn = () => {};
const listeners = new Set();

export function setAccess({ canOperate, onDenied } = {}) {
  if (canOperate) canOperateFn = canOperate;
  if (onDenied) onDeniedFn = onDenied;
  for (const fn of listeners) fn();
}

/** Whether operating (writes, alarm actions) is allowed right now. */
export function canOperate() {
  return canOperateFn();
}

/** `true` when allowed; otherwise tells the user why (`what`: e.g. "acknowledge alarms") and answers `false`. */
export function mayOperate(what) {
  if (canOperateFn()) return true;
  onDeniedFn(what);
  return false;
}

/** Called when access changes (sign-in, sign-out, role change). Returns an unsubscribe function. */
export function onAccessChange(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}
