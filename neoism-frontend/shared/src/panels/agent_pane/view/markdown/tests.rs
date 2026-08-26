use super::*;
use std::collections::BTreeSet;
use std::rc::Rc;

#[derive(Default)]
struct TestPane {
    raw_mermaid: BTreeSet<u64>,
}

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

    fn mermaid_raw_mode(&self, key: u64) -> bool {
        self.raw_mermaid.contains(&key)
    }
}

/// The renderer advances its draw cursor by exactly `markdown_block_height`
/// per block (plus the 6*s gap), and the card is sized by
/// `measure_markdown_blocks`. They must agree to the pixel, or the message
/// body either overflows its card or leaves a gap below it — the
/// "giant empty gaps in big messages" bug. This locks the card height to
/// the sum of the exact block heights.
#[test]
fn measured_card_equals_sum_of_block_heights() {
    let s = 1.0;
    let pane = TestPane::default();
    let blocks = vec![
        AssistantMarkdownBlock::Heading {
            level: 1,
            lines: vec!["Title".into(), "wrapped".into()],
        },
        AssistantMarkdownBlock::Paragraph(vec!["a".into(), "b".into(), "c".into()]),
        AssistantMarkdownBlock::Code {
            lang: "rust".into(),
            lines: Rc::new(vec!["fn main() {}".into(); 20]),
            copy_target: String::new(),
            content_width: 0.0,
        },
        AssistantMarkdownBlock::Stock(
            parse_stock_card(r#"{"symbol":"AAPL","price":297.2}"#).unwrap(),
        ),
        AssistantMarkdownBlock::Quote(vec!["quote".into()]),
        AssistantMarkdownBlock::Blank,
        AssistantMarkdownBlock::Bullet(vec!["item".into()]),
    ];

    let expected = 8.0 * s
        + blocks
            .iter()
            .map(|b| markdown_block_height(b, 360.0, &pane, s) + 6.0 * s)
            .sum::<f32>();
    assert_eq!(
        measure_markdown_blocks(&blocks, 360.0, &pane, s),
        expected.max(22.0 * s)
    );
}

/// A heading's measured height uses the same per-line height the draw path
/// advances by, so multi-line headings don't bleed into the next block.
#[test]
fn heading_height_matches_drawn_line_advance() {
    let s = 1.5;
    for level in 1..=4 {
        let lines = vec!["x".to_string(); 3];
        let block = AssistantMarkdownBlock::Heading {
            level,
            lines: lines.clone(),
        };
        let expected = 4.0 * s + lines.len() as f32 * heading_line_height(level, s);
        assert_eq!(
            markdown_block_height(&block, 360.0, &TestPane::default(), s),
            expected
        );
    }
}

#[test]
fn heading_wrap_and_draw_use_the_same_font_size() {
    let s = 1.5;
    assert_eq!(heading_font_size(1, s), 31.5);
    assert_eq!(heading_font_size(2, s), 27.0);
    assert_eq!(heading_font_size(3, s), 24.0);
    assert_eq!(heading_font_size(6, s), 24.0);
}

#[test]
fn visible_line_range_keeps_only_lines_near_clip() {
    assert_eq!(
        visible_line_range(100.0, 20.0, 10, [0.0, 140.0, 400.0, 40.0]),
        (1, 5)
    );
    assert_eq!(
        visible_line_range(100.0, 20.0, 10, [0.0, 0.0, 400.0, 20.0]),
        (0, 0)
    );
    assert_eq!(
        visible_line_range(100.0, 20.0, 10, [0.0, 280.0, 400.0, 40.0]),
        (8, 10)
    );
}

#[test]
fn table_height_grows_for_wrapped_cells() {
    let single = vec![vec!["short".to_string(), "ok".to_string()]];
    let wrapped = vec![vec!["short\ncontinued\nmore".to_string(), "ok".to_string()]];

    assert!(
        measure_laid_out_table_height(&wrapped, 1.0)
            > measure_laid_out_table_height(&single, 1.0)
    );
}

#[test]
fn wide_tables_keep_readable_wrapped_columns_and_overflow_horizontally() {
    let viewport_w = 360.0;
    let widths = resolve_table_column_widths(vec![120.0, 900.0, 460.0], viewport_w, 1.0);

    assert_eq!(widths, vec![TABLE_MIN_COLUMN_W, TABLE_MAX_COLUMN_W, 460.0]);
    assert!(widths.iter().sum::<f32>() > viewport_w);

    // A table that genuinely fits still fills the available viewport rather
    // than receiving an unnecessary scrollbar.
    assert_eq!(
        resolve_table_column_widths(vec![100.0], viewport_w, 1.0),
        vec![viewport_w]
    );

    // The overflow range includes enough trailing room to reveal the entire
    // closing rule instead of clipping it at the viewport boundary.
    assert_eq!(table_scroll_content_width(1_300.0, 1_000.0, 1.0), 1_314.0);
    assert_eq!(table_scroll_content_width(1_000.0, 1_000.0, 1.0), 1_000.0);
}

#[test]
fn tall_table_scrollbar_sticks_to_the_visible_bottom_above_composer() {
    // The table itself continues to y=900, but the timeline is clipped at
    // y=500 by the fixed composer. The scrollbar must therefore live in the
    // last 16px of the visible intersection, not at the unseen table bottom.
    let visible = intersect_rect([20.0, 100.0, 600.0, 800.0], [0.0, 40.0, 800.0, 460.0])
        .expect("table should intersect the timeline");
    let track = sticky_markdown_horizontal_scrollbar_track(visible, 30.0, 580.0, 1.0)
        .expect("visible table should expose its scrollbar");

    assert_eq!(track, [30.0, 484.0, 580.0, 16.0]);
}

#[test]
fn markdown_scrollbar_geometry_exposes_a_full_drag_target() {
    let geometry = markdown_horizontal_scrollbar_geometry(
        [30.0, 484.0, 580.0, 16.0],
        580.0,
        1_160.0,
        290.0,
        1.0,
    )
    .expect("overflow should produce scrollbar geometry");

    assert_eq!(geometry.rail, [30.0, 489.5, 580.0, 5.0]);
    assert_eq!(geometry.thumb, [175.0, 484.0, 290.0, 16.0]);
}

#[test]
fn table_cell_lines_keeps_empty_cells_visible() {
    assert_eq!(table_cell_lines(""), vec![String::new()]);
    assert_eq!(
        table_cell_lines(" one \n\n two "),
        vec!["one".to_string(), "two".to_string()]
    );
}

#[test]
fn table_cell_wrapping_reopens_bold_on_every_visual_line() {
    let tokens =
        inline_wrap_tokens("**Give $100, get $100 in account credit after 90 days**");
    assert!(tokens.len() > 1);
    assert!(tokens
        .iter()
        .all(|token| token.source().starts_with("**") && token.source().ends_with("**")));
    assert!(tokens
        .iter()
        .all(|token| !rendered_inline_text(&token.source()).contains('*')));
}

#[test]
fn copy_link_target_round_trips_code_text() {
    let text = "fn main() {\n    println!(\"hi%\");\n}";
    let target = format!("{COPY_LINK_PREFIX}{}", escape_copy_target(text));
    assert_eq!(copied_code_from_link_target(&target).as_deref(), Some(text));
    let lines = Rc::new(vec!["fn main() {".to_string(), "}".to_string()]);
    let ref_target = copy_ref_target_for_lines(lines.as_slice());
    register_copy_lines(&ref_target, lines);
    assert_eq!(
        copied_code_from_link_target(&ref_target).as_deref(),
        Some("fn main() {\n}")
    );
    assert_eq!(copied_code_from_link_target("file.rs"), None);
    assert_eq!(copied_code_from_link_target("neoism-copy://%zz"), None);
}

#[test]
fn mermaid_toggle_link_parses_hex_key() {
    let target = format!("{MERMAID_TOGGLE_LINK_PREFIX}{:016x}", 42u64);
    assert_eq!(mermaid_toggle_key_from_link_target(&target), Some(42));
    assert_eq!(
        mermaid_toggle_key_from_link_target("neoism-mermaid-toggle://nope"),
        None
    );
}

#[test]
fn layout_upgrades_mermaid_fence_to_mermaid_block() {
    let block = markdown_code_or_stock_block(
        "mermaid".into(),
        vec!["flowchart LR".into(), "A[Start] --> B{Done}".into()],
    );

    assert!(matches!(
        block,
        AssistantMarkdownBlock::Mermaid {
            diagram: Some(_),
            ..
        }
    ));
}

#[test]
fn styled_inline_spans_expose_word_wrap_opportunities() {
    let tokens = inline_wrap_tokens(
            "Something **The universe is wide** and ~~still styled~~ with [a long label](file.rs)",
        );
    let sources: Vec<String> = tokens.iter().map(InlineWrapToken::source).collect();

    assert_eq!(
        sources,
        vec![
            "Something",
            "**The**",
            "**universe**",
            "**is**",
            "**wide**",
            "and",
            "~~still~~",
            "~~styled~~",
            "with",
            "[a](file.rs)",
            "[long](file.rs)",
            "[label](file.rs)",
        ]
    );
    assert!(!tokens[0].whitespace_before);
    assert!(tokens.iter().skip(1).all(|token| token.whitespace_before));
}

#[test]
fn adjacent_inline_styles_do_not_invent_whitespace() {
    let tokens = inline_wrap_tokens("left**bold words**right");
    let rendered: Vec<(String, bool)> = tokens
        .iter()
        .map(|token| (token.source(), token.whitespace_before))
        .collect();

    assert_eq!(
        rendered,
        vec![
            ("left".into(), false),
            ("**bold**".into(), false),
            ("**words**".into(), true),
            ("right".into(), false),
        ]
    );
}

/// Drag-selecting assistant Markdown must copy the RENDERED text, never the
/// raw source: `**bold**` copies as `bold`, `` `code` `` as `code`, and a
/// `[label](url)` link as its visible `label`. The selectable line is
/// registered with exactly this string (see `draw_markdown_inline_line`), so
/// the clipboard is marker-free and `slice_line_by_x` divides the drawn width
/// over the rendered characters — no phantom space is reserved for the `**`,
/// `` ` ``, `~~`, or `[]()` markers that are measured-but-never-painted.
#[test]
fn selectable_line_text_is_rendered_not_raw_markdown() {
    let rendered = rendered_inline_text("**bold** and `code`");
    assert_eq!(rendered, "bold and code");
    // No leftover Markdown markers can survive into the clipboard / hit model.
    assert!(!rendered.contains('*'));
    assert!(!rendered.contains('`'));

    assert_eq!(
        rendered_inline_text("see [the docs](file.rs) now"),
        "see the docs now"
    );
    assert_eq!(rendered_inline_text("~~gone~~ kept"), "gone kept");

    // The width the selectable rect uses now derives from the rendered
    // segments (marker-free), so its character count matches the copied text
    // — the two must stay in lock-step for `slice_line_by_x` to map an x
    // range back to the right substring.
    assert_eq!(rendered.chars().count(), "bold and code".chars().count());
}

#[test]
fn web_links_render_as_clickable_labels_or_blue_bare_urls() {
    let segments = parsed_markdown_inline_line(
        "See [Search Engineer](https://jobs.example/one) or https://neoism.dev/docs.",
    );

    assert!(segments.iter().any(|segment| matches!(
        segment,
        MarkdownInlineSegment::MarkdownLink { label, target: Some(target), .. }
            if label == "Search Engineer" && target == "https://jobs.example/one"
    )));
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MarkdownInlineSegment::PlainToken { target: Some(target), .. }
            if target == "https://neoism.dev/docs"
    )));
    assert_eq!(
        rendered_inline_text("[Search Engineer](https://jobs.example/one)"),
        "Search Engineer"
    );

    let wrapped = inline_wrap_tokens("https://neoism.dev/a-very-long-path");
    assert!(wrapped.iter().all(|token| matches!(
        &token.style,
        InlineWrapStyle::MarkdownLink(target) if target == "https://neoism.dev/a-very-long-path"
    )));
}

