use super::*;

fn fixed_node(id: u64, height: f32) -> VirtualNode {
    VirtualNode::new(NodeId::new(id), VirtualNodeKind::MarkdownBlock)
        .with_geometry(NodeGeometry::fixed(height))
        .with_revision(1)
        .with_text_hash(id)
}

#[test]
fn visible_query_scales_with_viewport_not_document_size() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 800.0, 600.0, 1.0,
        )))
        .unwrap();

    let nodes = (0..100_000)
        .map(|ix| fixed_node(ix + 1, 20.0))
        .collect::<Vec<_>>();
    surface
        .apply(VirtualSurfaceCommand::ReplaceAll(nodes))
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 1_000_000.0,
            velocity_y: 0.0,
        }))
        .unwrap();

    let visible = surface.visible_set();
    assert_eq!(surface.metrics().node_count, 100_000);
    assert!(visible.content_height >= 2_000_000.0);
    assert!(
        visible.nodes.len() < 140,
        "visible len was {}",
        visible.nodes.len()
    );
    assert!(visible.nodes.first().unwrap().index > 40_000);
}

#[test]
fn measured_layouts_update_height_index_for_visible_virtual_renderers() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 80.0, 1.0,
        )))
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::ReplaceAll(vec![
            fixed_node(1, 20.0),
            fixed_node(2, 20.0),
            fixed_node(3, 20.0),
        ]))
        .unwrap();

    surface
        .apply(VirtualSurfaceCommand::CommitMeasuredLayouts(vec![
            VirtualMeasuredLayout::new(NodeId::new(2), NodeRevision(1), 100.0, 0.0, 5),
        ]))
        .unwrap();

    assert!((surface.content_height() - 140.0).abs() < 0.01);
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 80.0,
            velocity_y: 0.0,
        }))
        .unwrap();
    let visible = surface.visible_set();
    assert!(visible.nodes.iter().any(|node| node.index == 1));
}

#[test]
fn measured_layouts_clamp_scroll_after_content_shrinks() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 100.0, 1.0,
        )))
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::ReplaceAll(vec![
            fixed_node(1, 100.0),
            fixed_node(2, 100.0),
            fixed_node(3, 100.0),
        ]))
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 200.0,
            velocity_y: 0.0,
        }))
        .unwrap();

    surface
        .apply(VirtualSurfaceCommand::CommitMeasuredLayouts(vec![
            VirtualMeasuredLayout::new(NodeId::new(1), NodeRevision(1), 20.0, 0.0, 1),
            VirtualMeasuredLayout::new(NodeId::new(2), NodeRevision(1), 20.0, 0.0, 1),
            VirtualMeasuredLayout::new(NodeId::new(3), NodeRevision(1), 20.0, 0.0, 1),
        ]))
        .unwrap();

    assert_eq!(surface.scroll().scroll_y, 0.0);
    assert!(!surface.visible_set().nodes.is_empty());
}

#[test]
fn dirty_layout_reflows_only_after_changed_node() {
    let mut surface = VirtualSurface::default();
    surface
        .replace_all(vec![
            fixed_node(1, 10.0),
            fixed_node(2, 10.0),
            fixed_node(3, 10.0),
            fixed_node(4, 10.0),
        ])
        .unwrap();
    surface.resolve_dirty_layout();
    assert_eq!(surface.layouts()[3].bounds.y, 30.0);

    surface
        .apply(VirtualSurfaceCommand::UpsertNode(fixed_node(2, 30.0)))
        .unwrap();
    assert!(surface.metrics().dirty_layout_count > 0);
    surface.resolve_dirty_layout();

    assert_eq!(surface.layouts()[0].bounds.y, 0.0);
    assert_eq!(surface.layouts()[1].bounds.y, 10.0);
    assert_eq!(surface.layouts()[2].bounds.y, 40.0);
    assert_eq!(surface.layouts()[3].bounds.y, 50.0);
    assert_eq!(surface.content_height(), 60.0);
}

#[test]
fn draw_commands_are_visible_only_and_update_cache() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 100.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..1_000).map(|ix| fixed_node(ix + 1, 20.0)).collect())
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 10_000.0,
            velocity_y: 0.0,
        }))
        .unwrap();

    let commands = surface.build_draw_commands();
    assert!(!commands.is_empty());
    assert!(commands.len() < 260, "commands len was {}", commands.len());
    let stats = surface.cache_stats();
    assert!(stats.chunks > 0);
    assert!(stats.gpu_ready > 0);
}

#[test]
fn range_damage_marks_layout_draw_and_gpu() {
    let mut surface = VirtualSurface::default();
    surface
        .replace_all((0..20).map(|ix| fixed_node(ix + 1, 20.0)).collect())
        .unwrap();
    surface.resolve_dirty_layout();
    assert_eq!(surface.metrics().dirty_layout_count, 0);

    surface
        .apply(VirtualSurfaceCommand::MarkRangeDirty {
            start: 3,
            end: 8,
            kind: DirtyKind::Layout,
        })
        .unwrap();
    let metrics = surface.metrics();
    assert_eq!(metrics.dirty_layout_count, 5);
    assert_eq!(metrics.dirty_draw_count, 5);
    assert_eq!(metrics.dirty_gpu_count, 5);
}

