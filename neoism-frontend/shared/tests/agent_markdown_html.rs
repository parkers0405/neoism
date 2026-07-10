use std::rc::Rc;

use neoism_ui::panels::agent_pane::view::markdown::{
    measure_markdown_blocks, safe_canvas_markdown, AgentMarkdownPane,
    AssistantMarkdownBlock,
};
use neoism_ui::widgets::markdown::{
    is_table_delimiter_for_header, parse_table_row_trimmed,
};

#[derive(Default)]
struct TestPane;

impl AgentMarkdownPane for TestPane {
    fn cached_markdown_blocks_for(
        &self,
        _text: &str,
        _width: f32,
        _scale: f32,
    ) -> Option<Rc<Vec<AssistantMarkdownBlock>>> {
        None
    }

    fn store_markdown_blocks_for(
        &self,
        _text: &str,
        _width: f32,
        _scale: f32,
        _blocks: Rc<Vec<AssistantMarkdownBlock>>,
    ) {
    }

    fn register_selectable_line(&mut self, _text: &str, _rect: [f32; 4]) -> usize {
        0
    }

    fn selectable_line_highlight(&self, _index: usize) -> Option<(f32, f32)> {
        None
    }

    fn register_link_hit_rect(&mut self, _target: String, _rect: [f32; 4]) {}

    fn link_hovered(&self, _target: &str) -> bool {
        false
    }

    fn mermaid_raw_mode(&self, _key: u64) -> bool {
        false
    }
}

#[test]
fn non_rendering_markdown_has_no_layout_height() {
    assert_eq!(measure_markdown_blocks(&[], 360.0, &TestPane, 1.0), 0.0);
}

#[test]
fn ordinary_markdown_stays_borrowed_and_unchanged() {
    let markdown = "**ordinary markdown** with no HTML";
    assert!(matches!(
        safe_canvas_markdown(markdown),
        std::borrow::Cow::Borrowed(value) if value == markdown
    ));
}

#[test]
fn commonmark_html_comments_are_non_rendering() {
    let markdown = "**Assessing layout**\n<!-- -->\n**Planning tests**";

    assert_eq!(
        safe_canvas_markdown(markdown),
        "**Assessing layout**\n\n**Planning tests**"
    );
    assert!(safe_canvas_markdown("<!-- hidden -->").trim().is_empty());
    assert_eq!(
        safe_canvas_markdown("left<!-- hidden -->right"),
        "leftright"
    );
}

#[test]
fn provider_reasoning_summary_has_no_comment_or_phantom_tail() {
    let persisted = "**Interpreting ambiguous user phrase**\n\n<!-- -->";

    assert_eq!(
        safe_canvas_markdown(persisted),
        "**Interpreting ambiguous user phrase**"
    );
}

#[test]
fn multiline_and_streaming_html_comments_are_non_rendering() {
    assert_eq!(
        safe_canvas_markdown("before\n<!-- hidden\nacross lines -->\nafter"),
        "before\n\n\nafter"
    );
    assert!(safe_canvas_markdown("<!-- partial stream")
        .trim()
        .is_empty());
}

#[test]
fn comment_lookalikes_inside_code_are_preserved_verbatim() {
    let inline = "`<!-- keep this -->`";
    let fenced = "```html\n<!-- keep this -->\n```";
    let comparison = "keep 1 < 2 && 3 > 2";
    let autolink = "<https://example.com/path>";

    assert_eq!(safe_canvas_markdown(inline), inline);
    assert_eq!(safe_canvas_markdown(fenced), fenced);
    assert_eq!(safe_canvas_markdown(comparison), comparison);
    assert_eq!(safe_canvas_markdown(autolink), autolink);
}

#[test]
fn safe_html_markup_projects_to_canvas_text() {
    assert_eq!(
        safe_canvas_markdown("before <strong>bold</strong><br>after"),
        "before bold\nafter"
    );
    assert_eq!(safe_canvas_markdown("one<br><br>two"), "one\n\ntwo");
    assert_eq!(
        safe_canvas_markdown("<p>one</p><p>two</p>").trim(),
        "one\ntwo"
    );
    assert_eq!(
        safe_canvas_markdown("<span title=\"1 > 0\">kept</span>"),
        "kept"
    );
}

#[test]
fn executable_html_contents_are_suppressed() {
    assert_eq!(
        safe_canvas_markdown("before <script>alert(1)</script> after"),
        "before  after"
    );
    let block = "<style>\n.secret { display: block; }\n</style>\nvisible";
    assert_eq!(safe_canvas_markdown(block).trim(), "visible");
}

#[test]
fn declarations_and_processing_instructions_are_non_rendering() {
    assert!(safe_canvas_markdown("<!doctype html>").trim().is_empty());
    assert!(safe_canvas_markdown("<?agent hidden?>").trim().is_empty());
}

#[test]
fn persisted_pipe_prose_is_not_a_table_row() {
    let prose = "Neoism’s fallback treated prose such as `` `← | →` `` as a table.";

    assert_eq!(parse_table_row_trimmed(prose), None);
}

#[test]
fn gfm_table_requires_a_matching_delimiter_row() {
    let header = parse_table_row_trimmed("| Header | Header |").unwrap();
    let delimiter = parse_table_row_trimmed("| --- | --- |").unwrap();
    let ordinary_next_line = parse_table_row_trimmed("ordinary prose");

    assert!(is_table_delimiter_for_header(&header, &delimiter));
    assert!(ordinary_next_line.is_none());
}
