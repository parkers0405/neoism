fn collect_visible_items(pane: &mut MarkdownPane) -> Vec<VirtualMarkdownDrawItem> {
    let state = &mut pane.virtual_render;
    let visible = state.surface.visible_set();
    let mut items = Vec::with_capacity(visible.nodes.len());
    for visible_node in visible.nodes {
        let Some(node) = state.surface.nodes().get(visible_node.index) else {
            continue;
        };
        let Some(range) = node.source_range else {
            continue;
        };
        let content = node.content.as_ref();
        let text = if let Some(text) = source_slice(&state.source, range) {
            text.to_string()
        } else if let Some(content) = content {
            node_text_from_lines(
                &pane.lines,
                content.line_start as usize,
                content.line_count as usize,
            )
        } else {
            continue;
        };
        let measured_layout = state
            .surface
            .layouts()
            .get(visible_node.index)
            .is_some_and(|layout| layout.visual_line_count > 0);
        let first_line = content
            .map(|content| content.line_start as usize)
            .unwrap_or_else(|| state.line_for_byte(range.start as usize));
        let line_count = content
            .map(|content| content.line_count as usize)
            .unwrap_or_else(|| text.lines().count().max(1));
        let item = VirtualMarkdownDrawItem {
            node: node.id,
            revision: node.revision,
            kind: node.kind.clone(),
            text_hash: node.text_hash,
            bounds: visible_node.bounds,
            screen_y: visible_node.screen_y,
            first_line,
            line_count,
            text,
            measured_layout,
        };
        items.push(item);
    }
    for item in &mut items {
        if item.measured_layout {
            item.bounds.height = (item.bounds.height
                - notebook_cell_bottom_gap(pane, item.first_line, item.line_count))
            .max(1.0);
        }
    }
    items
}

fn source_slice(source: &str, range: NodeSourceRange) -> Option<&str> {
    if source.is_empty() {
        return None;
    }
    let start = range.start as usize;
    let end = (range.end as usize).min(source.len());
    source.get(start..end)
}

fn large_virtual_markdown(pane: &MarkdownPane) -> bool {
    pane.should_defer_block_parse()
}

fn virtual_item_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        vec![String::new()]
    } else {
        text.lines().map(ToOwned::to_owned).collect()
    }
}

