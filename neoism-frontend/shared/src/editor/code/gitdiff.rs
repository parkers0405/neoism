//! Line-level git gutter marks: the buffer diffed against the file's
//! HEAD baseline (Zed's `buffer_diff` shape, sized for our model — the
//! host feeds the baseline, we recompute marks per buffer revision
//! instead of anchoring hunks across edits).
//!
//! The diff is a plain Myers O((N+M)·D) over line hashes with a
//! prefix/suffix trim, which keeps typical edit sessions (small D)
//! linear. Callers gate by file size; pathological D is capped and
//! falls back to marking the whole trimmed span as modified.

use std::collections::HashMap;

#[path = "gitdiff_review.rs"]
mod review_builder;
pub use review_builder::ReviewDiffBuilder;

/// Build immutable review rows synchronously from current CodeBuffer text.
/// Positions are 1-based; `ensure_source_line` is 0-based. Unchanged files
/// produce no excerpt unless a caret row was requested. UI callers should poll
/// `ReviewDiffBuilder` instead of doing synchronous work in the paint path.
pub fn build_review_diff(
    base: &str,
    current: &str,
    context: usize,
    ensure_source_line: Option<usize>,
) -> Vec<crate::widgets::diff_card::DiffLine> {
    let mut builder = ReviewDiffBuilder::new(
        base.to_owned(),
        current.to_owned(),
        context,
        ensure_source_line,
    );
    loop {
        if let Some(rows) = builder.poll(16_384) {
            return rows;
        }
    }
}

