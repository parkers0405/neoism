//! One-shot startup authority for invite focus targets.
//!
//! The App must keep the activation alongside its queued invite batch until the
//! actual target window exists and is mapped (including cold startup). Call this
//! once at that target, not on each URI or every redraw. Invalid/rejected invites
//! should drop their authority rather than activating an unrelated window.

use neoism_backend::event::InviteActivationToken;
use neoism_window::window::Window;

/// Consume the batch's authority before attempting native activation. Returns
/// whether a request was sent, never whether the OS granted focus. A missing
/// token on Linux grants no right to raise a background window.
pub fn activate_invite_window(
    window: &Window,
    activation: &mut Option<InviteActivationToken>,
) -> bool {
    let token = activation.take();
    #[cfg(target_os = "linux")]
    {
        use neoism_window::platform::startup_notify::WindowExtStartupNotify;
        let Some(token) = token else {
            return false;
        };
        let token = neoism_window::window::ActivationToken::from_raw(token.into_string());
        match window.activate_with_token(&token) {
            Ok(()) => true,
            Err(_) => {
                // A compositor without xdg_activation cannot honor launcher authority.
                // Preserve ordinary best-effort focus (Wayland's is a no-op).
                window.focus_window();
                false
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = token;
        window.focus_window();
        true
    }
}
