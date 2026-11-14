//! Native Metal pre-present extension. No CPU pixels or independently presented surfaces.
use std::{collections::HashMap, ffi::c_void, sync::{Mutex, OnceLock}};

/// Borrowed command buffer, drawable texture, and pixels per logical point. Called on the
/// rendering thread with no encoder open, before commit/present. Do not commit, wait, present,
/// or reenter Rune. The buffer uses unretained references: retain every resource you encode
/// (including the texture) through a completion handler. Never retain the buffer itself there.
pub type Encoder = unsafe extern "C" fn(*mut c_void, *mut c_void, f64);
fn hooks() -> &'static Mutex<HashMap<usize, Encoder>> {
    static HOOKS: OnceLock<Mutex<HashMap<usize, Encoder>>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}
/// Installs a hook for one live AppKit view. Only Rune's native Metal backend invokes it;
/// consumers must retain their ordinary rendering fallback until first invocation succeeds.
///
/// # Safety
/// `view` must identify a live Rune window view and `encode` must obey `Encoder`'s contract.
/// Callback code must remain loaded through window destruction. Surface destruction removes it.
pub unsafe fn install(view: usize, encode: Encoder) { hooks().lock().unwrap().insert(view, encode); }
pub(crate) fn get(view: usize) -> Option<Encoder> { hooks().lock().unwrap().get(&view).copied() }
pub(crate) fn remove(view: usize) { hooks().lock().unwrap().remove(&view); }