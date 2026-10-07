use super::*;

#[derive(Default)]
struct Pane;
impl AgentMarkdownPane for Pane {
    fn cached_markdown_blocks_for(
        &self,
        _: &str,
        _: f32,
        _: f32,
    ) -> Option<Rc<Vec<AssistantMarkdownBlock>>> {
        None
    }
    fn store_markdown_blocks_for(
        &self,
        _: &str,
        _: f32,
        _: f32,
        _: Rc<Vec<AssistantMarkdownBlock>>,
    ) {
    }
    fn register_selectable_line(&mut self, _: &str, _: [f32; 4]) -> usize {
        0
    }
    fn selectable_line_highlight(&self, _: usize) -> Option<(f32, f32)> {
        None
    }
    fn register_link_hit_rect(&mut self, _: String, _: [f32; 4]) {}
    fn link_hovered(&self, _: &str) -> bool {
        false
    }
    fn mermaid_raw_mode(&self, _: u64) -> bool {
        false
    }
}

// Exercise the same sanitizer, semantic lines, fence delimiters, completed
// promotion and EOF fallback as layout, without requiring a GPU for text widths.
fn fence_block(text: &str) -> AssistantMarkdownBlock {
    let safe = safe_canvas_markdown(text);
    let normalized = normalize_multiline_markdown_links(&safe);
    let mut code: Option<(md::FenceDelimiter, String, Vec<String>)> = None;
    for raw in semantic_markdown_lines(&normalized) {
        if code
            .as_ref()
            .is_some_and(|(fence, _, _)| fence.closes(&raw))
        {
            let (_, lang, lines) = code.take().unwrap();
            return completed_fence(&lang, &lines)
                .unwrap_or_else(|| markdown_code_or_stock_block(lang, lines));
        }
        if let Some((_, _, lines)) = &mut code {
            lines.push(raw);
        } else if let Some((fence, info)) = md::fence_open(&raw) {
            code = Some((fence, info.to_owned(), Vec::new()));
        }
    }
    let (_, lang, lines) = code.unwrap();
    markdown_code_or_stock_block(lang, lines)
}

#[test]
fn completed_explicit_fences_preserve_html_source() {
    let source =
        "<!doctype html>\n<!-- keep -->\n<script>const a = '<b>';\nalert(a)</script>";
    for fence in ["```", "~~~~"] {
        let text = format!("{fence}neoism-html\n{source}\n{fence}");
        assert_eq!(safe_canvas_markdown(&text), text);
        let AssistantMarkdownBlock::HtmlArtifact {
            source: actual,
            copy_target,
        } = fence_block(&text)
        else {
            panic!("expected artifact")
        };
        assert_eq!(actual, source);
        assert_eq!(
            copy_target,
            format!("{COPY_LINK_PREFIX}{}", escape_copy_target(source))
        );
    }
}

#[test]
fn sanitizer_removes_surrounding_html_without_touching_artifact_payload() {
    let text = "<!-- outside -->\n\n```neoism-html\n<!-- inside -->\n<script>keep()</script>\n```\n\n<!-- tail -->";
    let safe = safe_canvas_markdown(text);
    assert!(!safe.contains("outside"));
    assert!(!safe.contains("tail"));
    let AssistantMarkdownBlock::HtmlArtifact { source, .. } = fence_block(text) else {
        panic!("expected artifact after sanitizing surrounding prose");
    };
    assert_eq!(source, "<!-- inside -->\n<script>keep()</script>");
}

#[test]
fn unfinished_or_mismatched_fences_remain_code() {
    for text in [
        "```neoism-html\n<script>keep()</script>",
        "````neoism-html\n<b>keep</b>\n```",
        "~~~neoism-html\n<b>keep</b>\n```",
    ] {
        assert_eq!(safe_canvas_markdown(text), text);
        assert!(
            matches!(fence_block(text), AssistantMarkdownBlock::Code { lang, .. } if lang == "neoism-html")
        );
    }
}

