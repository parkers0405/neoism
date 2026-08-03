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
    cells: &[NotebookCell],
) -> Vec<NotebookCellRange> {
    if fallback.is_empty() {
        return Vec::new();
    }

    let anchors = rendered_cell_anchors(lines);
    let mut discovered = Vec::with_capacity(fallback.len());
    for range in fallback {
        let expected_id = cells
            .get(range.cell_index)
            .and_then(encoded_notebook_cell_id);
        let Some((anchor_position, (_, _, start))) =
            anchors
                .iter()
                .enumerate()
                .find(|(_, (cell_index, cell_id, _))| {
                    expected_id
                        .as_ref()
                        .is_some_and(|expected| cell_id.as_ref() == Some(expected))
                        || (cell_id.is_none() && *cell_index == range.cell_index)
                })
        else {
            continue;
        };
        let end = anchors
            .get(anchor_position + 1)
            .map(|(_, _, next_start)| next_start.saturating_sub(1))
            .unwrap_or_else(|| lines.len().saturating_sub(1));
        let mut next = range.clone();
        next.line_start = *start;
        next.line_end = end.max(*start);
        next.run_line = None;
        discovered.push(next);
    }
    discovered
}

fn rendered_cell_anchors(lines: &[String]) -> Vec<(usize, Option<String>, usize)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(line, text)| {
            crate::editor::markdown::helpers::notebook_markdown_cell_index(text)
                .or_else(|| {
                    crate::editor::markdown::helpers::notebook_fenced_cell_index(text)
                })
                .map(|cell_index| {
                    let cell_id = text
                        .split_whitespace()
                        .find_map(|part| part.strip_prefix("neoism_notebook_id="))
                        .filter(|id| !id.is_empty())
                        .map(str::to_string);
                    (cell_index, cell_id, line)
                })
        })
        .collect()
}

pub(crate) fn encoded_notebook_cell_id(cell: &NotebookCell) -> Option<String> {
    notebook_cell_id(cell).map(|id| URL_SAFE_NO_PAD.encode(id.as_bytes()))
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
    if body
        .first()
        .is_some_and(|line| line.trim_start().starts_with(&opening))
    {
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
            trimmed != "---"
                && !is_notebook_output_marker_line(trimmed)
                && !crate::editor::markdown::helpers::is_notebook_markdown_cell_marker_line(
                    trimmed,
                )
        })
        .cloned()
        .collect()
}
