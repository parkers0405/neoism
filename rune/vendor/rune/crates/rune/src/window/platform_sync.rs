use winit::dpi::{LogicalPosition, LogicalSize};

use crate::runtime::Runtime;
use super::Win;

/// What a built frame hands the window besides its pixels: secure input, the IME, the cursor,
/// help tags and the clipboard.
pub(super) fn sync_platform<S: 'static>(l: &mut Win, rt: &mut Runtime<S>, secure_input: &mut crate::input::secure_input::SecureInput,
    ime: &mut bool, clipboard: &crate::clipboard::Client) {
    let focused = l.window.has_focus();
    secure_input.set(rt.has_secure_focus() && focused);
    let ime_allowed = rt.ime_area.is_some() && !rt.has_secure_focus() && focused;
    if std::mem::take(&mut rt.ime_reset) || ime_allowed != *ime {
        l.window.set_ime_allowed(false);
        l.window.set_ime_allowed(ime_allowed);
        *ime = ime_allowed;
    }
    l.apply_cursor(rt, focused);
    #[cfg(target_os = "macos")]
    l.native.help.sync(&l.window, rt.help_regions());
    // The process has one menu theme: the active window's, so its window menu matches it.
    #[cfg(target_os = "windows")]
    if focused { crate::platform::win32::dark_mode::menus(rt.backdrop_dark().unwrap_or(rt.env.system_dark)); }
    clipboard.flush(rt);
    if let Some(a) = rt.ime_area {
        l.window.set_ime_cursor_area(
            LogicalPosition::new(a[0] as f64, a[1] as f64),
            LogicalSize::new(a[2] as f64, a[3] as f64),
        );
    }
}

impl Win {
    /// The runtime's cursor while the pointer is in the window, the arrow once it leaves.
    /// `Hidden` needs focus, so it can't outlive the window being active.
    pub(super) fn apply_cursor<S>(&mut self, rt: &Runtime<S>, focused: bool) {
        let want = match rt.cursor() { crate::el::Cursor::Hidden if !focused => crate::el::Cursor::Default, c => c };
        self.cursor.apply(&self.window, want, rt.pointer().is_some());
    }
}
