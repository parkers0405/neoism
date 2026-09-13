//! Native platform adapters. Linux deliberately has no XWayland/CLI fallback.
use super::Display;
use anyhow::{ensure, Context};
#[cfg(not(target_os = "linux"))]
use enigo::Enigo;
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
use enigo::Mouse;

#[cfg(target_os = "linux")]
pub const NAME: &str = "wayland (libwayshot + virtual keyboard/pointer)";
#[cfg(target_os = "linux")]
pub const REQUIREMENTS: &str = "Run on the local Wayland desktop (Omarchy/Hyprland). Requires screencopy/image-copy, xdg-output, virtual-keyboard and wlr-virtual-pointer protocols, plus libwayland-client and libxkbcommon. No grim, ydotool, root or XWayland fallback. Screenshots support multiple outputs; pointer actions currently require one output.";
#[cfg(target_os = "macos")]
pub const NAME: &str = "macOS (CoreGraphics capture and CGEvent input)";
#[cfg(target_os = "macos")]
pub const REQUIREMENTS: &str = "Grant Screen Recording and Accessibility to the process hosting Neoism Agent in System Settings. Run in the logged-in graphical session. Secure/password fields and protected content may reject input/capture.";
#[cfg(target_os = "windows")]
pub const NAME: &str = "Windows (native capture + SendInput/SetCursorPos)";
#[cfg(target_os = "windows")]
pub const REQUIREMENTS: &str = "Run in the interactive user desktop. UIPI forbids controlling higher-integrity applications; UAC/secure desktop and protected content are unsupported. No elevation is attempted.";
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub const NAME: &str = "unsupported";
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub const REQUIREMENTS: &str = "This OS has no computer-use backend";

#[cfg(target_os = "linux")]
pub fn displays() -> anyhow::Result<Vec<Display>> {
    let connection = libwayshot::WayshotConnection::new().context("Cannot connect to Wayland capture protocols")?;
    let outputs = connection.get_all_outputs();
    ensure!(!outputs.is_empty(), "No Wayland outputs");
    Ok(outputs.iter().map(|o| Display { id:o.name.clone(),x:o.logical_position().x,y:o.logical_position().y,width:o.logical_size().width,height:o.logical_size().height }).collect())
}
#[cfg(target_os = "linux")]
pub fn capture(display: &Display) -> anyhow::Result<image_rs::DynamicImage> {
    let connection = libwayshot::WayshotConnection::new()?;
    let output = connection.get_all_outputs().iter().find(|o| o.name == display.id).context("Display disappeared")?;
    ensure!(output.logical_position().x == display.x && output.logical_position().y == display.y && output.logical_size().width == display.width && output.logical_size().height == display.height, "Display geometry changed before capture; retry capabilities");
    // Bound the native allocation, not only the later thumbnail.
    ensure!(u64::from(output.physical_size.width) * u64::from(output.physical_size.height) <= 64_000_000, "Physical display exceeds capture limit");
    // The single-output shortcut returns an untransformed framebuffer. The
    // output compositor path applies rotation/flips and normalizes its origin.
    ensure!(display.width > 0 && display.height > 0, "Invalid output dimensions");
    let scale = (f64::from(output.physical_size.height) / f64::from(display.height)).max(1.0);
    ensure!(f64::from(display.width) * f64::from(display.height) * scale * scale <= 64_000_000.0, "Composited display exceeds capture limit");
    Ok(connection.screenshot_outputs(std::slice::from_ref(output), false)?)
}
// Linux pointer scaling is owned by linux_pointer, not Enigo.
// Pointer-only Linux implementation deliberately has no Enigo adapter.


