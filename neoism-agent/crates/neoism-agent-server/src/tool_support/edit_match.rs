pub(crate) struct MatchResult {
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) span: String,
}

const MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD: f64 = 0.30;

pub(crate) fn find(content: &str, needle: &str) -> Option<MatchResult> {
    if needle.is_empty() {
        return Some(MatchResult {
            range: 0..0,
            span: String::new(),
        });
    }
    if let Some(start) = content.find(needle) {
        let end = start + needle.len();
        return Some(MatchResult {
            range: start..end,
            span: needle.to_string(),
        });
    }
    if let Some(found) = line_trimmed_match(content, needle) {
        return Some(found);
    }
    if let Some(found) = whitespace_normalized_match(content, needle) {
        return Some(found);
    }
    if let Some(found) = indentation_flexible_match(content, needle) {
        return Some(found);
    }
    if let Some(found) = escape_normalized_match(content, needle) {
        return Some(found);
    }
    if let Some(found) = trimmed_boundary_match(content, needle) {
        return Some(found);
    }
    if let Some(found) = block_anchor_match(content, needle) {
        return Some(found);
    }
    if let Some(found) = best_anchor_match(content, needle) {
        return Some(found);
    }
    None
}

fn best_anchor_match(content: &str, needle: &str) -> Option<MatchResult> {
    let needle_lines = needle
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if needle_lines.len() < 3 {
        return None;
    }
    let first = needle_lines.first()?.trim();
    let last = needle_lines.last()?.trim();
    if first.is_empty() || last.is_empty() {
        return None;
    }
    let mut best: Option<(f64, LineWindow<'_>)> = None;
    for candidate in anchor_windows(content, first, last) {
        let actual = candidate.text.split('\n').collect::<Vec<_>>();
        let similarity = middle_similarity(&actual, &needle_lines);
        if similarity >= MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD
            && best.as_ref().is_none_or(|(score, _)| similarity > *score)
        {
            best = Some((similarity, candidate));
        }
    }
    best.map(|(_, candidate)| candidate.into_match())
}

#[derive(Clone, Copy)]
struct LineWindow<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

impl LineWindow<'_> {
    fn into_match(self) -> MatchResult {
        MatchResult {
            range: self.start..self.end,
            span: self.text.to_string(),
        }
    }
}

fn line_windows(content: &str, line_count: usize) -> Vec<LineWindow<'_>> {
    if line_count == 0 {
        return Vec::new();
    }
    let starts = line_starts(content);
    if starts.len() < line_count {
        return Vec::new();
    }
    let mut windows = Vec::new();
    for start_line in 0..=(starts.len() - line_count) {
        let start = starts[start_line];
        let end_line = start_line + line_count;
        let end = if end_line < starts.len() {
            starts[end_line] - 1
        } else {
            content.len()
        };
        let end = end.min(content.len());
        windows.push(LineWindow {
            start,
            end,
            text: &content[start..end],
        });
    }
    windows
}

fn anchor_windows<'a>(content: &'a str, first: &str, last: &str) -> Vec<LineWindow<'a>> {
    let starts = line_starts(content);
    let mut windows = Vec::new();
    for start_line in 0..starts.len() {
        if line_text(content, &starts, start_line).trim() != first {
            continue;
        }
        for end_line in (start_line + 2)..starts.len() {
            if line_text(content, &starts, end_line).trim() != last {
                continue;
            }
            let start = starts[start_line];
            let end = if end_line + 1 < starts.len() {
                starts[end_line + 1] - 1
            } else {
                content.len()
            }
            .min(content.len());
            windows.push(LineWindow {
                start,
                end,
                text: &content[start..end],
            });
        }
    }
    windows
}

fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, ch) in content.char_indices() {
        if ch == '\n' && i + 1 < content.len() {
            starts.push(i + 1);
        }
    }
    starts
}

fn line_text<'a>(content: &'a str, starts: &[usize], idx: usize) -> &'a str {
    let start = starts[idx];
    let end = if idx + 1 < starts.len() {
        starts[idx + 1] - 1
    } else {
        content.len()
    };
    &content[start..end.min(content.len())]
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_escapes(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\\"", "\"")
        .replace("\\'", "'")
}

fn middle_similarity(actual: &[&str], wanted: &[&str]) -> f64 {
    if actual.len() <= 2 || wanted.len() <= 2 {
        return 0.0;
    }
    let count = actual.len().min(wanted.len()).saturating_sub(2);
    if count == 0 {
        return 0.0;
    }
    let score = (0..count)
        .map(|idx| line_similarity(actual[idx + 1].trim(), wanted[idx + 1].trim()))
        .sum::<f64>();
    score / count as f64
}