#[test]
fn remove_range_updates_height_and_indices() {
    let mut surface = VirtualSurface::default();
    surface
        .replace_all((0..10).map(|ix| fixed_node(ix + 1, 10.0)).collect())
        .unwrap();
    surface.remove_range(2, 5).unwrap();

    assert_eq!(surface.metrics().node_count, 7);
    assert_eq!(surface.content_height(), 70.0);
    assert_eq!(surface.nodes()[2].id, NodeId::new(6));
    assert_eq!(surface.layouts()[2].bounds.y, 20.0);
}

#[test]
fn upsert_nodes_appends_batch_with_one_surface_revision() {
    let mut surface = VirtualSurface::default();
    surface
        .replace_all((0..10).map(|ix| fixed_node(ix + 1, 10.0)).collect())
        .unwrap();
    let revision = surface.revision();

    surface
        .upsert_nodes((0..100).map(|ix| fixed_node(ix + 100, 12.0)).collect())
        .unwrap();

    assert_eq!(surface.metrics().node_count, 110);
    assert_eq!(surface.revision().0, revision.0 + 1);
    assert_eq!(surface.content_height(), 1_300.0);
    assert_eq!(surface.nodes()[10].id, NodeId::new(100));
}

#[test]
fn markdown_adapter_builds_virtual_nodes_for_common_blocks() {
    let mut adapter = VirtualMarkdownAdapter::new("test-md");
    let source = "# Title\n\nParagraph text\n\n```rust\nfn main() {}\n```\n\n| A | B |\n| - | - |\n";
    let batch =
        adapter.build_replace_batch("notes/test.md", source, VirtualSourceRevision(1));
    let mut surface = VirtualSurface::default();
    batch.apply_to(&mut surface).unwrap();

    let stats = adapter.stats();
    assert_eq!(stats.lines, source.lines().count());
    assert_eq!(stats.headings, 1);
    assert_eq!(stats.code_blocks, 1);
    assert_eq!(stats.tables, 1);
    assert!(surface
        .nodes()
        .iter()
        .any(|node| node.kind == VirtualNodeKind::Heading));
    assert!(surface
        .nodes()
        .iter()
        .any(|node| node.kind == VirtualNodeKind::CodeBlock));
    assert!(surface
        .nodes()
        .iter()
        .any(|node| node.kind == VirtualNodeKind::Table));
    let code = surface
        .nodes()
        .iter()
        .find(|node| node.kind == VirtualNodeKind::CodeBlock)
        .expect("code node should exist");
    assert_eq!(code.content.as_ref().unwrap().line_start, 4);
    assert_eq!(code.content.as_ref().unwrap().line_end(), 7);
}

#[test]
fn markdown_adapter_preserves_notebook_cell_boundary_nodes() {
    let mut adapter = VirtualMarkdownAdapter::new("notebook-md");
    let source = "%%neoism_notebook_cell 0 markdown\n# Title\nbody\n```python neoism_notebook_cell=1 neoism_state=idle neoism_count=_\nprint(1)\n```\n";
    let batch =
        adapter.build_replace_batch("notes/test.ipynb", source, VirtualSourceRevision(1));
    let mut surface = VirtualSurface::default();
    batch.apply_to(&mut surface).unwrap();

    let boundary = surface
        .nodes()
        .iter()
        .find(|node| node.kind == VirtualNodeKind::Custom("notebook-cell".to_string()))
        .expect("markdown notebook cell boundary node");
    assert_eq!(boundary.content.as_ref().unwrap().line_start, 0);
    assert!(surface
        .nodes()
        .iter()
        .any(|node| node.kind == VirtualNodeKind::Heading));
    assert!(surface
        .nodes()
        .iter()
        .any(|node| node.kind == VirtualNodeKind::CodeBlock));
}

#[test]
fn large_line_adapter_does_not_merge_across_notebook_cells() {
    let mut adapter = VirtualMarkdownAdapter::new("large-notebook-md");
    let lines = vec![
        "%%neoism_notebook_cell 0 markdown".to_string(),
        "first".to_string(),
        "%%neoism_notebook_cell 1 markdown".to_string(),
        "second".to_string(),
        "```python neoism_notebook_cell=2 neoism_state=idle neoism_count=_".to_string(),
        "print(1)".to_string(),
        "```".to_string(),
    ];
    let batch = adapter.build_replace_batch_from_lines(
        "notes/large.ipynb",
        &lines,
        VirtualSourceRevision(1),
    );
    let mut surface = VirtualSurface::default();
    batch.apply_to(&mut surface).unwrap();

    let boundaries = surface
        .nodes()
        .iter()
        .filter(|node| node.kind == VirtualNodeKind::Custom("notebook-cell".to_string()))
        .count();
    assert_eq!(boundaries, 2);
    assert!(surface.nodes().iter().all(|node| {
        let content = node.content.as_ref().expect("source-backed node");
        content.line_end() <= 4 || content.line_start >= 4
    }));
}