// Retain the previous implementation as an independent semantic oracle.
#[cfg(test)]
fn reference_review_diff(
    base: &str,
    current: &str,
    context: usize,
    ensure_source_line: Option<usize>,
) -> Vec<crate::widgets::diff_card::DiffLine> {
    use crate::widgets::diff_card::{DiffLine, DiffLineKind as K};
    // Keep terminators in the comparison: changing only EOF newline is a real
    // replacement, while an empty file has zero diff lines (not one blank line).
    let old: Vec<String> = base.split_inclusive('\n').map(str::to_owned).collect();
    let new: Vec<String> = current.split_inclusive('\n').map(str::to_owned).collect();
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len().min(new.len()) - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut ops = vec![LineOp::Keep; prefix];
    ops.extend(
        myers_line_ops_bounded(
            &old[prefix..old.len() - suffix],
            &new[prefix..new.len() - suffix],
            256,
        )
        .unwrap_or_else(|| {
            let mut ops = vec![LineOp::Delete; old.len() - prefix - suffix];
            ops.extend(vec![LineOp::Insert; new.len() - prefix - suffix]);
            ops
        }),
    );
    ops.extend(vec![LineOp::Keep; suffix]);
    let (mut oi, mut ni) = (0, 0);
    let mut rows = Vec::new();
    let mut unterminated = Vec::new();
    for op in ops {
        let (kind, text, old_num, new_num) = match op {
            LineOp::Keep => {
                oi += 1;
                ni += 1;
                (K::Context, &new[ni - 1], Some(oi as u32), Some(ni as u32))
            }
            LineOp::Delete => {
                oi += 1;
                (K::Remove, &old[oi - 1], Some(oi as u32), None)
            }
            LineOp::Insert => {
                ni += 1;
                (K::Add, &new[ni - 1], None, Some(ni as u32))
            }
        };
        unterminated.push(!text.ends_with('\n'));
        let display = text
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(text);
        rows.push(DiffLine {
            text: display.to_owned(),
            kind,
            line_number: new_num.or(old_num),
            old_line_number: old_num,
            new_line_number: new_num,
        });
    }
    // CodeBuffer represents an empty document (and a trailing-newline caret)
    // with a real empty source row. Include it ONLY when explicitly requested;
    // it is not an added line and must not affect diff counts.
    if let Some(line) = ensure_source_line {
        if (current.is_empty() && line == 0)
            || (current.ends_with('\n') && line == new.len())
        {
            rows.push(DiffLine {
                text: String::new(),
                kind: K::Context,
                line_number: Some(line as u32 + 1),
                old_line_number: None,
                new_line_number: Some(line as u32 + 1),
            });
            unterminated.push(false);
        }
    }
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if row.kind != K::Context
            || ensure_source_line
                .is_some_and(|line| row.new_line_number == Some(line as u32 + 1))
        {
            let start = i.saturating_sub(context);
            let end = i.saturating_add(context).saturating_add(1).min(rows.len());
            if let Some(last) = ranges.last_mut().filter(|last| start <= last.end) {
                last.end = last.end.max(end);
            } else {
                ranges.push(start..end);
            }
        }
    }
    let mut result = Vec::new();
    for range in ranges {
        let excerpt = &rows[range.clone()];
        let old_start = rows[..range.start]
            .iter()
            .filter(|r| r.old_line_number.is_some())
            .count();
        let new_start = rows[..range.start]
            .iter()
            .filter(|r| r.new_line_number.is_some())
            .count();
        let old_count = excerpt
            .iter()
            .filter(|r| r.old_line_number.is_some())
            .count();
        let new_count = excerpt
            .iter()
            .filter(|r| r.new_line_number.is_some())
            .count();
        result.push(DiffLine {
            text: format!(
                "@@ -{},{} +{},{} @@",
                old_start + usize::from(old_count > 0),
                old_count,
                new_start + usize::from(new_count > 0),
                new_count
            ),
            kind: K::Hunk,
            line_number: None,
            old_line_number: None,
            new_line_number: None,
        });
        for i in range {
            result.push(rows[i].clone());
            if unterminated[i] {
                result.push(DiffLine {
                    text: "\\ No newline at end of file".into(),
                    kind: K::Hunk,
                    line_number: None,
                    old_line_number: None,
                    new_line_number: None,
                });
            }
        }
    }
    result
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::widgets::diff_card::DiffLineKind as K;

    #[test]
    fn positions_unicode_insert_delete_and_eof() {
        let rows = build_review_diff("α\nold\nlast", "α\n新\nlast\n", 3, None);
        let actual: Vec<_> = rows
            .iter()
            .filter(|r| r.kind != K::Hunk)
            .map(|r| {
                (
                    r.kind,
                    r.old_line_number,
                    r.new_line_number,
                    r.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                (K::Context, Some(1), Some(1), "α"),
                (K::Remove, Some(2), None, "old"),
                (K::Remove, Some(3), None, "last"),
                (K::Add, None, Some(2), "新"),
                (K::Add, None, Some(3), "last")
            ]
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r.text == "\\ No newline at end of file")
                .count(),
            1
        );
        assert_eq!(rows[0].text, "@@ -1,3 +1,3 @@");
        assert!(build_review_diff("same", "same", 3, None).is_empty());
        assert!(build_review_diff("", "", 3, None).is_empty());
        assert_eq!(
            build_review_diff("", "x\n", 0, None)[0].text,
            "@@ -0,0 +1,1 @@"
        );
        assert_eq!(
            build_review_diff("x\n", "", 0, None)[0].text,
            "@@ -1,1 +0,0 @@"
        );
    }

    #[test]
    fn ensure_caret_context_merges_and_empty_documents() {
        let old = (0..30).map(|i| format!("line {i}\n")).collect::<String>();
        let current = old.replace("line 2\n", "changed\n");
        let rows = build_review_diff(&old, &current, 1, Some(25));
        assert_eq!(rows.iter().filter(|r| r.text.starts_with("@@")).count(), 2);
        assert!(rows
            .iter()
            .any(|r| r.new_line_number == Some(26) && r.text == "line 25"));
        assert!(!rows.iter().any(|r| r.new_line_number == Some(15)));
        let empty = build_review_diff("x", "", 0, Some(0));
        assert!(empty
            .iter()
            .any(|r| r.new_line_number == Some(1) && r.text.is_empty()));
        let same = build_review_diff("α\nβ", "α\nβ", 0, Some(1));
        assert!(same
            .iter()
            .any(|r| r.kind == K::Context && r.new_line_number == Some(2)));
    }

    #[test]
    fn live_buffer_insertion_deletion_keeps_cursor_in_projection() {
        use super::super::types::CodeBuffer;
        let baseline = "α\nβ\nγ";
        let mut buffer = CodeBuffer::from_text(baseline);
        buffer.set_cursor_position(1, 2, false);
        buffer.insert_text("\n新");
        let rows =
            build_review_diff(baseline, &buffer.text(), 0, Some(buffer.cursor_line));
        assert!(rows
            .iter()
            .any(|r| r.new_line_number == Some(buffer.cursor_line as u32 + 1)
                && r.text == "新"));
        buffer.set_cursor_position(1, 0, false);
        buffer.set_cursor_position(2, 3, true);
        buffer.delete_selection();
        let rows =
            build_review_diff(baseline, &buffer.text(), 0, Some(buffer.cursor_line));
        assert!(rows
            .iter()
            .any(|r| r.new_line_number == Some(buffer.cursor_line as u32 + 1)));
        assert!(rows.iter().any(|r| r.kind == K::Remove));
    }

    #[test]
    fn bounded_rewrite_and_repeated_lines_still_have_exact_positions() {
        let old = "old\n".repeat(300);
        let new = "new\n".repeat(300);
        let rows = build_review_diff(&old, &new, 0, None);
        assert_eq!(rows.iter().filter(|r| r.kind == K::Remove).count(), 300);
        assert_eq!(rows.iter().filter(|r| r.kind == K::Add).count(), 300);
        assert_eq!(rows.last().unwrap().new_line_number, Some(300));
        let rows = build_review_diff("a\nx\na\n", "a\na\n", 3, None);
        assert!(rows
            .iter()
            .any(|r| r.kind == K::Remove && r.old_line_number == Some(2)));
    }
}

