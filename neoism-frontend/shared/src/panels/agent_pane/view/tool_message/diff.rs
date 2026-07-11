use super::*;

#[derive(Clone, Debug)]
struct ParsedDiffRow {
    kind: DiffLineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
}

fn edit_diff_sections(message: &impl AgentToolMessage) -> Option<Vec<ToolDiffSection>> {
    if !is_edit_tool_message(message) && !looks_like_patch(message.detail()) {
        return None;
    }

    let mut sections = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(message.detail()) {
        if value.get("neoismToolDetail").and_then(Value::as_str) == Some("edit") {
            let metadata = value.get("metadata").unwrap_or(&Value::Null);
            sections = metadata_file_diff_sections(metadata);
            if sections.is_empty() {
                sections = snapshot_diff_sections(metadata);
            }
            if sections.is_empty() {
                let input = value.get("input").unwrap_or(&Value::Null);
                sections = input_diff_sections(
                    input,
                    metadata,
                    fallback_edit_path(metadata, message).as_deref(),
                );
            }
            attach_section_diagnostics(&mut sections, metadata);
        }
    }

    if sections.is_empty() {
        let fallback = fallback_edit_path(&Value::Null, message)
            .unwrap_or_else(|| "patch".to_string());
        for raw in [message.detail(), message.text()] {
            if looks_like_patch(raw) {
                sections = patch_diff_sections(raw, &fallback);
                if !sections.is_empty() {
                    break;
                }
            }
        }
    }

    sections.retain(|section| !section.lines.is_empty());
    (!sections.is_empty()).then_some(sections)
}

thread_local! {
    static EDIT_DIFF_SECTION_CACHE: RefCell<HashMap<EditDiffCacheKey, CachedToolDiffSections>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct EditDiffCacheKey {
    id: u64,
    title: u64,
    detail: u64,
    text: u64,
    status: u64,
    tool: u64,
}

pub fn cached_edit_diff_sections(
    message: &impl AgentToolMessage,
) -> Option<CachedToolDiffSections> {
    cached_edit_diff_sections_for_parts(
        message.id(),
        message.title(),
        message.text(),
        message.status(),
        message.tool(),
        message.detail(),
    )
}

pub fn cached_edit_diff_sections_for_parts(
    id: &str,
    title: &str,
    text: &str,
    status: &str,
    tool: &str,
    detail: &str,
) -> Option<CachedToolDiffSections> {
    let key = EditDiffCacheKey {
        id: hash_value(&id),
        title: hash_value(&title),
        detail: hash_value(&detail),
        text: hash_value(&text),
        status: hash_value(&status),
        tool: hash_value(&tool),
    };
    EDIT_DIFF_SECTION_CACHE.with(|cache| {
        if let Some(sections) = cache.borrow().get(&key).cloned() {
            return (!sections.is_empty()).then_some(sections);
        }
        super::super::derivations::bump_tool_diff_sections();
        let message = ToolMessageParts {
            id,
            title,
            text,
            status,
            tool,
            detail,
        };
        let sections = Rc::new(edit_diff_sections(&message).unwrap_or_default());
        let mut cache = cache.borrow_mut();
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(key, sections.clone());
        (!sections.is_empty()).then_some(sections)
    })
}

pub(crate) fn hash_value<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn is_edit_tool_message(message: &impl AgentToolMessage) -> bool {
    let normalized = message
        .tool()
        .chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "applypatch" | "patch" | "edit" | "write" | "multiedit"
    )
}

fn looks_like_patch(raw: &str) -> bool {
    raw.contains("*** Begin Patch")
        || raw.contains("diff --git ")
        || (raw.contains("\n--- ") && raw.contains("\n+++ ") && raw.contains("\n@@"))
}

