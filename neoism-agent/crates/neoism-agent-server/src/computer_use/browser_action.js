(request) => {
  const fail = error => ({ok:false,error});
  const state = globalThis.__neoismComputer;
  if (!state || state.token !== request.token) return fail('Stale observation; observe again');
  if (document.visibilityState !== 'visible' || !document.hasFocus() || !/^https?:$/.test(location.protocol) || location.href !== request.url) return fail('Tab is unfocused, navigated or unsupported');
  const elementAction = ['click','fill','select'].includes(request.action);
  const record = elementAction ? state.refs.get(request.ref) : null;
  const el = record?.el;
  if (elementAction) {
    if (!el || !el.isConnected || el.ownerDocument !== document) return fail('Element detached; observe again');
    if (state.signature(el) !== record.signature) return fail('Element changed since observation; observe again');
    const box = el.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0 || getComputedStyle(el).visibility !== 'visible') return fail('Element is not visible');
    const x = Math.max(0, Math.min(innerWidth - 1, box.x + box.width / 2));
    const y = Math.max(0, Math.min(innerHeight - 1, box.y + box.height / 2));
    const top = document.elementFromPoint(x, y);
    if (!top || (top !== el && !el.contains(top))) return fail('Element is obscured; observe again');
    if (el.matches(':disabled') || el.getAttribute('aria-disabled') === 'true' || el.closest('[inert]')) return fail('Element disabled');
    if (el instanceof HTMLInputElement && ['password','file','hidden'].includes(el.type)) return fail('Sensitive/unsupported input; use deliberate desktop interaction');
  }
  // Consume before any effect: never allow this observation to replay an action.
  state.token = null;
  if (request.action === 'click') {
    el.click();
  } else if (request.action === 'fill') {
    if (!(el instanceof HTMLTextAreaElement) && !(el instanceof HTMLInputElement && ['text','search','email','url','tel','number'].includes(el.type))) return fail('Not a supported text field');
    if (el.readOnly) return fail('Field is read-only');
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    el.focus();
    setter.call(el, request.value);
    el.dispatchEvent(new InputEvent('input', {bubbles:true,inputType:'insertText',data:request.value}));
    el.dispatchEvent(new Event('change', {bubbles:true}));
  } else if (request.action === 'select') {
    if (!(el instanceof HTMLSelectElement) || el.multiple) return fail('Not a single select');
    const option = Array.from(el.options).find(o => o.value === request.value && !o.disabled && !o.parentElement?.disabled);
    if (!option) return fail('No enabled option with that value');
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(el, request.value);
    el.dispatchEvent(new Event('input', {bubbles:true}));
    el.dispatchEvent(new Event('change', {bubbles:true}));
  } else if (request.action === 'scroll') {
    if (!['up','down'].includes(request.value)) return fail('Scroll direction must be up or down');
    const root = document.scrollingElement || document.documentElement;
    const before = root.scrollTop;
    const delta = Math.max(120, Math.floor(innerHeight * 0.8)) * (request.value === 'down' ? 1 : -1);
    root.scrollBy({top:delta,left:0,behavior:'instant'});
    if (root.scrollTop === before) return fail('Page cannot scroll further in that direction');
  } else if (request.action === 'back') {
    if (history.length <= 1) return fail('No browser history entry is available');
    history.back();
  } else if (request.action === 'navigate') {
    let next;
    try { next = new URL(request.value); } catch { return fail('Invalid navigation URL'); }
    if (!/^https?:$/.test(next.protocol) || next.href !== request.value) return fail('Navigation requires an exact normalized HTTP(S) URL');
    location.assign(next.href);
  } else return fail('Unsupported action');
  return {ok:true};
}
