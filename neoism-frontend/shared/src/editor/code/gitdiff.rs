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
    let n = old.len();
    let m = new.len();
    if n == 0 {
        return Some(vec![LineOp::Insert; m]);
    }
    if m == 0 {
        return Some(vec![LineOp::Delete; n]);
    }
    let max = (n + m).min(MAX_DIFF_DEPTH);
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
                let mut x = if k == -d_i
                    || (k != d_i && v[ki - 1] < v[ki + 1])
                {
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
