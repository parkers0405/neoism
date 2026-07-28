//! Unified-diff parser shared between native and web.
//!
//! [`parse_numstat`] and [`parse_diff_into`] are pure byte/string
//! slicing — no IO, no `Command`, no `fs` — so they compile on wasm.
//! [`load_diff`] is the one entry point that actually shells out to
//! `git diff` and is therefore native-only (`cfg(not(target_arch =
//! "wasm32"))`). The web build feeds parsed diffs to the panel via the
//! daemon's `ServiceReply`/host bridge instead.

use std::collections::HashMap;

use crate::widgets::diff_card::{DiffLine, DiffLineKind};

#[cfg(not(target_arch = "wasm32"))]
use super::types::FileChange;
use super::MAX_DIFF_BYTES;

pub fn parse_numstat(bytes: &[u8]) -> HashMap<String, (u32, u32)> {
    let mut out = HashMap::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let record = &bytes[start..i];
        i = i.saturating_add(1);
        if record.is_empty() {
            continue;
        }
        let s = match std::str::from_utf8(record) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut parts = s.splitn(3, '\t');
        let add = parts.next().unwrap_or("0");
        let del = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("");
        if path.is_empty() {
            // Renamed: numstat with -z emits "<add>\t<del>\t" then two
            // separate NUL records for old and new paths.
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
            let new_start = i;
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            let new_path = &bytes[new_start..i];
            i = i.saturating_add(1);
            let new_path = String::from_utf8_lossy(new_path).into_owned();
            let a: u32 = add.parse().unwrap_or(0);
            let d: u32 = del.parse().unwrap_or(0);
            out.insert(new_path, (a, d));
            continue;
        }
        let a: u32 = add.parse().unwrap_or(0);
        let d: u32 = del.parse().unwrap_or(0);
        out.insert(path.to_string(), (a, d));
    }
    out
}

/// Native-only: shell out to `git diff` for `file` and parse the
/// result. Returns an empty `Vec` on failure. Excluded from wasm
/// builds — the web path receives already-parsed `DiffLine`s via the
/// daemon protocol.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_diff(repo_root: &std::path::Path, file: &FileChange) -> Vec<DiffLine> {
    use super::types::FileStatus;

    let mut lines = Vec::new();
    if matches!(file.status, FileStatus::Untracked) {
        let abs = repo_root.join(&file.path);
        if let Ok(output) = crate::utils::background_command("git")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C")
            .arg(repo_root)
            .args(["diff", "--no-index", "--no-color", "--", "/dev/null"])
            .arg(&abs)
            .output()
        {
            parse_diff_into(&output.stdout, &mut lines);
        }
        return lines;
    }
    if let Ok(output) = crate::utils::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
        .args(["diff", "HEAD", "--no-color", "--", &file.path])
        .output()
    {
        parse_diff_into(&output.stdout, &mut lines);
    }
    lines
}

