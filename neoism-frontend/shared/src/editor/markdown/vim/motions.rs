use super::*;

pub(crate) fn operator_for_char(ch: char) -> Option<VimOperator> {
    Some(match ch {
        'd' => VimOperator::Delete,
        'c' => VimOperator::Change,
        'y' => VimOperator::Yank,
        '>' => VimOperator::Indent,
        '<' => VimOperator::Outdent,
        _ => return None,
    })
}

pub(crate) fn motion_for_char(ch: char) -> Option<VimMotion> {
    Some(match ch {
        'h' => VimMotion::Left,
        'l' => VimMotion::Right,
        'k' => VimMotion::Up,
        'j' => VimMotion::Down,
        '0' => VimMotion::LineStart,
        '$' => VimMotion::LineEnd,
        '^' | '_' => VimMotion::FirstNonBlank,
        '+' => VimMotion::LinesDownFirstNonBlank,
        '-' => VimMotion::LinesUpFirstNonBlank,
        'w' => VimMotion::WordForward { big: false },
        'W' => VimMotion::WordForward { big: true },
        'b' => VimMotion::WordBack { big: false },
        'B' => VimMotion::WordBack { big: true },
        'e' => VimMotion::WordEnd { big: false },
        'E' => VimMotion::WordEnd { big: true },
        '{' => VimMotion::ParagraphBack,
        '}' => VimMotion::ParagraphForward,
        '%' => VimMotion::MatchPair,
        ';' => VimMotion::RepeatFind { reverse: false },
        ',' => VimMotion::RepeatFind { reverse: true },
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Pure motion / text-object primitives over the line vector.
// ---------------------------------------------------------------------------

/// 0 = whitespace, 1 = word chars (`[[:alnum:]_]`), 2 = punctuation.
/// With `big` (WORD motions) everything non-whitespace is one class.
pub(crate) fn vim_char_class(ch: char, big: bool) -> u8 {
    if ch.is_whitespace() {
        0
    } else if big || ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

pub(crate) fn char_at(line: &str, col: usize) -> Option<char> {
    line.get(col..).and_then(|rest| rest.chars().next())
}

/// The char at a document position, where the end-of-line column reads
/// as a `'\n'` separator (except past the final line).
pub(crate) fn char_at_pos(lines: &[String], pos: MarkdownPosition) -> Option<char> {
    let line = lines.get(pos.line)?;
    if pos.col < line.len() {
        char_at(line, pos.col)
    } else if pos.line + 1 < lines.len() {
        Some('\n')
    } else {
        None
    }
}

pub(crate) fn next_pos(
    lines: &[String],
    pos: MarkdownPosition,
) -> Option<MarkdownPosition> {
    let line = lines.get(pos.line)?;
    if pos.col < line.len() {
        Some(MarkdownPosition {
            line: pos.line,
            col: next_char_boundary(line, pos.col),
        })
    } else if pos.line + 1 < lines.len() {
        Some(MarkdownPosition {
            line: pos.line + 1,
            col: 0,
        })
    } else {
        None
    }
}

pub(crate) fn prev_pos(
    lines: &[String],
    pos: MarkdownPosition,
) -> Option<MarkdownPosition> {
    if pos.col > 0 {
        let line = lines.get(pos.line)?;
        Some(MarkdownPosition {
            line: pos.line,
            col: prev_char_boundary(line, pos.col.min(line.len())),
        })
    } else if pos.line > 0 {
        Some(MarkdownPosition {
            line: pos.line - 1,
            col: lines.get(pos.line - 1).map(String::len).unwrap_or(0),
        })
    } else {
        None
    }
}

pub(crate) fn class_at_pos(lines: &[String], pos: MarkdownPosition, big: bool) -> u8 {
    char_at_pos(lines, pos)
        .map(|ch| vim_char_class(ch, big))
        .unwrap_or(0)
}

pub(crate) fn vim_word_forward(
    lines: &[String],
    pos: MarkdownPosition,
    big: bool,
) -> MarkdownPosition {
    let mut cur = pos;
    let class = class_at_pos(lines, cur, big);
    if class != 0 {
        while class_at_pos(lines, cur, big) == class {
            match next_pos(lines, cur) {
                Some(next) => cur = next,
                None => return cur,
            }
        }
    }
    while let Some(ch) = char_at_pos(lines, cur) {
        if vim_char_class(ch, big) != 0 {
            return cur;
        }
        match next_pos(lines, cur) {
            Some(next) => cur = next,
            None => return cur,
        }
        // An empty line is a word of its own.
        if cur.col == 0 && lines.get(cur.line).is_some_and(String::is_empty) {
            return cur;
        }
    }
    cur
}

pub(crate) fn vim_word_back(
    lines: &[String],
    pos: MarkdownPosition,
    big: bool,
) -> MarkdownPosition {
    let mut cur = pos;
    loop {
        let Some(prev) = prev_pos(lines, cur) else {
            return cur;
        };
        cur = prev;
        if cur.col == 0 && lines.get(cur.line).is_some_and(String::is_empty) {
            return cur;
        }
        let class = class_at_pos(lines, cur, big);
        if class == 0 {
            continue;
        }
        loop {
            let Some(prev) = prev_pos(lines, cur) else {
                return cur;
            };
            if prev.line == cur.line && class_at_pos(lines, prev, big) == class {
                cur = prev;
            } else {
                return cur;
            }
        }
    }
}

pub(crate) fn vim_word_end(
    lines: &[String],
    pos: MarkdownPosition,
    big: bool,
) -> MarkdownPosition {
    let Some(mut cur) = next_pos(lines, pos) else {
        return pos;
    };
    while let Some(ch) = char_at_pos(lines, cur) {
        if vim_char_class(ch, big) != 0 {
            break;
        }
        match next_pos(lines, cur) {
            Some(next) => cur = next,
            None => return cur,
        }
    }
    let class = class_at_pos(lines, cur, big);
    if class == 0 {
        return cur;
    }
    loop {
        let Some(next) = next_pos(lines, cur) else {
            return cur;
        };
        if next.line == cur.line && class_at_pos(lines, next, big) == class {
            cur = next;
        } else {
            return cur;
        }
    }
}

pub(crate) fn vim_word_end_back(
    lines: &[String],
    pos: MarkdownPosition,
    big: bool,
) -> MarkdownPosition {
    let start_class = class_at_pos(lines, pos, big);
    let Some(mut cur) = prev_pos(lines, pos) else {
        return pos;
    };
    if start_class != 0
        && cur.line == pos.line
        && class_at_pos(lines, cur, big) == start_class
    {
        // Still inside the word the cursor started on: back over the
        // rest of the run, then continue to the previous word's end.
        loop {
            let Some(prev) = prev_pos(lines, cur) else {
                return cur;
            };
            if prev.line == cur.line && class_at_pos(lines, prev, big) == start_class {
                cur = prev;
            } else {
                break;
            }
        }
        let Some(prev) = prev_pos(lines, cur) else {
            return cur;
        };
        cur = prev;
    }
    loop {
        if class_at_pos(lines, cur, big) != 0 {
            return cur;
        }
        let Some(prev) = prev_pos(lines, cur) else {
            return cur;
        };
        cur = prev;
    }
}

pub(crate) fn vim_first_non_blank(line: &str) -> usize {
    line.char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(ix, _)| ix)
        .unwrap_or(0)
}

pub(crate) fn vim_line_is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

pub(crate) fn vim_paragraph_forward(lines: &[String], line: usize) -> usize {
    let mut ix = line;
    while ix < lines.len() && vim_line_is_blank(&lines[ix]) {
        ix += 1;
    }
    while ix < lines.len() && !vim_line_is_blank(&lines[ix]) {
        ix += 1;
    }
    ix.min(lines.len().saturating_sub(1))
}

pub(crate) fn vim_paragraph_back(lines: &[String], line: usize) -> usize {
    let mut ix = line.min(lines.len().saturating_sub(1));
    while ix > 0 && vim_line_is_blank(&lines[ix]) {
        ix -= 1;
    }
    while ix > 0 && !vim_line_is_blank(&lines[ix]) {
        ix -= 1;
    }
    ix
}

/// `f`/`F`/`t`/`T` within one line. Returns the target column. With
/// `skip_adjacent` (used by `;`/`,` on till-kinds) the scan starts one
/// char further so the repeat doesn't get stuck on the adjacent match.
pub(crate) fn vim_find_col(
    line: &str,
    col: usize,
    kind: VimFindKind,
    target: char,
    count: usize,
    skip_adjacent: bool,
) -> Option<usize> {
    let count = count.max(1);
    let col = floor_char_boundary(line, col.min(line.len()));
    match kind {
        VimFindKind::To | VimFindKind::Till => {
            if col >= line.len() {
                return None;
            }
            let mut from = next_char_boundary(line, col);
            if skip_adjacent
                && matches!(kind, VimFindKind::Till)
                && from < line.len()
                && char_at(line, from) == Some(target)
            {
                from = next_char_boundary(line, from);
            }
            let mut found = None;
            let mut remaining = count;
            let mut ix = from;
            while ix < line.len() {
                if char_at(line, ix) == Some(target) {
                    remaining -= 1;
                    if remaining == 0 {
                        found = Some(ix);
                        break;
                    }
                }
                ix = next_char_boundary(line, ix);
            }
            let hit = found?;
            match kind {
                VimFindKind::To => Some(hit),
                _ => {
                    let before = prev_char_boundary(line, hit);
                    (before > col).then_some(before)
                }
            }
        }
        VimFindKind::ToBack | VimFindKind::TillBack => {
            let mut until = col;
            if skip_adjacent && matches!(kind, VimFindKind::TillBack) && until > 0 {
                let before = prev_char_boundary(line, until);
                if char_at(line, before) == Some(target) {
                    until = before;
                }
            }
            let mut hits = Vec::new();
            let mut ix = 0;
            while ix < until {
                if char_at(line, ix) == Some(target) {
                    hits.push(ix);
                }
                ix = next_char_boundary(line, ix);
            }
            if hits.len() < count {
                return None;
            }
            let hit = hits[hits.len() - count];
            match kind {
                VimFindKind::ToBack => Some(hit),
                _ => {
                    let after = next_char_boundary(line, hit);
                    (after < col).then_some(after)
                }
            }
        }
    }
}

pub(crate) fn vim_matching_bracket(
    lines: &[String],
    pos: MarkdownPosition,
) -> Option<(MarkdownPosition, MarkdownPosition)> {
    const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
    let line = lines.get(pos.line)?;
    let col = floor_char_boundary(line, pos.col.min(line.len()));
    let (start_col, bracket) = line
        .get(col..)?
        .char_indices()
        .map(|(ix, ch)| (col + ix, ch))
        .find(|(_, ch)| PAIRS.iter().any(|(o, c)| ch == o || ch == c))?;
    let start = MarkdownPosition {
        line: pos.line,
        col: start_col,
    };
    let (open, close, forward) = PAIRS
        .iter()
        .find_map(|&(o, c)| {
            if bracket == o {
                Some((o, c, true))
            } else if bracket == c {
                Some((o, c, false))
            } else {
                None
            }
        })
        .unwrap_or(('(', ')', true));
    let mut depth = 1usize;
    let mut cur = start;
    loop {
        let step = if forward {
            next_pos(lines, cur)
        } else {
            prev_pos(lines, cur)
        };
        cur = step?;
        match char_at_pos(lines, cur) {
            Some(ch) if ch == open => {
                if forward {
                    depth += 1;
                } else {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start, cur));
                    }
                }
            }
            Some(ch) if ch == close => {
                if forward {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start, cur));
                    }
                } else {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

/// A resolved operator range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VimOpRange {
    /// Charwise; `end` exclusive.
    Chars {
        start: MarkdownPosition,
        end: MarkdownPosition,
    },
    /// Linewise; both inclusive.
    Lines { first: usize, last: usize },
}

pub(crate) fn vim_word_object(
    lines: &[String],
    pos: MarkdownPosition,
    big: bool,
    around: bool,
) -> Option<VimOpRange> {
    let line = lines.get(pos.line)?;
    if line.is_empty() {
        return None;
    }
    let mut col = floor_char_boundary(line, pos.col.min(line.len()));
    if col >= line.len() {
        col = prev_char_boundary(line, line.len());
    }
    let class = vim_char_class(char_at(line, col)?, big);
    let mut start = col;
    while start > 0 {
        let prev = prev_char_boundary(line, start);
        if char_at(line, prev).map(|ch| vim_char_class(ch, big)) == Some(class) {
            start = prev;
        } else {
            break;
        }
    }
    let mut end = next_char_boundary(line, col);
    while end < line.len() {
        if char_at(line, end).map(|ch| vim_char_class(ch, big)) == Some(class) {
            end = next_char_boundary(line, end);
        } else {
            break;
        }
    }
    if around {
        let mut extended = false;
        while end < line.len() && char_at(line, end).is_some_and(|ch| ch.is_whitespace())
        {
            end = next_char_boundary(line, end);
            extended = true;
        }
        if class == 0 && !extended {
            // `aw` starting on whitespace swallows the following word.
            let word_class = char_at(line, end).map(|ch| vim_char_class(ch, big));
            if let Some(word_class) = word_class.filter(|class| *class != 0) {
                while end < line.len()
                    && char_at(line, end).map(|ch| vim_char_class(ch, big))
                        == Some(word_class)
                {
                    end = next_char_boundary(line, end);
                }
                extended = true;
            }
        }
        if !extended {
            while start > 0 {
                let prev = prev_char_boundary(line, start);
                if char_at(line, prev).is_some_and(|ch| ch.is_whitespace()) {
                    start = prev;
                } else {
                    break;
                }
            }
        }
    }
    (start < end).then_some(VimOpRange::Chars {
        start: MarkdownPosition {
            line: pos.line,
            col: start,
        },
        end: MarkdownPosition {
            line: pos.line,
            col: end,
        },
    })
}

pub(crate) fn vim_quote_object(
    lines: &[String],
    pos: MarkdownPosition,
    quote: char,
    around: bool,
) -> Option<VimOpRange> {
    let line = lines.get(pos.line)?;
    let col = floor_char_boundary(line, pos.col.min(line.len()));
    let mut quotes = Vec::new();
    let mut ix = 0;
    while ix < line.len() {
        if char_at(line, ix) == Some(quote) {
            quotes.push(ix);
        }
        ix = next_char_boundary(line, ix);
    }
    let pair = quotes
        .chunks_exact(2)
        .find(|pair| col <= pair[1])
        .map(|pair| (pair[0], pair[1]))?;
    let (open, close) = pair;
    let quote_len = quote.len_utf8();
    if around {
        let mut start = open;
        let mut end = close + quote_len;
        let mut extended = false;
        while end < line.len() && char_at(line, end).is_some_and(|ch| ch.is_whitespace())
        {
            end = next_char_boundary(line, end);
            extended = true;
        }
        if !extended {
            while start > 0 {
                let prev = prev_char_boundary(line, start);
                if char_at(line, prev).is_some_and(|ch| ch.is_whitespace()) {
                    start = prev;
                } else {
                    break;
                }
            }
        }
        Some(VimOpRange::Chars {
            start: MarkdownPosition {
                line: pos.line,
                col: start,
            },
            end: MarkdownPosition {
                line: pos.line,
                col: end,
            },
        })
    } else {
        Some(VimOpRange::Chars {
            start: MarkdownPosition {
                line: pos.line,
                col: open + quote_len,
            },
            end: MarkdownPosition {
                line: pos.line,
                col: close,
            },
        })
    }
}

pub(crate) fn vim_pair_object(
    lines: &[String],
    pos: MarkdownPosition,
    open: char,
    close: char,
    around: bool,
) -> Option<VimOpRange> {
    let at_cursor = char_at_pos(lines, pos);
    let open_pos = if at_cursor == Some(open) {
        pos
    } else {
        let mut depth = 1usize;
        let mut cur = pos;
        loop {
            cur = prev_pos(lines, cur)?;
            match char_at_pos(lines, cur) {
                Some(ch) if ch == close => depth += 1,
                Some(ch) if ch == open => {
                    depth -= 1;
                    if depth == 0 {
                        break cur;
                    }
                }
                _ => {}
            }
        }
    };
    let close_pos = if at_cursor == Some(close) {
        pos
    } else {
        let mut depth = 1usize;
        let mut cur = open_pos;
        loop {
            cur = next_pos(lines, cur)?;
            match char_at_pos(lines, cur) {
                Some(ch) if ch == open => depth += 1,
                Some(ch) if ch == close => {
                    depth -= 1;
                    if depth == 0 {
                        break cur;
                    }
                }
                _ => {}
            }
        }
    };
    if around {
        Some(VimOpRange::Chars {
            start: open_pos,
            end: next_pos(lines, close_pos).unwrap_or(MarkdownPosition {
                line: close_pos.line,
                col: lines
                    .get(close_pos.line)
                    .map(String::len)
                    .unwrap_or(close_pos.col),
            }),
        })
    } else {
        let start = next_pos(lines, open_pos)?;
        (start <= close_pos).then_some(VimOpRange::Chars {
            start,
            end: close_pos,
        })
    }
}

pub(crate) fn vim_paragraph_object(
    lines: &[String],
    line: usize,
    around: bool,
) -> Option<VimOpRange> {
    let line = line.min(lines.len().saturating_sub(1));
    let on_blank = vim_line_is_blank(lines.get(line)?);
    let mut first = line;
    let mut last = line;
    while first > 0 && vim_line_is_blank(&lines[first - 1]) == on_blank {
        first -= 1;
    }
    while last + 1 < lines.len() && vim_line_is_blank(&lines[last + 1]) == on_blank {
        last += 1;
    }
    if around {
        if on_blank {
            // Blank run plus the paragraph that follows it.
            while last + 1 < lines.len() && !vim_line_is_blank(&lines[last + 1]) {
                last += 1;
            }
        } else {
            // Paragraph plus the blank run after it (or before it when
            // nothing follows).
            let mut extended = false;
            while last + 1 < lines.len() && vim_line_is_blank(&lines[last + 1]) {
                last += 1;
                extended = true;
            }
            if !extended {
                while first > 0 && vim_line_is_blank(&lines[first - 1]) {
                    first -= 1;
                }
            }
        }
    }
    Some(VimOpRange::Lines { first, last })
}

/// Forward document search for a plain pattern, starting strictly after
/// `from`, wrapping around. `whole_word` requires word-class boundaries.
pub(crate) fn vim_search_forward(
    lines: &[String],
    from: MarkdownPosition,
    pattern: &str,
    whole_word: bool,
) -> Option<MarkdownPosition> {
    if pattern.is_empty() || lines.is_empty() {
        return None;
    }
    let start_line = from.line.min(lines.len() - 1);
    let line_count = lines.len();
    for step in 0..=line_count {
        let line_ix = (start_line + step) % line_count;
        let line = &lines[line_ix];
        let search_from = if step == 0 {
            let col = from.col.min(line.len());
            if col >= line.len() {
                continue;
            }
            next_char_boundary(line, floor_char_boundary(line, col))
        } else {
            0
        };
        let limit = if step == line_count {
            from.col.min(line.len())
        } else {
            line.len()
        };
        if let Some(col) =
            find_in_line(line, search_from, limit, pattern, whole_word, false)
        {
            return Some(MarkdownPosition { line: line_ix, col });
        }
    }
    None
}

pub(crate) fn vim_search_backward(
    lines: &[String],
    from: MarkdownPosition,
    pattern: &str,
    whole_word: bool,
) -> Option<MarkdownPosition> {
    if pattern.is_empty() || lines.is_empty() {
        return None;
    }
    let line_count = lines.len();
    let start_line = from.line.min(line_count - 1);
    for step in 0..=line_count {
        let line_ix = (start_line + line_count - (step % line_count)) % line_count;
        let line = &lines[line_ix];
        let limit = if step == 0 {
            from.col.min(line.len())
        } else {
            line.len()
        };
        let search_from = if step == line_count {
            from.col.min(line.len())
        } else {
            0
        };
        if let Some(col) =
            find_in_line(line, search_from, limit, pattern, whole_word, true)
        {
            return Some(MarkdownPosition { line: line_ix, col });
        }
    }
    None
}

pub(crate) fn find_in_line(
    line: &str,
    from: usize,
    limit: usize,
    pattern: &str,
    whole_word: bool,
    last: bool,
) -> Option<usize> {
    let from = floor_char_boundary(line, from.min(line.len()));
    let limit = floor_char_boundary(line, limit.min(line.len()));
    if from > limit {
        return None;
    }
    let mut found = None;
    for (offset, _) in line[from..].match_indices(pattern) {
        let start = from + offset;
        if start >= limit {
            break;
        }
        if whole_word && !is_whole_word_match(line, start, pattern.len()) {
            continue;
        }
        if last {
            found = Some(start);
        } else {
            return Some(start);
        }
    }
    found
}

pub(crate) fn is_whole_word_match(line: &str, start: usize, len: usize) -> bool {
    let before_ok = start == 0
        || char_at(line, prev_char_boundary(line, start))
            .map(|ch| vim_char_class(ch, false) != 1)
            .unwrap_or(true);
    let end = start + len;
    let after_ok = end >= line.len()
        || char_at(line, end)
            .map(|ch| vim_char_class(ch, false) != 1)
            .unwrap_or(true);
    before_ok && after_ok
}

/// The word-class run at (or, vim-style, after) the cursor on its line.
pub(crate) fn vim_word_under_cursor(line: &str, col: usize) -> Option<(usize, usize)> {
    let mut col = floor_char_boundary(line, col.min(line.len()));
    if col >= line.len() {
        if line.is_empty() {
            return None;
        }
        col = prev_char_boundary(line, line.len());
    }
    if char_at(line, col).map(|ch| vim_char_class(ch, false)) != Some(1) {
        let mut ix = col;
        loop {
            if ix >= line.len() {
                return None;
            }
            if char_at(line, ix).map(|ch| vim_char_class(ch, false)) == Some(1) {
                col = ix;
                break;
            }
            ix = next_char_boundary(line, ix);
        }
    }
    let mut start = col;
    while start > 0 {
        let prev = prev_char_boundary(line, start);
        if char_at(line, prev).map(|ch| vim_char_class(ch, false)) == Some(1) {
            start = prev;
        } else {
            break;
        }
    }
    let mut end = next_char_boundary(line, col);
    while end < line.len() {
        if char_at(line, end).map(|ch| vim_char_class(ch, false)) == Some(1) {
            end = next_char_boundary(line, end);
        } else {
            break;
        }
    }
    Some((start, end))
}
