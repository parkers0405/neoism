use super::*;

#[derive(Default)]
struct Pane {
    layout: Option<(String, HtmlArtifactLayout)>,
    status: Option<HtmlArtifactStatus>,
}
impl AgentMarkdownPane for Pane {
    fn html_artifact_status(&self, _: &str) -> HtmlArtifactStatus {
        self.status.unwrap_or(HtmlArtifactStatus::Disabled)
    }
    fn html_artifact_height(
        &self,
        key: &str,
        source: &str,
        width: f32,
        scale: f32,
    ) -> Option<f32> {
        self.layout
            .as_ref()
            .filter(|(identity, layout)| {
                identity == key && layout.matches(source, width, scale)
            })
            .map(|(_, layout)| layout.height)
    }
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
            markdown_block_height(&incomplete, 500.0, &Pane::default(), scale),
            160.0 * scale
        );
        assert_eq!(
            markdown_block_height(&complete, 500.0, &Pane::default(), scale),
            160.0 * scale
        );
        // Availability is host draw state, not a property of measured blocks.
        assert_eq!(
            measure_markdown_blocks(&[complete.clone()], 500.0, &Pane::default(), scale),
            174.0 * scale
        );
        assert_eq!(
            estimated_body_height(&format!("```neoism-html\n{source}```"), 80, scale),
            160.0 * scale
        );
        assert_eq!(
            estimated_body_height(&format!("```neoism-html\n{source}"), 80, scale),
            160.0 * scale
        );
    }
}

#[test]
fn initial_viewport_and_measurement_agree_at_all_scales() {
    let block = fence_block("```neoism-html\n<p>hello</p>\n```");
    for scale in [0.75, 1.0, 1.5, 2.0] {
        let h = markdown_block_height(&block, 500.0, &Pane::default(), scale);
        assert_eq!(h, 160.0 * scale);
        assert_eq!(
            measure_markdown_blocks(
                std::slice::from_ref(&block),
                500.0,
                &Pane::default(),
                scale
            ),
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
    let mut pane = Pane::default();
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
fn learned_height_measurement_matches_render_cursor_and_retains_identity() {
    let source = "<p>hello</p>";
    let block = fence_block(&format!("```neoism-html\n{source}\n```"));
    for scale in [0.75, 1.0, 2.0] {
        for (css, bounded) in [(12.0, 32.0), (48.0, 48.0), (520.0, 520.0), (900.0, 640.0)]
        {
            // A preceding ordinary paragraph must count toward the artifact key.
            let blocks =
                vec![AssistantMarkdownBlock::Paragraph(Vec::new()), block.clone()];
            let pane = Pane {
                layout: Some((
                    artifact_key("part:a", 1),
                    HtmlArtifactLayout {
                        source_hash: html_artifact_source_hash(source),
                        width: 500.0 - 30.0 * scale,
                        scale,
                        height: css,
                    },
                )),
                ..Pane::default()
            };
            let render_height = markdown_block_height_namespaced(
                &block, 500.0, &pane, scale, "part:a", 1,
            );
            assert_eq!(render_height, bounded * scale);
            assert_eq!(
                measure_markdown_blocks_namespaced(
                    &blocks, 500.0, &pane, scale, "part:a"
                ),
                (8.0 + 19.0 + 6.0 + bounded + 6.0) * scale
            );
            // Hidden/offscreen passes are read-only and retain the learned metric.
            for _ in 0..3 {
                assert_eq!(
                    markdown_block_height_namespaced(
                        &block, 500.0, &pane, scale, "part:a", 1
                    ),
                    render_height
                );
            }
            assert_eq!(
                markdown_block_height_namespaced(
                    &block, 501.0, &pane, scale, "part:a", 1
                ),
                INITIAL_HEIGHT * scale
            );
            assert_eq!(
                markdown_block_height_namespaced(
                    &block, 500.0, &pane, scale, "part:b", 1
                ),
                INITIAL_HEIGHT * scale
            );
            assert_eq!(
                markdown_block_height_namespaced(
                    &block, 500.0, &pane, scale, "part:a", 0
                ),
                INITIAL_HEIGHT * scale
            );
            assert_eq!(
                resolved_height(
                    &pane,
                    "part:a",
                    1,
                    "changed",
                    500.0 - 30.0 * scale,
                    scale
                ),
                INITIAL_HEIGHT * scale
            );
        }
    }
}

#[test]
fn snapshot_compatibility_rejects_obsolete_source_size_scale_and_tiny_raster() {
    let frame = HtmlArtifactFrame {
        image_id: 1,
        width: 100,
        height: 160,
        source_hash: html_artifact_source_hash("source"),
        viewport: [100.0, 160.0],
        scale: 1.0,
    };
    assert!(frame.matches("source", [100.25, 160.25], 1.0));
    for dims in [
        [100.5, 160.0],
        [100.0, 160.5],
        [200.0, 160.0],
        [100.0, 32.0],
        [0.0, 160.0],
        [f32::NAN, 160.0],
    ] {
        assert!(!frame.matches("source", dims, 1.0));
    }
    assert!(!frame.matches("changed", frame.viewport, 1.0));
    assert!(!frame.matches("source", frame.viewport, 1.0001));
    assert!(!HtmlArtifactFrame { width: 0, ..frame }.matches(
        "source",
        frame.viewport,
        1.0
    ));
    let layout = HtmlArtifactLayout {
        source_hash: frame.source_hash,
        width: 100.0,
        scale: 1.0,
        height: 48.0,
    };
    assert!(layout.matches("source", 100.25, 1.0));
    assert!(!layout.matches("source", 100.5, 1.0));
    assert!(!layout.matches("changed", 100.0, 1.0));
    assert!(!layout.matches("source", 100.0, 1.0001));
}

#[test]
fn clipped_uvs_are_translation_invariant_and_use_resolved_height() {
    for height in [32.0, 160.0, 640.0] {
        let piece = image_piece(
            [10.0, 100.0, 200.0, height],
            [60.0, 100.0 + height * 0.25, 100.0, height * 0.5],
        )
        .unwrap();
        assert_eq!(piece.source_rect, [0.25, 0.25, 0.75, 0.75]);
        let translated = image_piece(
            [10.0, -200.0, 200.0, height],
            [60.0, -200.0 + height * 0.25, 100.0, height * 0.5],
        )
        .unwrap();
        assert_eq!(piece.source_rect, translated.source_rect);
    }
}

#[test]
fn pending_and_unavailable_are_distinct_without_affecting_placeholder_height() {
    assert_eq!(
        Pane::default().html_artifact_status("key"),
        HtmlArtifactStatus::Disabled
    );
    for status in [
        HtmlArtifactStatus::Pending,
        HtmlArtifactStatus::Failed,
        HtmlArtifactStatus::Disabled,
    ] {
        let pane = Pane {
            status: Some(status),
            ..Pane::default()
        };
        assert_eq!(pane.html_artifact_status("key"), status);
        assert_eq!(
            resolved_height(&pane, "part", 0, "source", 100.0, 1.0),
            160.0
        );
    }
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
