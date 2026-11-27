// Toast notifications. One stack per document, bottom centre, touch-dismissable.
//   toast('Saved');  toast('Write failed: …', { kind: 'error' });

import { h } from './svg.js';

let stack = null;

function ensureStack() {
  if (stack && document.body.contains(stack)) return stack;
  stack = h('div', { class: 'mc-toasts', role: 'status', 'aria-live': 'polite' });
  document.body.appendChild(stack);
  return stack;
}

/**
 * @param {string} message
 * @param {{kind?:'info'|'ok'|'warn'|'error', timeout?:number}} [opts]
 * @returns {() => void} dismiss
 */
export function toast(message, { kind = 'info', timeout = kind === 'error' ? 6000 : 3500 } = {}) {
  const host = ensureStack();
  const el = h('div', { class: `mc-toast mc-toast-${kind}` }, h('span', { class: 'mc-toast-dot' }), h('span', { text: message }));
  const dismiss = () => {
    el.classList.add('mc-toast-out');
    setTimeout(() => el.remove(), 250);
  };
  el.addEventListener('click', dismiss);
  host.appendChild(el);
  while (host.children.length > 5) host.firstChild.remove();
  if (timeout > 0) setTimeout(dismiss, timeout);
  return dismiss;
}
