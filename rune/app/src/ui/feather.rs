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
    Right,
    // The chrome: the rail's panels and the tab rows.
    Folder,
    Notes,
    Sparkles,
    Server,
    Settings,
    // What a call does, beside its row (`agent/tools/card.rs`).
    Terminal,
    File,
    Search,
    Edit,
    Globe,
    Branch,
    Checklist,
    Plug,
    Wrench,
    // The diff's view options (`git/review.rs`).
    Columns,
    Rows,
    Fold,
    Unfold,
    SidePane,
    Refresh,
    Comment,
    External,
    Merge,
    Download,
    Wrap,
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
            Icon::Right => r#"<polyline points="9 18 15 12 9 6"/>"#,
            Icon::Folder => r#"<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>"#,
            Icon::Notes => r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/>"#,
            Icon::Sparkles => r#"<path d="M9.94 15.5A2 2 0 0 0 8.5 14.06l-6.14-1.58a.5.5 0 0 1 0-.96L8.5 9.94A2 2 0 0 0 9.94 8.5l1.58-6.14a.5.5 0 0 1 .96 0L14.06 8.5A2 2 0 0 0 15.5 9.94l6.14 1.58a.5.5 0 0 1 0 .96L15.5 14.06a2 2 0 0 0-1.44 1.44l-1.58 6.14a.5.5 0 0 1-.96 0z"/><path d="M20 3v4"/><path d="M22 5h-4"/>"#,
            Icon::Server => r#"<rect x="2" y="2" width="20" height="8" rx="2" ry="2"/><rect x="2" y="14" width="20" height="8" rx="2" ry="2"/><line x1="6" y1="6" x2="6.01" y2="6"/><line x1="6" y1="18" x2="6.01" y2="18"/>"#,
            Icon::Settings => r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>"#,
            Icon::Terminal => r#"<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>"#,
            Icon::File => r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/>"#,
            Icon::Search => r#"<circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>"#,
            Icon::Edit => r#"<path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/>"#,
            Icon::Globe => r#"<circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>"#,
            Icon::Branch => r#"<line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#,
            Icon::Checklist => r#"<polyline points="9 11 12 14 22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>"#,
            Icon::Plug => r#"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8z"/>"#,
            Icon::Columns => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><line x1="12" y1="3" x2="12" y2="21"/>"#,
            Icon::Rows => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><line x1="3" y1="12" x2="21" y2="12"/>"#,
            Icon::Fold => r#"<polyline points="7 20 12 15 17 20"/><polyline points="7 4 12 9 17 4"/>"#,
            Icon::Unfold => r#"<polyline points="7 15 12 20 17 15"/><polyline points="7 9 12 4 17 9"/>"#,
            Icon::SidePane => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><line x1="15" y1="3" x2="15" y2="21"/>"#,
            Icon::Refresh => r#"<polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>"#,
            Icon::Comment => r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#,
            Icon::External => r#"<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/>"#,
            Icon::Merge => r#"<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M6 21V9a9 9 0 0 0 9 9"/>"#,
            Icon::Download => r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/>"#,
            Icon::Wrap => r#"<line x1="3" y1="6" x2="21" y2="6"/><path d="M3 12h15a3 3 0 0 1 0 6h-4"/><polyline points="16 16 14 18 16 20"/><line x1="3" y1="18" x2="10" y2="18"/>"#,
            Icon::Wrench => r#"<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>"#,
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
