// Browser text services and semantic DOM accessibility mirror.
const coarse = () => matchMedia('(pointer: coarse)').matches;
const high = n => n >= 0xd800 && n <= 0xdbff;
const low = n => n >= 0xdc00 && n <= 0xdfff;
function boundary(text, at, forward) {
    if (at > 0 && at < text.length && high(text.charCodeAt(at - 1)) && low(text.charCodeAt(at))) at += forward ? 1 : -1;
    return at;
}

function replacement(old, next, composition, caret) {
    let start = 0;
    while (start < old.length && start < next.length && old[start] === next[start]) start++;
    start = boundary(old, start, false);
    let end = old.length, nextEnd = next.length;
    while (end > start && nextEnd > start && old[end - 1] === next[nextEnd - 1]) { end--; nextEnd--; }
    const adjusted = boundary(old, end, true) - end;
    end += adjusted; nextEnd += adjusted;
    if (composition && caret >= composition.length && next.slice(caret - composition.length, caret) === composition) {
        const from = caret - composition.length;
        const to = caret + old.length - next.length;
        if (from <= to && to <= old.length) {
            if (old === next) { start = from; end = to; nextEnd = caret; }
            else { start = Math.min(start, from); end = Math.max(end, to); nextEnd = end + next.length - old.length; }
        }
    }
    return { start, end, text: next.slice(start, nextEnd) };
}

function browserShortcut(event) {
    const chord = event.metaKey || event.ctrlKey;
    if (event.key === 'F5' || event.key === 'F12') return true;
    if (!chord) return false;
    const k = event.key.toLowerCase();
    // Leave copy ('c') un-prevented too so the browser fires its synchronous `copy` event.
    return ['c', 'r', 'l', 't', 'w', 'n', 'f', 'g', '=', '+', '-', '0'].includes(k)
        || (event.altKey && ['i', 'j', 'u'].includes(k));
}

// Keys that scroll a page when nothing in Rune has focus.
const SCROLL_KEYS = [' ', 'PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown'];

// What a page's mirror needs: focus rings on its links and buttons, and a selection highlight
// over its transparent text. The page can restyle both.
const PAGE_CSS = `[data-rune-a11y] :focus-visible{outline:2px solid var(--rune-focus,Highlight);outline-offset:2px;border-radius:4px}
[data-rune-a11y] ::selection{background:var(--rune-selection,rgba(80,150,255,.35));color:transparent}
[data-rune-a11y] a,[data-rune-a11y] button{color:transparent;-webkit-tap-highlight-color:transparent}
[data-rune-a11y] [data-rune-scroll-x]::-webkit-scrollbar{display:none}`;