pub fn parse_diff_into(bytes: &[u8], out: &mut Vec<DiffLine>) {
    let slice = if bytes.len() > MAX_DIFF_BYTES {
        &bytes[..MAX_DIFF_BYTES]
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(slice);
    // Track old/new line numbers independently so replacement rows can render
    // as `53 old` followed by `53 new` instead of one delete block then one add block.
    let mut old_line: u32 = 0;
    let mut new_line: u32 = 0;
    let mut hunk_rows: Vec<DiffLine> = Vec::new();
    for raw in text.lines() {
        if raw.starts_with("diff --git")
            || raw.starts_with("index ")
            || raw.starts_with("--- ")
            || raw.starts_with("+++ ")
            || raw.starts_with("new file mode")
            || raw.starts_with("deleted file mode")
            || raw.starts_with("similarity index")
            || raw.starts_with("rename from")
            || raw.starts_with("rename to")
        {
            // File-header preamble — skip; the card header above
            // already names the file.
            continue;
        } else if raw.starts_with("@@") {
            flush_hunk_rows(out, &mut hunk_rows);
            // Re-anchor the new-file line counter from the hunk header
            // but DON'T emit a row for it — the user explicitly didn't
            // want the `@@ -A,B +C,D @@` line cluttering the preview.
            if let Some(c) = parse_old_start(raw) {
                old_line = c;
            }
            if let Some(c) = parse_new_start(raw) {
                new_line = c;
            }
        } else if let Some(rest) = raw.strip_prefix('+') {
            let n = new_line;
            new_line = new_line.saturating_add(1);
            hunk_rows.push(DiffLine {
                text: rest.to_string(),
                kind: DiffLineKind::Add,
                line_number: Some(n),
                old_line_number: None,
                new_line_number: Some(n),
            });
        } else if let Some(rest) = raw.strip_prefix('-') {
            let n = old_line;
            old_line = old_line.saturating_add(1);
            hunk_rows.push(DiffLine {
                text: rest.to_string(),
                kind: DiffLineKind::Remove,
                line_number: Some(n),
                old_line_number: Some(n),
                new_line_number: None,
            });
        } else {
            let body = raw.strip_prefix(' ').unwrap_or(raw).to_string();
            let old = old_line;
            let new = new_line;
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
            hunk_rows.push(DiffLine {
                text: body,
                kind: DiffLineKind::Context,
                line_number: Some(new),
                old_line_number: Some(old),
                new_line_number: Some(new),
            });
        }
    }
    flush_hunk_rows(out, &mut hunk_rows);
}

/// Pull the new-file starting line out of a `@@ -A,B +C,D @@` header.
pub fn parse_new_start(hunk: &str) -> Option<u32> {
    let after = hunk.strip_prefix("@@")?.trim_start();
    let plus = after.split_whitespace().find(|s| s.starts_with('+'))?;
    let body = plus.trim_start_matches('+');
    let start = body.split(',').next()?;
    start.parse().ok()
}

fn parse_old_start(hunk: &str) -> Option<u32> {
    let after = hunk.strip_prefix("@@")?.trim_start();
    let minus = after.split_whitespace().find(|s| s.starts_with('-'))?;
    let body = minus.trim_start_matches('-');
    let start = body.split(',').next()?;
    start.parse().ok()
}

fn flush_hunk_rows(out: &mut Vec<DiffLine>, rows: &mut Vec<DiffLine>) {
    let mut input = std::mem::take(rows).into_iter().peekable();
    while let Some(row) = input.next() {
        if row.kind != DiffLineKind::Remove {
            out.push(row);
            continue;
        }

        let mut removes = vec![row];
        while input
            .peek()
            .is_some_and(|next| next.kind == DiffLineKind::Remove)
        {
            removes.push(input.next().unwrap());
        }

        let mut adds = Vec::new();
        while input
            .peek()
            .is_some_and(|next| next.kind == DiffLineKind::Add)
        {
            adds.push(input.next().unwrap());
        }

        if adds.is_empty() {
            out.extend(removes);
            continue;
        }

        for index in 0..removes.len().max(adds.len()) {
            if let Some(remove) = removes.get(index) {
                out.push(remove.clone());
            }
            if let Some(add) = adds.get(index) {
                out.push(add.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diff_pairs_replacement_rows_by_line_number() {
        let diff = b"diff --git a/src/lib.rs b/src/lib.rs\n\
--- a/src/lib.rs\n\
+++ b/src/lib.rs\n\
@@ -53,3 +53,3 @@\n\
-old one\n\
-old two\n\
+new one\n\
+new two\n\
 context\n";
        let mut rows = Vec::new();
        parse_diff_into(diff, &mut rows);

        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].kind, DiffLineKind::Remove);
        assert_eq!(rows[0].text, "old one");
        assert_eq!(rows[0].line_number, Some(53));
        assert_eq!(rows[1].kind, DiffLineKind::Add);
        assert_eq!(rows[1].text, "new one");
        assert_eq!(rows[1].line_number, Some(53));
        assert_eq!(rows[2].kind, DiffLineKind::Remove);
        assert_eq!(rows[2].line_number, Some(54));
        assert_eq!(rows[3].kind, DiffLineKind::Add);
        assert_eq!(rows[3].line_number, Some(54));
        assert_eq!(rows[4].kind, DiffLineKind::Context);
        assert_eq!(rows[4].line_number, Some(55));
    }
}