#[test]
fn markdown_adapter_chunks_large_sources_without_line_per_node_parse() {
    let mut adapter = VirtualMarkdownAdapter::new("large-md");
    let source = (0..12_000)
        .map(|ix| {
            format!(
                "## Section {ix}\n- [ ] item {ix} with [[dir/file-{ix}.md]] and `code`\n- [x] done {ix}\n| A | B |\n| - | - |\n| {ix} | value |\n```rust\nfn section_{ix}() {{}}\n```\n"
            )
        })
        .collect::<String>();
    assert!(source.len() > 2 * 1024 * 1024);
    let line_count = source.lines().count();

    let batch =
        adapter.build_replace_batch("notes/huge.md", &source, VirtualSourceRevision(1));
    let mut surface = VirtualSurface::default();
    batch.apply_to(&mut surface).unwrap();

    let stats = adapter.stats();
    assert_eq!(stats.lines, line_count);
    assert!(stats.headings > 0);
    assert!(stats.tables > 0);
    assert!(stats.code_blocks > 0);
    assert!(
        stats.nodes < line_count,
        "large markdown built {} nodes for {line_count} lines",
        stats.nodes
    );
    assert_eq!(stats.nodes, surface.nodes().len());
}

#[test]
fn markdown_adapter_stream_append_uses_upsert_batch() {
    let mut adapter = VirtualMarkdownAdapter::new("agent-md");
    let mut surface = VirtualSurface::default();
    adapter
        .build_replace_batch("agent-session", "# Start\n", VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();
    let revision = surface.revision();

    let append = (0..128)
        .map(|ix| format!("streamed paragraph {ix}\n"))
        .collect::<String>();
    let batch = adapter
        .build_append_batch("agent-session", &append, VirtualSourceRevision(2))
        .expecting_surface_revision(revision);
    batch.apply_to(&mut surface).unwrap();

    assert_eq!(surface.revision().0, revision.0 + 1);
    assert_eq!(adapter.stats().nodes, surface.nodes().len());
    assert_eq!(
        adapter.stats().lines,
        "# Start\n".lines().count() + append.lines().count()
    );
    assert!(surface.nodes().len() > 100);
    assert!(surface
        .nodes()
        .last()
        .and_then(|node| node.source_range)
        .is_some_and(|range| range.start > "# Start\n".len() as u64));
    assert_eq!(
        surface
            .nodes()
            .last()
            .unwrap()
            .content
            .as_ref()
            .unwrap()
            .line_end(),
        adapter.stats().lines as u64
    );
}

#[test]
fn markdown_adapter_append_at_preserves_source_line_windows() {
    let mut adapter = VirtualMarkdownAdapter::new("append-at-md");
    let mut surface = VirtualSurface::default();
    let initial = "alpha";
    adapter
        .build_replace_batch("notes/append.md", initial, VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();

    let appended = "beta\n- [x] task\n";
    adapter
        .build_append_batch_at(
            "notes/append.md",
            appended,
            initial.len() as u64 + 1,
            1,
            VirtualSourceRevision(2),
        )
        .apply_to(&mut surface)
        .unwrap();

    let last = surface.nodes().last().unwrap();
    let content = last.content.as_ref().unwrap();
    assert_eq!(content.line_start, 2);
    assert_eq!(content.line_end(), 3);
    assert_eq!(last.source_range.unwrap().start, initial.len() as u64 + 1);
    assert_eq!(adapter.stats().lines, 3);
}

#[test]
fn markdown_adapter_content_hash_survives_byte_offset_shifts() {
    let mut first_adapter = VirtualMarkdownAdapter::new("stable-md");
    let mut first_surface = VirtualSurface::default();
    first_adapter
        .build_replace_batch(
            "notes/stable.md",
            "short\n- [ ] reusable task\n",
            VirtualSourceRevision(1),
        )
        .apply_to(&mut first_surface)
        .unwrap();

    let mut second_adapter = VirtualMarkdownAdapter::new("stable-md");
    let mut second_surface = VirtualSurface::default();
    second_adapter
        .build_replace_batch(
            "notes/stable.md",
            "much longer first line\n- [ ] reusable task\n",
            VirtualSourceRevision(2),
        )
        .apply_to(&mut second_surface)
        .unwrap();

    let first_task = first_surface
        .nodes()
        .iter()
        .find(|node| node.source_range.is_some_and(|range| range.start > 0))
        .expect("first task node should exist");
    let second_task = second_surface
        .nodes()
        .iter()
        .find(|node| node.source_range.is_some_and(|range| range.start > 0))
        .expect("second task node should exist");
    assert_ne!(first_task.source_range, second_task.source_range);
    assert_eq!(first_task.id, second_task.id);
    assert_eq!(first_task.revision, second_task.revision);
    assert_eq!(first_task.text_hash, second_task.text_hash);
    assert_eq!(
        first_task.content.as_ref().unwrap().hash,
        second_task.content.as_ref().unwrap().hash
    );
    assert_ne!(
        first_task.content.as_ref().unwrap().id,
        second_task.content.as_ref().unwrap().id
    );
}

#[test]
fn replace_all_reuses_unchanged_markdown_chunks_after_source_shift() {
    let mut adapter = VirtualMarkdownAdapter::new("reuse-md");
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 600.0, 400.0, 1.0,
        )))
        .unwrap();
    adapter
        .build_replace_batch(
            "notes/reuse.md",
            "short\n- [ ] reusable task\n",
            VirtualSourceRevision(1),
        )
        .apply_to(&mut surface)
        .unwrap();
    let first_frame = surface.build_frame_transaction();
    let first_task = surface
        .nodes()
        .iter()
        .find(|node| node.source_range.is_some_and(|range| range.start > 0))
        .map(|node| node.id)
        .expect("task node should exist");
    surface
        .commit_frame_transaction(&first_frame, &first_frame.successful_commit())
        .unwrap();

    adapter
        .build_replace_batch(
            "notes/reuse.md",
            "much longer first line\n- [ ] reusable task\n",
            VirtualSourceRevision(2),
        )
        .apply_to(&mut surface)
        .unwrap();
    let second_frame = surface.build_frame_transaction();
    assert!(second_frame.plan.nodes.iter().any(|plan| {
        plan.node == first_task && plan.action == VirtualFrameAction::Reuse
    }));
}