fn virtual_item_line(text: &str, line: usize) -> &str {
    if text.is_empty() && line == 0 {
        ""
    } else {
        text.lines().nth(line).unwrap_or_default()
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (ix, ch) in source.char_indices() {
        if ch == '\n' && ix + 1 < source.len() {
            starts.push(ix + 1);
        }
    }
    starts
}

fn line_starts_from_lines(lines: &[String]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(lines.len().max(1));
    let mut offset = 0usize;
    for line in lines {
        starts.push(offset);
        offset = offset.saturating_add(line.len()).saturating_add(1);
    }
    if starts.is_empty() {
        starts.push(0);
    }
    starts
}

fn apply_large_line_starts_edit(
    starts: &mut Vec<usize>,
    edit: LargeLineStartsEdit,
    lines: &[String],
) {
    match edit {
        LargeLineStartsEdit::Insert { line, byte_delta } => {
            let line = line.min(lines.len());
            let inserted_start = if line == 0 {
                0
            } else {
                starts
                    .get(line - 1)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(lines.get(line - 1).map(String::len).unwrap_or(0))
                    .saturating_add(1)
            };
            starts.insert(line.min(starts.len()), inserted_start);
            shift_line_starts(&mut starts[line.saturating_add(1)..], byte_delta);
        }
        LargeLineStartsEdit::Delete { line, byte_delta } => {
            if starts.is_empty() {
                starts.push(0);
                return;
            }
            let line = line.min(starts.len().saturating_sub(1));
            starts.remove(line);
            if starts.is_empty() {
                starts.push(0);
            } else {
                let shift_from = line.min(starts.len());
                shift_line_starts(&mut starts[shift_from..], byte_delta);
            }
        }
    }
}

fn shift_line_starts(starts: &mut [usize], byte_delta: i64) {
    for start in starts {
        *start = if byte_delta >= 0 {
            start.saturating_add(byte_delta as usize)
        } else {
            start.saturating_sub(byte_delta.unsigned_abs() as usize)
        };
    }
}

fn node_text_from_lines(lines: &[String], start: usize, count: usize) -> String {
    let end = start.saturating_add(count).min(lines.len());
    if start >= end {
        return String::new();
    }
    let mut out = String::with_capacity(joined_line_range_len(lines, start, end));
    for (ix, line) in lines[start..end].iter().enumerate() {
        if ix > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn joined_line_range_len(lines: &[String], start: usize, end: usize) -> usize {
    let end = end.min(lines.len());
    if start >= end {
        return 0;
    }
    lines[start..end].iter().map(String::len).sum::<usize>() + end - start - 1
}

fn node_for_line(
    state: &MarkdownVirtualRenderState,
    line: usize,
) -> Option<(usize, NodeId, usize, usize, VirtualNodeKind)> {
    let nodes = state.surface.nodes();
    let mut lo = 0usize;
    let mut hi = nodes.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some(content) = nodes[mid].content.as_ref() else {
            return None;
        };
        let start = content.line_start as usize;
        let count = (content.line_count as usize).max(1);
        let end = start.saturating_add(count);
        if line < start {
            hi = mid;
        } else if line >= end {
            lo = mid + 1;
        } else {
            return Some((mid, nodes[mid].id, start, count, nodes[mid].kind.clone()));
        }
    }
    None
}

fn appended_line_segment<'a>(
    previous: &str,
    next: &'a str,
    previous_line_count: usize,
) -> Option<(&'a str, u64, u64)> {
    if previous.is_empty() {
        return Some((next, 0, 0));
    }
    if !next.starts_with(previous) || next.len() <= previous.len() {
        return None;
    }
    let suffix = &next[previous.len()..];
    if !suffix.starts_with('\n') {
        return None;
    }
    let source_start_byte = previous.len().saturating_add(1);
    Some((
        &next[source_start_byte..],
        source_start_byte as u64,
        previous_line_count as u64,
    ))
}

fn apply_tail_inline_append(
    pane: &mut MarkdownPane,
    source_id: &str,
    content_w: f32,
    viewport_y: f32,
    viewport_h: f32,
) -> bool {
    let Some(suffix) = tail_inline_append_suffix(pane, source_id) else {
        return false;
    };
    let revision = pane.source_revision.max(1);
    let state = &mut pane.virtual_render;
    let Some(last_index) = state.surface.nodes().len().checked_sub(1) else {
        return false;
    };
    let Some(last_node) = state.surface.nodes().get(last_index).cloned() else {
        return false;
    };
    let Some(range) = last_node.source_range else {
        return false;
    };
    let source_start_line = last_node
        .content
        .as_ref()
        .map(|content| content.line_start)
        .unwrap_or_else(|| state.line_starts.len().saturating_sub(1) as u64);

    state.source.push_str(&suffix);
    let batch = state.adapter.build_tail_update_batch_at(
        source_id,
        &state.source,
        last_index as u64,
        range.start,
        source_start_line,
        last_node.kind,
        VirtualSourceRevision(revision),
    );
    if batch.apply_to(&mut state.surface).is_err() {
        return false;
    }
    state.source_id = source_id.to_string();
    state.source_revision = revision;

    state
        .surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, viewport_y, content_w, viewport_h, 1.0,
        )))
        .is_ok()
        && state
            .surface
            .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
                scroll_y: pane.scroll_y.max(0.0),
                velocity_y: pane.scroll_velocity_px_s,
            }))
            .is_ok()
}

fn tail_inline_append_suffix(pane: &MarkdownPane, source_id: &str) -> Option<String> {
    let state = &pane.virtual_render;
    if state.source_id != source_id
        || state.source.is_empty()
        || state.line_starts.len() != pane.lines.len()
    {
        return None;
    }
    let last_start = *state.line_starts.last()?;
    let previous_tail = state.source.get(last_start..)?;
    let current_tail = pane.lines.last()?.as_str();
    let suffix = current_tail.strip_prefix(previous_tail)?;
    if suffix.is_empty() || suffix.contains(['\r', '\n']) {
        return None;
    }
    Some(suffix.to_string())
}

fn floor_char_boundary(text: &str, mut ix: usize) -> usize {
    ix = ix.min(text.len());
    while ix > 0 && !text.is_char_boundary(ix) {
        ix -= 1;
    }
    ix
}
