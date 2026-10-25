//! Zed-style Settings panel — a full-screen overlay bound to
//! `config.json`. See [`state`] for the data model + interaction and
//! [`view`] for the painter.

#[cfg(feature = "neoui")]
mod neoui;
#[cfg_attr(not(feature = "neoui"), path = "legacy_state.rs")]
mod state;
#[cfg_attr(not(feature = "neoui"), path = "legacy_view.rs")]
mod view;

pub use state::{NeoismSettingsPane, PointerOutcome, SettingsAction};

impl NeoismSettingsPane {
    /// Paint the panel across the whole window. No-op when inactive.
    pub fn render(
        &mut self,
        sugarloaf: &mut sugarloaf::Sugarloaf,
        win_w: f32,
        win_h: f32,
        theme: &crate::primitives::ide_theme::IdeTheme,
        scale: f32,
        mouse: Option<(f32, f32)>,
    ) {
        view::render(self, sugarloaf, win_w, win_h, theme, scale, mouse);
    }
}
