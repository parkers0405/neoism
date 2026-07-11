use sugarloaf::text::DrawOpts;
use sugarloaf::{
    DirtyKind, NodeId, NodeRevision, NodeSource, NodeSourceRange, VirtualMeasuredLayout,
    VirtualNodeKind, VirtualRevealAlign, VirtualRevealTarget, VirtualScroll,
    VirtualSourceRevision, VirtualSurfaceCommand, VirtualSurfaceConfig, VirtualViewport,
};
use sugarloaf::{Sugarloaf, VirtualBounds};
use web_time::Instant;

use crate::editor::markdown::helpers::{
    block_handle_rect, is_divider, parse_markdown_list_marker, quote_marker_len,
    source_from_lines,
};
use crate::editor::markdown::{
    source_map::InlineSourceMap, MarkdownListMarker, MarkdownListMarkerKind,
    MarkdownOutlineEntry, MarkdownPane, MarkdownPendingLineEdit,
    MarkdownVirtualMeasureKey, MarkdownVirtualMeasurement, MarkdownVirtualRenderState,
    MarkdownWrapHitRow, MarkdownWrapRow, CURSOR_REVEAL_FAST_REPEAT,
};
use crate::editor::neodraw::{render_scene, Camera, Vec2};
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::truncate_to_fit;
use crate::syntax::{highlight_line, syn_color, Lang};
use crate::widgets::markdown::{list_depth_from_indent, parse_heading_line};
use crate::widgets::mermaid::{mermaid_scene, parse_mermaid_diagram};

use super::bullet_align;
use super::draw::{
    caret_height, cursor_cell_width, cursor_position_for_text_prefix,
    cursor_y_for_text_line, draw_block_actions, draw_copy_button, draw_drag_ghost,
    draw_if_visible, draw_list_guides, draw_rect_clipped, draw_rounded_rect_clipped,
    draw_search_matches_for_line, draw_selection_for_line, draw_task_checkbox,
    draw_wrapped, line_height, list_indent_px, markdown_font, visible_markdown_prefix,
};
use super::illuminated::{
    draw_illuminated_inline, illuminated_inline_metrics, parse_illuminate_token,
    IlluminatedToken,
};
use super::inline::{clean_inline_with_active_link, markdown_link_label};
use super::scrollbar::draw_markdown_scrollbar;
use super::table::{measure_table, parse_table, render_table_with_source_base};
use super::types::{
    BLOCK_RADIUS, CODE_BLOCK_BODY_PAD, CODE_BLOCK_HEADER_H, DEPTH, ORDER_BG, ORDER_TEXT,
};

include!("virtualized/surface.rs");
include!("virtualized/draw_blocks.rs");
include!("virtualized/inline_layout.rs");
include!("virtualized/block_widgets.rs");
include!("virtualized/cursor.rs");
include!("virtualized/remote_carets.rs");
include!("virtualized/roster.rs");
include!("virtualized/source_items.rs");
include!("virtualized/state.rs");
