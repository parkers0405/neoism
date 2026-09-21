use super::DocumentationNotebook;
use crate::primitives::IdeTheme;
use sugarloaf::Sugarloaf;

/// Notebook navigation lives in Alt+N. Notebook-bound Markdown keeps its
/// session/history semantics, but the center pane remains ordinary Markdown.
pub fn render_navigation(
    _sugarloaf: &mut Sugarloaf,
    book: &mut DocumentationNotebook,
    rect: [f32; 4],
    _theme: &IdeTheme,
    _mouse: Option<[f32; 2]>,
    _occlusions: &[[f32; 4]],
) -> [f32; 4] {
    book.hit_regions.clear();
    book.rail_rect = None;
    rect
}
