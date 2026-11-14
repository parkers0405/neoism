//! Fetches the full Inter faces once a page's first frame is up (see `text::full_faces`).

use std::sync::atomic::{AtomicBool, Ordering};

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The URL of the app's `app.js`, which a web build serves the faces beside.
    #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
    static MODULE_URL: String;
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the fetch, once per page; `wake` runs when the faces are in.
pub(super) fn fetch(wake: impl FnOnce() + 'static) {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let base = MODULE_URL.with(String::clone);
    let files: Vec<_> = crate::text::full_faces::FILES.iter().map(|file| {
        let url = super::platform::resolve_url(file, &base);
        (url.clone(), super::platform::fetch_bytes(&url))
    }).collect();
    wasm_bindgen_futures::spawn_local(async move {
        let mut faces = Vec::new();
        for (url, bytes) in files {
            match wasm_bindgen_futures::JsFuture::from(bytes).await {
                Ok(bytes) => faces.push(js_sys::Uint8Array::new(&bytes).to_vec()),
                Err(e) => {
                    let note = format!("rune: {url} did not load, so text outside Latin, Greek and Cyrillic draws in fallback fonts:");
                    web_sys::console::warn_2(&note.into(), &e);
                    return;
                }
            }
        }
        crate::text::full_faces::arrive(faces);
        wake();
    });
}