#[test]
fn generic_html_and_other_info_strings_never_promote() {
    for lang in ["html", "HTML", "neoism-html extra", "NEOISM-HTML", "xml"] {
        let text = format!("```{lang}\n<script>inert()</script>\n```");
        assert_eq!(safe_canvas_markdown(&text), text);
        assert!(matches!(
            fence_block(&text),
            AssistantMarkdownBlock::Code { .. }
        ));
    }
    assert_eq!(
        safe_canvas_markdown("before <script>unsafe()</script> after"),
        "before  after"
    );
}

#[test]
fn long_source_completion_keeps_the_same_headerless_slot() {
    let source = "<p>one HTML source line</p>\n".repeat(500);
    let incomplete = fence_block(&format!("```neoism-html\n{source}"));
    let complete = fence_block(&format!("```neoism-html\n{source}```"));
    for scale in [0.75, 1.0, 1.5, 2.0] {
        assert_eq!(
            markdown_block_height(&incomplete, 500.0, &Pane, scale),
            320.0 * scale
        );
        assert_eq!(
            markdown_block_height(&complete, 500.0, &Pane, scale),
            320.0 * scale
        );
        // Availability is host draw state, not a property of measured blocks.
        assert_eq!(
            measure_markdown_blocks(&[complete.clone()], 500.0, &Pane, scale),
            334.0 * scale
        );
        assert_eq!(
            estimated_body_height(&format!("```neoism-html\n{source}```"), 80, scale),
            320.0 * scale
        );
        assert_eq!(
            estimated_body_height(&format!("```neoism-html\n{source}"), 80, scale),
            320.0 * scale
        );
    }
}

#[test]
fn fixed_viewport_and_measurement_agree_at_all_scales() {
    let block = fence_block("```neoism-html\n<p>hello</p>\n```");
    for scale in [0.75, 1.0, 1.5, 2.0] {
        let h = markdown_block_height(&block, 500.0, &Pane, scale);
        assert_eq!(h, 320.0 * scale);
        assert_eq!(
            measure_markdown_blocks(std::slice::from_ref(&block), 500.0, &Pane, scale),
            h + 14.0 * scale
        );
    }
}

#[test]
fn identity_is_namespace_and_index_not_source_revision() {
    assert_eq!(
        artifact_key("thread:part", 7),
        "markdown:thread:part:html:7"
    );
    assert_ne!(artifact_key("thread:a", 7), artifact_key("thread:b", 7));
    assert_ne!(artifact_key("thread:a", 7), artifact_key("thread:a", 8));
    let mut pane = Pane;
    assert_eq!(
        pane.html_artifact_frame(HtmlArtifactRequest {
            key: artifact_key("thread:part", 7),
            html: "changed source".into(),
            viewport: [0.0, 0.0, 100.0, 320.0],
            visible_rect: [0.0, 0.0, 100.0, 320.0],
            scale: 1.0,
        }),
        None
    );
}

#[test]
fn host_activation_requires_actual_visibility_without_occlusion_or_suppression() {
    let viewport = [10.0, 100.0, 200.0, 320.0];
    let clip = [0.0, 0.0, 300.0, 200.0];
    assert_eq!(
        request_visible_rect(viewport, clip, false, &[]),
        Some([10.0, 100.0, 200.0, 100.0])
    );
    assert_eq!(request_visible_rect(viewport, clip, true, &[]), None);
    assert_eq!(
        request_visible_rect(viewport, [0.0, 0.0, 300.0, 50.0], false, &[]),
        None
    );
    assert_eq!(
        request_visible_rect(viewport, clip, false, &[[20.0, 110.0, 10.0, 10.0]]),
        None
    );
    assert!(
        request_visible_rect(viewport, clip, false, &[[20.0, 300.0, 10.0, 10.0]])
            .is_some()
    );
}
