//! Native accessibility and appearance preferences, sampled when a host is created or focused
//! (appearance also on each redraw, which a change asks for).
//! Headless runtimes use their explicit Env instead.

#[cfg(target_os = "macos")]
pub(crate) fn reduced_motion() -> bool {
    use objc2::{class, msg_send};
    // SAFETY: NSWorkspace's shared instance lives for the process; these selectors return
    // an object pointer and Objective-C BOOL respectively. Native hosts call on the UI thread.
    unsafe {
        let workspace: *mut objc2::runtime::AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let reduced: objc2::runtime::Bool = msg_send![workspace, accessibilityDisplayShouldReduceMotion];
        reduced.as_bool()
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    let mut animations: i32 = 1;
    // SAFETY: this SPI writes one BOOL to a valid, aligned local variable.
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut animations as *mut i32).cast(), 0) != 0 && animations == 0 }
}

#[cfg(target_os = "linux")]
pub(crate) fn reduced_motion() -> bool {
    // KDE exposes animation duration in kdeglobals; zero disables animation.
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")));
    if std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').any(|d| d.eq_ignore_ascii_case("KDE"))
        && let Some(config) = config
        && let Ok(contents) = std::fs::read_to_string(config.join("kdeglobals"))
        && let Some(factor) = contents.lines().filter_map(|line| line.trim().split_once('='))
            .find_map(|(k, v)| (k.trim() == "AnimationDurationFactor").then(|| v.trim().parse::<f64>().ok()).flatten())
    { return factor == 0.0; }
    // Optional GNOME GSettings, with schema lookup before constructing settings (missing
    // schemas otherwise abort GLib). No subprocess or required GNOME dependency.
    unsafe { gnome_animations().is_some_and(|enabled| !enabled) }
}

#[cfg(target_os = "linux")]
unsafe fn gnome_animations() -> Option<bool> {
    use std::ffi::{c_char, c_void};
    type Ptr = *mut c_void;
    // SAFETY: signatures match GIO/GLib's public C API; every reference is released while
    // the library is loaded. Null source/schema/settings are handled before use.
    unsafe {
        static GIO: std::sync::OnceLock<Option<libloading::Library>> = std::sync::OnceLock::new();
        let gio = GIO.get_or_init(|| libloading::Library::new("libgio-2.0.so.0").ok()).as_ref()?;
        let default = gio.get::<unsafe extern "C" fn() -> Ptr>(b"g_settings_schema_source_get_default\0").ok()?;
        let lookup = gio.get::<unsafe extern "C" fn(Ptr, *const c_char, i32) -> Ptr>(b"g_settings_schema_source_lookup\0").ok()?;
        let has_key = gio.get::<unsafe extern "C" fn(Ptr, *const c_char) -> i32>(b"g_settings_schema_has_key\0").ok()?;
        let new = gio.get::<unsafe extern "C" fn(Ptr, Ptr, *const c_char) -> Ptr>(b"g_settings_new_full\0").ok()?;
        let get = gio.get::<unsafe extern "C" fn(Ptr, *const c_char) -> i32>(b"g_settings_get_boolean\0").ok()?;
        let schema_unref = gio.get::<unsafe extern "C" fn(Ptr)>(b"g_settings_schema_unref\0").ok()?;
        let unref = gio.get::<unsafe extern "C" fn(Ptr)>(b"g_object_unref\0").ok()?;
        let source = default();
        if source.is_null() { return None; }
        let schema = lookup(source, c"org.gnome.desktop.interface".as_ptr(), 1);
        if schema.is_null() { return None; }
        if has_key(schema, c"enable-animations".as_ptr()) == 0 { schema_unref(schema); return None; }
        let settings = new(schema, std::ptr::null_mut(), std::ptr::null());
        schema_unref(schema);
        if settings.is_null() { return None; }
        let enabled = get(settings, c"enable-animations".as_ptr()) != 0;
        unref(settings);
        // The static library stays loaded for GLib's retained backend callbacks.
        Some(enabled)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub(crate) fn reduced_motion() -> bool { false }

/// The desktop's light or dark preference where winit reports none (Linux: the XDG desktop
/// portal's `color-scheme`). None leaves the host's own default.
#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) fn dark() -> Option<bool> { crate::platform::portal_appearance::dark() }

#[cfg(not(all(target_os = "linux", feature = "dbus")))]
pub(crate) fn dark() -> Option<bool> { None }

/// [`dark`] for a window winit gives no theme; None leaves the window's appearance as it is.
pub(crate) fn desktop_dark(window: &winit::window::Window) -> Option<bool> {
    dark().filter(|_| window.theme().is_none())
}

/// A new window's appearance: `--theme` when given, else winit's theme, else [`desktop_dark`],
/// else dark. On Linux a change of the desktop's preference redraws the window, which rereads it.
pub(crate) fn window_dark(window: &std::sync::Arc<winit::window::Window>, theme: Option<&str>) -> bool {
    #[cfg(all(target_os = "linux", feature = "dbus"))]
    crate::platform::portal_appearance::follow(window);
    match theme {
        Some("light") => false,
        Some("dark") => true,
        _ => window.theme().map_or_else(|| desktop_dark(window).unwrap_or(true), |t| t == winit::window::Theme::Dark),
    }
}

/// macOS "Keyboard navigation" (Full Keyboard Access): when off, Tab visits only text fields
/// and lists, as in AppKit. Windows and Linux always Tab through every control.
#[cfg(target_os = "macos")]
pub(crate) fn full_keyboard_access() -> bool {
    use objc2::{class, msg_send};
    // SAFETY: NSApplication's shared instance lives for the process; the selector returns an
    // Objective-C BOOL. Native hosts call on the UI thread.
    unsafe {
        let app: *mut objc2::runtime::AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let enabled: objc2::runtime::Bool = msg_send![app, isFullKeyboardAccessEnabled];
        enabled.as_bool()
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn full_keyboard_access() -> bool { true }