fn input_diff_sections(
    input: &Value,
    metadata: &Value,
    fallback_path: Option<&str>,
) -> Vec<ToolDiffSection> {
    if let Some(patch) = string_field(input, &["patchText", "patch", "diff", "content"])
        .filter(|patch| looks_like_patch(patch))
    {
        let fallback = fallback_path.unwrap_or("patch");
        let sections = patch_diff_sections(patch, fallback);
        if !sections.is_empty() {
            return sections;
        }
    }

    let path = string_field(input, &["filePath", "file_path", "path"])
        .or_else(|| metadata_path(metadata))
        .or(fallback_path)
        .unwrap_or("edit");
    let old_text = string_field(input, &["oldString", "old", "oldText"]).unwrap_or("");
    let new_text =
        string_field(input, &["newString", "new", "newText", "content"]).unwrap_or("");
    if old_text.is_empty() && new_text.is_empty() {
        return Vec::new();
    }
    vec![snapshot_section_from_text(
        path.to_string(),
        old_text,
        new_text,
    )]
}

fn metadata_file_diff_sections(metadata: &Value) -> Vec<ToolDiffSection> {
    let mut sections = metadata
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(metadata_file_diff_section)
        .collect::<Vec<_>>();
    if !sections.is_empty() {
        return sections;
    }
    if let Some(diff) = metadata
        .get("diff")
        .and_then(Value::as_str)
        .filter(|diff| looks_like_patch(diff))
    {
        sections = unified_patch_sections(diff, "patch");
    }
    sections
}

fn metadata_file_diff_section(file: &Value) -> Option<ToolDiffSection> {
    let patch = file.get("patch").and_then(Value::as_str)?;
    let raw_path = file
        .get("relativePath")
        .or_else(|| file.get("filePath"))
        .and_then(Value::as_str)
        .unwrap_or("patch");
    let mut sections = unified_patch_sections(patch, raw_path);
    let mut section = sections.pop()?;
    let status = file.get("type").and_then(Value::as_str).unwrap_or("update");
    let label = match status {
        "add" | "added" => format!("A {raw_path}"),
        "delete" | "deleted" => format!("D {raw_path}"),
        "move" | "moved" => file
            .get("movePath")
            .and_then(Value::as_str)
            .map(|target| format!("R {raw_path} -> {target}"))
            .unwrap_or_else(|| format!("R {raw_path}")),
        _ => format!("M {raw_path}"),
    };
    section.path = label;
    if let Some(additions) = file.get("additions").and_then(Value::as_u64) {
        section.additions = additions.min(u32::MAX as u64) as u32;
    }
    if let Some(deletions) = file.get("deletions").and_then(Value::as_u64) {
        section.deletions = deletions.min(u32::MAX as u64) as u32;
    }
    Some(section)
}

fn snapshot_diff_sections(metadata: &Value) -> Vec<ToolDiffSection> {
    metadata
        .get("snapshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(snapshot_diff_section)
        .collect()
}

fn snapshot_diff_section(snapshot: &Value) -> Option<ToolDiffSection> {
    let path = snapshot.get("path").and_then(Value::as_str)?.to_string();
    let before = snapshot_text(snapshot.get("before")?)?;
    let after = snapshot_text(snapshot.get("after")?)?;
    Some(snapshot_section_from_text(path, &before, &after))
}

fn snapshot_text(state: &Value) -> Option<String> {
    if !state
        .get("exists")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(String::new());
    }
    let encoded = state
        .get("contentBase64")
        .or_else(|| state.get("content_base64"))
        .and_then(Value::as_str)?;
    String::from_utf8(STANDARD.decode(encoded).ok()?).ok()
}

pub(crate) fn snapshot_section_from_text(
    path: String,
    before: &str,
    after: &str,
) -> ToolDiffSection {
    let before_lines = before.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let after_lines = after.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let rows = compact_line_diff(&before_lines, &after_lines);
    section_from_rows(path, rows)
}

