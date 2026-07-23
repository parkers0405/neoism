//! `:s` substitute family for the code pane (vim `:[range]s/pat/rep/[flags]`).
//!
//! Patterns are PLAIN TEXT (consistent with the pane's `/` search —
//! no regex engine yet); `\` escapes the separator inside pattern or
//! replacement. Supported ranges: none (current line), `%` (whole
//! file), `N`, `N,M`, `.`, `$`, `'<,'>` (current selection). Flags:
//! `g` (every occurrence on each line, not just the first) and `i`
//! (ASCII-case-insensitive match); unknown flags are ignored.

use super::types::CodeBuffer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubstituteRange {
    /// No range given: the cursor's line.
    CurrentLine,
    /// `%`: every line.
    WholeFile,
    /// 1-based inclusive line span (`N`, `N,M`, `.`, `$` resolved at
    /// apply time for `.`/`$` via the markers below).
    Lines(SubstituteLine, SubstituteLine),
    /// `'<,'>`: the active selection's line span (falls back to the
    /// cursor line without one).
    Selection,
}

/// One end of an explicit range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubstituteLine {
    /// 1-based absolute line number.
    Absolute(usize),
    /// `.` — the cursor line.
    Cursor,
    /// `$` — the last line.
    Last,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubstituteSpec {
    pub range: SubstituteRange,
    pub pattern: String,
    pub replacement: String,
    /// `g`: all occurrences per line instead of the first.
    pub global: bool,
    /// `i`: ASCII-case-insensitive matching.
    pub case_insensitive: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubstituteOutcome {
    pub substitutions: usize,
    pub lines_changed: usize,
    /// Position of the LAST substitution (line, byte col of the match
    /// start) — vim parks the cursor there.
    pub last: Option<(usize, usize)>,
}

/// Parse the ex line (already stripped of the leading `:`) as a
/// substitute command. Returns `None` when it isn't one (`:set`,
/// `:sp`, plain `:42`, …) so other ex heads keep working.
pub fn parse_substitute_command(input: &str) -> Option<SubstituteSpec> {
    let input = input.trim();
    let (range, rest) = parse_range(input);
    let rest = rest.strip_prefix('s')?;
    let mut chars = rest.chars();
    let sep = chars.next()?;
    if !matches!(sep, '/' | '#' | ',' | '|') {
        return None;
    }
    let body: &str = chars.as_str();

    let (pattern, after_pattern) = split_at_separator(body, sep);
    if pattern.is_empty() {
        return None;
    }
    let (replacement, after_replacement) = match after_pattern {
        Some(rest) => split_at_separator(rest, sep),
        None => (String::new(), None),
    };
    let flags = after_replacement.unwrap_or("");

    Some(SubstituteSpec {
        range,
        pattern,
        replacement,
        global: flags.contains('g'),
        case_insensitive: flags.contains('i'),
    })
}

/// Split a finder-style `pattern/replacement` query at its first
/// unescaped `/` (same escaping as `:s`). Returns the pattern and the
/// replacement (`None` while the user hasn't typed the `/` yet — the
/// Replace UI treats that as search-only preview).
pub fn split_replace_query(input: &str) -> (String, Option<String>) {
    let (pattern, rest) = split_at_separator(input, '/');
    (
        pattern,
        rest.map(|rest| split_at_separator(rest, '/').0),
    )
}

/// Split `input` at the first unescaped `sep`, unescaping `\sep` (and
/// `\\`) in the consumed chunk. Returns the chunk and the remainder
/// after the separator (`None` when the separator never appears —
/// vim's "trailing separator optional" rule).
fn split_at_separator(input: &str, sep: char) -> (String, Option<&str>) {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices();
    while let Some((ix, c)) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some((_, next)) if next == sep || next == '\\' => out.push(next),
                Some((_, next)) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            }
            continue;
        }
        if c == sep {
            return (out, Some(&input[ix + sep.len_utf8()..]));
        }
        out.push(c);
    }
    (out, None)
}

/// Strip a leading range from the ex line, returning the remainder
/// (which should start at `s`).
fn parse_range(input: &str) -> (SubstituteRange, &str) {
    if let Some(rest) = input.strip_prefix('%') {
        return (SubstituteRange::WholeFile, rest);
    }
    if let Some(rest) = input.strip_prefix("'<,'>") {
        return (SubstituteRange::Selection, rest);
    }
    if let Some((start, rest)) = parse_range_line(input) {
        if let Some(rest_after_comma) = rest.strip_prefix(',') {
            if let Some((end, rest)) = parse_range_line(rest_after_comma) {
                return (SubstituteRange::Lines(start, end), rest);
            }
            // `N,` with no end — not a range we accept; let the whole
            // parse fail on the missing `s`.
            return (SubstituteRange::CurrentLine, input);
        }
        return (SubstituteRange::Lines(start, start), rest);
    }
    (SubstituteRange::CurrentLine, input)
}