#[test]
fn markdown_adapter_edit_batch_marks_source_blocks_dirty() {
    let mut adapter = VirtualMarkdownAdapter::new("edit-md");
    let source = "# Title\n\nParagraph text\n\n- [ ] task\n";
    let mut surface = VirtualSurface::default();
    adapter
        .build_replace_batch("notes/edit.md", source, VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();
    surface.resolve_dirty_layout();
    let frame = surface.build_frame_transaction();
    surface
        .commit_frame_transaction(&frame, &frame.successful_commit())
        .unwrap();

    let paragraph_range = surface
        .nodes()
        .iter()
        .find(|node| {
            node.kind == VirtualNodeKind::MarkdownBlock
                && node.source_range.is_some_and(|range| {
                    source[range.start as usize..range.end as usize].contains("Paragraph")
                })
        })
        .and_then(|node| node.source_range)
        .expect("paragraph block should have a source range");
    let before = surface.revision();
    adapter
        .build_edit_batch(
            "notes/edit.md",
            paragraph_range,
            NodeSourceRange::new(paragraph_range.start, paragraph_range.end + 8),
            VirtualSourceRevision(2),
            DirtyKind::Draw,
        )
        .expecting_surface_revision(before)
        .apply_to(&mut surface)
        .unwrap();

    assert_eq!(surface.revision().0, before.0 + 1);
    let dirty = surface.build_frame_transaction();
    assert!(dirty.plan.nodes.iter().any(|plan| {
        surface.nodes()[plan.index].source_range == Some(paragraph_range)
            && plan.action == VirtualFrameAction::RebuildDraw
    }));
}

#[test]
fn agent_adapter_edit_batch_marks_message_dirty() {
    let mut adapter = VirtualAgentAdapter::new("edit-agent");
    let messages = vec![
        VirtualAgentMessage {
            id: "user-1".to_string(),
            role: VirtualAgentRole::User,
            markdown: "first message\n".to_string(),
            tool_name: None,
        },
        VirtualAgentMessage {
            id: "assistant-1".to_string(),
            role: VirtualAgentRole::Assistant,
            markdown: "assistant markdown\n- [ ] todo\n".to_string(),
            tool_name: None,
        },
        VirtualAgentMessage {
            id: "tool-1".to_string(),
            role: VirtualAgentRole::Tool,
            markdown: "tool output\n".to_string(),
            tool_name: Some("shell".to_string()),
        },
    ];
    let mut surface = VirtualSurface::default();
    adapter
        .build_replace_batch("session-edit", &messages, VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();
    surface.resolve_dirty_layout();
    let frame = surface.build_frame_transaction();
    surface
        .commit_frame_transaction(&frame, &frame.successful_commit())
        .unwrap();

    let old_range = surface.nodes()[1].source_range.unwrap();
    let old_revision = surface.nodes()[1].revision;
    let before = surface.revision();
    adapter
        .build_update_message_batch(
            "session-edit",
            VirtualAgentMessageUpdate {
                index: 1,
                message: VirtualAgentMessage {
                    markdown: "assistant markdown\n- [x] todo\nnew tail\n".to_string(),
                    ..messages[1].clone()
                },
                old_range,
                new_range: NodeSourceRange::new(old_range.start, old_range.end + 9),
                kind: DirtyKind::Draw,
            },
            VirtualSourceRevision(2),
        )
        .expecting_surface_revision(before)
        .apply_to(&mut surface)
        .unwrap();

    assert_eq!(surface.revision().0, before.0 + 1);
    assert_ne!(surface.nodes()[1].revision, old_revision);
    assert_eq!(surface.nodes()[1].revision.0, surface.nodes()[1].text_hash);
    assert_eq!(
        surface.nodes()[1].content.as_ref().unwrap().byte_len,
        "assistant markdown\n- [x] todo\nnew tail\n".len() as u64
    );
    let dirty = surface.build_frame_transaction();
    assert!(dirty.plan.nodes.iter().any(|plan| {
        plan.index == 1 && plan.action == VirtualFrameAction::RebuildDraw
    }));
}

#[test]
fn sparse_text_line_index_seeks_without_per_line_entries() {
    let source = (0..100_000)
        .map(|ix| format!("line {ix}: some markdown or code payload\n"))
        .collect::<String>();
    let index = VirtualTextLineIndex::with_checkpoint_lines(&source, 512);
    let stats = index.stats();

    assert_eq!(stats.line_count, 100_000);
    assert_eq!(stats.byte_len, source.len() as u64);
    assert!(stats.checkpoints < 200);
    assert!(stats.trailing_newline);

    let line_90k = source.find("line 90000").unwrap() as u64;
    assert_eq!(index.line_for_byte(&source, line_90k), 90_000);
    let range = index.line_range(&source, 90_000).unwrap();
    assert_eq!(
        &source[range.start as usize..range.end as usize],
        "line 90000: some markdown or code payload\n"
    );
    let slice = index.byte_range_for_lines(&source, 90_000..90_010).unwrap();
    assert!(slice.len() < 512);
}

#[test]
fn code_adapter_streams_big_buffers_into_stable_tiles() {
    let mut adapter = VirtualCodeAdapter::with_config(
        "test-code",
        VirtualCodeAdapterConfig {
            tile_lines: 3,
            line_height_px: 10,
            glyph_width_px: 7,
            index_checkpoint_lines: 4,
        },
    );
    let source = (0..10)
        .map(|ix| format!("let line_{ix} = compute_value({ix});\n"))
        .collect::<String>();
    let mut surface = VirtualSurface::default();
    adapter
        .build_replace_batch("buffer://test.rs", &source, VirtualSourceRevision(7))
        .apply_to(&mut surface)
        .unwrap();

    let stats = adapter.stats();
    assert_eq!(stats.lines, 10);
    assert_eq!(stats.tiles, 4);
    assert_eq!(stats.bytes, source.len());
    assert!(stats.trailing_newline);
    assert!(stats.max_line_bytes >= "let line_9".len());
    assert_eq!(surface.nodes().len(), 4);
    let line_index = adapter.build_line_index(&source);
    assert_eq!(line_index.stats().checkpoints, 3);
    assert_eq!(
        line_index.line_for_byte(&source, source.find("line_8").unwrap() as u64),
        8
    );
    assert_eq!(
        line_index.byte_range_for_lines(&source, 7..10).unwrap().end,
        source.len() as u64
    );

    let first = &surface.nodes()[0];
    assert_eq!(first.kind, VirtualNodeKind::CodeTile);
    assert_eq!(first.geometry.fixed_height, Some(30.0));
    assert!(first.geometry.min_width > 0.0);
    assert_eq!(first.content.as_ref().unwrap().line_count, 3);
    assert_eq!(first.content.as_ref().unwrap().line_start, 0);
    assert_eq!(first.content.as_ref().unwrap().line_end(), 3);
    assert_eq!(
        first.content.as_ref().unwrap().byte_len,
        first.source_range.unwrap().len()
    );
    assert_eq!(first.source_range.unwrap().start, 0);

    let target_start = source.find("line_8").unwrap() as u64;
    let matches = surface.source_matches(VirtualSourceQuery::new(
        NodeSource::CodeBuffer {
            buffer: "buffer://test.rs".to_string(),
        },
        NodeSourceRange::new(target_start, target_start + 6),
    ));
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].index, 2);
}