export function mount(canvas, callback, embedded, fitHeight, page) {
    const textarea = document.createElement('textarea');
    textarea.setAttribute('aria-label', 'Text input');
    textarea.setAttribute('autocapitalize', 'sentences');
    const password = document.createElement('input');
    password.type = 'password';
    password.setAttribute('aria-label', 'Password input');
    password.autocomplete = 'off';
    password.setAttribute('autocorrect', 'off');
    password.setAttribute('autocapitalize', 'off');
    password.spellcheck = false;
    for (const field of [textarea, password]) {
        field.style.cssText = 'position:fixed;left:0;top:0;width:1px;height:1px;padding:0;border:0;opacity:0;font-size:16px;resize:none;pointer-events:none;';
        field.tabIndex = -1;
        field.inputMode = coarse() ? 'none' : 'text';
        document.body.append(field);
    }
    const a11y = document.createElement('div');
    a11y.setAttribute('data-rune-a11y', '');
    a11y.setAttribute('role', 'document');
    let band = null;
    if (page) {
        // The canvas is a band of the page, not a window fixed to the screen: clipped to the
        // page's own height, in the document's flow, so the browser's compositor scrolls the
        // pixels Rune drew rather than moving a fixed layer at a frame's delay.
        band = document.createElement('div');
        band.setAttribute('data-rune-band', '');
        band.style.cssText = 'position:absolute;left:0;top:0;width:100%;height:0;overflow:hidden;overflow:clip;pointer-events:none;';
        canvas.replaceWith(band);
        band.append(canvas);
        a11y.style.cssText = 'position:absolute;left:0;top:0;width:100%;height:0;overflow-x:clip;pointer-events:none;z-index:1;color:transparent;touch-action:pan-y pinch-zoom;';
        const style = document.createElement('style');
        style.textContent = PAGE_CSS;
        a11y.append(style);
        document.body.append(a11y);
    } else if (embedded && canvas.parentElement) {
        if (getComputedStyle(canvas.parentElement).position === 'static') canvas.parentElement.style.position = 'relative';
        a11y.style.cssText = 'position:absolute;inset:0;overflow:hidden;pointer-events:none;z-index:1;color:transparent;';
        canvas.after(a11y);
    } else {
        a11y.style.cssText = 'position:fixed;inset:0;overflow:hidden;pointer-events:none;z-index:1;color:transparent;';
        document.body.append(a11y);
    }
    const h = { canvas, textarea, password, a11y, a11yNodes: new Map(), field: textarea, key: '', base: 0, value: '',
        copyText: '', composing: false, composition: '', shift: false, listeners: [], syncing: false, disposed: false,
        embedded, fitHeight, page, focused: false, presented: false, swapped: false, touchDown: false,
        lastScrollAt: 0, staticTimer: 0, scrollEls: new Map(), band };
    const send = event => {
        const message = JSON.stringify(event);
        if (h.syncing) queueMicrotask(() => { if (!h.disposed) callback(message); });
        else if (!h.disposed) callback(message);
    };
    h.send = send;
    const listen = (target, name, handler, options) => {
        target.addEventListener(name, handler, options);
        h.listeners.push(() => target.removeEventListener(name, handler, options));
    };
    function changed(preedit = false) {
        const backward = textarea.selectionDirection === 'backward';
        send({ kind: preedit ? 'preedit' : 'edit', key: h.key, base: h.base, expected: h.value,
            ...replacement(h.value, textarea.value, preedit ? h.composition : '', textarea.selectionEnd),
            anchor: backward ? textarea.selectionEnd : textarea.selectionStart,
            focus: backward ? textarea.selectionStart : textarea.selectionEnd });
    }
    const key = (name, event = {}) => send({ kind: 'key', key: name, code: event.code ?? '', shift: !!event.shiftKey,
        ctrl: !!event.ctrlKey, alt: !!event.altKey, meta: !!event.metaKey, repeat: !!event.repeat });
    listen(textarea, 'input', event => {
        // No Rune field has the keyboard: nothing to edit, so the field stays empty.
        if (!h.key) { textarea.value = ''; return; }
        changed(h.composing || event.isComposing);
    });
    listen(textarea, 'compositionstart', () => { h.composing = true; h.composition = ''; });
    listen(textarea, 'compositionupdate', event => {
        h.composition = event.data ?? '';
        queueMicrotask(() => { if (!h.disposed && h.composing) changed(true); });
    });
    listen(textarea, 'compositionend', () => {
        changed();
        h.composing = false; h.composition = '';
        send({ kind: 'sync' });
    });
    listen(password, 'input', () => {
        const text = password.value;
        password.value = '';
        if (text) send({ kind: 'secure', key: h.key, text });
    });
    for (const field of [textarea, password]) {
        listen(field, 'paste', event => {
            // Only a paste into a Rune field is Rune's; the browser keeps the rest.
            if (!h.key) return;
            event.preventDefault();
            const data = event.clipboardData;
            h.image = field === textarea ? [...(data?.items ?? [])].find(i => i.kind === 'file' && i.type === 'image/png')?.getAsFile() ?? undefined : undefined;
            try {
                send({ kind: 'paste', text: data?.getData('text/plain') ?? '', html: field === textarea ? data?.getData('text/html') ?? '' : '', image: !!h.image });
            } finally {
                if (h.syncing) queueMicrotask(() => { h.image = undefined; });
                else h.image = undefined;
            }
        });
        listen(field, 'beforeinput', event => {
            if (event.inputType === 'insertLineBreak' || event.inputType === 'insertParagraph') {
                event.preventDefault();
                if (!h.composing && !event.isComposing) key('Enter', { shiftKey: h.shift, code: 'Enter' });
            } else if (field === password && event.inputType.startsWith('delete')) {
                event.preventDefault();
                const forward = event.inputType.endsWith('Forward');
                key(forward ? 'Delete' : 'Backspace',
                    { altKey: event.inputType.includes('Word'), code: forward ? 'Delete' : 'Backspace' });
            }
        });
        listen(field, 'keydown', event => {
            h.shift = event.shiftKey;
            if (event.isComposing || h.composing || event.keyCode === 229) return;
            const chord = event.metaKey || event.ctrlKey;
            const editing = ['Enter', 'Tab', 'Escape', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'];
            if (field === password) editing.push('Backspace', 'Delete');
            if (!chord && !editing.includes(event.key)) return;
            if (chord && event.key.toLowerCase() === 'v') return;
            event.preventDefault();
            key(event.key, event);
        });
        listen(field, 'keyup', event => { h.shift = event.shiftKey; });
        listen(field, 'blur', () => { h.shift = false; });
    }
    listen(textarea, 'select', () => {
        if (h.syncing || h.composing || document.activeElement !== textarea) return;
        const backward = textarea.selectionDirection === 'backward';
        send({ kind: 'selection', key: h.key, base: h.base, expected: h.value,
            anchor: backward ? textarea.selectionEnd : textarea.selectionStart,
            focus: backward ? textarea.selectionStart : textarea.selectionEnd });
    });
    // Synchronous copy support for read-only `El::selectable` selections (`Edit › Copy` / ⌘C).
    listen(document, 'copy', event => {
        const active = document.activeElement;
        if (!h.key && h.copyText && event.clipboardData && (active === canvas || active === document.body)) {
            event.preventDefault();
            event.clipboardData.setData('text/plain', h.copyText);
        }
    });
    listen(canvas, 'pointerdown', event => event.preventDefault(), { capture: true });
    listen(canvas, 'keydown', event => {
        if (page && !h.focused && SCROLL_KEYS.includes(event.key) && !event.metaKey && !event.ctrlKey) return;
        if (!browserShortcut(event)) event.preventDefault();
    });
    if (page) {
        const scrolled = () => { h.lastScrollAt = performance.now(); send({ kind: 'scroll', y: window.scrollY }); };
        listen(window, 'scroll', scrolled, { passive: true });
        // A finger down or a page still coasting moves the export's layout under the reader; the
        // swap to the live mirror waits for it (see `dropStatic`).
        listen(window, 'touchstart', () => { h.touchDown = true; deferStatic(h); }, { passive: true });
        listen(window, 'touchend', () => { h.touchDown = false; deferStatic(h); }, { passive: true });
        listen(window, 'touchcancel', () => { h.touchDown = false; deferStatic(h); }, { passive: true });
        // Mirrored text and links take the pointer from the canvas; hover still reaches Rune.
        listen(a11y, 'pointermove', event => {
            if (event.pointerType === 'mouse') send({ kind: 'hover', x: event.clientX, y: event.clientY });
        }, { passive: true });
        queueMicrotask(() => { if (!h.disposed) scrolled(); });
    }
    if (!fitHeight) {
        listen(canvas, 'wheel', event => event.preventDefault(), { passive: false });
        listen(document, 'gesturestart', event => event.preventDefault(), { passive: false });
    }
    queueMicrotask(() => {
        if (!h.disposed && !embedded && !page && document.activeElement === document.body) canvas.focus({ preventScroll: true });
    });
    return h;
}

export function sync(h, key, text, base, anchor, focus, secure) {
    if (h.composing && key === h.key) return;
    h.composing = false;
    h.syncing = true;
    const editing = document.activeElement === h.textarea || document.activeElement === h.password;
    h.key = key; h.base = base; h.value = text;
    h.password.value = '';
    h.field = secure ? h.password : h.textarea;
    if (!key) {
        h.textarea.value = '';
        if (editing) h.canvas.focus({ preventScroll: true });
        h.textarea.inputMode = h.password.inputMode = coarse() ? 'none' : 'text';
    } else {
        if (h.textarea.value !== text) h.textarea.value = text;
        if (!secure) {
            const start = Math.min(anchor, focus), end = Math.max(anchor, focus);
            const direction = anchor > focus ? 'backward' : 'forward';
            if (h.textarea.selectionStart !== start || h.textarea.selectionEnd !== end || h.textarea.selectionDirection !== direction) {
                h.textarea.setSelectionRange(start, end, direction);
            }
        }
        if (editing && document.activeElement !== h.field) activate(h);
    }
    h.syncing = false;
}

export function syncCopy(h, text) {
    h.copyText = text;
}

function webLink(url) {
    return url.startsWith('https://') || url.startsWith('http://') || url.startsWith('mailto:');
}

const TAGS = { heading: null, button: 'button', link: 'a', image: 'div', text: 'p', scroll_x: 'div' };

function family(name) {
    if (name === 'sans') return 'var(--rune-sans,sans-serif)';
    if (name === 'mono') return 'var(--rune-mono,monospace)';
    return `"${name.replace(/["\\]/g, '')}",var(--rune-sans,sans-serif)`;
}

// The mirror's first item element, after its stylesheet.
function firstItem(el) {
    let c = el.firstElementChild;
    while (c && c.tagName === 'STYLE') c = c.nextElementSibling;
    return c;
}

function runs(h, el, item) {
    for (const run of item.u) {
        if (!run.h) { el.append(document.createTextNode(run.t)); continue; }
        const a = document.createElement('a');
        a.textContent = run.t;
        a.style.cssText = `color:transparent;text-decoration:none;pointer-events:${h.page ? 'auto' : 'none'};`;
        if (webLink(run.h)) {
            a.href = run.h;
            a.target = '_blank';
            a.rel = 'noopener noreferrer';
        } else {
            a.href = '#';
            a.addEventListener('click', ev => {
                ev.preventDefault();
                h.send({ kind: 'a11y_link', path: run.p, link: run.i });
            });
        }
        el.append(a);
    }
}

function itemStyle(h, item, origin) {
    const [x, y, w, bh] = item.r;
    let css = `position:absolute;left:${x - origin[0]}px;top:${y - origin[1]}px;width:${w}px;height:${bh}px;margin:0;padding:0;border:0;background:transparent;color:transparent;`;
    if (item.k === 'group') return css + 'overflow:visible;list-style:none;pointer-events:none;';
    // A page's horizontal scroller: the browser scrolls it natively, so a flick inside a code
    // block scrolls the block at the compositor's rate while a vertical drag still scrolls the
    // page.
    if (item.k === 'scroll_x') return css + 'overflow-x:auto;overflow-y:hidden;pointer-events:auto;touch-action:pan-x pan-y pinch-zoom;scrollbar-width:none;';
    if (item.k === 'link') return css + 'display:block;overflow:hidden;text-decoration:none;cursor:pointer;pointer-events:auto;font:14px/1.25 sans-serif;';
    if (item.k === 'image' || item.k === 'button') return css + 'overflow:hidden;pointer-events:none;font:14px/1.25 sans-serif;';
    const fs = item.s || 14;
    if (!item.f) return css + `font:${fs}px/1.25 sans-serif;overflow:hidden;white-space:pre-wrap;pointer-events:none;`;
    const [fam, weight, lh, tracking, italic, align] = item.f;
    css += `font:${italic ? 'italic ' : ''}${weight} ${fs}px/${lh}px ${family(fam)};letter-spacing:${tracking}px;text-align:${align};white-space:pre-wrap;overflow-wrap:break-word;overflow:visible;`;
    return css + (item.e ? 'pointer-events:auto;user-select:text;-webkit-user-select:text;cursor:text;' : 'pointer-events:none;user-select:none;-webkit-user-select:none;');
}

export function syncA11y(h, json) {
    if (h.disposed) return;
    const items = JSON.parse(json);
    const seen = new Set();
    const els = [];
    const cursors = new Map([[h.a11y, firstItem(h.a11y)]]);
    for (const item of items) {
        const tag = item.k === 'group' ? item.g : item.k === 'heading' ? `h${item.l || 2}` : TAGS[item.k] ?? 'p';
        const key = `${tag}:${item.p}`;
        seen.add(key);
        let el = h.a11yNodes.get(key);
        if (!el) {
            el = document.createElement(tag);
            if (tag === 'button') {
                el.type = 'button';
                el.tabIndex = h.page ? 0 : -1;
                el.addEventListener('click', ev => {
                    ev.preventDefault();
                    h.send({ kind: 'a11y_press', path: item.p });
                });
            }
            if (item.k === 'image') el.setAttribute('role', 'img');
            if (item.k === 'scroll_x') {
                el.setAttribute('data-rune-scroll-x', '');
                // The browser scrolls this container itself; Rune draws the content where the
                // container says it is.
                el.addEventListener('scroll', () => {
                    const path = el._path;
                    if (path && !h.disposed) h.send({ kind: 'scroll_x', path, x: el.scrollLeft });
                }, { passive: true });
            }
            h.a11yNodes.set(key, el);
        }
        const parent = item.a === undefined ? null : items[item.a];
        const parentEl = item.a === undefined ? h.a11y : els[item.a];
        // Writing `cssText` invalidates the node's style and its layout: only do it when the
        // mirror's item actually moved or changed, so one animating element never rewrites a whole
        // page's worth of nodes mid-scroll.
        const css = itemStyle(h, item, parent ? parent.r : [0, 0]);
        if (el._css !== css) { el._css = css; el.style.cssText = css; }
        if (item.i) el.id = item.i; else el.removeAttribute('id');
        if (tag === 'button') {
            if (item.x === true || item.x === false) el.setAttribute('aria-expanded', String(item.x));
            else el.removeAttribute('aria-expanded');
        }
        if (item.k === 'group' || item.k === 'image') {
            if (item.t) el.setAttribute('aria-label', item.t); else el.removeAttribute('aria-label');
        } else if (item.k === 'link') {
            el.href = item.h;
            if (!h.page && webLink(item.h)) { el.target = '_blank'; el.rel = 'noopener noreferrer'; }
        }
        const container = item.k === 'group' || item.k === 'scroll_x';
        if (item.k === 'scroll_x' && el._path !== item.p) { el._path = item.p; h.scrollEls.set(item.p, el); }
        const sig = container || item.k === 'image' ? '' : item.u?.length ? JSON.stringify(item.u) : item.t ?? '';
        if (!container && el._sig !== sig) {
            el._sig = sig;
            el.replaceChildren();
            if (item.u?.length) runs(h, el, item);
            else if (item.k !== 'image') el.textContent = item.t ?? '';
        }
        els.push(el);
        if (container && !cursors.has(el)) cursors.set(el, el.firstElementChild);
        const cursor = cursors.get(parentEl) ?? null;
        if (cursor !== el) parentEl.insertBefore(el, cursor);
        else cursors.set(parentEl, cursor.nextElementSibling);
    }
    for (const [key, el] of h.a11yNodes) {
        if (!seen.has(key)) {
            if (el._path && h.scrollEls.get(el._path) === el) h.scrollEls.delete(el._path);
            el.remove();
            h.a11yNodes.delete(key);
        }
    }
}

// A page's height and whether a Rune control has the keyboard (scroll keys are the page's
// when none has).
export function syncPage(h, height, focused) {
    h.focused = focused;
    h.a11y.style.height = `${height}px`;
    // The clipper is as tall as the page, so the band's overscan past the document's end is
    // clipped instead of making the page scrollable past its foot.
    if (h.band && h.band.style.height !== `${height}px`) h.band.style.height = `${height}px`;
}

// A horizontal viewport Rune scrolled itself (a wheel, a key): move the mirror's container to
// match, so the two stay the same text. The container's own scroll event echoes back the same
// offset, which Rune ignores.
export function syncScrollX(h, path, x) {
    const el = h.scrollEls.get(path);
    if (el && Math.abs(el.scrollLeft - x) > 0.5) el.scrollLeft = x;
}

// Rune's first frame is on screen: the canvas announces `runepresent`, and a page swaps its
// static export (`[data-rune-static]`) for the live mirror, keeping the section that was at
// the top of the window where it was. The export is hidden, not removed: removing the
// element a browser counted as the largest contentful paint takes the metric away. It goes
// two frames later (the canvas covers it by then), as a phone may not have painted the page
// before Rune's first frame, and content never painted can't count at all.
export function presented(h) {
    if (h.disposed || h.presented) return;
    h.presented = true;
    h.canvas.dispatchEvent(new CustomEvent('runepresent', { bubbles: true }));
    if (h.page) requestAnimationFrame(() => requestAnimationFrame(() => { if (!h.disposed) dropStatic(h); }));
}

function dropStatic(h) {
    if (h.disposed || h.swapped) return;
    const parts = [...document.querySelectorAll('[data-rune-static]')];
    const exported = parts.find((el) => el.tagName !== 'STYLE');
    if (!exported) { h.swapped = true; return; }
    // A finger is down, or the page is still coasting: hiding the export would take it out of the
    // page's flow under a scroll that is under way, and iOS stops momentum dead when the scroll
    // position moves underneath it. The touchend or the next scroll arms the retry.
    if (h.touchDown) return;
    if (performance.now() - h.lastScrollAt < 120) { deferStatic(h); return; }
    h.swapped = true;
    let anchor = null, offset = 0;
    if (window.scrollY > 0) {
        for (const el of exported.querySelectorAll('[id]')) {
            const r = el.getBoundingClientRect();
            if (r.bottom > 0) { anchor = el.id; offset = r.top; break; }
        }
    }
    for (const el of parts) {
        if (el.tagName === 'STYLE') { el.remove(); continue; }
        // Out of the page's flow, find, focus and accessibility tree, its ids the mirror's.
        el.style.cssText = 'position:absolute;top:0;left:0;width:0;height:0;overflow:hidden;visibility:hidden;';
        el.inert = true;
        el.setAttribute('aria-hidden', 'true');
        el.removeAttribute('id');
        for (const named of el.querySelectorAll('[id]')) named.removeAttribute('id');
        el.removeAttribute('data-rune-static');
        el.setAttribute('data-rune-static-hidden', '');
    }
    const live = anchor && h.a11y.querySelector(`#${CSS.escape(anchor)}`);
    if (live) {
        const delta = live.getBoundingClientRect().top - offset;
        // The export's CSS and the live layout differ by a pixel or two here and there; taking the
        // page's scroll for that would stop a flick for nothing.
        if (Math.abs(delta) > 2) window.scrollTo({ top: window.scrollY + delta, behavior: 'instant' });
    }
}

// Swaps once the page has settled. Only a scroll or a touch arms it, so a still page doesn't poll.
function deferStatic(h) {
    if (h.staticTimer || h.swapped || !h.presented || h.disposed) return;
    h.staticTimer = setTimeout(() => { h.staticTimer = 0; if (!h.disposed) dropStatic(h); }, 150);
}

export function activate(h) {
    // A page's canvas is `tabindex="-1"`: focusing it on a tap would only drop the reader's text
    // selection or a link's focus. An input that took the keyboard still gets it.
    if (h.page && !h.key) return;
    const field = h.key ? h.field : h.canvas;
    if (h.key) field.inputMode = 'text';
    if (document.activeElement !== field) field.focus({ preventScroll: true });
}

export async function pastedImage(h, max) {
    const file = h.image;
    h.image = undefined;
    return file && file.size <= max ? new Uint8Array(await file.arrayBuffer()) : undefined;
}

export function destroy(h) {
    h.disposed = true;
    h.image = undefined;
    clearTimeout(h.staticTimer);
    for (const remove of h.listeners) remove();
    h.password.value = ''; h.textarea.value = ''; h.copyText = '';
    h.a11yNodes.clear();
    h.scrollEls.clear();
    h.a11y.remove();
    h.band?.remove();
    h.password.remove(); h.textarea.remove();
}