fn parse_range_line(input: &str) -> Option<(SubstituteLine, &str)> {
    if let Some(rest) = input.strip_prefix('.') {
        return Some((SubstituteLine::Cursor, rest));
    }
    if let Some(rest) = input.strip_prefix('$') {
        return Some((SubstituteLine::Last, rest));
    }
    let digits_end = input
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(ix, _)| ix)
        .unwrap_or(input.len());
    if digits_end == 0 {
        return None;
    }
    let value: usize = input[..digits_end].parse().ok()?;
    Some((SubstituteLine::Absolute(value.max(1)), &input[digits_end..]))
}

/// ASCII-case-insensitive plain-text find from `from` (byte index on a
/// char boundary). Returns the match's byte start.
fn find_ci(haystack: &str, needle: &str, from: usize, ci: bool) -> Option<usize> {
    if !ci {
        return haystack[from..].find(needle).map(|ix| from + ix);
    }
    if needle.is_empty() {
        return None;
    }
    let hay = haystack.as_bytes();
    let ned = needle.as_bytes();
    let mut ix = from;
    while ix + ned.len() <= hay.len() {
        if !haystack.is_char_boundary(ix) {
            ix += 1;
            continue;
        }
        if hay[ix..ix + ned.len()].eq_ignore_ascii_case(ned) {
            return Some(ix);
        }
        ix += 1;
    }
    None
}