fn compact_line_diff(before: &[String], after: &[String]) -> Vec<ParsedDiffRow> {
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix + prefix < before.len()
        && suffix + prefix < after.len()
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let before_change_end = before.len().saturating_sub(suffix);
    let after_change_end = after.len().saturating_sub(suffix);
    let context = 3usize;
    let before_start = prefix.saturating_sub(context);
    let before_context_end = prefix;
    let before_tail_end = (before_change_end + context).min(before.len());
    let after_tail_end = (after_change_end + context).min(after.len());

    let mut rows = Vec::new();
    if before_start > 0 {
        rows.push(ParsedDiffRow {
            kind: DiffLineKind::Hunk,
            old_line: None,
            new_line: None,
            text: "...".to_string(),
        });
    }
    for index in before_start..before_context_end {
        rows.push(ParsedDiffRow {
            kind: DiffLineKind::Context,
            old_line: Some(index + 1),
            new_line: Some(index + 1),
            text: before[index].clone(),
        });
    }
    rows.extend(paired_line_diff(
        &before[prefix..before_change_end],
        &after[prefix..after_change_end],
        prefix + 1,
        prefix + 1,
    ));
    for (before_index, after_index) in
        (before_change_end..before_tail_end).zip(after_change_end..after_tail_end)
    {
        rows.push(ParsedDiffRow {
            kind: DiffLineKind::Context,
            old_line: Some(before_index + 1),
            new_line: Some(after_index + 1),
            text: after[after_index].clone(),
        });
    }
    rows
}

fn paired_line_diff(
    before: &[String],
    after: &[String],
    old_start: usize,
    new_start: usize,
) -> Vec<ParsedDiffRow> {
    let mut rows = Vec::new();
    for index in 0..before.len().max(after.len()) {
        match (before.get(index), after.get(index)) {
            (Some(old), Some(new)) if old == new => rows.push(ParsedDiffRow {
                kind: DiffLineKind::Context,
                old_line: Some(old_start + index),
                new_line: Some(new_start + index),
                text: new.clone(),
            }),
            (Some(old), Some(new)) => {
                rows.push(ParsedDiffRow {
                    kind: DiffLineKind::Remove,
                    old_line: Some(old_start + index),
                    new_line: None,
                    text: old.clone(),
                });
                rows.push(ParsedDiffRow {
                    kind: DiffLineKind::Add,
                    old_line: None,
                    new_line: Some(new_start + index),
                    text: new.clone(),
                });
            }
            (Some(old), None) => rows.push(ParsedDiffRow {
                kind: DiffLineKind::Remove,
                old_line: Some(old_start + index),
                new_line: None,
                text: old.clone(),
            }),
            (None, Some(new)) => rows.push(ParsedDiffRow {
                kind: DiffLineKind::Add,
                old_line: None,
                new_line: Some(new_start + index),
                text: new.clone(),
            }),
            (None, None) => {}
        }
    }
    rows
}

fn pair_adjacent_change_runs(rows: Vec<ParsedDiffRow>) -> Vec<ParsedDiffRow> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if rows[index].kind != DiffLineKind::Remove {
            out.push(rows[index].clone());
            index += 1;
            continue;
        }
        let remove_start = index;
        while index < rows.len() && rows[index].kind == DiffLineKind::Remove {
            index += 1;
        }
        let add_start = index;
        while index < rows.len() && rows[index].kind == DiffLineKind::Add {
            index += 1;
        }
        if add_start == index {
            out.extend_from_slice(&rows[remove_start..add_start]);
            continue;
        }
        let removes = &rows[remove_start..add_start];
        let adds = &rows[add_start..index];
        for pair_index in 0..removes.len().max(adds.len()) {
            if let Some(remove) = removes.get(pair_index) {
                out.push(remove.clone());
            }
            if let Some(add) = adds.get(pair_index) {
                out.push(add.clone());
            }
        }
    }
    out
}

fn patch_diff_sections(patch: &str, fallback_path: &str) -> Vec<ToolDiffSection> {
    if patch.contains("*** Begin Patch") {
        let sections = v4a_patch_sections(patch);
        if !sections.is_empty() {
            return sections;
        }
    }
    if patch.contains("diff --git ")
        || (patch.contains("\n--- ") && patch.contains("\n+++ "))
    {
        return unified_patch_sections(patch, fallback_path);
    }
    Vec::new()
}

