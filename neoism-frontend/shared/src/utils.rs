//! Chrome utility helpers shared between native and web hosts.
//!
//! The native fork at `frontends/neoism/src/chrome/utils.rs` reaches
//! into `crate::constants`, `crate::layout::ContextDimension`,
//! `neoism_backend::config::{Config, Navigation, NavigationMode}`,
//! `neoism_window::window::Theme`, and `teletypewriter::WinsizeBuilder`
//! — none of which exist in the shared crate. For the web/shared host
//! the work is split:
//!
//! * `padding_top_from_config` and `terminal_dimensions` are ported in
//!   full against the POD shims declared in this module; both shims
//!   already round-trip with the native types (host wraps the result
//!   in `WinsizeBuilder` at the call site).
//! * `update_colors_based_on_theme` stays a "host-callback" stub
//!   because mutating `neoism_backend::config::Config` requires the
//!   crate. Hosts compute the swap themselves and pass the resolved
//!   palette into the chrome via setters — see the doc-comment below
//!   for the contract.

#[cfg(not(target_arch = "wasm32"))]
pub fn background_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let executable = std::env::current_exe().unwrap_or_default();
        if !executable
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("neoism.exe"))
        {
            let mut command = std::process::Command::new(program);
            command.creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
            );
            return command;
        }
        let mut command = std::process::Command::new(executable);
        command
            .arg("--neoism-internal-background-command")
            .arg(program)
            .creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
            );
        return command;
    }
    #[cfg(not(windows))]
    std::process::Command::new(program)
}

/// Constants the native fork pulled from `crate::constants`. Reproduced
/// here so the shared `padding_top_from_config` body matches the desktop
/// path byte-for-byte instead of accepting yet another parameter.
mod constants {
    #[cfg(not(target_os = "macos"))]
    pub const PADDING_Y: f32 = 2.0;

    #[cfg(target_os = "macos")]
    pub const PADDING_Y: f32 = 26.0;

    #[cfg(target_os = "macos")]
    pub const ADDITIONAL_PADDING_Y_ON_UNIFIED_TITLEBAR: f32 = 2.0;
}

/// POD mirror of `neoism_backend::config::navigation::NavigationMode`.
/// Only the variants the desktop's `padding_top_from_config` branches
/// against are reproduced — hosts translate their richer enum into one
/// of these before calling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavigationMode {
    #[default]
    Tab,
    NativeTab,
}

/// POD mirror of `neoism_backend::config::navigation::Navigation`.
/// Carries the fields the chrome's padding/strip logic actually reads;
/// hosts populate it once per frame from their real `Navigation`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NavigationShim {
    pub enabled: bool,
    pub hide_if_single: bool,
    pub mode: NavigationMode,
}

impl NavigationShim {
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// Compute the top padding that the chrome leaves above the editor /
/// terminal content. Ported from
/// `frontends/neoism/src/chrome/utils.rs::padding_top_from_config` —
/// the only divergence is that the native version took a real
/// `&Navigation`, while we take the [`NavigationShim`] POD. Behaviour
/// is byte-for-byte equivalent.
#[inline]
pub fn padding_top_from_config(
    navigation: &NavigationShim,
    padding_y_top: f32,
    #[allow(unused)] num_tabs: usize,
    #[allow(unused)] macos_use_unified_titlebar: bool,
) -> f32 {
    // When navigation is enabled (Tab mode), start content below island
    if navigation.is_enabled() {
        // If hide_if_single is true and there's only one tab, the island is
        // hidden so render from 0 + configured margin.
        if navigation.hide_if_single && num_tabs <= 1 {
            return constants::PADDING_Y + padding_y_top;
        }

        return crate::widgets::island::ISLAND_HEIGHT + padding_y_top;
    }

    let default_padding = constants::PADDING_Y + padding_y_top;

    #[cfg(target_os = "macos")]
    {
        if navigation.mode == NavigationMode::NativeTab {
            let additional = if macos_use_unified_titlebar {
                constants::ADDITIONAL_PADDING_Y_ON_UNIFIED_TITLEBAR
            } else {
                0.0
            };
            return additional + padding_y_top;
        }
    }

    default_padding
}

/// POD mirror of `crate::layout::Delta` margins used by the native
/// `ContextDimension`. The desktop version stored these as nested
/// fields on the dimension; we flatten them so callers can populate
/// the shim directly without pulling in the layout module.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContextMarginShim {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// POD mirror of `crate::layout::ContextDimension`. Carries the
/// width/height/columns/lines fields that drive PTY-size math.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContextDimensionShim {
    pub width: f32,
    pub height: f32,
    pub columns: usize,
    pub lines: usize,
    pub margin: ContextMarginShim,
}

