//! Read-only pane for an image file opened from the workspace.
use std::path::Path;

use rune::{El, Image, SurfaceFit, col, el, row, text};

use crate::{app::{App, Focus}, tabs::TabKind, ui::theme::rgb};

pub(crate) const ICON: &str = "\u{f03e}";

pub(crate) fn title(path: &str) -> &str {
    Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or(path)
}

pub(crate) fn is_path(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpeg" | "jpg" | "gif" | "webp" | "bmp" | "svg" | "svgz")
    })
}

impl App {
    pub(crate) fn open_image_file(&mut self, path: String) {
        let kind = TabKind::Image { path };
        if let Some(tab) = self.tabs.iter().find(|tab| tab.kind == kind) {
            self.select_tab(tab.id);
        } else {
            self.open_tab(kind);
        }
        self.focus = Focus::Content;
    }
}

pub(crate) fn view(app: &App, path: &str) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let image = app.viewer_file_image(app.active_tab().id, path);
    let mut header = row().w_full().h(38.0 * s).px(16.0 * s).gap(12.0 * s).center_y().fill(rgb(p.surface))
        .child(text(title(path)).font(app.ty.sans(13.0, 600.0)).color(rgb(p.fg)).truncate().grow(1.0).shrink());
    if let Some([w, h]) = image.as_ref().and_then(Image::size) {
        header = header.child(text(format!("{w} \u{d7} {h}")).font(app.ty.small()).color(rgb(p.dim)));
    }
    let canvas = el().w_full().grow(1.0).shrink().center().key("image-canvas");
    let canvas = match image {
        Some(image) if image.is_ready() && image.error().is_none() => canvas.pixels_fit(image.pixels(), SurfaceFit::Fit),
        image => canvas.child(text(if image.is_some_and(|image| !image.is_ready()) { "Loading image…" } else { "Image unavailable" })
            .font(app.ty.sans(14.0, 400.0)).color(rgb(p.muted))),
    };
    col().fill(rgb(p.bg)).child(header).child(col().w_full().grow(1.0).shrink().padding(20.0 * s).child(canvas))
}