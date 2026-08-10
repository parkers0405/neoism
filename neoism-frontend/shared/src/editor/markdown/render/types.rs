pub(super) const DEPTH: f32 = 0.0;
pub(super) const ORDER_BG: u8 = 3;
pub(super) const ORDER_TEXT: u8 = 8;
pub(super) const LIST_INDENT_PX: f32 = 26.0;
pub(super) const MARKDOWN_BODY_FONT_SIZE: f32 = 17.0;
pub(super) const GLOBAL_TEXT_BASELINE_FONT_SIZE: f32 = 14.0;
pub(super) const BLOCK_RADIUS: f32 = 7.0;
// Fenced code blocks render as a git-diff-style card: a `surface` header bar
// (naming the language) over a `bg` body of inner code lines. The ``` fences
// themselves are not drawn, so only these dimensions drive the block height.
pub(super) const CODE_BLOCK_HEADER_H: f32 = 30.0;
pub(super) const CODE_BLOCK_BODY_PAD: f32 = 8.0;
pub(super) const MARKDOWN_SCROLLBAR_WIDTH: f32 = 6.0;
pub(super) const MARKDOWN_SCROLLBAR_MARGIN: f32 = 6.0;
pub(super) const MARKDOWN_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 28.0;

// SpellcheckWord / SPELLCHECK_DICTIONARY / SPELLCHECK_DICT_PATHS now
// live in `neoism_ui::editor::markdown::spellcheck`; native render
// reaches them via the `pub use` in `render/inline.rs`.

#[derive(Clone, Copy)]
pub(super) enum RenderLineKind {
    Empty,
    Heading(u8),
    Paragraph,
    Task {
        checked: bool,
        depth: usize,
    },
    Bullet {
        depth: usize,
    },
    Ordered {
        depth: usize,
    },
    CodeFence,
    Code,
    Callout {
        kind: crate::widgets::markdown::MarkdownCalloutKind,
    },
    Quote,
    Divider,
}

#[derive(Clone, Copy)]
pub(super) struct ParsedRenderLine<'a> {
    pub(super) kind: RenderLineKind,
    pub(super) text: &'a str,
    pub(super) marker_len: usize,
    pub(super) list_marker: Option<&'a str>,
}

pub(super) struct ParsedTable {
    pub(super) header: Vec<String>,
    pub(super) rows: Vec<Vec<String>>,
    pub(super) end_line: usize,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MermaidDirection {
    TopDown,
    LeftRight,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MermaidNodeShape {
    Rect,
    Round,
    Decision,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(super) struct MermaidNode {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) shape: MermaidNodeShape,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(super) struct MermaidEdge {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) label: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(super) struct MermaidDiagram {
    pub(super) direction: MermaidDirection,
    pub(super) nodes: Vec<MermaidNode>,
    pub(super) edges: Vec<MermaidEdge>,
}

pub(super) struct TableCursorPosition {
    pub(super) x: f32,
    pub(super) visual_line: usize,
    pub(super) cell_ix: usize,
}

pub(super) struct InlineWikiLink {
    pub(super) raw_start: usize,
    pub(super) raw_end: usize,
    pub(super) inner: String,
    pub(super) label: String,
}

pub(super) struct InlineTag {
    pub(super) raw_start: usize,
    pub(super) label: String,
}
