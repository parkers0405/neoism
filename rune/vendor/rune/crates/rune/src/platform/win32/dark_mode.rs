//! Dark native menus. Windows draws pop-up, context, window and tray menus light unless the
//! process asks for the dark theme through two functions uxtheme exports by ordinal only:
//! `SetPreferredAppMode` (135) and `FlushMenuThemes` (136). Both have kept their ordinals since
//! Windows 10 1903 (build 18362); earlier builds export something else there, so they're left
//! alone and menus stay light.
use std::sync::{OnceLock, atomic::{AtomicU8, Ordering}};

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

/// `PreferredAppMode`: follow the system's app mode, or force one.
const ALLOW_DARK: i32 = 1;
const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;

const FIRST_BUILD: u32 = 18362;

struct Uxtheme {
    set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32,
    flush_menu_themes: unsafe extern "system" fn(),
}

/// The mode last asked for, so a frame that doesn't change it costs nothing.
static APPLIED: AtomicU8 = AtomicU8::new(0);

/// Menus follow the system's dark or light app mode until a window says what it draws.
pub(crate) fn follow_system() { apply(ALLOW_DARK); }

/// Menus from now on are dark or light, whatever the system's app mode.
pub(crate) fn menus(dark: bool) { apply(if dark { FORCE_DARK } else { FORCE_LIGHT }); }

fn apply(mode: i32) {
    let Some(uxtheme) = uxtheme() else { return };
    if APPLIED.swap(mode as u8, Ordering::Relaxed) == mode as u8 { return; }
    // SAFETY: both are uxtheme's own exports on a build that has them with these signatures.
    unsafe {
        (uxtheme.set_preferred_app_mode)(mode);
        (uxtheme.flush_menu_themes)();
    }
}

fn uxtheme() -> Option<&'static Uxtheme> {
    static UXTHEME: OnceLock<Option<Uxtheme>> = OnceLock::new();
    UXTHEME.get_or_init(|| {
        if build()? < FIRST_BUILD { return None; }
        // SAFETY: loads a system DLL from System32 and looks up two exports by ordinal (a
        // pointer value below 0x10000). The module stays loaded for the process.
        unsafe {
            let module = LoadLibraryExW(wide("uxtheme.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if module.is_null() { return None; }
            let set = GetProcAddress(module, 135 as *const u8)?;
            let flush = GetProcAddress(module, 136 as *const u8)?;
            Some(Uxtheme {
                set_preferred_app_mode: std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(i32) -> i32>(set),
                flush_menu_themes: std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn()>(flush),
            })
        }
    }).as_ref()
}

/// `OSVERSIONINFOW`, as `RtlGetVersion` fills it.
#[repr(C)]
struct VersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

/// The Windows build number. `RtlGetVersion` reports it whatever the app's manifest says, which
/// `GetVersionEx` doesn't.
fn build() -> Option<u32> {
    // SAFETY: ntdll is loaded in every process; RtlGetVersion fills the struct whose size it is
    // given and returns an NTSTATUS.
    unsafe {
        let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
        if ntdll.is_null() { return None; }
        let get = GetProcAddress(ntdll, c"RtlGetVersion".as_ptr().cast())?;
        let get = std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(*mut VersionInfo) -> i32>(get);
        let mut info = VersionInfo { size: size_of::<VersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
        (get(&mut info) == 0).then_some(info.build)
    }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
