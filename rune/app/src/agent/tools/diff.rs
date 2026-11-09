//! File changes a tool made, as diff sections: parsed from the edit record the daemon stores in
//! `detail` (`neoismToolDetail: "edit"` with `metadata.files` / `snapshots` / `input`), from raw
//! V4A (`*** Begin Patch`) or unified patches, from patch stacks, and from compose programs.
//! Rules ported from `agent_pane/view/tool_message/diff.rs`.

use base64::Engine;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    Hunk,
    Add,
    Remove,
    Context,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffLine {
    /// Prefixed with `+`, `-` or ` ` (hunks carry their own text).
    pub text: String,
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
}

impl DiffLine {
    /// The number the gutter shows for this row.
    pub fn number(&self) -> Option<u32> {
        match self.kind {
            LineKind::Remove => self.old,
            LineKind::Add | LineKind::Context => self.new.or(self.old),
            LineKind::Hunk => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub error: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// Display path; `A `, `D `, `M `, `R ` prefixes mark created/deleted/modified/moved files.
    pub path: String,
    /// The real file, empty for non-file sections (compose source, audit notices).
    pub link_target: String,
    pub additions: u32,
    pub deletions: u32,
    pub lines: Vec<DiffLine>,
    pub diagnostics: Vec<Diagnostic>,
}

pub const MAX_DIAGNOSTICS: usize = 6;

/// Tools whose rows are file edits (`apply_patch`, `edit`, `write`, ...).
pub fn is_edit_tool(tool: &str) -> bool {
    matches!(normalized(tool).as_str(), "applypatch" | "patch" | "edit" | "write" | "multiedit")
}

/// Patches stream in; their diff is only trusted once the call settles.
pub fn is_streaming_patch_tool(tool: &str) -> bool {
    matches!(normalized(tool).as_str(), "applypatch" | "patch")
}

pub fn is_unsettled(tool: &str, status: &str) -> bool {
    is_streaming_patch_tool(tool) && matches!(status.trim().to_ascii_lowercase().as_str(), "pending" | "running" | "streaming")
}

fn normalized(tool: &str) -> String {
    tool.chars().filter(|c| *c != '_' && *c != '-').flat_map(char::to_lowercase).collect()
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, Rc<Vec<Section>>>> = RefCell::new(HashMap::new());
}

fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The diff sections of a tool call, cached per content; empty when it changed no files.
pub fn sections(tool: &str, status: &str, title: &str, text: &str, detail: &str) -> Rc<Vec<Section>> {
    if is_unsettled(tool, status) {
        return Rc::default();
    }
    let key = hash_of(&(tool, status, title, text, detail));
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let parsed = Rc::new(parse(tool, title, text, detail));
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= 512 { cache.clear(); }
        cache.insert(key, parsed.clone());
    });
    parsed
}

fn parse(tool: &str, title: &str, text: &str, detail: &str) -> Vec<Section> {
    let json = serde_json::from_str::<Value>(detail).ok();
    if let Some(value) = &json {
        if value["neoismToolDetail"] == "patchStack" {
            return patch_stack(value["calls"].as_array().map(Vec::as_slice).unwrap_or(&[]));
        }
    }
    if tool == "compose" {
        return json.as_ref().filter(|v| v["neoismToolDetail"] == "composition").map(composition).unwrap_or_default();
    }
    if !is_edit_tool(tool) && !looks_like_patch(detail) {
        return Vec::new();
    }
    let mut sections = Vec::new();
    if let Some(value) = json.as_ref().filter(|v| v["neoismToolDetail"] == "edit") {
        let metadata = &value["metadata"];
        sections = metadata["snapshots"].as_array().into_iter().flatten().filter_map(snapshot_section).collect();
        for section in metadata_file_sections(metadata) {
            if !sections.iter().any(|s| s.link_target == section.link_target) {
                sections.push(section);
            }
        }
        if sections.is_empty() {
            sections = metadata["acpDiffs"].as_array().into_iter().flatten().filter_map(|diff| {
                Some(text_section(diff["path"].as_str()?.to_owned(), diff["oldText"].as_str()?, diff["newText"].as_str()?))
            }).collect();
        }
        if sections.is_empty() {
            sections = input_sections(&value["input"], metadata, fallback_path(metadata, title).as_deref());
        }
        attach_diagnostics(&mut sections, metadata);
        if let Some(notice) = value["compositionNotice"].as_str().filter(|s| !s.trim().is_empty()) {
            let rows = notice.lines().map(|line| Row::context(None, None, line)).collect();
            let mut section = from_rows("Audit notice".into(), rows);
            section.link_target.clear();
            sections.push(section);
        }
    }
    if sections.is_empty() {
        let fallback = fallback_path(&Value::Null, title).unwrap_or_else(|| "patch".into());
        for raw in [detail, text] {
            if looks_like_patch(raw) {
                sections = patch_sections(raw, &fallback);
                if !sections.is_empty() { break; }
            }
        }
    }
    sections.retain(|s| !s.lines.is_empty() || s.path.starts_with("A ") || s.path.starts_with("D "));
    sections
}