#[test]
fn code_adapter_append_keeps_byte_ranges_monotonic() {
    let mut adapter = VirtualCodeAdapter::with_config(
        "append-code",
        VirtualCodeAdapterConfig {
            tile_lines: 2,
            ..VirtualCodeAdapterConfig::default()
        },
    );
    let mut surface = VirtualSurface::default();
    let first = "alpha\nbeta\n";
    adapter
        .build_replace_batch("buffer://append.rs", first, VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();

    let second = "gamma\ndelta\n";
    adapter
        .build_append_batch("buffer://append.rs", second, VirtualSourceRevision(2))
        .expecting_surface_revision(surface.revision())
        .apply_to(&mut surface)
        .unwrap();

    assert_eq!(adapter.stats().lines, 4);
    assert_eq!(adapter.stats().tiles, 2);
    let last = surface.nodes().last().unwrap();
    let range = last.source_range.unwrap();
    assert_eq!(range.start, first.len() as u64);
    assert_eq!(range.end, (first.len() + second.len()) as u64);
    assert_eq!(last.content.as_ref().unwrap().line_count, 2);
    assert_eq!(last.content.as_ref().unwrap().line_start, 2);
}

#[test]
fn source_text_store_resolves_visible_tiles_from_one_backing_source() {
    let mut adapter = VirtualCodeAdapter::with_config(
        "source-code",
        VirtualCodeAdapterConfig {
            tile_lines: 2,
            index_checkpoint_lines: 2,
            ..VirtualCodeAdapterConfig::default()
        },
    );
    let source_text = "alpha\nbeta\ngamma\ndelta\n";
    let mut surface = VirtualSurface::default();
    adapter
        .build_replace_batch("buffer://source.rs", source_text, VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();

    let node = &surface.nodes()[1];
    let content = node.content.as_ref().unwrap().clone();
    let mut store = VirtualSourceTextStore::new();
    store.insert(
        NodeSource::CodeBuffer {
            buffer: "buffer://source.rs".to_string(),
        },
        NodeRevision(1),
        source_text,
    );

    let payload = store
        .resolve_content(&VirtualGpuContentRequest {
            content: content.clone(),
            text_plan: node.text_plan.clone(),
        })
        .unwrap();
    assert_eq!(payload.text, "gamma\ndelta\n");
    assert_eq!(store.len(), 1);
    let index = store
        .line_index(&content.source, content.revision)
        .expect("source line index should exist");
    assert_eq!(index.line_for_byte(source_text, content.range.start), 2);
    assert_eq!(index.stats().checkpoints, 2);
}

#[test]
fn source_text_store_append_reindexes_only_tail_window() {
    let source = NodeSource::AgentMessage {
        session: "stream".to_string(),
        message: "tail".to_string(),
    };
    let revision = NodeRevision(7);
    let initial = (0..2_048)
        .map(|ix| format!("streamed markdown line {ix}\n"))
        .collect::<String>();
    let append = (0..64)
        .map(|ix| format!("next model chunk {ix}\n"))
        .collect::<String>();
    let mut store = VirtualSourceTextStore::new();
    store.insert_with_index_config(
        source.clone(),
        revision,
        initial.clone(),
        VirtualTextLineIndexConfig {
            checkpoint_lines: 128,
        },
    );

    let next_revision = NodeRevision(8);
    let stats = store
        .append_revision(source.clone(), revision, next_revision, &append)
        .unwrap();
    assert_eq!(stats.previous_line_count, 2_048);
    assert_eq!(stats.line_count, 2_112);
    assert_eq!(stats.appended_bytes, append.len());
    assert_eq!(stats.reindexed_from_line, 1_920);
    assert!(stats.reindexed_bytes < append.len() as u64 + 128 * 32);

    assert!(store.entry(&source, revision).is_none());
    let entry = store.entry(&source, next_revision).unwrap();
    assert_eq!(entry.text.len(), initial.len() + append.len());
    assert_eq!(
        entry
            .line_index
            .line_for_byte(&entry.text, initial.len() as u64),
        2_048
    );
    let appended_range = entry.line_index.line_range(&entry.text, 2_050).unwrap();
    assert!(
        entry.text[appended_range.start as usize..appended_range.end as usize]
            .starts_with("next model chunk 2")
    );
}

#[test]
fn source_text_store_replace_range_reindexes_from_nearest_checkpoint() {
    let source = NodeSource::CodeBuffer {
        buffer: "buffer://edit.rs".to_string(),
    };
    let revision = NodeRevision(10);
    let initial = (0..2_048)
        .map(|ix| format!("let value_{ix} = {ix};\n"))
        .collect::<String>();
    let mut store = VirtualSourceTextStore::new();
    store.insert_with_index_config(
        source.clone(),
        revision,
        initial,
        VirtualTextLineIndexConfig {
            checkpoint_lines: 128,
        },
    );
    let edit_range = {
        let entry = store.entry(&source, revision).unwrap();
        entry.line_index.line_range(&entry.text, 2_000).unwrap()
    };
    let next_revision = NodeRevision(11);
    let replacement = "let value_2000 = patched();\n";
    let stats = store
        .replace_range_revision(
            source.clone(),
            revision,
            next_revision,
            edit_range,
            replacement,
        )
        .unwrap();

    assert_eq!(stats.previous_line_count, 2_048);
    assert_eq!(stats.line_count, 2_048);
    assert_eq!(stats.reindexed_from_line, 1_920);
    assert!(stats.reindexed_bytes < 8_192);
    assert!(store.entry(&source, revision).is_none());
    let entry = store.entry(&source, next_revision).unwrap();
    let patched_range = entry.line_index.line_range(&entry.text, 2_000).unwrap();
    assert_eq!(
        &entry.text[patched_range.start as usize..patched_range.end as usize],
        replacement
    );

    let content = VirtualContentRef::new(
        VirtualContentId(0xED17),
        VirtualContentKind::Code { language: None },
        source,
        patched_range,
        next_revision,
        0xFEED,
        1,
    )
    .with_line_start(2_000);
    let payload = store
        .resolve_content(&VirtualGpuContentRequest {
            content,
            text_plan: None,
        })
        .unwrap();
    assert_eq!(payload.text, replacement);
}

#[test]
fn source_edit_command_marks_old_and_new_source_ranges_dirty() {
    let mut adapter = VirtualCodeAdapter::with_config(
        "edit-command-code",
        VirtualCodeAdapterConfig {
            tile_lines: 4,
            line_height_px: 10,
            ..VirtualCodeAdapterConfig::default()
        },
    );
    let source = (0..16)
        .map(|ix| format!("let edit_line_{ix} = {ix};\n"))
        .collect::<String>();
    let mut surface = VirtualSurface::default();
    adapter
        .build_replace_batch(
            "buffer://edit-command.rs",
            &source,
            VirtualSourceRevision(1),
        )
        .apply_to(&mut surface)
        .unwrap();
    surface.resolve_dirty_layout();
    let frame = surface.build_frame_transaction();
    surface
        .commit_frame_transaction(&frame, &frame.successful_commit())
        .unwrap();

    let old_range = surface.nodes()[1].source_range.unwrap();
    let new_range = NodeSourceRange::new(old_range.start + 8, old_range.end + 12);
    let before = surface.revision();
    adapter
        .build_edit_batch(
            "buffer://edit-command.rs",
            old_range,
            new_range,
            VirtualSourceRevision(2),
            DirtyKind::Draw,
        )
        .expecting_surface_revision(before)
        .apply_to(&mut surface)
        .unwrap();

    assert_eq!(surface.revision().0, before.0 + 1);
    let dirty = surface.build_frame_transaction();
    assert!(dirty.plan.nodes.iter().any(|plan| {
        plan.index == 1 && plan.action == VirtualFrameAction::RebuildDraw
    }));
}

#[test]
fn gpu_packet_prefetch_content_is_bounded() {
    let mut adapter = VirtualCodeAdapter::with_config(
        "prefetch-code",
        VirtualCodeAdapterConfig {
            tile_lines: 1,
            line_height_px: 10,
            ..VirtualCodeAdapterConfig::default()
        },
    );
    let source = (0..80)
        .map(|ix| format!("let prefetch_line_{ix} = {ix};\n"))
        .collect::<String>();
    let mut surface = VirtualSurface::new(VirtualSurfaceConfig {
        warm_distance_px: 1_000.0,
        ..VirtualSurfaceConfig::default()
    });
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 600.0, 20.0, 1.0,
        )))
        .unwrap();
    adapter
        .build_replace_batch("buffer://prefetch.rs", &source, VirtualSourceRevision(1))
        .apply_to(&mut surface)
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 100.0,
            velocity_y: 2_000.0,
        }))
        .unwrap();

    let frame = surface.build_frame_transaction();
    assert!(frame.plan.prefetch.len() > 2);
    let packet = VirtualGpuFramePacket::from_transaction_with_content_prefetch_policy(
        &frame,
        VirtualGpuContentPrefetchPolicy {
            max_requests: 2,
            max_bytes: 128,
        },
    );
    packet.validate().unwrap();

    assert!(!packet.content_requests.is_empty());
    assert!(packet.prefetch_content_requests.len() <= 2);
    assert!(packet.stats().prefetch_content_bytes <= 128);
    assert!(packet.stats().source_windows > 0);
    assert!(packet.stats().prefetch_source_windows <= 2);
    assert!(packet.stats().source_window_bytes > 0);
    let visible_ids = packet
        .content_requests
        .iter()
        .map(|request| request.content.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(packet
        .prefetch_content_requests
        .iter()
        .all(|request| !visible_ids.contains(&request.content.id)));
    let visible_window = packet
        .source_windows
        .iter()
        .find(|window| !window.prefetch)
        .expect("visible source window should exist");
    assert_eq!(visible_window.line_start, 10);
    assert!(visible_window.line_end > visible_window.line_start);
}