#[test]
fn explicit_web_links_ignore_destination_edge_whitespace() {
    for source in [
        " [ZhangHanDong](https://github.com/ZhangHanDong) ",
        "[ZhangHanDong]( https://github.com/ZhangHanDong )",
    ] {
        let segments = parsed_markdown_inline_line(source);
        assert!(segments.iter().any(|segment| matches!(
            segment,
            MarkdownInlineSegment::MarkdownLink {
                label,
                source_target,
                target: Some(target),
            } if label == "ZhangHanDong"
                && source_target == "https://github.com/ZhangHanDong"
                && target == "https://github.com/ZhangHanDong"
        )));
    }
}

#[test]
fn file_uri_links_render_as_clickable_labels() {
    let source = "[Report](file:///tmp/Patriot%20Nursing%20Report.md)";
    let segments = parsed_markdown_inline_line(source);
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MarkdownInlineSegment::MarkdownLink { label, target: Some(target), .. }
            if label == "Report" && target == "file:///tmp/Patriot%20Nursing%20Report.md"
    )));
    assert_eq!(rendered_inline_text(source), "Report");
}

#[test]
fn hard_wrapped_file_link_is_joined_without_touching_fenced_code() {
    let source = "[`Reports/Patriot Nursing Messaging and Payment\nReconciliation.md`](file:///tmp/Patriot%20Nursing%20\nReconciliation.md)\n```md\n[a\nb](file:///tmp/a%20\nb.md)\n```";
    let normalized = normalize_multiline_markdown_links(source);
    assert!(normalized.starts_with(
        "[`Reports/Patriot Nursing Messaging and Payment Reconciliation.md`](file:///tmp/Patriot%20Nursing%20Reconciliation.md)"
    ));
    assert!(normalized.contains("```md\n[a\nb](file:///tmp/a%20\nb.md)\n```"));

    let prose = "[IMPORTANT]\nKeep this paragraph.\n[Report](file:///tmp/report.md)";
    assert!(matches!(
        normalize_multiline_markdown_links(prose),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn code_styled_markdown_link_label_drops_its_nested_markers() {
    let source = "[`Reports/Patriot.md`](file:///tmp/Patriot.md)";
    assert_eq!(rendered_inline_text(source), "Reports/Patriot.md");
}

#[test]
fn commonmark_backslash_escapes_render_literal_markers() {
    let source = r"\* note and \** partial";
    assert_eq!(rendered_inline_text(source), "* note and ** partial");
    assert!(!parsed_markdown_inline_line(source)
        .iter()
        .any(|segment| matches!(segment, MarkdownInlineSegment::Bold(_))));
}

#[test]
fn one_link_bridges_whitespace_between_wrapped_word_fragments() {
    let segments = parsed_markdown_inline_line(
        "[Open](https://neoism.dev) [the](https://neoism.dev) [website](https://neoism.dev)",
    );
    assert_eq!(
        bridged_inline_whitespace_target(&segments, 1),
        Some("https://neoism.dev")
    );
    assert_eq!(
        bridged_inline_whitespace_target(&segments, 3),
        Some("https://neoism.dev")
    );

    let different = parsed_markdown_inline_line(
        "[left](https://one.example) [right](https://two.example)",
    );
    assert_eq!(bridged_inline_whitespace_target(&different, 1), None);
}

#[test]
fn outer_blank_blocks_never_inflate_a_message_card() {
    let mut blocks = vec![
        AssistantMarkdownBlock::Blank,
        AssistantMarkdownBlock::Paragraph(vec!["visible".into()]),
        AssistantMarkdownBlock::Blank,
    ];

    trim_outer_blank_blocks(&mut blocks);

    assert!(matches!(
        blocks.as_slice(),
        [AssistantMarkdownBlock::Paragraph(lines)]
            if lines.len() == 1 && lines[0] == "visible"
    ));
}