fn composition(value: &Value) -> Vec<Section> {
    [("Program source.js", "source"), ("Selected output", "result")].into_iter()
        .filter(|(_, key)| *key == "source" || value[*key].as_str().is_some_and(|s| !s.is_empty()))
        .map(|(label, key)| {
            let text = value[key].as_str().filter(|s| !s.is_empty()).unwrap_or("\u{2014}");
            let lines = text.lines().map(|line| DiffLine { text: if key == "source" { line.to_owned() } else { format!(" {line}") }, kind: LineKind::Context, old: None, new: None }).collect();
            Section { path: label.into(), link_target: String::new(), additions: 0, deletions: 0, lines, diagnostics: Vec::new() }
        })
        .collect()
}

fn patch_stack(calls: &[Value]) -> Vec<Section> {
    let mut entries: Vec<(Section, Option<Value>)> = Vec::new();
    for call in calls {
        let tool = call["tool"].as_str().unwrap_or("edit");
        let detail = call["detail"].as_str().unwrap_or("");
        let parsed = serde_json::from_str::<Value>(detail).ok();
        for section in parse(tool, call["title"].as_str().unwrap_or(""), call["text"].as_str().unwrap_or(""), detail) {
            let snapshot = parsed.as_ref().filter(|d| d["neoismToolDetail"] == "edit")
                .and_then(|d| complete_snapshot(&d["metadata"], &section.link_target));
            let previous = entries.iter().rposition(|(p, _)| !section.link_target.is_empty() && p.link_target == section.link_target);
            if let Some(index) = previous {
                let merged = entries[index].1.as_ref().zip(snapshot.as_ref()).and_then(|(a, b)| chain_snapshots(a, b));
                if let Some(merged) = merged {
                    if let Some(mut cumulative) = snapshot_section(&merged) {
                        cumulative.diagnostics = section.diagnostics;
                        entries[index] = (cumulative, Some(merged));
                        continue;
                    }
                }
            }
            entries.push((section, snapshot));
        }
    }
    entries.into_iter().map(|(s, _)| s).collect()
}

fn complete_snapshot(metadata: &Value, path: &str) -> Option<Value> {
    if path.is_empty() { return None; }
    let mut matching = metadata["snapshots"].as_array()?.iter().filter(|s| s["path"].as_str() == Some(path));
    let snapshot = matching.next()?;
    if matching.next().is_some() { return None; }
    snapshot_text(&snapshot["before"])?;
    snapshot_text(&snapshot["after"])?;
    Some(snapshot.clone())
}

/// Two edits to one file chain when the second started from exactly what the first left.
fn chain_snapshots(previous: &Value, next: &Value) -> Option<Value> {
    if previous["path"].as_str()? != next["path"].as_str()?
        || previous["absolute"].as_bool().unwrap_or(false) != next["absolute"].as_bool().unwrap_or(false)
        || !previous["after"]["exists"].as_bool()?
        || !next["before"]["exists"].as_bool()?
        || (!previous["before"]["exists"].as_bool()? && !next["after"]["exists"].as_bool()?)
        || snapshot_text(&previous["after"])? != snapshot_text(&next["before"])?
    {
        return None;
    }
    let mut merged = previous.clone();
    merged["after"] = next["after"].clone();
    Some(merged)
}

pub fn looks_like_patch(raw: &str) -> bool {
    raw.contains("*** Begin Patch") || raw.contains("diff --git ")
        || (raw.contains("\n--- ") && raw.contains("\n+++ ") && raw.contains("\n@@"))
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value[*key].as_str()).map(str::trim).filter(|s| !s.is_empty())
}

