use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/platform/web/platform.js")]
extern "C" {
    #[wasm_bindgen(js_name = writeClipboard)]
    pub(crate) fn write_clipboard(text: &str, html: &str, png: &[u8]) -> js_sys::Promise;
    #[wasm_bindgen(js_name = readClipboard)]
    pub(crate) fn read_clipboard(rich: bool, image: bool, max_image: u32) -> js_sys::Promise;
    #[wasm_bindgen(catch, js_name = openLink)]
    pub(crate) fn open_link(url: &str) -> Result<(), JsValue>;
    /// `[absolute href, on this page's origin]`.
    #[wasm_bindgen(catch, js_name = resolveLink)]
    pub(crate) fn resolve_link(url: &str) -> Result<js_sys::Array, JsValue>;
    #[wasm_bindgen(catch, js_name = assignLink)]
    pub(crate) fn assign_link(href: &str) -> Result<(), JsValue>;
    /// Resolves to `[width, height, Uint8Array]`: straight sRGB RGBA, or `[0, 0, svg bytes]`.
    #[wasm_bindgen(js_name = fetchImage)]
    pub(crate) fn fetch_image(url: &str, max_side: u32) -> js_sys::Promise;
    /// Resolves to `[width, height, Uint8Array]`, straight sRGB RGBA the browser decoded from
    /// encoded image bytes (JPEG, WebP, AVIF, GIF's first frame).
    #[wasm_bindgen(js_name = decodeImage)]
    pub(crate) fn decode_image(bytes: &[u8], max_side: u32) -> js_sys::Promise;
    /// Resolves to the `Uint8Array` body of `url`, relative to the page.
    #[wasm_bindgen(js_name = fetchBytes)]
    pub(crate) fn fetch_bytes(url: &str) -> js_sys::Promise;
    /// `url` resolved against `base`.
    #[wasm_bindgen(js_name = resolveUrl)]
    pub(crate) fn resolve_url(url: &str, base: &str) -> String;
    #[wasm_bindgen(js_name = objectUrl)]
    pub(crate) fn object_url(bytes: &[u8], kind: &str) -> String;
    #[wasm_bindgen(js_name = revokeUrl)]
    pub(crate) fn revoke_url(url: &str);
    /// Sets the canvas's CSS `cursor`, or clears it when `css` is empty.
    #[wasm_bindgen(js_name = setCursor)]
    pub(crate) fn set_cursor(canvas: &web_sys::HtmlCanvasElement, css: &str);
    /// `navigator.userAgentData.platform` or `navigator.platform`; `None` without a navigator.
    #[wasm_bindgen(js_name = browserPlatform)]
    fn browser_platform_name() -> Option<String>;
    #[wasm_bindgen(js_name = touchPoints)]
    fn touch_points() -> u32;
    /// Resolves in a later task, letting the browser draw and handle input in between.
    #[wasm_bindgen(js_name = nextTask)]
    pub(crate) fn next_task() -> js_sys::Promise;
    pub(super) fn viewport(canvas: &web_sys::HtmlCanvasElement, callback: &js_sys::Function, embedded: bool) -> js_sys::Function;
}

/// The browser's platform name and its touch points, as `FontSmoothing`'s web default reads them.
pub(crate) fn browser_platform() -> Option<(String, u32)> {
    Some((browser_platform_name()?, touch_points()))
}
