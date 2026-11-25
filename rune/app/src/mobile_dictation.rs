//! Text from Apple's Speech framework belongs to the draft that started recording.
use crate::app::App;
use std::{ffi::{CStr, c_char, c_void}, sync::{Arc, Mutex}};

type ResultSlot = Arc<Mutex<Option<Result<Option<String>, String>>>>;

pub(crate) struct Dictation { tab: u64, reply: ResultSlot }

unsafe extern "C" {
    fn neoism_mobile_dictate(context: *mut c_void, reply: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char));
    fn neoism_mobile_dictate_finish();
}

pub(crate) fn toggle(app: &mut App, tab: u64) {
    if app.mobile_dictation.is_some() {
        unsafe { neoism_mobile_dictate_finish() };
        return;
    }
    app.close_prompt_picker();
    app.tabs[app.active].draft.focused = false;
    let reply: ResultSlot = Default::default();
    let context = Box::new((reply.clone(), app.waker.clone()));
    app.mobile_dictation = Some(Dictation { tab, reply });
    unsafe { neoism_mobile_dictate(Box::into_raw(context).cast(), completed) };
}

unsafe extern "C" fn completed(context: *mut c_void, text: *const c_char, error: *const c_char) {
    let (slot, wake) = *unsafe { Box::from_raw(context.cast::<(ResultSlot, Option<rune::Waker>)>()) };
    let copy = |s| unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    if let Ok(mut slot) = slot.lock() {
        *slot = Some(if error.is_null() { Ok((!text.is_null()).then(|| copy(text))) } else { Err(copy(error)) });
    }
    if let Some(wake) = wake { wake(); }
}

pub(crate) fn poll(app: &mut App) -> bool {
    let Some(work) = &app.mobile_dictation else { return false };
    let Some(result) = work.reply.lock().unwrap().take() else { return false };
    let tab = work.tab;
    app.mobile_dictation = None;
    match result {
        Ok(Some(text)) if !text.trim().is_empty() => {
            if let Some(tab) = app.tabs.iter_mut().find(|t| t.id == tab) { tab.draft.paste(text.trim()); }
        }
        Err(error) => app.modal = Some(crate::ui::modal::Modal::notice("Dictation", &error)),
        _ => {},
    }
    true
}