fn input_sections(input: &Value, metadata: &Value, fallback: Option<&str>) -> Vec<Section> {
    if let Some(patch) = string_field(input, &["patchText", "patch", "diff", "content"]).filter(|p| looks_like_patch(p)) {
        let sections = patch_sections(patch, fallback.unwrap_or("patch"));
        if !sections.is_empty() { return sections; }
    }
    let path = string_field(input, &["filePath", "file_path", "path"]).or_else(|| metadata_path(metadata)).or(fallback).unwrap_or("edit");
    let old = string_field(input, &["oldString", "old", "oldText"]);
    let new = string_field(input, &["newString", "new", "newText", "content"]).unwrap_or("");
    if old.unwrap_or("").is_empty() && new.is_empty() { return Vec::new(); }
    let Some(old) = old else {
        let mut rows = vec![Row::hunk("New content \u{b7} original baseline unavailable")];
        rows.extend(new.lines().map(|line| Row::context(None, None, line)));
        return vec![from_rows(path.to_owned(), rows)];
    };
    vec![text_section(path.to_owned(), old, new)]
}

fn metadata_file_sections(metadata: &Value) -> Vec<Section> {
    let sections: Vec<Section> = metadata["files"].as_array().into_iter().flatten().filter_map(metadata_file_section).collect();
    if !sections.is_empty() { return sections; }
    metadata["diff"].as_str().filter(|d| looks_like_patch(d)).map(|d| unified_sections(d, "patch")).unwrap_or_default()
}

fn metadata_file_section(file: &Value) -> Option<Section> {
    let patch = file["patch"].as_str().unwrap_or("");
    let raw_path = file["relativePath"].as_str().or_else(|| file["filePath"].as_str()).unwrap_or("patch");
    let status = file["type"].as_str().unwrap_or("update");
    let mut section = match unified_sections(patch, raw_path).pop() {
        Some(section) => section,
        None if matches!(status, "add" | "added" | "delete" | "deleted") => from_rows(raw_path.to_owned(), Vec::new()),
        None => return None,
    };
    section.path = match status {
        "add" | "added" => format!("A {raw_path}"),
        "delete" | "deleted" => format!("D {raw_path}"),
        "move" | "moved" => file["movePath"].as_str().map(|to| format!("R {raw_path} -> {to}")).unwrap_or_else(|| format!("R {raw_path}")),
        _ => format!("M {raw_path}"),
    };
    if let Some(n) = file["additions"].as_u64() { section.additions = n.min(u32::MAX as u64) as u32; }
    if let Some(n) = file["deletions"].as_u64() { section.deletions = n.min(u32::MAX as u64) as u32; }
    Some(section)
}

fn snapshot_section(snapshot: &Value) -> Option<Section> {
    let raw = snapshot["path"].as_str()?;
    let path = if snapshot["before"]["exists"] == false {
        format!("A {raw}")
    } else if snapshot["after"]["exists"] == false {
        format!("D {raw}")
    } else {
        raw.to_owned()
    };
    Some(text_section(path, &snapshot_text(&snapshot["before"])?, &snapshot_text(&snapshot["after"])?))
}

fn snapshot_text(state: &Value) -> Option<String> {
    if !state["exists"].as_bool()? { return Some(String::new()); }
    let encoded = state["contentBase64"].as_str().or_else(|| state["content_base64"].as_str())?;
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(encoded).ok()?).ok()
}

/// Before/after text as a compact diff: three lines of context around the changed middle.
fn text_section(path: String, before: &str, after: &str) -> Section {
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();
    from_rows(path, compact_line_diff(&before, &after))
}