/// POD mirror of `teletypewriter::WinsizeBuilder`. Hosts translate the
/// shared shape into their native PTY type at the call site.
#[derive(Clone, Copy, Debug, Default)]
pub struct WinsizeShim {
    pub width: u16,
    pub height: u16,
    pub cols: u16,
    pub rows: u16,
}

/// Compute the PTY-size builder for a given context layout. Ported
/// from `frontends/neoism/src/chrome/utils.rs::terminal_dimensions` —
/// the host wraps the returned [`WinsizeShim`] in
/// `teletypewriter::WinsizeBuilder` at the call site (the fields line
/// up one-for-one).
#[inline]
pub fn terminal_dimensions(layout: &ContextDimensionShim) -> WinsizeShim {
    let width = layout.width - layout.margin.left - layout.margin.right;
    let height = layout.height - layout.margin.top - layout.margin.bottom;
    WinsizeShim {
        width: width as u16,
        height: height as u16,
        cols: layout.columns as u16,
        rows: layout.lines as u16,
    }
}

/// Deprecated alias kept for callers that pre-date the parameterized
/// `terminal_dimensions`. Returns a zeroed [`WinsizeShim`] — same shape
/// as the previous stub.
#[deprecated(note = "use `terminal_dimensions(&ContextDimensionShim)`")]
#[inline]
pub fn terminal_dimensions_stub() -> WinsizeShim {
    WinsizeShim::default()
}

/// Platform theme hint forwarded by the host (winit's
/// `neoism_window::window::Theme`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeHint {
    Light,
    Dark,
}

/// Swap the chrome's working palette to match the platform's reported
/// light/dark theme when an `adaptive_colors` block is configured.
///
/// This *stays* a host-callback shim: mutating
/// `neoism_backend::config::Config` requires the backend crate, which
/// `neoism-ui` deliberately does not depend on. The host's wrapper —
/// e.g. `frontends/neoism/src/chrome/utils.rs::update_colors_based_on_theme`
/// — owns the live `Config`, calls this function to learn the host's
/// theme intent, and then writes the resolved palette into the chrome
/// via whatever setter the host already exposes (`set_colors`, etc.).
///
/// The shared version returns the [`ThemeHint`] back to the caller so
/// hosts can fold it into a single closure invocation; if no hint is
/// supplied the function is a no-op (mirrors the native early-return
/// when `theme_opt` is `None`).
#[inline]
pub fn update_colors_based_on_theme(theme_opt: Option<ThemeHint>) -> Option<ThemeHint> {
    theme_opt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_dimensions_subtracts_margins() {
        let layout = ContextDimensionShim {
            width: 800.0,
            height: 600.0,
            columns: 80,
            lines: 24,
            margin: ContextMarginShim {
                left: 10.0,
                right: 10.0,
                top: 5.0,
                bottom: 5.0,
            },
        };
        let win = terminal_dimensions(&layout);
        assert_eq!(win.width, 780);
        assert_eq!(win.height, 590);
        assert_eq!(win.cols, 80);
        assert_eq!(win.rows, 24);
    }

    #[test]
    fn padding_top_returns_island_height_when_nav_enabled_multi_tab() {
        let nav = NavigationShim {
            enabled: true,
            hide_if_single: true,
            mode: NavigationMode::Tab,
        };
        let pad = padding_top_from_config(&nav, 4.0, 3, false);
        assert!(pad >= crate::widgets::island::ISLAND_HEIGHT);
    }

    #[test]
    fn padding_top_collapses_for_hide_if_single() {
        let nav = NavigationShim {
            enabled: true,
            hide_if_single: true,
            mode: NavigationMode::Tab,
        };
        let pad = padding_top_from_config(&nav, 4.0, 1, false);
        assert_eq!(pad, constants::PADDING_Y + 4.0);
    }
}
