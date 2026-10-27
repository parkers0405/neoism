// Fixed isolated-world code. Never interpolate page content or model text as code.
(token) => {
  if (!/^https?:$/.test(location.protocol)) throw new Error('Only HTTP(S) page observations are supported');
  if (document.visibilityState !== 'visible' || !document.hasFocus()) throw new Error('Tab content is not focused; select it deliberately and observe again');
  const previous = globalThis.__neoismComputer;
  const state = previous || { ids: new WeakMap(), next: 0 };
  state.token = token;
  state.refs = new Map();
  globalThis.__neoismComputer = state;
  const trim = (text, max = 240) => String(text || '').replace(/\s+/g, ' ').trim().slice(0, max);
  // Walk open shadow trees without unbounding the scan on component-heavy pages.
  function* descendants(root, type) {
    const walkers = [document.createTreeWalker(root, type | NodeFilter.SHOW_ELEMENT)];
    let scanned = 0;
    while (walkers.length && scanned < 3000) {
      const node = walkers[walkers.length - 1].nextNode();
      if (!node) { walkers.pop(); continue; }
      scanned++;
      if (node.nodeType === Node.ELEMENT_NODE && node.shadowRoot) {
        walkers.push(document.createTreeWalker(node.shadowRoot, type | NodeFilter.SHOW_ELEMENT));
      }
      if (node.nodeType === Node.ELEMENT_NODE ? type & NodeFilter.SHOW_ELEMENT : type & NodeFilter.SHOW_TEXT) yield node;
    }
    if (walkers.length) scanTruncated = true;
  }
  let scanTruncated = false;
  const labelledText = el => {
    const root = el.getRootNode();
    return (el.getAttribute('aria-labelledby') || '').split(/\s+/).filter(Boolean).slice(0, 8)
      .map(id => root.getElementById?.(id)?.textContent?.slice(0, 240) || '').join(' ');
  };
  const visible = el => {
    for (let ancestor = el; ancestor; ancestor = ancestor.parentElement || ancestor.getRootNode().host) {
      if (ancestor.matches('[hidden],[aria-hidden="true"],[inert]')) return false;
    }
    const box = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return box.width > 0 && box.height > 0 && box.bottom > 0 && box.right > 0 &&
      box.top < innerHeight && box.left < innerWidth && style.visibility === 'visible' && style.display !== 'none';
  };
  const name = el => {
    return trim(labelledText(el) ||
      el.getAttribute('aria-label') || Array.from(el.labels || []).map(l => l.textContent).join(' ') ||
      el.getAttribute('alt') || el.getAttribute('placeholder') ||
      (el instanceof HTMLInputElement && ['button','submit','reset'].includes(el.type) ? el.value : '') ||
      el.innerText || el.getAttribute('title'));
  };
  const interactiveRoles = new Set(['button','link','checkbox','radio','switch','tab','menuitem','menuitemcheckbox','menuitemradio','option','combobox','textbox','searchbox','spinbutton','slider']);
  const excludedRoles = new Set(['heading','presentation','none','group','region','main','article','document','list','listitem','table','row','cell']);
  const actions = el => {
    const role = (el.getAttribute('role') || '').toLowerCase();
    if (excludedRoles.has(role)) return [];
    if (el instanceof HTMLSelectElement) return ['click','select'];
    if (el instanceof HTMLInputElement && el.type === 'checkbox') return ['click','check','uncheck'];
    if (el instanceof HTMLInputElement && el.type === 'radio') return ['click','check'];
    if (el instanceof HTMLTextAreaElement || el.isContentEditable ||
        (el instanceof HTMLInputElement && ['text','search','email','url','tel','number'].includes(el.type))) return ['click','fill'];
    if (el.matches('a[href],button,summary,input[type="button"],input[type="submit"],input[type="reset"],input[type="image"],input[type="checkbox"],input[type="radio"]') || interactiveRoles.has(role)) return ['click'];
    return [];
  };
  const nearby = el => {
    const container = el.closest('label,li,form,nav,section,article,dialog,[role="dialog"],[role="menu"]');
    const context = trim(container?.innerText, 360);
    const own = name(el);
    return context && context !== own ? context : '';
  };
  state.signature = el => JSON.stringify([
    el.tagName, el.getAttribute('type'), el.getAttribute('href'), el.getAttribute('aria-label'),
    el.innerText?.slice(0,240), el.getAttribute('role'), el.getAttribute('placeholder'),
    el.getAttribute('alt'), el.getAttribute('title'),
    Array.from(el.labels || []).map(l=>l.textContent?.slice(0,240)),
    labelledText(el),
    el instanceof HTMLInputElement && ['button','submit','reset'].includes(el.type) ? el.value : null,
  ]);
  const elements = [];
  let scanned = 0;
  for (const el of descendants(document.body || document.documentElement, NodeFilter.SHOW_ELEMENT)) {
    scanned++;
    if (elements.length >= 120) { scanTruncated = true; break; }
    if (!el.matches('a[href],button,input,textarea,select,summary,[role],[contenteditable="true"],[tabindex]') || !visible(el)) continue;
    if (el.matches('input[type="hidden"],input[type="password"],input[type="file"]')) continue;
    const availableActions = actions(el);
    if (!availableActions.length) continue;
    let ref = state.ids.get(el);
    if (!ref) { ref = `e${++state.next}`; state.ids.set(el, ref); }
    const label = name(el);
    const role = el.getAttribute('role') || ({A:'link',BUTTON:'button',TEXTAREA:'textbox',SELECT:'combobox',INPUT:['button','submit','reset','image'].includes(el.type) ? 'button' : el.type === 'checkbox' ? 'checkbox' : el.type === 'radio' ? 'radio' : 'textbox'}[el.tagName]) || el.tagName.toLowerCase();
    const item = {ref, role, name: label, actions:availableActions, actionable:true, disabled: el.matches(':disabled') || el.getAttribute('aria-disabled') === 'true'};
    if (el instanceof HTMLAnchorElement && el.href) item.href = el.href.slice(0,2048);
    const context = nearby(el);
    if (context) item.nearby = context;
    if (el instanceof HTMLTextAreaElement || (el instanceof HTMLInputElement && ['text','search','email','url','tel','number'].includes(el.type))) {
      item.value = el.value.slice(0,512);
      item.readOnly = el.readOnly;
    }
    if (el.isContentEditable) item.value = (el.textContent || '').slice(0,512);
    if (el instanceof HTMLInputElement && ['checkbox','radio'].includes(el.type)) item.checked = el.checked;
    if (el instanceof HTMLSelectElement) item.options = Array.from(el.options).slice(0, 50).map(o => ({value: o.value.slice(0,240),name: trim(o.label),selected:o.selected,disabled:o.disabled || !!o.parentElement?.disabled}));
    elements.push(item);
    state.refs.set(ref, {el, name:label, role, signature:state.signature(el)});
  }
  // Avoid serializing an entire large DOM or hidden script/style text.
  let text = '', count = 0;
  for (const node of descendants(document.body || document.documentElement, NodeFilter.SHOW_TEXT)) {
    count++;
    if (text.length >= 12000) { scanTruncated = true; break; }
    const parent = node.parentElement;
    if (!parent || parent.closest('script,style,noscript,textarea,[hidden],[aria-hidden="true"]') || !visible(parent)) continue;
    const value = trim(node.textContent, 1000);
    if (value) text += value + '\n';
  }
  const root = document.scrollingElement || document.documentElement;
  return {url:location.href, title:document.title.slice(0,300), visible:document.visibilityState === 'visible', focused:document.hasFocus(),
    readyState:document.readyState, text:text.slice(0,12000), elements,
    canScrollUp:root.scrollTop > 1, canScrollDown:root.scrollTop + innerHeight < root.scrollHeight - 1, historyLength:Math.min(history.length,1000),
    truncated:scanTruncated || scanned >= 3000 || elements.length >= 120 || count >= 3000 || text.length >= 12000,
    iframeCount:document.querySelectorAll('iframe,frame').length};
}