fn v4a_patch_sections(patch: &str) -> Vec<ToolDiffSection> {
    let lines = patch.lines().collect::<Vec<_>>();
    let mut sections = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(rest) = line.strip_prefix("*** Add File:") {
            let path = rest.trim().to_string();
            index += 1;
            let mut rows = Vec::new();
            let mut new_line = 1usize;
            while index < lines.len() && !lines[index].starts_with("***") {
                if let Some(text) = lines[index].strip_prefix('+') {
                    rows.push(ParsedDiffRow {
                        kind: DiffLineKind::Add,
                        old_line: None,
                        new_line: Some(new_line),
                        text: text.to_string(),
                    });
                    new_line += 1;
                }
                index += 1;
            }
            sections.push(section_from_rows(path, pair_adjacent_change_runs(rows)));
            continue;
        }
        if let Some(rest) = line.strip_prefix("*** Delete File:") {
            sections.push(section_from_rows(
                rest.trim().to_string(),
                vec![ParsedDiffRow {
                    kind: DiffLineKind::Remove,
                    old_line: None,
                    new_line: None,
                    text: "(deleted file)".to_string(),
                }],
            ));
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("*** Update File:") {
            let mut path = rest.trim().to_string();
            index += 1;
            if index < lines.len() {
                if let Some(rest) = lines[index].strip_prefix("*** Move to:") {
                    path = format!("{path} -> {}", rest.trim());
                    index += 1;
                }
            }
            let mut rows = Vec::new();
            let mut old_line = None;
            let mut new_line = None;
            while index < lines.len()
                && !lines[index].starts_with("*** Update File:")
                && !lines[index].starts_with("*** Add File:")
                && !lines[index].starts_with("*** Delete File:")
                && lines[index].trim() != "*** End Patch"
            {
                let line = lines[index];
                if line == "*** End of File" {
                    index += 1;
                    continue;
                }
                if line.starts_with("@@") {
                    if let Some((old_start, new_start)) = parse_hunk_header(line) {
                        old_line = Some(old_start);
                        new_line = Some(new_start);
                    }
                    rows.push(ParsedDiffRow {
                        kind: DiffLineKind::Hunk,
                        old_line: None,
                        new_line: None,
                        text: line.to_string(),
                    });
                } else if let Some(text) = line.strip_prefix('+') {
                    let current = new_line;
                    new_line = new_line.map(|line| line + 1);
                    rows.push(ParsedDiffRow {
                        kind: DiffLineKind::Add,
                        old_line: None,
                        new_line: current,
                        text: text.to_string(),
                    });
                } else if let Some(text) = line.strip_prefix('-') {
                    let current = old_line;
                    old_line = old_line.map(|line| line + 1);
                    rows.push(ParsedDiffRow {
                        kind: DiffLineKind::Remove,
                        old_line: current,
                        new_line: None,
                        text: text.to_string(),
                    });
                } else {
                    let text = line.strip_prefix(' ').unwrap_or(line);
                    let current_old = old_line;
                    let current_new = new_line;
                    old_line = old_line.map(|line| line + 1);
                    new_line = new_line.map(|line| line + 1);
                    rows.push(ParsedDiffRow {
                        kind: DiffLineKind::Context,
                        old_line: current_old,
                        new_line: current_new,
                        text: text.to_string(),
                    });
                }
                index += 1;
            }
            sections.push(section_from_rows(path, pair_adjacent_change_runs(rows)));
            continue;
        }
        index += 1;
    }
    sections
}

