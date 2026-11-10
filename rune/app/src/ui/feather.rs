//! GPUiX's terminal action artwork (`terminal/icons.ts`): Feather icons, Copyright (c) 2013-2023
//! Cole Bemis, MIT. Drawn as SVG tinted to the theme colour, at the size they display.

use crate::app::App;
use rune::{El, el};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Filter,
    More,
    Star,
    StarFilled,
    Down,
    Up,
}

impl Icon {
    fn body(self) -> &'static str {
        match self {
            Icon::Filter => r#"<polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/>"#,
            Icon::More => r#"<circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/>"#,
            Icon::Star => r#"<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/>"#,
            Icon::StarFilled => r#"<polygon fill="currentColor" points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/>"#,
            Icon::Down => r#"<polyline points="6 9 12 15 18 9"/>"#,
            Icon::Up => r#"<polyline points="18 15 12 9 6 15"/>"#,
        }
    }

    /// The SVG document with `currentColor` resolved to `rgb`.
    pub fn svg(self, rgb: u32) -> String {
        let view = if self == Icon::More { "-6 -6 36 36" } else { "0 0 24 24" };
        let doc = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="{view}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#, self.body());
        doc.replace("currentColor", &format!("#{rgb:06x}"))
    }
}

/// `icon` at `size` points in `rgb` (an empty box until the image cache exists).
pub fn view(app: &App, icon: Icon, size: f32, rgb: u32) -> El {
    let image = app.images.as_ref().map(|images| images.svg(icon.svg(rgb).into_bytes(), size, 2.0));
    match image {
        Some(image) => el().w(size).h(size).pixels(image.pixels()),
        None => el().w(size).h(size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_resolve_their_colour() {
        let svg = Icon::StarFilled.svg(0xf9e2af);
        assert!(svg.contains("#f9e2af") && !svg.contains("currentColor"));
        assert!(Icon::More.svg(0).contains("-6 -6 36 36"));
    }
}