#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn displays() -> anyhow::Result<Vec<Display>> {
    #[cfg(target_os = "windows")] let _dpi = DpiGuard::new()?;
    xcap::Monitor::all()?.into_iter().map(|m| Ok(Display { id:m.id()?.to_string(),x:m.x()?,y:m.y()?,width:m.width()?,height:m.height()? })).collect()
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn capture(display: &Display) -> anyhow::Result<image_rs::DynamicImage> {
    #[cfg(target_os = "macos")]
    ensure!(screen_capture_allowed(), "Screen Recording permission is not granted to the Neoism Agent host process");
    #[cfg(target_os = "windows")] let _dpi = DpiGuard::new()?;
    let monitor = xcap::Monitor::all()?.into_iter().find(|m| m.id().is_ok_and(|id| id.to_string() == display.id)).context("Display disappeared")?;
    ensure!(monitor.x()? == display.x && monitor.y()? == display.y && monitor.width()? == display.width && monitor.height()? == display.height, "Display geometry changed before capture; retry capabilities");
    let scale = f64::from(monitor.scale_factor()?);
    ensure!(scale.is_finite() && scale > 0.0 && f64::from(display.width) * f64::from(display.height) * scale * scale <= 64_000_000.0, "Physical display exceeds capture limit");
    Ok(image_rs::DynamicImage::ImageRgba8(monitor.capture_image()?))
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn coordinates(display:&Display,x:u32,y:u32,w:u32,h:u32,_:&[Display],_:&Enigo) -> anyhow::Result<(i32,i32)> {
    Ok((display.x.checked_add(scale(x,w,display.width)?).context("X overflow")?,display.y.checked_add(scale(y,h,display.height)?).context("Y overflow")?))
}

/// Probes are observational: no keypress, pointer movement, capture or OS prompt.
pub fn capabilities() -> serde_json::Value {
    let display_info = match displays() {
        Ok(displays) => serde_json::json!({"available":true,"items":displays}),
        Err(e) => serde_json::json!({"available":false,"error":format!("{e:#}")}),
    };
    serde_json::json!({"backend":NAME,"requirements":REQUIREMENTS,"displays":display_info,"native":native_capabilities()})
}

#[cfg(target_os = "linux")]
fn native_capabilities() -> serde_json::Value {
    use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};
    #[derive(Default)] struct Probe(Vec<String>);
    impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
        fn event(state:&mut Self,_:&wl_registry::WlRegistry,event:wl_registry::Event,_:&(),_:&Connection,_:&QueueHandle<Self>) {
            if let wl_registry::Event::Global { interface, .. } = event { state.0.push(interface); }
        }
    }
    let probe = (|| -> anyhow::Result<Vec<String>> {
        let connection = Connection::connect_to_env()?;
        let mut queue = connection.new_event_queue::<Probe>();
        connection.display().get_registry(&queue.handle(), ());
        let mut state = Probe::default();
        queue.roundtrip(&mut state)?;
        Ok(state.0)
    })();
    match probe {
        Ok(protocols) => {
            let has = |p:&str| protocols.iter().any(|v| v == p);
            serde_json::json!({"clipboardDataControlProtocol":has("zwlr_data_control_manager_v1"),"clipboardPasteMethod":"wayland_clipboard_paste","clipboardPastePolicy":"explicit replace only; no previous-content read/restore; history may retain text","pointerProtocol":has("zwlr_virtual_pointer_manager_v1"),"keyboardProtocol":has("zwp_virtual_keyboard_manager_v1"),"screenCopyProtocol":has("zwlr_screencopy_manager_v1") || has("ext_image_copy_capture_manager_v1"),"capturePermission":"not_tested","pointerLayout":"single-output only; observed xdg-output name and logical geometry must match","pointerMethod":"wayland.wlr_virtual_pointer","pointerKeyboardSideEffects":false,"pointerOutputBinding":"manager v2 output-bound; v1 single-output fallback"})
        }
        Err(e) => serde_json::json!({"available":false,"error":format!("{e:#}")}),
    }
}
#[cfg(target_os = "macos")]
fn screen_capture_allowed() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" { fn CGPreflightScreenCaptureAccess() -> bool; }
    unsafe { CGPreflightScreenCaptureAccess() }
}
#[cfg(target_os = "macos")]
fn native_capabilities() -> serde_json::Value {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" { fn AXIsProcessTrusted() -> bool; }
    serde_json::json!({"screenRecordingGranted":screen_capture_allowed(),"accessibilityGranted":unsafe { AXIsProcessTrusted() }})
}
#[cfg(target_os = "windows")]
fn native_capabilities() -> serde_json::Value {
    serde_json::json!({"input":"SendInput; subject to UIPI and interactive-desktop restrictions","capturePermission":"not_tested","virtualDesktopPointer":true})
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn native_capabilities() -> serde_json::Value { serde_json::json!({"available":false}) }

#[cfg(any(test, not(target_os="linux")))]
fn scale(value:u32,from:u32,to:u32) -> anyhow::Result<i32> {
    ensure!(from > 0 && to > 0 && value < from, "Invalid image coordinate extent");
    Ok(i32::try_from(u64::from(value) * u64::from(to) / u64::from(from))?)
}
#[cfg(target_os = "windows")]
pub fn type_character(input:&mut Enigo,ch:char) -> anyhow::Result<()> {
    use enigo::{Keyboard, Key, Direction};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    // Keep control characters out of the Unicode packet path. In particular,
    // don't emit both a Return key and a second Unicode LF packet.
    if matches!(ch, '\n' | '\r' | '\t') {
        let key = if ch == '\t' { Key::Tab } else { Key::Return };
        let pressed = input.key(key,Direction::Press);
        let released = input.key(key,Direction::Release);
        pressed?; released?;
        return Ok(());
    }
    for unit in ch.encode_utf16(&mut [0;2]) {
        let event = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk:0,wScan:*unit,dwFlags:flags,time:0,dwExtraInfo:0 } },
        };
        let down = event(KEYEVENTF_UNICODE);
        let up = event(KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        // Attempt key-up even after partial/rejected delivery. Do not send an
        // unbounded text batch whose tail could be cut off before its releases.
        let pressed = unsafe { SendInput(1,&down,std::mem::size_of::<INPUT>() as i32) };
        let released = unsafe { SendInput(1,&up,std::mem::size_of::<INPUT>() as i32) };
        ensure!(pressed == 1 && released == 1,"Unicode SendInput rejected (possibly UIPI or an inactive desktop)");
    }
    Ok(())
}
#[cfg(target_os = "macos")]
pub fn type_character(input:&mut Enigo,ch:char) -> anyhow::Result<()> {
    use enigo::{Keyboard, Key, Direction};
    use core_graphics::{event::{CGEvent, CGEventFlags, CGEventTapLocation},event_source::{CGEventSource,CGEventSourceStateID}};
    if matches!(ch, '\n' | '\r' | '\t') {
        let key = if ch == '\t' { Key::Tab } else { Key::Return };
        let pressed = input.key(key,Direction::Press);
        let released = input.key(key,Direction::Release);
        pressed?; released?;
        return Ok(());
    }
    let source = CGEventSource::new(CGEventSourceStateID::Private).map_err(|_| anyhow::anyhow!("Cannot create CGEvent source"))?;
    // Enigo's macOS fast_text only posts key-down. Construct both events
    // before posting either, and always post key-up for our Unicode packet.
    let down = CGEvent::new_keyboard_event(source.clone(),0,true).map_err(|_| anyhow::anyhow!("Cannot create Unicode key-down"))?;
    let up = CGEvent::new_keyboard_event(source,0,false).map_err(|_| anyhow::anyhow!("Cannot create Unicode key-up"))?;
    down.set_string(&ch.to_string());
    up.set_string(&ch.to_string());
    down.set_flags(CGEventFlags::CGEventFlagNull);
    up.set_flags(CGEventFlags::CGEventFlagNull);
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    Ok(())
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn type_character(_: &mut Enigo,_:char) -> anyhow::Result<()> { anyhow::bail!(REQUIREMENTS) }

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn move_pointer(input:&mut Enigo,x:i32,y:i32) -> anyhow::Result<()> {
    input.move_mouse(x,y,enigo::Coordinate::Abs)?;
    Ok(())
}
#[cfg(target_os = "windows")]
pub fn move_pointer(_: &mut Enigo,x:i32,y:i32) -> anyhow::Result<()> {
    let _dpi = DpiGuard::new()?;
    // Unlike Enigo's primary-monitor normalization, SetCursorPos supports
    // the full virtual desktop, including monitors left/above the primary.
    ensure!(unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(x,y) } != 0, "SetCursorPos failed: {}", std::io::Error::last_os_error());
    Ok(())
}
#[cfg(target_os = "windows")]
pub(super) struct DpiGuard(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);
#[cfg(target_os = "windows")]
impl DpiGuard {
    pub(super) fn new() -> anyhow::Result<Self> {
        use windows_sys::Win32::UI::HiDpi::*;
        let prior = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure!(!prior.is_null(), "Cannot establish per-monitor DPI coordinate space");
        Ok(Self(prior))
    }
}
#[cfg(target_os = "windows")]
impl Drop for DpiGuard { fn drop(&mut self) { unsafe { windows_sys::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(self.0); } } }

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn displays() -> anyhow::Result<Vec<Display>> { anyhow::bail!(REQUIREMENTS) }
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn capture(_: &Display) -> anyhow::Result<image_rs::DynamicImage> { anyhow::bail!(REQUIREMENTS) }
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn coordinates(_: &Display,_:u32,_:u32,_:u32,_:u32,_:&[Display],_:&Enigo) -> anyhow::Result<(i32,i32)> { anyhow::bail!(REQUIREMENTS) }

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn scaling_is_bounded() {
        assert_eq!(scale(800,1600,3840).unwrap(),1920);
        assert_eq!(scale(1599,1600,3840).unwrap(),3837);
        assert!(scale(1600,1600,3840).is_err());
        assert!(scale(0,0,3840).is_err());
    }
}