fn unified_patch_sections(patch: &str, fallback_path: &str) -> Vec<ToolDiffSection> {
    let mut old_line = None;
    let mut new_line = None;
    let mut rows = Vec::new();
    let mut path = fallback_path.to_string();
    let mut sections = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if !rows.is_empty() {
                sections.push(section_from_rows(
                    std::mem::take(&mut path),
                    pair_adjacent_change_runs(std::mem::take(&mut rows)),
                ));
            }
            let mut parts = rest.split_whitespace();
            let _old = parts.next();
            if let Some(new_path) = parts.next() {
                path = trim_diff_path(new_path).to_string();
            }
            old_line = None;
            new_line = None;
            continue;
        }
        if line.starts_with("index ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            if rest != "/dev/null" {
                path = trim_diff_path(rest).to_string();
            }
            continue;
        }
        if line.starts_with("--- ") {
            continue;
        }
        if line.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(line) {
                old_line = Some(old_start);
                new_line = Some(new_start);
            }
            rows.push(ParsedDiffRow {
                kind: DiffLineKind::Hunk,
                old_line: None,
                new_line: None,
                text: line.to_string(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            let current = new_line;
            new_line = new_line.map(|line| line + 1);
            rows.push(ParsedDiffRow {
                kind: DiffLineKind::Add,
                old_line: None,
                new_line: current,
                text: rest.to_string(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix('-') {
            let current = old_line;
            old_line = old_line.map(|line| line + 1);
            rows.push(ParsedDiffRow {
                kind: DiffLineKind::Remove,
                old_line: current,
                new_line: None,
                text: rest.to_string(),
            });
            continue;
        }
        let rest = line.strip_prefix(' ').unwrap_or(line);
        let current_old = old_line;
        let current_new = new_line;
        old_line = old_line.map(|line| line + 1);
        new_line = new_line.map(|line| line + 1);
        rows.push(ParsedDiffRow {
            kind: DiffLineKind::Context,
            old_line: current_old,
            new_line: current_new,
            text: rest.to_string(),
        });
    }
    if !rows.is_empty() {
        sections.push(section_from_rows(path, rows));
    }
    sections
}

fn section_from_rows(path: String, rows: Vec<ParsedDiffRow>) -> ToolDiffSection {
    let link_target = clean_diff_link_target(&path);
    let rows = rows
        .into_iter()
        .filter(|row| row.kind != DiffLineKind::Hunk || !row.text.starts_with("@@"))
        .collect::<Vec<_>>();
    let additions = rows
        .iter()
        .filter(|row| row.kind == DiffLineKind::Add)
        .count() as u32;
    let deletions = rows
        .iter()
        .filter(|row| row.kind == DiffLineKind::Remove)
        .count() as u32;
    let omitted = rows.len().saturating_sub(MAX_STORED_DIFF_ROWS);
    let mut lines = rows
        .into_iter()
        .take(MAX_STORED_DIFF_ROWS)
        .map(diff_line_from_row)
        .collect::<Vec<_>>();
    if omitted > 0 {
        lines.push(DiffLine {
            text: format!("... +{omitted} lines omitted"),
            kind: DiffLineKind::Hunk,
            line_number: None,
            old_line_number: None,
            new_line_number: None,
        });
    }
    let fingerprint = diff_section_fingerprint(&lines, omitted);
    ToolDiffSection {
        path,
        link_target,
        additions,
        deletions,
        lines,
        omitted,
        fingerprint,
        diagnostics: Vec::new(),
    }
}

fn clean_diff_link_target(path: &str) -> String {
    let path = path.trim();
    let path = path
        .strip_prefix("A ")
        .or_else(|| path.strip_prefix("M "))
        .or_else(|| path.strip_prefix("D "))
        .or_else(|| path.strip_prefix("R "))
        .unwrap_or(path)
        .trim();
    path.split(" -> ").last().unwrap_or(path).trim().to_string()
}

fn diff_section_fingerprint(lines: &[DiffLine], omitted: usize) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    omitted.hash(&mut hasher);
    lines.hash(&mut hasher);
    hasher.finish()
}

/// Pull LSP diagnostics out of the tool's `metadata.diagnostics` and hang them
/// off the matching diff section, so the card can render an opencode-style
/// diagnostics footer. Each metadata entry is `{ path, source, diagnostics: [..] }`;
/// "touched" entries match the file the edit changed (their path == the section's
/// link target), while "project" entries for other files simply don't match any
/// section and are dropped here.
fn attach_section_diagnostics(sections: &mut [ToolDiffSection], metadata: &Value) {
    let Some(entries) = metadata.get("diagnostics").and_then(Value::as_array) else {
        return;
    };
    for entry in entries {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            continue;
        };
        let lines = diag_lines_from_entry(entry);
        if lines.is_empty() {
            continue;
        }
        if let Some(section) = sections
            .iter_mut()
            .find(|section| diag_path_matches(&section.link_target, path))
        {
            section.diagnostics = lines;
        }
    }
}

fn diag_path_matches(link_target: &str, diag_path: &str) -> bool {
    let link_target = link_target.trim();
    let diag_path = diag_path.trim();
    link_target == diag_path
        || link_target.ends_with(diag_path)
        || diag_path.ends_with(link_target)
}

fn diag_lines_from_entry(entry: &Value) -> Vec<DiagLine> {
    let Some(items) = entry.get("diagnostics").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    let mut others = Vec::new();
    for item in items {
        let Some(message) = item.get("message").and_then(Value::as_str) else {
            continue;
        };
        let severity = item
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let (line, col) = item
            .get("range")
            .and_then(|range| range.get("start"))
            .map(|start| {
                (
                    start.get("line").and_then(Value::as_u64).unwrap_or(0) + 1,
                    start.get("character").and_then(Value::as_u64).unwrap_or(0) + 1,
                )
            })
            .unwrap_or((1, 1));
        let label = match severity {
            "error" => "ERROR",
            "warning" => "WARN",
            "information" => "INFO",
            "hint" => "HINT",
            _ => "ERROR",
        };
        let diag = DiagLine {
            is_error: severity == "error",
            text: format!("{label} [{line}:{col}] {}", message.trim()),
        };
        if diag.is_error {
            errors.push(diag);
        } else {
            others.push(diag);
        }
    }
    // Errors first so the most actionable lines survive the per-card cap.
    errors.extend(others);
    errors
}

pub(crate) fn diff_link_target(path: &str) -> Option<&str> {
    let path = path.trim();
    (!path.is_empty()
        && path != "patch"
        && !path.starts_with("/dev/null")
        && !path.starts_with("(deleted"))
    .then_some(path)
}

fn diff_line_from_row(row: ParsedDiffRow) -> DiffLine {
    let line_number = row
        .new_line
        .or(row.old_line)
        .and_then(|line| u32::try_from(line).ok());
    let old_line_number = row.old_line.and_then(|line| u32::try_from(line).ok());
    let new_line_number = row.new_line.and_then(|line| u32::try_from(line).ok());
    let text = match row.kind {
        DiffLineKind::Add => format!("+{}", row.text),
        DiffLineKind::Remove => format!("-{}", row.text),
        DiffLineKind::Context => format!(" {}", row.text),
        DiffLineKind::Hunk => row.text,
    };
    DiffLine {
        text,
        kind: row.kind,
        line_number,
        old_line_number,
        new_line_number,
    }
}

fn visible_diff_lines(section: &ToolDiffSection, expanded: bool) -> Cow<'_, [DiffLine]> {
    let max_rows = if expanded {
        EXPANDED_DIFF_ROWS
    } else {
        COLLAPSED_DIFF_ROWS
    };
    if section.lines.len() <= max_rows {
        return Cow::Borrowed(section.lines.as_slice());
    }
    let hidden = section.lines.len().saturating_sub(max_rows) + section.omitted;
    let mut rows = section
        .lines
        .iter()
        .take(max_rows)
        .cloned()
        .collect::<Vec<_>>();
    rows.push(DiffLine {
        text: if expanded {
            format!("... +{hidden} lines")
        } else {
            format!("... +{hidden} lines (click to expand)")
        },
        kind: DiffLineKind::Hunk,
        line_number: None,
        old_line_number: None,
        new_line_number: None,
    });
    Cow::Owned(rows)
}

fn visible_diff_lines_owned(section: &ToolDiffSection, expanded: bool) -> Vec<DiffLine> {
    match visible_diff_lines(section, expanded) {
        Cow::Borrowed(lines) => lines.to_vec(),
        Cow::Owned(lines) => lines,
    }
}

pub(crate) fn cached_diff_card_view(
    section: &ToolDiffSection,
    card_w: f32,
    s: f32,
    expanded: bool,
) -> Rc<ToolDiffCardView> {
    let body_width = diff_card::body_text_width(card_w, s);
    let key = ToolDiffCardViewKey {
        section_fingerprint: section.fingerprint,
        body_width_bits: body_width.to_bits(),
        scale_bits: s.to_bits(),
        expanded,
    };
    if let Some(hit) = TOOL_DIFF_CARD_VIEW_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }

    let preview_rows = visible_diff_lines_owned(section, false);
    let preview_visual_rows = diff_card::visual_row_count(&preview_rows, body_width, s);
    let rows = if expanded {
        Rc::new(visible_diff_lines_owned(section, true))
    } else {
        Rc::new(preview_rows)
    };
    let visual_row_offsets = Rc::new(diff_card::warm_render_cache(
        rows.as_slice(),
        body_width,
        s,
        Lang::from_path(&section.path),
    ));
    let visual_rows = visual_row_offsets.last().copied().unwrap_or(0).max(1);
    let view = Rc::new(ToolDiffCardView {
        rows,
        visual_row_offsets,
        visual_rows,
        preview_visual_rows,
    });
    TOOL_DIFF_CARD_VIEW_CACHE.with(|cache| cache.borrow_mut().insert(key, view.clone()));
    view
}