#[test]
fn frame_plan_distinguishes_build_reuse_and_dirty_rebuild() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 100.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..20).map(|ix| fixed_node(ix + 1, 20.0)).collect())
        .unwrap();

    let first = surface.build_frame_transaction();
    assert!(first.plan.build > 0);
    assert_eq!(first.plan.reused, 0);
    surface
        .commit_frame_transaction(&first, &first.successful_commit())
        .unwrap();

    let second = surface.build_frame_transaction();
    assert_eq!(second.plan.build, 0);
    assert!(second.plan.reused > 0);
    surface
        .commit_frame_transaction(&second, &second.successful_commit())
        .unwrap();

    surface
        .apply(VirtualSurfaceCommand::MarkDirty {
            node: NodeId::new(3),
            kind: DirtyKind::Draw,
        })
        .unwrap();
    let dirty = surface.build_frame_transaction();
    assert_eq!(dirty.plan.rebuild_draw, 1);
}

#[test]
fn frame_transaction_carries_resource_ops_and_commands() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 120.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..100).map(|ix| fixed_node(ix + 1, 24.0)).collect())
        .unwrap();

    let first = surface.build_frame_transaction();
    assert!(first.plan.build > 0);
    assert!(!first.resource_ops.is_empty());
    assert!(!first.commands.is_empty());
    assert!(first.command_bytes > 0);
    assert!(first.resource_ops.iter().any(|op| matches!(
        op,
        VirtualResourceOp::BuildDrawList(descriptor)
            if descriptor.kind == VirtualResourceKind::CpuDrawList
    )));
    assert!(first.resource_ops.iter().any(|op| matches!(
        op,
        VirtualResourceOp::BuildHitRegion(descriptor)
            if descriptor.kind == VirtualResourceKind::HitRegion
    )));
    assert!(first.resource_ops.iter().any(|op| matches!(
        op,
        VirtualResourceOp::UploadGpuBuffer(descriptor)
            if descriptor.kind == VirtualResourceKind::GpuDrawBuffer
    )));
    let stats = first.stats();
    assert_eq!(stats.build_draw_list, first.plan.build);
    assert_eq!(stats.build_hit_region, first.plan.build);
    assert_eq!(stats.upload_gpu_buffer, first.plan.build);
    surface
        .commit_frame_transaction(&first, &first.successful_commit())
        .unwrap();

    let second = surface.build_frame_transaction();
    assert_eq!(second.plan.build, 0);
    assert!(second.plan.reused > 0);
    assert!(second
        .resource_ops
        .iter()
        .any(|op| matches!(op, VirtualResourceOp::Retain(_))));
}