/// Gutter mark for one buffer line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeGitMark {
    /// Line does not exist in the baseline.
    Added,
    /// Line replaces baseline content.
    Modified,
}

/// The computed gutter state: per-line marks plus the buffer lines that
/// have one or more baseline lines DELETED directly above them
/// (rendered as a thin marker at the row's top edge; nvim gitsigns'
/// `_` / `‾` analog).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodeGitMarks {
    pub lines: HashMap<usize, CodeGitMark>,
    pub deleted_above: Vec<usize>,
}

impl CodeGitMarks {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.deleted_above.is_empty()
    }
}

/// Bound on the Myers search depth. An edit distance beyond this (huge
/// rewrites) marks the whole changed span Modified instead of walking
/// the full quadratic frontier.
const MAX_DIFF_DEPTH: usize = 2048;

/// Diff `current` against `baseline` (both as line slices), producing
/// gutter marks in CURRENT-buffer line numbers.
pub fn compute_git_marks(baseline: &[String], current: &[String]) -> CodeGitMarks {
    let mut marks = CodeGitMarks::default();
    if baseline == current {
        return marks;
    }

    // Trim the common prefix/suffix — the Myers walk then only sees
    // the changed core, which for normal editing is tiny.
    let mut prefix = 0usize;
    let max_prefix = baseline.len().min(current.len());
    while prefix < max_prefix && baseline[prefix] == current[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    let max_suffix = max_prefix - prefix;
    while suffix < max_suffix
        && baseline[baseline.len() - 1 - suffix] == current[current.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old = &baseline[prefix..baseline.len() - suffix];
    let new = &current[prefix..current.len() - suffix];

    let ops = myers_line_ops(old, new);
    let Some(ops) = ops else {
        // Depth cap hit: whole core span is "modified", with a
        // deletion marker if the baseline core was longer.
        for (offset, _) in new.iter().enumerate() {
            marks.lines.insert(prefix + offset, CodeGitMark::Modified);
        }
        if old.len() > new.len() {
            marks.deleted_above.push(prefix + new.len());
        }
        if new.is_empty() && !old.is_empty() {
            marks.deleted_above.push(prefix);
        }
        marks.deleted_above.sort_unstable();
        marks.deleted_above.dedup();
        return marks;
    };

    // Walk the op list pairing deletions with insertions into
    // "modified" runs (a delete immediately followed by an insert at
    // the same cursor is an edit, not remove+add — gitsigns/Zed
    // convention).
    let mut new_line = prefix;
    let mut pending_deletes = 0usize;
    for op in ops {
        match op {
            LineOp::Keep => {
                if pending_deletes > 0 {
                    marks.deleted_above.push(new_line);
                    pending_deletes = 0;
                }
                new_line += 1;
            }
            LineOp::Delete => {
                pending_deletes += 1;
            }
            LineOp::Insert => {
                let mark = if pending_deletes > 0 {
                    pending_deletes -= 1;
                    CodeGitMark::Modified
                } else {
                    CodeGitMark::Added
                };
                marks.lines.insert(new_line, mark);
                new_line += 1;
            }
        }
    }
    if pending_deletes > 0 {
        marks.deleted_above.push(new_line);
    }
    marks.deleted_above.sort_unstable();
    marks.deleted_above.dedup();
    marks
}

/// Exact unchanged-line correspondence. Changed/new lines are None, never
/// matched by a global text lookup (which misattributes repeated lines).
/// On the bounded Myers fallback only proven prefix/suffix lines survive.
pub fn unchanged_line_map(baseline: &[String], current: &[String]) -> Vec<Option<usize>> {
    unchanged_line_map_checked(baseline, current).0
}

/// The boolean is false when the edit-distance budget was exhausted. Callers
/// must distinguish unknown middle spans from genuinely uncommitted lines.
pub fn unchanged_line_map_checked(
    baseline: &[String],
    current: &[String],
) -> (Vec<Option<usize>>, bool) {
    let mut complete = false;
    let mut map = vec![None; current.len()];
    let mut prefix = 0;
    while prefix < baseline.len().min(current.len())
        && baseline[prefix] == current[prefix]
    {
        map[prefix] = Some(prefix);
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < baseline.len().min(current.len()) - prefix
        && baseline[baseline.len() - 1 - suffix] == current[current.len() - 1 - suffix]
    {
        map[current.len() - 1 - suffix] = Some(baseline.len() - 1 - suffix);
        suffix += 1;
    }
    if let Some(ops) = myers_line_ops_bounded(
        &baseline[prefix..baseline.len() - suffix],
        &current[prefix..current.len() - suffix],
        128,
    ) {
        complete = true;
        let (mut old, mut new) = (prefix, prefix);
        for op in ops {
            match op {
                LineOp::Keep => {
                    map[new] = Some(old);
                    old += 1;
                    new += 1;
                }
                LineOp::Delete => old += 1,
                LineOp::Insert => new += 1,
            }
        }
    }
    (map, complete)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineOp {
    Keep,
    Delete,
    Insert,
}

/// Classic Myers over the trimmed cores; `None` when the edit distance
/// exceeds [`MAX_DIFF_DEPTH`]. Ops are emitted old-to-new with Delete
/// ordered before Insert inside a changed region.
fn myers_line_ops(old: &[String], new: &[String]) -> Option<Vec<LineOp>> {
    myers_line_ops_bounded(old, new, MAX_DIFF_DEPTH)
}

fn myers_line_ops_bounded(
    old: &[String],
    new: &[String],
    depth_limit: usize,
) -> Option<Vec<LineOp>> {
    let n = old.len();
    let m = new.len();
    if n == 0 {
        return Some(vec![LineOp::Insert; m]);
    }
    if m == 0 {
        return Some(vec![LineOp::Delete; n]);
    }
    let max = (n + m).min(depth_limit);
    let offset = max;
    // v[k + offset] = furthest x on diagonal k; one frontier per depth
    // is kept for backtracking.
    let mut v = vec![0usize; 2 * max + 1];
    let mut trace: Vec<Vec<usize>> = Vec::with_capacity(max + 1);
    'outer: {
        for d in 0..=max {
            trace.push(v.clone());
            let d_i = d as isize;
            let mut k = -d_i;
            while k <= d_i {
                let ki = (k + offset as isize) as usize;
                let mut x = if k == -d_i || (k != d_i && v[ki - 1] < v[ki + 1]) {
                    v[ki + 1]
                } else {
                    v[ki - 1] + 1
                };
                let mut y = (x as isize - k) as usize;
                while x < n && y < m && old[x] == new[y] {
                    x += 1;
                    y += 1;
                }
                v[ki] = x;
                if x >= n && y >= m {
                    break 'outer;
                }
                k += 2;
            }
            if d == max {
                return None;
            }
        }
        return None;
    }

    // Backtrack from (n, m) through the saved frontiers.
    let mut ops_rev: Vec<LineOp> = Vec::with_capacity(n + m);
    let mut x = n;
    let mut y = m;
    for d in (1..trace.len()).rev() {
        let v = &trace[d];
        let d_i = d as isize;
        let k = x as isize - y as isize;
        let ki = (k + offset as isize) as usize;
        let prev_k = if k == -d_i || (k != d_i && v[ki - 1] < v[ki + 1]) {
            k + 1
        } else {
            k - 1
        };
        let prev_ki = (prev_k + offset as isize) as usize;
        let prev_x = v[prev_ki];
        let prev_y = (prev_x as isize - prev_k) as usize;
        // Snake (diagonal run) back to the frontier point.
        while x > prev_x && y > prev_y {
            ops_rev.push(LineOp::Keep);
            x -= 1;
            y -= 1;
        }
        if x == prev_x {
            ops_rev.push(LineOp::Insert);
            y -= 1;
        } else {
            ops_rev.push(LineOp::Delete);
            x -= 1;
        }
    }
    // Leading snake at depth 0.
    while x > 0 && y > 0 {
        ops_rev.push(LineOp::Keep);
        x -= 1;
        y -= 1;
    }
    while x > 0 {
        ops_rev.push(LineOp::Delete);
        x -= 1;
    }
    while y > 0 {
        ops_rev.push(LineOp::Insert);
        y -= 1;
    }
    ops_rev.reverse();

    // Normalize: within each changed region, deletes before inserts so
    // the modified-pairing walk is deterministic.
    let mut ops: Vec<LineOp> = Vec::with_capacity(ops_rev.len());
    let mut run: Vec<LineOp> = Vec::new();
    for op in ops_rev {
        match op {
            LineOp::Keep => {
                flush_run(&mut ops, &mut run);
                ops.push(LineOp::Keep);
            }
            other => run.push(other),
        }
    }
    flush_run(&mut ops, &mut run);
    Some(ops)
}

fn flush_run(ops: &mut Vec<LineOp>, run: &mut Vec<LineOp>) {
    let deletes = run.iter().filter(|op| **op == LineOp::Delete).count();
    for _ in 0..deletes {
        ops.push(LineOp::Delete);
    }
    for _ in 0..(run.len() - deletes) {
        ops.push(LineOp::Insert);
    }
    run.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn identical_is_empty() {
        let a = lines(&["a", "b"]);
        assert!(compute_git_marks(&a, &a).is_empty());
    }

    #[test]
    fn pure_insert_marks_added() {
        let base = lines(&["a", "c"]);
        let cur = lines(&["a", "b", "c"]);
        let marks = compute_git_marks(&base, &cur);
        assert_eq!(marks.lines.get(&1), Some(&CodeGitMark::Added));
        assert_eq!(marks.lines.len(), 1);
        assert!(marks.deleted_above.is_empty());
    }

    #[test]
    fn edit_marks_modified() {
        let base = lines(&["a", "b", "c"]);
        let cur = lines(&["a", "B", "c"]);
        let marks = compute_git_marks(&base, &cur);
        assert_eq!(marks.lines.get(&1), Some(&CodeGitMark::Modified));
        assert_eq!(marks.lines.len(), 1);
        assert!(marks.deleted_above.is_empty());
    }

    #[test]
    fn deletion_marks_the_next_line() {
        let base = lines(&["a", "b", "c"]);
        let cur = lines(&["a", "c"]);
        let marks = compute_git_marks(&base, &cur);
        assert!(marks.lines.is_empty());
        assert_eq!(marks.deleted_above, vec![1]);
    }

    #[test]
    fn replace_two_with_three_pairs_then_adds() {
        let base = lines(&["a", "x", "y", "d"]);
        let cur = lines(&["a", "1", "2", "3", "d"]);
        let marks = compute_git_marks(&base, &cur);
        assert_eq!(marks.lines.get(&1), Some(&CodeGitMark::Modified));
        assert_eq!(marks.lines.get(&2), Some(&CodeGitMark::Modified));
        assert_eq!(marks.lines.get(&3), Some(&CodeGitMark::Added));
        assert!(marks.deleted_above.is_empty());
    }

    #[test]
    fn trailing_deletion_lands_past_last_line() {
        let base = lines(&["a", "b"]);
        let cur = lines(&["a"]);
        let marks = compute_git_marks(&base, &cur);
        assert_eq!(marks.deleted_above, vec![1]);
    }

    #[test]
    fn empty_baseline_marks_everything_added() {
        let base = lines(&[]);
        let cur = lines(&["a", "b"]);
        let marks = compute_git_marks(&base, &cur);
        assert_eq!(marks.lines.len(), 2);
        assert_eq!(marks.lines.get(&0), Some(&CodeGitMark::Added));
    }
}