fn compact_line_diff(before: &[&str], after: &[&str]) -> Vec<Row> {
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] { prefix += 1; }
    let mut suffix = 0;
    while suffix + prefix < before.len() && suffix + prefix < after.len()
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let before_end = before.len() - suffix;
    let after_end = after.len() - suffix;
    const CONTEXT: usize = 3;
    let start = prefix.saturating_sub(CONTEXT);
    let mut rows = Vec::new();
    if start > 0 { rows.push(Row::hunk("...")); }
    for i in start..prefix {
        rows.push(Row::context(Some(i + 1), Some(i + 1), before[i]));
    }
    for i in 0..(before_end - prefix).max(after_end - prefix) {
        let (old, new) = (before.get(prefix + i).filter(|_| prefix + i < before_end), after.get(prefix + i).filter(|_| prefix + i < after_end));
        match (old, new) {
            (Some(o), Some(n)) if o == n => rows.push(Row::context(Some(prefix + i + 1), Some(prefix + i + 1), n)),
            (o, n) => {
                if let Some(o) = o { rows.push(Row { kind: LineKind::Remove, old: Some(prefix + i + 1), new: None, text: (*o).to_owned() }); }
                if let Some(n) = n { rows.push(Row { kind: LineKind::Add, old: None, new: Some(prefix + i + 1), text: (*n).to_owned() }); }
            }
        }
    }
    let tail_before = (before_end + CONTEXT).min(before.len());
    let tail_after = (after_end + CONTEXT).min(after.len());
    for (b, a) in (before_end..tail_before).zip(after_end..tail_after) {
        rows.push(Row::context(Some(b + 1), Some(a + 1), after[a]));
    }
    rows
}

/// A parsed diff row before it becomes a [`DiffLine`].
#[derive(Clone)]
struct Row {
    kind: LineKind,
    old: Option<usize>,
    new: Option<usize>,
    text: String,
}

impl Row {
    fn hunk(text: &str) -> Row { Row { kind: LineKind::Hunk, old: None, new: None, text: text.into() } }
    fn context(old: Option<usize>, new: Option<usize>, text: &str) -> Row { Row { kind: LineKind::Context, old, new, text: text.into() } }
}

/// Removals and additions of one change run alternate, so each old line sits above its new one.
fn pair_change_runs(rows: Vec<Row>) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != LineKind::Remove {
            out.push(rows[i].clone());
            i += 1;
            continue;
        }
        let remove_start = i;
        while i < rows.len() && rows[i].kind == LineKind::Remove { i += 1; }
        let add_start = i;
        while i < rows.len() && rows[i].kind == LineKind::Add { i += 1; }
        let (removes, adds) = (&rows[remove_start..add_start], &rows[add_start..i]);
        for k in 0..removes.len().max(adds.len()) {
            if let Some(r) = removes.get(k) { out.push(r.clone()); }
            if let Some(a) = adds.get(k) { out.push(a.clone()); }
        }
    }
    out
}

fn patch_sections(patch: &str, fallback: &str) -> Vec<Section> {
    if patch.contains("*** Begin Patch") {
        let sections = v4a_sections(patch);
        if !sections.is_empty() { return sections; }
    }
    if patch.contains("diff --git ") || (patch.contains("\n--- ") && patch.contains("\n+++ ")) {
        return unified_sections(patch, fallback);
    }
    Vec::new()
}

fn v4a_sections(patch: &str) -> Vec<Section> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut sections = Vec::new();
    let mut i = 0;
    let is_file_header = |l: &str| l.starts_with("*** Update File:") || l.starts_with("*** Add File:") || l.starts_with("*** Delete File:");
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("*** Add File:") {
            i += 1;
            let mut rows = Vec::new();
            let mut new = 1;
            while i < lines.len() && !lines[i].starts_with("***") {
                if let Some(text) = lines[i].strip_prefix('+') {
                    rows.push(Row { kind: LineKind::Add, old: None, new: Some(new), text: text.into() });
                    new += 1;
                }
                i += 1;
            }
            sections.push(from_rows(format!("A {}", rest.trim()), pair_change_runs(rows)));
        } else if let Some(rest) = line.strip_prefix("*** Delete File:") {
            sections.push(from_rows(format!("D {}", rest.trim()), vec![Row::hunk("Deleted file \u{b7} original content unavailable")]));
            i += 1;
        } else if let Some(rest) = line.strip_prefix("*** Update File:") {
            let mut path = rest.trim().to_owned();
            i += 1;
            if let Some(to) = lines.get(i).and_then(|l| l.strip_prefix("*** Move to:")) {
                path = format!("{path} -> {}", to.trim());
                i += 1;
            }
            let (mut rows, mut old, mut new) = (Vec::new(), None, None);
            while i < lines.len() && !is_file_header(lines[i]) && lines[i].trim() != "*** End Patch" {
                push_patch_line(lines[i], &mut rows, &mut old, &mut new);
                i += 1;
            }
            sections.push(from_rows(path, pair_change_runs(rows)));
        } else {
            i += 1;
        }
    }
    sections
}