/// Run the substitution on the buffer: one undo step, cursor parked on
/// the last substitution, selection cleared. Returns what happened so
/// the host can toast it.
pub fn apply_substitute(buffer: &mut CodeBuffer, spec: &SubstituteSpec) -> SubstituteOutcome {
    let last_line = buffer.line_count().saturating_sub(1);
    let resolve = |marker: SubstituteLine| -> usize {
        match marker {
            SubstituteLine::Absolute(n) => (n - 1).min(last_line),
            SubstituteLine::Cursor => buffer.cursor_line.min(last_line),
            SubstituteLine::Last => last_line,
        }
    };
    let (start, end) = match &spec.range {
        SubstituteRange::CurrentLine => {
            let line = buffer.cursor_line.min(last_line);
            (line, line)
        }
        SubstituteRange::WholeFile => (0, last_line),
        SubstituteRange::Lines(a, b) => {
            let (a, b) = (resolve(*a), resolve(*b));
            (a.min(b), a.max(b))
        }
        SubstituteRange::Selection => match buffer.visual_anchor {
            Some(anchor) => {
                let a = anchor.line.min(last_line);
                let b = buffer.cursor_line.min(last_line);
                (a.min(b), a.max(b))
            }
            None => {
                let line = buffer.cursor_line.min(last_line);
                (line, line)
            }
        },
    };

    let mut outcome = SubstituteOutcome::default();
    if spec.pattern.is_empty() {
        return outcome;
    }
    // Scan first: no match in range → no undo entry, no edit, clean
    // "Pattern not found" report.
    let any_match = (start..=end).any(|line_ix| {
        find_ci(&buffer.lines[line_ix], &spec.pattern, 0, spec.case_insensitive)
            .is_some()
    });
    if !any_match {
        return outcome;
    }

    buffer.break_undo_group();
    buffer.save_undo();
    for line_ix in start..=end {
        let mut from = 0usize;
        let mut replaced_here = false;
        loop {
            let Some(found) =
                find_ci(&buffer.lines[line_ix], &spec.pattern, from, spec.case_insensitive)
            else {
                break;
            };
            let match_len = spec.pattern.len();
            buffer.lines[line_ix]
                .replace_range(found..found + match_len, &spec.replacement);
            outcome.substitutions += 1;
            outcome.last = Some((line_ix, found));
            replaced_here = true;
            from = found + spec.replacement.len();
            // Empty replacement of an empty-progress guard: always
            // advance at least one byte so `g` can't loop forever.
            if spec.replacement.is_empty() && match_len == 0 {
                from += 1;
            }
            if !spec.global {
                break;
            }
        }
        if replaced_here {
            outcome.lines_changed += 1;
        }
    }

    if let Some((line, col)) = outcome.last {
        buffer.cursor_line = line;
        buffer.cursor_col = col;
    }
    buffer.visual_anchor = None;
    buffer.extra_carets.clear();
    buffer.clamp_cursor();
    buffer.follow_cursor = true;
    buffer.mark_edited();
    buffer.commit_undo();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(lines: &[&str]) -> CodeBuffer {
        CodeBuffer::from_text(&lines.join("\n"))
    }

    #[test]
    fn parses_whole_file_substitute() {
        let spec = parse_substitute_command("%s/foo/bar/g").unwrap();
        assert_eq!(spec.range, SubstituteRange::WholeFile);
        assert_eq!(spec.pattern, "foo");
        assert_eq!(spec.replacement, "bar");
        assert!(spec.global);
        assert!(!spec.case_insensitive);
    }

    #[test]
    fn parses_bare_line_and_ranges() {
        assert_eq!(
            parse_substitute_command("s/a/b").unwrap().range,
            SubstituteRange::CurrentLine
        );
        assert_eq!(
            parse_substitute_command("3,7s/a/b/").unwrap().range,
            SubstituteRange::Lines(
                SubstituteLine::Absolute(3),
                SubstituteLine::Absolute(7)
            )
        );
        assert_eq!(
            parse_substitute_command(".,$s/a/b").unwrap().range,
            SubstituteRange::Lines(SubstituteLine::Cursor, SubstituteLine::Last)
        );
        assert_eq!(
            parse_substitute_command("'<,'>s/a/b").unwrap().range,
            SubstituteRange::Selection
        );
    }

    #[test]
    fn rejects_non_substitute_heads() {
        assert!(parse_substitute_command("set number").is_none());
        assert!(parse_substitute_command("sp").is_none());
        assert!(parse_substitute_command("42").is_none());
        assert!(parse_substitute_command("w").is_none());
    }

    #[test]
    fn pattern_may_contain_spaces_and_escaped_separators() {
        let spec = parse_substitute_command("%s/foo bar/baz qux/").unwrap();
        assert_eq!(spec.pattern, "foo bar");
        assert_eq!(spec.replacement, "baz qux");
        let spec = parse_substitute_command("s/a\\/b/c/").unwrap();
        assert_eq!(spec.pattern, "a/b");
        // Trailing separator + replacement both optional.
        let spec = parse_substitute_command("s/gone").unwrap();
        assert_eq!(spec.pattern, "gone");
        assert_eq!(spec.replacement, "");
    }

    #[test]
    fn substitutes_first_per_line_without_g() {
        let mut buffer = buffer(&["foo foo", "foo"]);
        let spec = parse_substitute_command("%s/foo/X/").unwrap();
        let outcome = apply_substitute(&mut buffer, &spec);
        assert_eq!(buffer.lines, vec!["X foo".to_string(), "X".to_string()]);
        assert_eq!(outcome.substitutions, 2);
        assert_eq!(outcome.lines_changed, 2);
        assert_eq!(outcome.last, Some((1, 0)));
    }

    #[test]
    fn substitutes_all_with_g_and_ci_with_i() {
        let mut buffer = buffer(&["Foo foo FOO"]);
        let spec = parse_substitute_command("s/foo/x/gi").unwrap();
        let outcome = apply_substitute(&mut buffer, &spec);
        assert_eq!(buffer.lines, vec!["x x x".to_string()]);
        assert_eq!(outcome.substitutions, 3);
    }

    #[test]
    fn replacement_containing_pattern_does_not_loop() {
        let mut buffer = buffer(&["ab"]);
        let spec = parse_substitute_command("s/ab/abab/g").unwrap();
        let outcome = apply_substitute(&mut buffer, &spec);
        assert_eq!(buffer.lines, vec!["abab".to_string()]);
        assert_eq!(outcome.substitutions, 1);
    }

    #[test]
    fn range_substitute_touches_only_the_span() {
        let mut buffer = buffer(&["a", "a", "a", "a"]);
        let spec = parse_substitute_command("2,3s/a/b/").unwrap();
        apply_substitute(&mut buffer, &spec);
        assert_eq!(
            buffer.lines,
            vec![
                "a".to_string(),
                "b".to_string(),
                "b".to_string(),
                "a".to_string()
            ]
        );
    }

    #[test]
    fn no_match_reports_zero_and_leaves_buffer_alone() {
        let mut buffer = buffer(&["hello"]);
        let before = buffer.lines.clone();
        let spec = parse_substitute_command("%s/xyz/q/").unwrap();
        let outcome = apply_substitute(&mut buffer, &spec);
        assert_eq!(outcome.substitutions, 0);
        assert_eq!(buffer.lines, before);
    }

    #[test]
    fn undo_reverts_the_whole_substitute_as_one_step() {
        let mut buffer = buffer(&["foo", "foo"]);
        let spec = parse_substitute_command("%s/foo/bar/").unwrap();
        apply_substitute(&mut buffer, &spec);
        assert_eq!(buffer.lines, vec!["bar".to_string(), "bar".to_string()]);
        assert!(buffer.undo());
        assert_eq!(buffer.lines, vec!["foo".to_string(), "foo".to_string()]);
    }
}