#[test]
fn frame_transaction_does_not_reuse_until_backend_commit() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 120.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..20).map(|ix| fixed_node(ix + 1, 24.0)).collect())
        .unwrap();

    let first = surface.build_frame_transaction();
    assert!(first.plan.build > 0);
    let second = surface.build_frame_transaction();
    assert_eq!(second.plan.reused, 0);
    assert!(second.plan.rebuild_draw > 0);
}

#[test]
fn failed_backend_commit_preserves_dirty_gpu_state() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 120.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..20).map(|ix| fixed_node(ix + 1, 24.0)).collect())
        .unwrap();

    let frame = surface.build_frame_transaction();
    let mut commit = frame.successful_commit();
    let failed = commit.ready.pop().unwrap();
    commit.failed.push(VirtualResourceFailure {
        id: failed,
        message: "synthetic failure".to_string(),
    });

    let err = surface
        .commit_frame_transaction(&frame, &commit)
        .unwrap_err();
    assert_eq!(err, VirtualSurfaceError::ResourceCommitFailed { failed: 1 });
    assert!(surface.metrics().dirty_gpu_count > 0);
}

#[test]
fn splittable_large_nodes_emit_tile_resource_descriptors() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 300.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all(vec![VirtualNode::new(
            NodeId::new(1),
            VirtualNodeKind::CodeBlock,
        )
        .with_geometry(NodeGeometry {
            estimated_height: Some(4_096.0),
            fixed_height: Some(4_096.0),
            can_split: true,
            ..NodeGeometry::default()
        })
        .with_revision(1)
        .with_text_hash(42)])
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 1_100.0,
            velocity_y: 0.0,
        }))
        .unwrap();

    let frame = surface.build_frame_transaction();
    let plan = frame.plan.nodes.first().unwrap();
    assert!(plan.tile_range.start > 0);
    assert!(plan.tile_range.len() < 8);
    assert!(frame
        .resource_ops
        .iter()
        .filter_map(VirtualResourceOp::descriptor)
        .any(|descriptor| descriptor.tile.is_some()));
}

