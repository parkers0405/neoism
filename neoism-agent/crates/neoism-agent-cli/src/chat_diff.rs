use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;

use crate::chat_ui::{highlight_code_line, terminal_size, truncate_for_terminal};
use crate::{DIM, RESET};

// Dark diff palette: tinted success/error over a dark terminal.
const CODE_BG: &str = "\x1b[48;2;16;16;16m";
const GREEN_BG: &str = "\x1b[48;2;16;43;25m";
const RED_BG: &str = "\x1b[48;2;50;20;24m";
const GREEN_LN_BG: &str = "\x1b[48;2;11;34;18m";
const RED_LN_BG: &str = "\x1b[48;2;42;16;20m";
const CODE_LN_BG: &str = "\x1b[48;2;18;18;18m";
const DIFF_ADD: &str = "\x1b[38;2;127;216;143m";
const DIFF_REMOVE: &str = "\x1b[38;2;224;108;117m";
const DIFF_LN: &str = "\x1b[38;2;143;143;143m";

#[derive(Clone)]
pub(crate) struct SnapshotDiff {
    pub(crate) path: String,
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) omitted: usize,
}

#[derive(Clone)]
pub(crate) struct DiffRow {
    kind: DiffKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffKind {
    Context,
    Add,
    Remove,
    Hunk,
}

pub(crate) struct PatchSection {
    pub(crate) path: String,
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) omitted: usize,
}

pub(crate) fn snapshot_diffs(metadata: &Value) -> Vec<SnapshotDiff> {
    metadata
        .get("snapshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(snapshot_diff)
        .collect()
}

fn snapshot_diff(snapshot: &Value) -> Option<SnapshotDiff> {
    let path = snapshot.get("path").and_then(Value::as_str)?.to_string();
    let before = snapshot_text(snapshot.get("before")?)?;
    let after = snapshot_text(snapshot.get("after")?)?;
    let before_lines = before.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let after_lines = after.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let (rows, added, removed, omitted) = compact_line_diff(&before_lines, &after_lines);
    Some(SnapshotDiff {
        path,
        rows,
        added,
        removed,
        omitted,
    })
}

fn snapshot_text(state: &Value) -> Option<String> {
    if !state
        .get("exists")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(String::new());
    }
    let encoded = state.get("contentBase64").and_then(Value::as_str)?;
    String::from_utf8(STANDARD.decode(encoded).ok()?).ok()
}

fn compact_line_diff(
    before: &[String],
    after: &[String],
) -> (Vec<DiffRow>, usize, usize, usize) {
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
    let after_start = prefix.saturating_sub(context);
    let before_context_end = prefix;
    let before_tail_end = (before_change_end + context).min(before.len());
    let after_tail_end = (after_change_end + context).min(after.len());
    let added = after_change_end.saturating_sub(prefix);
    let removed = before_change_end.saturating_sub(prefix);

    let mut rows = Vec::new();
    if before_start > 0 || after_start > 0 {
        rows.push(DiffRow {
            kind: DiffKind::Hunk,
            old_line: None,
            new_line: None,
            text: "...".to_string(),
        });
    }
    for index in before_start..before_context_end {
        rows.push(DiffRow {
            kind: DiffKind::Context,
            old_line: Some(index + 1),
            new_line: Some(index + 1),
            text: before[index].clone(),
        });
    }
    for index in prefix..before_change_end {
        rows.push(DiffRow {
            kind: DiffKind::Remove,
            old_line: Some(index + 1),
            new_line: None,
            text: before[index].clone(),
        });
    }
    for index in prefix..after_change_end {
        rows.push(DiffRow {
            kind: DiffKind::Add,
            old_line: None,
            new_line: Some(index + 1),
            text: after[index].clone(),
        });
    }
    for (before_index, after_index) in
        (before_change_end..before_tail_end).zip(after_change_end..after_tail_end)
    {
        rows.push(DiffRow {
            kind: DiffKind::Context,
            old_line: Some(before_index + 1),
            new_line: Some(after_index + 1),
            text: after[after_index].clone(),
        });
    }

    let max_rows = 72usize;
    let omitted = rows.len().saturating_sub(max_rows);
    rows.truncate(max_rows);
    (rows, added, removed, omitted)
}

