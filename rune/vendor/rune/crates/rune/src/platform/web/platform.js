// Plain text, with HTML and a PNG image (a Uint8Array) beside it when there are some (`html`
// is '' and `png` empty otherwise). Empty text isn't written beside another form.
export async function writeClipboard(text, html, png) {
    const clipboard = navigator.clipboard;
    if (!clipboard) throw new Error('Clipboard requires HTTPS.');
    if ((!html && !png.length) || typeof ClipboardItem === 'undefined') return clipboard.writeText(text);
    const forms = {};
    if (text) forms['text/plain'] = new Blob([text], { type: 'text/plain' });
    if (html) forms['text/html'] = new Blob([html], { type: 'text/html' });
    if (png.length) forms['image/png'] = new Blob([png], { type: 'image/png' });
    return clipboard.write([new ClipboardItem(forms)]);
}

// `{ text, html, png }`, each undefined when the clipboard lacks it; `html` only when `rich`,
// and `png` (a Uint8Array of at most `maxImage` bytes) only when `image`. `read()` starts
// before the first await, inside the input handler; where it fails, plain `readText()` is the
// fallback.
export async function readClipboard(rich, image, maxImage) {
    const clipboard = navigator.clipboard;
    if (!clipboard) throw new Error('Clipboard requires HTTPS.');
    if (rich && clipboard.read) {
        try {
            let text, html, png;
            for (const item of await clipboard.read()) {
                if (text === undefined && item.types.includes('text/plain')) text = await (await item.getType('text/plain')).text();
                if (html === undefined && item.types.includes('text/html')) html = await (await item.getType('text/html')).text();
                if (image && png === undefined && item.types.includes('image/png')) png = await pngBytes(await item.getType('image/png'), maxImage);
            }
            return { text, html, png };
        } catch {}
    }
    return { text: await clipboard.readText() };
}

// A PNG blob's bytes, or undefined when it's larger than `max`.
export async function pngBytes(blob, max) {
    return blob && blob.size <= max ? new Uint8Array(await blob.arrayBuffer()) : undefined;
}

export function openLink(url) {
    const parsed = new URL(url);
    if (!['https:', 'http:', 'mailto:'].includes(parsed.protocol)) throw new Error('Unsupported link scheme.');
    window.open(parsed.href, '_blank', 'noopener,noreferrer');
}

// `url` resolved against the page, and whether it stays on the page's origin.
export function resolveLink(url) {
    const parsed = new URL(url, document.baseURI);
    if (!['https:', 'http:', 'mailto:'].includes(parsed.protocol)) throw new Error('Unsupported link scheme.');
    return [parsed.href, parsed.origin === location.origin];
}

export function assignLink(href) {
    location.assign(href);
}

// `url` (relative to the page) decoded by the browser's own codecs, which run off the main
// thread: `[width, height, rgba]` with straight-alpha sRGB RGBA bytes, upright by EXIF. SVG comes
// back as `[0, 0, bytes]` for Rune to rasterize. Images over `maxSide` a side are refused
// before their pixels are read. It resolves in a task of its own, so Rune's work on one image
// (its mips) never runs in the same long task as another's or a frame's.
export async function fetchImage(url, maxSide) {
    const reply = await readImage(url, maxSide);
    await nextTask();
    return reply;
}

async function readImage(url, maxSide) {
    const response = await fetch(new URL(url, document.baseURI));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const blob = await response.blob();
    const type = (response.headers.get('content-type') || blob.type).split(';')[0].trim().toLowerCase();
    const path = response.url ? new URL(response.url).pathname : '';
    if (type === 'image/svg+xml' || /\.svgz?$/i.test(path)) return [0, 0, new Uint8Array(await blob.arrayBuffer())];
    return blobRgba(blob, maxSide);
}

export async function decodeImage(bytes, maxSide) {
    const reply = await blobRgba(new Blob([bytes]), maxSide);
    await nextTask();
    return reply;
}

async function blobRgba(blob, maxSide) {
    const bitmap = await createImageBitmap(blob, { imageOrientation: 'from-image', colorSpaceConversion: 'default' });
    try {
        const { width, height } = bitmap;
        if (width > maxSide || height > maxSide) throw new Error(`${width}×${height} is over ${maxSide} pixels a side`);
        const canvas = typeof OffscreenCanvas === 'function' ? new OffscreenCanvas(width, height)
            : Object.assign(document.createElement('canvas'), { width, height });
        const context = canvas.getContext('2d', { willReadFrequently: true });
        context.drawImage(bitmap, 0, 0);
        return [width, height, new Uint8Array(context.getImageData(0, 0, width, height).data.buffer)];
    } finally {
        bitmap.close();
    }
}

