pub(crate) mod canvas_input;
pub(crate) mod chord;
mod chord_label;
pub(crate) mod drop_pointer;
#[cfg(target_os = "macos")]
pub(crate) mod fn_key;
pub(crate) mod key_diagnostics;
pub(crate) mod keycaps;
pub(crate) mod physical_input;
#[cfg(any(test, target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
pub(crate) mod pointer_observer;
pub(crate) mod secure_input;
pub(crate) mod time;