pub(crate) fn diff_body_height(visual_rows: usize, s: f32) -> f32 {
    diff_card::BODY_TOP_PAD * s
        + diff_card::BODY_BOTTOM_PAD * s
        + visual_rows.max(1) as f32 * diff_card::LINE_HEIGHT * s
}

/// Number of diagnostics footer rows a section renders (capped, with a trailing
/// "+N more" row when truncated). Zero when the file is clean.
pub(crate) fn diag_footer_rows(section: &ToolDiffSection) -> usize {
    let total = section.diagnostics.len();
    if total == 0 {
        return 0;
    }
    let shown = total.min(MAX_DIAG_LINES_PER_CARD);
    shown + usize::from(total > MAX_DIAG_LINES_PER_CARD)
}

pub(crate) fn diag_footer_height(section: &ToolDiffSection, s: f32) -> f32 {
    let rows = diag_footer_rows(section);
    if rows == 0 {
        0.0
    } else {
        // Small top gap so the diagnostics read as a distinct band under the diff.
        4.0 * s + rows as f32 * DIAG_LINE_HEIGHT * s
    }
}

pub(crate) fn tool_diff_card_width(width: f32, s: f32) -> f32 {
    (width - 38.0 * s).max(120.0 * s)
}

fn fallback_edit_path(
    metadata: &Value,
    message: &impl AgentToolMessage,
) -> Option<String> {
    metadata_path(metadata)
        .map(str::to_string)
        .or_else(|| path_from_title(message.title()))
}

fn metadata_path(metadata: &Value) -> Option<&str> {
    metadata
        .get("paths")
        .and_then(Value::as_array)
        .and_then(|paths| paths.first())
        .and_then(Value::as_str)
        .or_else(|| metadata.get("path").and_then(Value::as_str))
}

fn path_from_title(title: &str) -> Option<String> {
    let open = title.find('(')?;
    let rest = &title[open + 1..];
    let close = rest.find(')')?;
    let path = rest[..close].trim();
    (!path.is_empty()).then(|| path.to_string())
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn trim_diff_path(path: &str) -> &str {
    path.trim()
        .strip_prefix("a/")
        .or_else(|| path.trim().strip_prefix("b/"))
        .unwrap_or_else(|| path.trim())
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    parts.next()?;
    let old = parts.next()?.trim_start_matches('-');
    let new = parts.next()?.trim_start_matches('+');
    Some((parse_hunk_start(old)?, parse_hunk_start(new)?))
}

fn parse_hunk_start(value: &str) -> Option<usize> {
    value
        .split(',')
        .next()
        .and_then(|value| value.parse::<usize>().ok())
}