#[test]
fn hit_test_uses_height_index_and_scroll_offset() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            10.0, 20.0, 400.0, 100.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..20).map(|ix| fixed_node(ix + 1, 25.0)).collect())
        .unwrap();
    surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: 50.0,
            velocity_y: 0.0,
        }))
        .unwrap();

    let hit = surface
        .hit_test(VirtualHitTest { x: 15.0, y: 30.0 })
        .expect("point should hit a visible node");
    assert_eq!(hit.index, 2);
    assert_eq!(hit.node, NodeId::new(3));
    assert_eq!(hit.local_y, 10.0);

    assert!(surface
        .hit_test(VirtualHitTest { x: 0.0, y: 30.0 })
        .is_none());
}

#[test]
fn replace_all_rejects_duplicate_node_ids() {
    let mut surface = VirtualSurface::default();
    let err = surface
        .replace_all(vec![fixed_node(7, 10.0), fixed_node(7, 20.0)])
        .unwrap_err();
    assert_eq!(err, VirtualSurfaceError::DuplicateNode(NodeId::new(7)));
    assert_eq!(surface.metrics().node_count, 0);
}

#[test]
fn frame_transaction_emits_drops_for_removed_retained_chunks() {
    let mut surface = VirtualSurface::default();
    surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, 0.0, 400.0, 120.0, 1.0,
        )))
        .unwrap();
    surface
        .replace_all((0..20).map(|ix| fixed_node(ix + 1, 24.0)).collect())
        .unwrap();

    let first = surface.build_frame_transaction();
    assert!(first.plan.build > 0);
    surface
        .commit_frame_transaction(&first, &first.successful_commit())
        .unwrap();

    surface.remove_node(NodeId::new(1)).unwrap();
    let second = surface.build_frame_transaction();
    assert!(second
        .resource_ops
        .iter()
        .any(|op| matches!(op, VirtualResourceOp::Drop(_))));
}
