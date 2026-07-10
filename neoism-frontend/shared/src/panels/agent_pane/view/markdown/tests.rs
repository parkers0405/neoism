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
    fn table_cell_lines_keeps_empty_cells_visible() {
        assert_eq!(table_cell_lines(""), vec![String::new()]);
        assert_eq!(
            table_cell_lines(" one \n\n two "),
            vec!["one".to_string(), "two".to_string()]
        );
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
