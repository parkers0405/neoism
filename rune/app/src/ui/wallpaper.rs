//! The window wallpaper (`ui.window.background-image`; GPUiX `AppGeneration` + `config.ts`): the
//! theme background, the image over it (cover fit, its opacity, 0.35 when unset), then the app
//! with page-level surfaces left clear so the image shows through. Cards and chrome stay opaque.

use super::theme::rgb;
use crate::app::App;
use rune::{Color, El, SurfaceFit, el};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wallpaper {
    pub path: Option<PathBuf>,
    pub opacity: f32,
}

impl Wallpaper {
    /// From a config document: nothing when disabled, unset or a zero-opacity image.
    pub fn from_config(config: &Value) -> Wallpaper {
        let window = &config["ui"]["window"];
        if window["background-image-disabled"].as_bool() == Some(true) { return Wallpaper::default(); }
        let image = &window["background-image"];
        let Some(path) = image["path"].as_str().filter(|p| !p.trim().is_empty()) else { return Wallpaper::default() };
        let opacity = image["opacity"].as_f64().unwrap_or(0.35).clamp(0.0, 1.0) as f32;
        if opacity <= 0.0 { return Wallpaper::default(); }
        let path = match path.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(path)),
            None => PathBuf::from(path),
        };
        Wallpaper { path: Some(path), opacity }
    }

    pub fn active(&self) -> bool { self.path.is_some() }
}

impl App {
    /// A page-level background: clear while a wallpaper shows, so the image is visible behind.
    pub fn surface(&self, color: u32) -> Color {
        if self.wallpaper.active() { Color::TRANSPARENT } else { rgb(color) }
    }

    /// Re-read the wallpaper from `config` (or the config file) after a background change.
    pub fn reload_wallpaper_from(&mut self, config: Option<Value>) {
        let config = config.unwrap_or_else(crate::config::read);
        self.wallpaper = Wallpaper::from_config(&config);
    }
}

/// The image layer behind the whole window (decoded off the UI thread on first use).
pub fn view(app: &App) -> El {
    let Some(path) = &app.wallpaper.path else { return el() };
    let Some(image) = app.background_preview(path) else { return el() };
    el().key("app-wallpaper").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .pixels_fit(image.pixels(), SurfaceFit::Fill)
        .opacity(app.wallpaper.opacity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_rules() {
        let set = json!({"ui": {"window": {"background-image": {"path": "/tmp/a.png", "opacity": 0.5}}}});
        assert_eq!(Wallpaper::from_config(&set), Wallpaper { path: Some("/tmp/a.png".into()), opacity: 0.5 });
        let default_opacity = json!({"ui": {"window": {"background-image": {"path": "/tmp/a.png"}}}});
        assert_eq!(Wallpaper::from_config(&default_opacity).opacity, 0.35);
        let disabled = json!({"ui": {"window": {"background-image-disabled": true, "background-image": {"path": "/tmp/a.png"}}}});
        assert!(!Wallpaper::from_config(&disabled).active());
        assert!(!Wallpaper::from_config(&json!({"ui": {"window": {"background-image": null}}})).active());
    }
}