export function objectUrl(bytes, type) {
    return URL.createObjectURL(new Blob([bytes], { type }));
}

export function revokeUrl(url) {
    URL.revokeObjectURL(url);
}

// Sets `canvas`'s CSS cursor, or clears it when `css` is empty. Safari reads `image-set()` in a
// cursor only with its `-webkit-` prefix.
export function setCursor(canvas, css) {
    if (!css) return void canvas.style.removeProperty('cursor');
    canvas.style.cursor = css;
    if (!canvas.style.cursor) canvas.style.cursor = `-webkit-${css}`;
}

export function browserPlatform() {
    const nav = globalThis.navigator;
    return nav ? (nav.userAgentData?.platform || nav.platform || '') : undefined;
}

export function touchPoints() {
    return globalThis.navigator?.maxTouchPoints || 0;
}

export function nextTask() {
    return globalThis.scheduler?.yield ? globalThis.scheduler.yield() : new Promise((resolve) => setTimeout(resolve, 0));
}

export async function fetchBytes(url) {
    const response = await fetch(new URL(url, document.baseURI));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}

export function resolveUrl(url, base) {
    return new URL(url, base).href;
}

export function viewport(canvas, callback, embedded) {
    const probe = document.createElement('div');
    probe.style.cssText = 'position:fixed;visibility:hidden;pointer-events:none;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)';
    document.body.append(probe);
    // The largest viewport: on iOS Safari `100lvh` is the viewport with the toolbar collapsed, so
    // a canvas or band sized from it doesn't change as the toolbar comes and goes mid-scroll.
    const largest = document.createElement('div');
    largest.style.cssText = 'position:fixed;top:0;left:0;width:0;height:100lvh;visibility:hidden;pointer-events:none';
    document.body.append(largest);
    const update = () => {
        if (!embedded) {
            const v = window.visualViewport;
            canvas.style.width = `${v?.width ?? innerWidth}px`;
            canvas.style.height = `${v?.height ?? innerHeight}px`;
            canvas.style.left = `${v?.offsetLeft ?? 0}px`;
            canvas.style.top = `${v?.offsetTop ?? 0}px`;
        }
        const s = getComputedStyle(probe);
        callback(JSON.stringify({ kind: 'viewport', h: largest.offsetHeight || innerHeight,
            insets: embedded ? [0, 0, 0, 0] : [s.paddingTop, s.paddingRight, s.paddingBottom, s.paddingLeft].map(parseFloat) }));
    };
    const motion = matchMedia('(prefers-reduced-motion: reduce)');
    const reduced = () => callback(JSON.stringify({ kind: 'motion', reduced: motion.matches }));
    const focus = () => callback(JSON.stringify({ kind: 'focus', focused: document.hasFocus() }));
    const visibility = () => callback(JSON.stringify({ kind: 'visibility', visible: !document.hidden }));
    let dprMedia = null;
    const watchDpr = () => {
        dprMedia?.removeEventListener('change', onDpr);
        dprMedia = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
        dprMedia.addEventListener('change', onDpr);
    };
    function onDpr() {
        watchDpr();
        callback(JSON.stringify({ kind: 'dpr', scale: window.devicePixelRatio || 1 }));
    }
    watchDpr();
    window.visualViewport?.addEventListener('resize', update);
    window.visualViewport?.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    motion.addEventListener('change', reduced);
    window.addEventListener('focus', focus);
    window.addEventListener('blur', focus);
    document.addEventListener('visibilitychange', visibility);
    // Mount is called while Rust is installing the shell; enter it only after installation.
    queueMicrotask(() => { update(); reduced(); focus(); visibility(); });
    return () => {
        dprMedia?.removeEventListener('change', onDpr);
        window.visualViewport?.removeEventListener('resize', update);
        window.visualViewport?.removeEventListener('scroll', update);
        window.removeEventListener('resize', update);
        motion.removeEventListener('change', reduced);
        window.removeEventListener('focus', focus);
        window.removeEventListener('blur', focus);
        document.removeEventListener('visibilitychange', visibility);
        probe.remove();
        largest.remove();
    };
}
