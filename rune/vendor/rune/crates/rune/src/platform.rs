pub(crate) mod accent;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod activation;
pub(crate) mod app_icon;
#[cfg(target_os = "macos")]
pub(crate) mod display_link;
#[cfg(target_vendor = "apple")]
pub mod embed;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod layer_shell;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
pub(crate) mod main_queue;
#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) mod portal_appearance;
pub(crate) mod preferences;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod ready_signal;
pub(crate) mod reopen;
pub(crate) mod scroll_bars;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod web_math;
#[cfg(target_os = "windows")]
pub(crate) mod win32;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) mod x11;
