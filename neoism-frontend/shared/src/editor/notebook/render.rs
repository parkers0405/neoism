use super::*;

pub(crate) fn append_cell_source(markdown: &mut String, source: &str) {
    if source.is_empty() {
        markdown.push('\n');
    } else {
        markdown.push_str(source);
        ensure_trailing_newline(markdown);
    }
}

pub(crate) fn markdown_line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

pub(crate) fn discover_rendered_cell_ranges(
    lines: &[String],
    fallback: &[NotebookCellRange],
) -> Vec<NotebookCellRange> {
    if fallback.is_empty() {
        return Vec::new();
    }

    let mut ranges = fallback.to_vec();
    let mut present = vec![false; ranges.len()];
    for (idx, range) in ranges.iter_mut().enumerate() {
        if range.kind != NotebookCellType::Code {
            continue;
        }

        let Some((start, end)) = find_rendered_code_cell_range(lines, range.cell_index)
        else {
            continue;
        };
        range.line_start = start;
        range.line_end = end;
        range.run_line = None;
        present[idx] = true;
    }

    let mut segment_start = 0usize;
    let mut non_code_segment = Vec::new();
    for idx in 0..ranges.len() {
        if present[idx] && ranges[idx].kind == NotebookCellType::Code {
            let code_start = ranges[idx].line_start;
            let code_end = ranges[idx].line_end;
            assign_non_code_segment(
                &mut ranges,
                &mut present,
                &non_code_segment,
                segment_start,
                code_start,
            );
            non_code_segment.clear();
            segment_start = code_end.saturating_add(1);
        } else if ranges[idx].kind != NotebookCellType::Code {
            non_code_segment.push(idx);
        }
    }
    assign_non_code_segment(
        &mut ranges,
        &mut present,
        &non_code_segment,
        segment_start,
        lines.len(),
    );

    ranges
        .into_iter()
        .enumerate()
        .filter_map(|(idx, range)| present[idx].then_some(range))
        .collect()
}

pub(crate) fn assign_non_code_segment(
    ranges: &mut [NotebookCellRange],
    present: &mut [bool],
    indices: &[usize],
    start: usize,
    end_exclusive: usize,
) {
    if indices.is_empty() || start >= end_exclusive {
        return;
    }

    let mut cursor = start;
    for (pos, idx) in indices.iter().copied().enumerate() {
        let remaining_cells = indices.len().saturating_sub(pos + 1);
        let remaining_available = end_exclusive.saturating_sub(cursor);
        if remaining_available == 0 {
            break;
        }

        let fallback_len = ranges[idx]
            .line_end
            .saturating_sub(ranges[idx].line_start)
            .saturating_add(1)
            .max(1);
        let take = if remaining_cells == 0 {
            remaining_available
        } else {
            fallback_len
                .min(remaining_available.saturating_sub(remaining_cells))
                .max(1)
        };

        ranges[idx].line_start = cursor;
        ranges[idx].line_end = cursor.saturating_add(take).saturating_sub(1);
        present[idx] = true;
        cursor = cursor.saturating_add(take);
    }
}

pub(crate) fn find_rendered_code_cell_range(
    lines: &[String],
    target_cell_index: usize,
) -> Option<(usize, usize)> {
    let mut idx = 0usize;
    while idx < lines.len() {
        let Some(cell_index) = notebook_cell_index_from_fence(&lines[idx]) else {
            idx += 1;
            continue;
        };
        let start = idx;
        idx += 1;
        while idx < lines.len() && lines[idx].trim() != "```" {
            idx += 1;
        }
        if idx < lines.len() {
            idx += 1;
        }
        while idx < lines.len() && is_notebook_output_marker_line(&lines[idx]) {
            idx += 1;
        }
        if cell_index == target_cell_index {
            return Some((start, idx.saturating_sub(1)));
        }
    }
    None
}

pub(crate) fn notebook_cell_index_from_fence(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("```") {
        return None;
    }
    trimmed
        .split_whitespace()
        .find_map(|part| part.strip_prefix("neoism_notebook_cell="))
        .and_then(|value| value.parse::<usize>().ok())
}

pub(crate) fn notebook_cell_id(cell: &NotebookCell) -> Option<&str> {
    cell.extra
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
}

pub(crate) fn generated_cell_id(
    index: usize,
    cell: &NotebookCell,
    used: &BTreeSet<String>,
) -> String {
    let mut hasher = DefaultHasher::new();
    index.hash(&mut hasher);
    notebook_cell_type_tag(cell.cell_type).hash(&mut hasher);
    cell.source.as_str().hash(&mut hasher);
    let mut candidate = format!("neoism-{index}-{:016x}", hasher.finish());
    let mut suffix = 1usize;
    while used.contains(&candidate) {
        candidate = format!("neoism-{index}-{suffix}");
        suffix = suffix.saturating_add(1);
    }
    candidate
}

pub(crate) fn notebook_cell_type_tag(kind: NotebookCellType) -> &'static str {
    match kind {
        NotebookCellType::Markdown => "markdown",
        NotebookCellType::Code => "code",
        NotebookCellType::Raw => "raw",
    }
}

pub(crate) fn source_from_rendered_cell(
    lines: &[String],
    kind: NotebookCellType,
) -> String {
    match kind {
        NotebookCellType::Markdown => {
            let body = trim_generated_separators(lines);
            if body.len() == 1 && body[0].is_empty() {
                String::new()
            } else {
                join_rendered_source_lines(&body)
            }
        }
        NotebookCellType::Raw => unfence_rendered_source(lines, "text"),
        NotebookCellType::Code => {
            let mut start = None;
            let mut end = None;
            for (idx, line) in lines.iter().enumerate() {
                if start.is_none() && line.trim_start().starts_with("```") {
                    start = Some(idx + 1);
                    continue;
                }
                if start.is_some() && line.trim_start() == "```" {
                    end = Some(idx);
                    break;
                }
            }
            match (start, end) {
                (Some(start), Some(end)) if start <= end && end <= lines.len() => {
                    let body = &lines[start..end];
                    if body.len() == 1 && body[0].is_empty() {
                        String::new()
                    } else {
                        join_rendered_source_lines(body)
                    }
                }
                _ => String::new(),
            }
        }
    }
}

pub(crate) fn join_rendered_source_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut source = lines.join("\n");
        if lines.last().is_some_and(|line| line.is_empty()) {
            source.push('\n');
        }
        source
    }
}

pub(crate) fn unfence_rendered_source(lines: &[String], lang: &str) -> String {
    let opening = format!("```{lang}");
    let mut body = lines;
    if body.first().is_some_and(|line| line.trim() == opening) {
        body = &body[1..];
    }
    if body.last().is_some_and(|line| line.trim() == "```") {
        body = &body[..body.len().saturating_sub(1)];
    }
    join_rendered_source_lines(body)
}

pub(crate) fn trim_generated_separators(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != "---" && !is_notebook_output_marker_line(trimmed)
        })
        .cloned()
        .collect()
}