pub(crate) fn patch_diff_sections(patch: &str, fallback_path: &str) -> Vec<PatchSection> {
    if patch.contains("*** Begin Patch") {
        let sections = v4a_patch_sections(patch);
        if !sections.is_empty() {
            return sections;
        }
    }
    unified_patch_sections(patch, fallback_path)
}

fn v4a_patch_sections(patch: &str) -> Vec<PatchSection> {
    let lines: Vec<&str> = patch.lines().collect();
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
                    rows.push(DiffRow {
                        kind: DiffKind::Add,
                        old_line: None,
                        new_line: Some(new_line),
                        text: text.to_string(),
                    });
                    new_line += 1;
                }
                index += 1;
            }
            sections.push(patch_section(path, rows));
            continue;
        }
        if let Some(rest) = line.strip_prefix("*** Delete File:") {
            sections.push(patch_section(
                rest.trim().to_string(),
                vec![DiffRow {
                    kind: DiffKind::Remove,
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
                    rows.push(DiffRow {
                        kind: DiffKind::Hunk,
                        old_line: None,
                        new_line: None,
                        text: line.to_string(),
                    });
                } else if let Some(text) = line.strip_prefix('+') {
                    let current = new_line;
                    new_line = new_line.map(|line| line + 1);
                    rows.push(DiffRow {
                        kind: DiffKind::Add,
                        old_line: None,
                        new_line: current,
                        text: text.to_string(),
                    });
                } else if let Some(text) = line.strip_prefix('-') {
                    let current = old_line;
                    old_line = old_line.map(|line| line + 1);
                    rows.push(DiffRow {
                        kind: DiffKind::Remove,
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
                    rows.push(DiffRow {
                        kind: DiffKind::Context,
                        old_line: current_old,
                        new_line: current_new,
                        text: text.to_string(),
                    });
                }
                index += 1;
            }
            sections.push(patch_section(path, rows));
            continue;
        }
        index += 1;
    }
    sections
}

fn unified_patch_sections(patch: &str, fallback_path: &str) -> Vec<PatchSection> {
    let mut old_line = None;
    let mut new_line = None;
    let mut rows = Vec::new();
    let mut path = fallback_path.to_string();
    let mut sections = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if !rows.is_empty() {
                sections.push(patch_section(
                    std::mem::take(&mut path),
                    std::mem::take(&mut rows),
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
            rows.push(DiffRow {
                kind: DiffKind::Hunk,
                old_line: None,
                new_line: None,
                text: line.to_string(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            let current = new_line;
            new_line = new_line.map(|line| line + 1);
            rows.push(DiffRow {
                kind: DiffKind::Add,
                old_line: None,
                new_line: current,
                text: rest.to_string(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix('-') {
            let current = old_line;
            old_line = old_line.map(|line| line + 1);
            rows.push(DiffRow {
                kind: DiffKind::Remove,
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
        rows.push(DiffRow {
            kind: DiffKind::Context,
            old_line: current_old,
            new_line: current_new,
            text: rest.to_string(),
        });
    }
    if !rows.is_empty() {
        sections.push(patch_section(path, rows));
    }
    sections
}

fn patch_section(path: String, rows: Vec<DiffRow>) -> PatchSection {
    let added = rows
        .iter()
        .filter(|row| matches!(row.kind, DiffKind::Add))
        .count();
    let removed = rows
        .iter()
        .filter(|row| matches!(row.kind, DiffKind::Remove))
        .count();
    let max_rows = 72usize;
    let omitted = rows.len().saturating_sub(max_rows);
    let rows = rows.into_iter().take(max_rows).collect();
    PatchSection {
        path,
        rows,
        added,
        removed,
        omitted,
    }
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

pub(crate) fn render_diff_rows(path: &str, rows: &[DiffRow]) {
    let lang = language_for_path(path);
    let width = terminal_size().0 as usize;
    let prefix_width = 8usize;
    let available = width.saturating_sub(prefix_width).max(20);
    for row in rows {
        if matches!(row.kind, DiffKind::Hunk) {
            println!("{DIM}      {}{RESET}", row.text);
            continue;
        }
        let number = row
            .new_line
            .or(row.old_line)
            .map(|line| format!("{line:>5}"))
            .unwrap_or_else(|| "     ".to_string());
        let (bg, line_bg, sign, sign_color, number_color) = match row.kind {
            DiffKind::Add => (GREEN_BG, GREEN_LN_BG, "+", DIFF_ADD, DIFF_ADD),
            DiffKind::Remove => (RED_BG, RED_LN_BG, "-", DIFF_REMOVE, DIFF_REMOVE),
            DiffKind::Context | DiffKind::Hunk => {
                (CODE_BG, CODE_LN_BG, " ", DIFF_LN, DIFF_LN)
            }
        };
        let text = truncate_for_terminal(&row.text, available);
        let padding =
            " ".repeat(width.saturating_sub(prefix_width + text.chars().count()));
        let highlighted =
            highlight_code_line(lang, &text).replace(RESET, &format!("{RESET}{bg}"));
        println!(
            "{line_bg}{number_color}{number}{RESET}{bg} {sign_color}{sign}{RESET}{bg} {highlighted}{padding}{RESET}"
        );
    }
}

fn language_for_path(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4a_patch_sections_keep_file_counts_and_line_numbers() {
        let patch = r#"*** Begin Patch
*** Add File: src/new.rs
+fn main() {
+    println!("hi");
+}
*** Update File: src/lib.rs
@@ -10,2 +10,3 @@
 fn old() {
-    before();
+    after();
+    again();
 }
*** End Patch"#;

        let sections = patch_diff_sections(patch, "fallback.rs");

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].path, "src/new.rs");
        assert_eq!(sections[0].added, 3);
        assert_eq!(sections[0].removed, 0);
        assert_eq!(sections[0].rows[0].new_line, Some(1));
        assert_eq!(sections[0].rows[2].new_line, Some(3));

        assert_eq!(sections[1].path, "src/lib.rs");
        assert_eq!(sections[1].added, 2);
        assert_eq!(sections[1].removed, 1);
        let removed = sections[1]
            .rows
            .iter()
            .find(|row| row.kind == DiffKind::Remove)
            .expect("removed row");
        let added = sections[1]
            .rows
            .iter()
            .find(|row| row.kind == DiffKind::Add)
            .expect("added row");
        assert_eq!(removed.old_line, Some(11));
        assert_eq!(added.new_line, Some(11));
    }

    #[test]
    fn unified_patch_sections_parse_paths_and_hunk_lines() {
        let patch = r#"diff --git a/TASK.md b/TASK.md
index 111..222 100644
--- a/TASK.md
+++ b/TASK.md
@@ -3,2 +3,3 @@
 old context
-before
+after
+again
"#;

        let sections = patch_diff_sections(patch, "fallback.md");

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].path, "TASK.md");
        assert_eq!(sections[0].added, 2);
        assert_eq!(sections[0].removed, 1);
        let numbers = sections[0]
            .rows
            .iter()
            .filter(|row| !matches!(row.kind, DiffKind::Hunk))
            .map(|row| (row.old_line, row.new_line))
            .collect::<Vec<_>>();
        assert_eq!(
            numbers,
            vec![
                (Some(3), Some(3)),
                (Some(4), None),
                (None, Some(4)),
                (None, Some(5))
            ]
        );
    }

    #[test]
    fn compact_line_diff_limits_to_changed_window() {
        let before = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let after = ["a", "b", "changed", "d", "e"]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        let (rows, added, removed, omitted) = compact_line_diff(&before, &after);

        assert_eq!(added, 1);
        assert_eq!(removed, 1);
        assert_eq!(omitted, 0);
        assert!(rows.iter().any(|row| row.kind == DiffKind::Remove
            && row.old_line == Some(3)
            && row.text == "c"));
        assert!(rows.iter().any(|row| row.kind == DiffKind::Add
            && row.new_line == Some(3)
            && row.text == "changed"));
    }
}