fn line_similarity(left: &str, right: &str) -> f64 {
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    1.0 - levenshtein(left, right) as f64 / max_len as f64
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];
    for (i, left_ch) in left.chars().enumerate() {
        current[0] = i + 1;
        for (j, right_ch) in right_chars.iter().enumerate() {
            let cost = usize::from(left_ch != *right_ch);
            current[j + 1] = (current[j] + 1)
                .min(previous[j + 1] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_indentation_flexible_match() {
        let content = "fn main() {\n    println!(\"hi\");\n}\n";
        let found = find(content, "fn main() {\nprintln!(\"hi\");\n}").unwrap();
        assert_eq!(found.span, "fn main() {\n    println!(\"hi\");\n}");
    }

    #[test]
    fn finds_escape_normalized_match() {
        let content = "first\nsecond\n";
        let found = find(content, "first\\nsecond").unwrap();
        assert_eq!(found.span, "first\nsecond");
    }

    #[test]
    fn replace_rejects_ambiguous_single_edit() {
        let error = replace("a\na\n", "a", "b", false).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("multiple"));
        assert!(message.contains("Candidate locations"));
        assert!(message.contains("1: a"));
        assert!(message.contains("2: a"));
    }

    #[test]
    fn replace_all_uses_fuzzy_matches_repeatedly() {
        let (updated, count, remaining) = replace("  a\n  a\n", "a", "b", true).unwrap();
        assert_eq!(updated, "  b\n  b\n");
        assert_eq!(count, 2);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn replace_all_terminates_when_new_contains_old() {
        // Regression: `new` contains `old`, so re-scanning from 0 would re-find
        // the just-inserted text forever (100% CPU hang). Must terminate once.
        let (updated, count, _) = replace("foo\n", "foo", "foo_bar", true).unwrap();
        assert_eq!(updated, "foo_bar\n");
        assert_eq!(count, 1);
    }

    #[test]
    fn replace_all_terminates_on_multi_occurrence_superset() {
        // The real freeze: an identical block appears 3× and the replacement is
        // a superset that still contains the original lines (replaceAll edit).
        let content = "let x = a\nlet x = a\nlet x = a\n";
        let (updated, count, _) =
            replace(content, "let x = a", "let x = a\n    .b()", true).unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            updated,
            "let x = a\n    .b()\nlet x = a\n    .b()\nlet x = a\n    .b()\n"
        );
    }
}

pub(crate) fn replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> anyhow::Result<(String, usize, usize)> {
    if replace_all {
        // Mirror opencode's edit.ts: run the fuzzy matchers ONCE to discover a
        // single concrete `search` span, then replace every occurrence of that
        // concrete string. opencode uses `content.replaceAll(search, new)`;
        // Rust's `str::replace` is the exact equivalent — it matches the
        // original (non-overlapping, left-to-right) and never re-scans the text
        // it just inserted. The old code instead looped `find(&current, old)`,
        // re-running the fuzzy cascade over the mutated buffer from position 0,
        // so when `new` contains (or fuzzy-matches) `old` it re-found the
        // just-inserted replacement forever — a 100% CPU hang on any replaceAll
        // edit whose replacement is a superset of the target.
        let found = find(content, old).filter(|found| !found.span.is_empty());
        let Some(found) = found else {
            anyhow::bail!(
                "old text was not found — even after fuzzy matching. Re-read the file and supply the exact text you want to replace, including indentation."
            );
        };
        let count = content.matches(found.span.as_str()).count();
        let updated = content.replace(found.span.as_str(), new);
        return Ok((updated, count, 0));
    }

    let found = find(content, old).ok_or_else(|| {
        anyhow::anyhow!(
            "old text was not found — even after fuzzy matching. Re-read the file and supply the exact text you want to replace, including indentation."
        )
    })?;
    let remaining = count_matches_after(content, old, &found.span, found.range.clone());
    if remaining > 0 {
        anyhow::bail!(ambiguous_message(content, old, &found.span, &found.range));
    }
    let mut updated = String::with_capacity(content.len() + new.len());
    updated.push_str(&content[..found.range.start]);
    updated.push_str(new);
    updated.push_str(&content[found.range.end..]);
    Ok((updated, 1, 0))
}

fn ambiguous_message(
    content: &str,
    old: &str,
    span: &str,
    replaced: &std::ops::Range<usize>,
) -> String {
    let mut previews = candidate_previews(content, span);
    if previews.len() < 2 {
        let mut remaining = content.to_string();
        remaining.replace_range(replaced.clone(), "");
        if let Some(found) = find(&remaining, old) {
            previews.push(preview_at(&remaining, found.range.start));
        }
    }
    let preview_text = if previews.is_empty() {
        String::new()
    } else {
        format!("\nCandidate locations:\n{}", previews.join("\n---\n"))
    };
    format!(
        "old text matched multiple locations. Provide more surrounding lines that make the target unique, or set replaceAll to true if every match should change.{preview_text}"
    )
}

fn candidate_previews(content: &str, span: &str) -> Vec<String> {
    let mut previews = Vec::new();
    for (start, _) in content.match_indices(span).take(4) {
        previews.push(preview_at(content, start));
    }
    previews
}

fn preview_at(content: &str, byte_offset: usize) -> String {
    let starts = line_starts(content);
    let line_index = starts
        .iter()
        .enumerate()
        .take_while(|(_, start)| **start <= byte_offset)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(0);
    let start_line = line_index.saturating_sub(1);
    let end_line = (line_index + 2).min(starts.len());
    let mut lines = Vec::new();
    for index in start_line..end_line {
        lines.push(format!(
            "{}: {}",
            index + 1,
            line_text(content, &starts, index)
        ));
    }
    lines.join("\n")
}

fn count_matches_after(
    content: &str,
    old: &str,
    span: &str,
    replaced: std::ops::Range<usize>,
) -> usize {
    let direct = content.matches(span).count();
    if direct > 1 {
        return direct - 1;
    }
    let mut remaining = content.to_string();
    remaining.replace_range(replaced, "");
    usize::from(find(&remaining, old).is_some())
}

fn line_trimmed_match(content: &str, needle: &str) -> Option<MatchResult> {
    let needle_lines: Vec<&str> = needle.split('\n').collect();
    if needle_lines.is_empty() {
        return None;
    }
    let trimmed_needle: Vec<&str> = needle_lines.iter().map(|l| l.trim()).collect();
    let mut line_starts: Vec<usize> = vec![0];
    for (i, ch) in content.char_indices() {
        if ch == '\n' {
            line_starts.push(i + 1);
        }
    }
    let total_lines = line_starts.len();
    if total_lines < trimmed_needle.len() {
        return None;
    }
    for start_line in 0..=(total_lines - trimmed_needle.len()) {
        let mut all_match = true;
        for (offset, want) in trimmed_needle.iter().enumerate() {
            let line_idx = start_line + offset;
            let line_start = line_starts[line_idx];
            let line_end = if line_idx + 1 < total_lines {
                line_starts[line_idx + 1] - 1
            } else {
                content.len()
            };
            let line = &content[line_start..line_end.min(content.len())];
            if line.trim() != *want {
                all_match = false;
                break;
            }
        }
        if all_match {
            let start = line_starts[start_line];
            let end_line = start_line + trimmed_needle.len();
            let end = if end_line < total_lines {
                line_starts[end_line] - 1
            } else {
                content.len()
            };
            return Some(MatchResult {
                range: start..end.min(content.len()),
                span: content[start..end.min(content.len())].to_string(),
            });
        }
    }
    None
}

fn whitespace_normalized_match(content: &str, needle: &str) -> Option<MatchResult> {
    let wanted = normalize_whitespace(needle);
    if wanted.is_empty() {
        return None;
    }
    for candidate in line_windows(content, needle.split('\n').count().max(1)) {
        if normalize_whitespace(candidate.text) == wanted {
            return Some(candidate.into_match());
        }
    }
    for candidate in line_windows(content, 1) {
        let normalized_line = normalize_whitespace(candidate.text);
        if !normalized_line.contains(&wanted) {
            continue;
        }
        return Some(candidate.into_match());
    }
    None
}

fn indentation_flexible_match(content: &str, needle: &str) -> Option<MatchResult> {
    let needle_lines = needle.split('\n').collect::<Vec<_>>();
    if needle_lines.is_empty() {
        return None;
    }
    let wanted = needle_lines
        .iter()
        .map(|line| line.trim_start())
        .collect::<Vec<_>>();
    for candidate in line_windows(content, wanted.len()) {
        let actual = candidate
            .text
            .split('\n')
            .map(|line| line.trim_start())
            .collect::<Vec<_>>();
        if actual == wanted {
            return Some(candidate.into_match());
        }
    }
    None
}

fn escape_normalized_match(content: &str, needle: &str) -> Option<MatchResult> {
    let normalized_needle = normalize_escapes(needle);
    if normalized_needle == needle {
        return None;
    }
    find(content, &normalized_needle)
}

fn trimmed_boundary_match(content: &str, needle: &str) -> Option<MatchResult> {
    let trimmed = needle.trim();
    if trimmed.is_empty() || trimmed == needle {
        return None;
    }
    find(content, trimmed)
}

fn block_anchor_match(content: &str, needle: &str) -> Option<MatchResult> {
    let needle_lines: Vec<&str> = needle.split('\n').collect();
    if needle_lines.len() < 3 {
        return None;
    }
    let first = needle_lines.first()?.trim();
    let last = needle_lines.last()?.trim();
    if first.is_empty() || last.is_empty() {
        return None;
    }
    let mut line_starts: Vec<usize> = vec![0];
    for (i, ch) in content.char_indices() {
        if ch == '\n' {
            line_starts.push(i + 1);
        }
    }
    let total_lines = line_starts.len();
    let line_text = |idx: usize| -> &str {
        let s = line_starts[idx];
        let e = if idx + 1 < total_lines {
            line_starts[idx + 1] - 1
        } else {
            content.len()
        };
        &content[s..e.min(content.len())]
    };
    for start_line in 0..total_lines {
        if line_text(start_line).trim() != first {
            continue;
        }
        let want_end = start_line + needle_lines.len() - 1;
        if want_end >= total_lines {
            continue;
        }
        if line_text(want_end).trim() != last {
            continue;
        }
        let start = line_starts[start_line];
        let end = if want_end + 1 < total_lines {
            line_starts[want_end + 1] - 1
        } else {
            content.len()
        };
        return Some(MatchResult {
            range: start..end.min(content.len()),
            span: content[start..end.min(content.len())].to_string(),
        });
    }
    None
}