/// One body line of a V4A or unified hunk, advancing the old/new counters.
fn push_patch_line(line: &str, rows: &mut Vec<Row>, old: &mut Option<usize>, new: &mut Option<usize>) {
    if line == "*** End of File" { return; }
    if line.starts_with("@@") {
        if let Some((o, n)) = hunk_starts(line) {
            *old = Some(o);
            *new = Some(n);
        }
        rows.push(Row::hunk(line));
    } else if let Some(text) = line.strip_prefix('+') {
        rows.push(Row { kind: LineKind::Add, old: None, new: *new, text: text.into() });
        *new = new.map(|n| n + 1);
    } else if let Some(text) = line.strip_prefix('-') {
        rows.push(Row { kind: LineKind::Remove, old: *old, new: None, text: text.into() });
        *old = old.map(|n| n + 1);
    } else {
        rows.push(Row::context(*old, *new, line.strip_prefix(' ').unwrap_or(line)));
        *old = old.map(|n| n + 1);
        *new = new.map(|n| n + 1);
    }
}

fn unified_sections(patch: &str, fallback: &str) -> Vec<Section> {
    let (mut old, mut new) = (None, None);
    let mut rows = Vec::new();
    let mut path = fallback.to_owned();
    let mut created = false;
    let mut sections = Vec::new();
    let flush = |path: &mut String, rows: &mut Vec<Row>, sections: &mut Vec<Section>| {
        if !rows.is_empty() || path.starts_with("A ") || path.starts_with("D ") {
            sections.push(from_rows(std::mem::take(path), pair_change_runs(std::mem::take(rows))));
        }
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(&mut path, &mut rows, &mut sections);
            if let Some(target) = rest.split_whitespace().nth(1) { path = trim_ab(target).to_owned(); }
            (old, new, created) = (None, None, false);
        } else if line.starts_with("index ") || line.starts_with("old mode ") || line.starts_with("new mode ")
            || line.starts_with("similarity index ") || line.starts_with("rename from ") || line.starts_with("rename to ")
        {
        } else if line.starts_with("new file mode ") {
            created = true;
            path = format!("A {}", link_target_of(&path));
        } else if line.starts_with("deleted file mode ") {
            path = format!("D {}", link_target_of(&path));
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            path = if rest == "/dev/null" { format!("D {path}") } else if created { format!("A {}", trim_ab(rest)) } else { trim_ab(rest).to_owned() };
        } else if let Some(rest) = line.strip_prefix("--- ") {
            created = rest == "/dev/null";
            if !created { path = trim_ab(rest).to_owned(); }
        } else {
            push_patch_line(line, &mut rows, &mut old, &mut new);
        }
    }
    if !rows.is_empty() || path.starts_with("A ") || path.starts_with("D ") {
        sections.push(from_rows(path, rows));
    }
    sections
}

/// Counts and gutter numbers come from the rows; `@@` hunk headers are dropped (the gutter
/// carries the line numbers instead).
fn from_rows(path: String, rows: Vec<Row>) -> Section {
    let rows: Vec<Row> = rows.into_iter().filter(|r| r.kind != LineKind::Hunk || !r.text.starts_with("@@")).collect();
    let additions = rows.iter().filter(|r| r.kind == LineKind::Add).count() as u32;
    let deletions = rows.iter().filter(|r| r.kind == LineKind::Remove).count() as u32;
    let lines = rows.into_iter().map(|r| DiffLine {
        text: match r.kind {
            LineKind::Add => format!("+{}", r.text),
            LineKind::Remove => format!("-{}", r.text),
            LineKind::Context => format!(" {}", r.text),
            LineKind::Hunk => r.text,
        },
        kind: r.kind,
        old: r.old.and_then(|n| u32::try_from(n).ok()),
        new: r.new.and_then(|n| u32::try_from(n).ok()),
    }).collect();
    Section { link_target: link_target_of(&path), path, additions, deletions, lines, diagnostics: Vec::new() }
}

/// The file a display path names: status prefix dropped, a move's destination.
fn link_target_of(path: &str) -> String {
    let path = path.trim();
    let path = ["A ", "M ", "D ", "R "].iter().find_map(|p| path.strip_prefix(p)).unwrap_or(path).trim();
    path.split(" -> ").last().unwrap_or(path).trim().to_owned()
}

fn attach_diagnostics(sections: &mut [Section], metadata: &Value) {
    for entry in metadata["diagnostics"].as_array().into_iter().flatten() {
        let Some(path) = entry["path"].as_str() else { continue };
        let mut errors = Vec::new();
        let mut others = Vec::new();
        for item in entry["diagnostics"].as_array().into_iter().flatten() {
            let Some(message) = item["message"].as_str() else { continue };
            let severity = item["severity"].as_str().unwrap_or("error");
            let start = &item["range"]["start"];
            let (line, col) = (start["line"].as_u64().unwrap_or(0) + 1, start["character"].as_u64().unwrap_or(0) + 1);
            let label = match severity { "warning" => "WARN", "information" => "INFO", "hint" => "HINT", _ => "ERROR" };
            let diagnostic = Diagnostic { error: severity == "error", text: format!("{label} [{line}:{col}] {}", message.trim()) };
            if diagnostic.error { errors.push(diagnostic) } else { others.push(diagnostic) }
        }
        errors.extend(others);
        if errors.is_empty() { continue; }
        let path = path.trim();
        if let Some(section) = sections.iter_mut().find(|s| s.link_target == path || s.link_target.ends_with(path) || path.ends_with(s.link_target.as_str())) {
            section.diagnostics = errors;
        }
    }
}

fn fallback_path(metadata: &Value, title: &str) -> Option<String> {
    metadata_path(metadata).map(str::to_owned).or_else(|| {
        let rest = &title[title.find('(')? + 1..];
        let path = rest[..rest.find(')')?].trim();
        (!path.is_empty()).then(|| path.to_owned())
    })
}

fn metadata_path(metadata: &Value) -> Option<&str> {
    metadata["paths"].as_array().and_then(|p| p.first()).and_then(Value::as_str).or_else(|| metadata["path"].as_str())
}

fn trim_ab(path: &str) -> &str {
    let path = path.trim();
    path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path)
}

fn hunk_starts(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace().skip(1);
    let start = |s: &str| s.split(',').next()?.parse().ok();
    Some((start(parts.next()?.trim_start_matches('-'))?, start(parts.next()?.trim_start_matches('+'))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4a_update_numbers_and_pairs_changes() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@ -10,3 +10,3 @@\n keep\n-old one\n-old two\n+new one\n+new two\n*** End Patch";
        let s = &sections("apply_patch", "completed", "ApplyPatch", "", patch)[0];
        assert_eq!(s.path, "src/a.rs");
        assert_eq!((s.additions, s.deletions), (2, 2));
        let texts: Vec<&str> = s.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, [" keep", "-old one", "+new one", "-old two", "+new two"]);
        assert_eq!(s.lines[0].number(), Some(10));
        assert_eq!(s.lines[1].number(), Some(11));
        assert_eq!(s.lines[2].number(), Some(11));
    }

    #[test]
    fn edit_record_files_carry_status_prefixes() {
        let detail = serde_json::json!({"neoismToolDetail": "edit", "metadata": {"files": [
            {"relativePath": "notes.md", "type": "add", "patch": "--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1,2 @@\n+one\n+two\n", "additions": 2, "deletions": 0}
        ]}}).to_string();
        let s = &sections("apply_patch", "completed", "ApplyPatch", "", &detail)[0];
        assert_eq!(s.path, "A notes.md");
        assert_eq!(s.link_target, "notes.md");
        assert_eq!(s.additions, 2);
    }

    #[test]
    fn running_patches_have_no_diff_yet() {
        assert!(sections("apply_patch", "running", "", "", "*** Begin Patch\n*** Add File: a\n+x").is_empty());
    }

    #[test]
    fn text_edits_keep_three_lines_of_context() {
        let before = (1..=10).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let after = before.replace("5", "five");
        let s = text_section("f".into(), &before, &after);
        let texts: Vec<&str> = s.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["...", " 2", " 3", " 4", "-5", "+five", " 6", " 7", " 8"]);
    }
}
