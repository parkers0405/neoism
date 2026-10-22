//! Cooperative review projection. No document-sized work is done by `new`.
//!
//! Each work unit scans or compares at most 256 input bytes (per side), copies
//! at most 256 output bytes, or visits one Myers frontier/backtrack cell or
//! metadata row. Indexing can record at most 256 line boundaries in one unit.
//! Fixed-size headers/markers and amortized collection growth are the only
//! overhead outside those units. In particular, a single enormous line cannot
//! hide an unbudgeted string comparison or copy inside a poll.

use super::LineOp;
use crate::widgets::diff_card::{DiffLine, DiffLineKind as K};
use std::ops::Range;

const BYTE_CHUNK: usize = 256;

#[derive(Clone, Copy)]
struct Row {
    op: LineOp,
    old: usize,
    new: usize,
    synthetic: bool,
}

impl Row {
    fn has_old(self) -> bool {
        !self.synthetic && self.op != LineOp::Insert
    }
    fn has_new(self) -> bool {
        self.op != LineOp::Delete
    }
}

enum Phase {
    Index {
        side: usize,
        start: usize,
        byte: usize,
    },
    Prefix(usize),
    Suffix(usize),
    Snapshot(usize),
    Diagonal,
    Snake(usize),
    Backtrack {
        d: usize,
        x: usize,
        y: usize,
    },
    BackSnake {
        d: usize,
        x: usize,
        y: usize,
        px: usize,
        py: usize,
    },
    Leading {
        x: usize,
        y: usize,
    },
    PrefixRows(usize),
    Normalize,
    Flush {
        deletes: usize,
        inserts: usize,
        keep: bool,
    },
    Fallback {
        deletes: usize,
        inserts: usize,
    },
    SuffixRows(usize),
    Caret,
    Excerpt {
        range: usize,
        row: usize,
        header: bool,
    },
    Copy {
        range: usize,
        row: usize,
        byte: usize,
        end: usize,
        text: String,
    },
    Done,
}

/// Owned, reusable on native workers and wasm event-loop continuations.
/// `poll(0)` never advances. Completion is returned exactly once; subsequent
/// polls return `None`. Dropping the builder cancels an unfinished job.
pub struct ReviewDiffBuilder {
    source: [String; 2],
    lines: [Vec<Range<usize>>; 2],
    context: usize,
    ensure: Option<usize>,
    phase: Phase,
    prefix: usize,
    suffix: usize,
    n: usize,
    m: usize,
    max: usize,
    d: usize,
    k: isize,
    x: usize,
    y: usize,
    v: Vec<usize>,
    trace: Vec<Vec<usize>>,
    reverse: Vec<LineOp>,
    fallback: bool,
    pending_deletes: usize,
    pending_inserts: usize,
    old: usize,
    new: usize,
    rows: Vec<Row>,
    ranges: Vec<Range<usize>>,
    result: Vec<DiffLine>,
    delivered: bool,
}

impl ReviewDiffBuilder {
    pub fn new(
        base: String,
        current: String,
        context: usize,
        ensure: Option<usize>,
    ) -> Self {
        Self {
            source: [base, current],
            lines: [Vec::new(), Vec::new()],
            context,
            ensure,
            phase: Phase::Index {
                side: 0,
                start: 0,
                byte: 0,
            },
            prefix: 0,
            suffix: 0,
            n: 0,
            m: 0,
            max: 0,
            d: 0,
            k: 0,
            x: 0,
            y: 0,
            v: Vec::new(),
            trace: Vec::new(),
            reverse: Vec::new(),
            fallback: false,
            pending_deletes: 0,
            pending_inserts: 0,
            old: 0,
            new: 0,
            rows: Vec::new(),
            ranges: Vec::new(),
            result: Vec::new(),
            delivered: false,
        }
    }

    /// Advance at most `budget` bounded work units. Byte-oriented units process
    /// up to 256 bytes; Myers/row units process one cell/row. This is an
    /// algorithmic bound, not a wall-clock guarantee around allocator growth.
    pub fn poll(&mut self, budget: usize) -> Option<Vec<DiffLine>> {
        for _ in 0..budget {
            if matches!(self.phase, Phase::Done) {
                break;
            }
            let phase = std::mem::replace(&mut self.phase, Phase::Done);
            self.phase = self.step(phase);
        }
        if matches!(self.phase, Phase::Done) && !self.delivered {
            self.delivered = true;
            Some(std::mem::take(&mut self.result))
        } else {
            None
        }
    }

    // None means more bytes remain; exact comparison, not hash equality.
    fn equal(&self, old: usize, new: usize, byte: usize) -> Option<bool> {
        let a = &self.lines[0][old];
        let b = &self.lines[1][new];
        if a.len() != b.len() {
            return Some(false);
        }
        if byte == a.len() {
            return Some(true);
        }
        let end = byte.saturating_add(BYTE_CHUNK).min(a.len());
        if self.source[0].as_bytes()[a.start + byte..a.start + end]
            != self.source[1].as_bytes()[b.start + byte..b.start + end]
        {
            Some(false)
        } else if end == a.len() {
            Some(true)
        } else {
            None
        }
    }

    fn begin_search(&mut self) -> Phase {
        self.n = self.lines[0].len() - self.prefix - self.suffix;
        self.m = self.lines[1].len() - self.prefix - self.suffix;
        if self.n == 0 && self.m == 0 && self.ensure.is_none() {
            return Phase::Done;
        }
        if self.n == 0 || self.m == 0 {
            self.fallback = true;
            return Phase::PrefixRows(0);
        }
        self.max = (self.n + self.m).min(256);
        // Initialize the (at most 513-cell) frontier cooperatively as well.
        Phase::Snapshot(0)
    }

    fn emit(&mut self, op: LineOp, synthetic: bool) {
        let row = Row {
            op,
            old: self.old,
            new: self.new,
            synthetic,
        };
        let i = self.rows.len();
        if op != LineOp::Keep
            || self
                .ensure
                .is_some_and(|line| row.has_new() && line == row.new)
        {
            let start = i.saturating_sub(self.context);
            let end = i.saturating_add(self.context).saturating_add(1);
            if let Some(last) = self.ranges.last_mut().filter(|last| start <= last.end) {
                last.end = last.end.max(end);
            } else {
                self.ranges.push(start..end);
            }
        }
        self.old += usize::from(row.has_old());
        self.new += usize::from(row.has_new());
        self.rows.push(row);
    }

    fn plain(text: String) -> DiffLine {
        DiffLine {
            text,
            kind: K::Hunk,
            line_number: None,
            old_line_number: None,
            new_line_number: None,
        }
    }

    fn step(&mut self, phase: Phase) -> Phase {
        match phase {
            Phase::Index {
                side,
                mut start,
                mut byte,
            } => {
                let source = &self.source[side];
                if byte < source.len() {
                    let end = byte.saturating_add(BYTE_CHUNK).min(source.len());
                    for (offset, &value) in
                        source.as_bytes()[byte..end].iter().enumerate()
                    {
                        if value == b'\n' {
                            let line_end = byte + offset + 1;
                            self.lines[side].push(start..line_end);
                            start = line_end;
                        }
                    }
                    byte = end;
                    Phase::Index { side, start, byte }
                } else {
                    if start < byte {
                        self.lines[side].push(start..byte);
                    }
                    if side == 0 {
                        Phase::Index {
                            side: 1,
                            start: 0,
                            byte: 0,
                        }
                    } else {
                        Phase::Prefix(0)
                    }
                }
            }
            Phase::Prefix(byte) => {
                if self.prefix == self.lines[0].len().min(self.lines[1].len()) {
                    return Phase::Suffix(0);
                }
                match self.equal(self.prefix, self.prefix, byte) {
                    None => Phase::Prefix(byte + BYTE_CHUNK),
                    Some(true) => {
                        self.prefix += 1;
                        Phase::Prefix(0)
                    }
                    Some(false) => Phase::Suffix(0),
                }
            }
            Phase::Suffix(byte) => {
                if self.suffix
                    == self.lines[0].len().min(self.lines[1].len()) - self.prefix
                {
                    return self.begin_search();
                }
                let a = self.lines[0].len() - self.suffix - 1;
                let b = self.lines[1].len() - self.suffix - 1;
                match self.equal(a, b, byte) {
                    None => Phase::Suffix(byte + BYTE_CHUNK),
                    Some(true) => {
                        self.suffix += 1;
                        Phase::Suffix(0)
                    }
                    Some(false) => self.begin_search(),
                }
            }
            Phase::Snapshot(i) => {
                if self.v.len() < 2 * self.max + 1 {
                    self.v.push(0);
                    return Phase::Snapshot(0);
                }
                if i == 0 {
                    self.trace.push(Vec::new());
                }
                if i < self.v.len() {
                    self.trace.last_mut().unwrap().push(self.v[i]);
                    Phase::Snapshot(i + 1)
                } else {
                    self.k = -(self.d as isize);
                    Phase::Diagonal
                }
            }
            Phase::Diagonal => {
                let ki = (self.k + self.max as isize) as usize;
                let d = self.d as isize;
                self.x =
                    if self.k == -d || (self.k != d && self.v[ki - 1] < self.v[ki + 1]) {
                        self.v[ki + 1]
                    } else {
                        self.v[ki - 1] + 1
                    };
                self.y = (self.x as isize - self.k) as usize;
                Phase::Snake(0)
            }
            Phase::Snake(byte) => {
                if self.x < self.n && self.y < self.m {
                    match self.equal(self.prefix + self.x, self.prefix + self.y, byte) {
                        None => return Phase::Snake(byte + BYTE_CHUNK),
                        Some(true) => {
                            self.x += 1;
                            self.y += 1;
                            return Phase::Snake(0);
                        }
                        Some(false) => {}
                    }
                }
                self.v[(self.k + self.max as isize) as usize] = self.x;
                if self.x >= self.n && self.y >= self.m {
                    Phase::Backtrack {
                        d: self.d,
                        x: self.n,
                        y: self.m,
                    }
                } else if self.k < self.d as isize {
                    self.k += 2;
                    Phase::Diagonal
                } else if self.d < self.max {
                    self.d += 1;
                    Phase::Snapshot(0)
                } else {
                    self.fallback = true;
                    Phase::PrefixRows(0)
                }
            }
            Phase::Backtrack { d, x, y } => {
                if d == 0 {
                    return Phase::Leading { x, y };
                }
                let v = &self.trace[d];
                let k = x as isize - y as isize;
                let ki = (k + self.max as isize) as usize;
                let prev_k =
                    if k == -(d as isize) || (k != d as isize && v[ki - 1] < v[ki + 1]) {
                        k + 1
                    } else {
                        k - 1
                    };
                let px = v[(prev_k + self.max as isize) as usize];
                let py = (px as isize - prev_k) as usize;
                Phase::BackSnake { d, x, y, px, py }
            }
            Phase::BackSnake {
                d,
                mut x,
                mut y,
                px,
                py,
            } => {
                if x > px && y > py {
                    self.reverse.push(LineOp::Keep);
                    x -= 1;
                    y -= 1;
                    Phase::BackSnake { d, x, y, px, py }
                } else {
                    if x == px {
                        self.reverse.push(LineOp::Insert);
                        y -= 1;
                    } else {
                        self.reverse.push(LineOp::Delete);
                        x -= 1;
                    }
                    Phase::Backtrack { d: d - 1, x, y }
                }
            }
            Phase::Leading { mut x, mut y } => {
                if x > 0 && y > 0 {
                    self.reverse.push(LineOp::Keep);
                    x -= 1;
                    y -= 1;
                } else if x > 0 {
                    self.reverse.push(LineOp::Delete);
                    x -= 1;
                } else if y > 0 {
                    self.reverse.push(LineOp::Insert);
                    y -= 1;
                } else {
                    return Phase::PrefixRows(0);
                }
                Phase::Leading { x, y }
            }
            Phase::PrefixRows(i) => {
                if i < self.prefix {
                    self.emit(LineOp::Keep, false);
                    Phase::PrefixRows(i + 1)
                } else if self.fallback {
                    Phase::Fallback {
                        deletes: self.n,
                        inserts: self.m,
                    }
                } else {
                    Phase::Normalize
                }
            }
            Phase::Normalize => match self.reverse.pop() {
                Some(LineOp::Delete) => {
                    self.pending_deletes += 1;
                    Phase::Normalize
                }
                Some(LineOp::Insert) => {
                    self.pending_inserts += 1;
                    Phase::Normalize
                }
                op => {
                    let deletes = std::mem::take(&mut self.pending_deletes);
                    let inserts = std::mem::take(&mut self.pending_inserts);
                    Phase::Flush {
                        deletes,
                        inserts,
                        keep: op.is_some(),
                    }
                }
            },
            Phase::Flush {
                deletes,
                inserts,
                keep,
            } => {
                if deletes > 0 {
                    self.emit(LineOp::Delete, false);
                    Phase::Flush {
                        deletes: deletes - 1,
                        inserts,
                        keep,
                    }
                } else if inserts > 0 {
                    self.emit(LineOp::Insert, false);
                    Phase::Flush {
                        deletes,
                        inserts: inserts - 1,
                        keep,
                    }
                } else if keep {
                    self.emit(LineOp::Keep, false);
                    Phase::Normalize
                } else {
                    Phase::SuffixRows(0)
                }
            }
            Phase::Fallback { deletes, inserts } => {
                if deletes > 0 {
                    self.emit(LineOp::Delete, false);
                    Phase::Fallback {
                        deletes: deletes - 1,
                        inserts,
                    }
                } else if inserts > 0 {
                    self.emit(LineOp::Insert, false);
                    Phase::Fallback {
                        deletes,
                        inserts: inserts - 1,
                    }
                } else {
                    Phase::SuffixRows(0)
                }
            }
            Phase::SuffixRows(i) => {
                if i < self.suffix {
                    self.emit(LineOp::Keep, false);
                    Phase::SuffixRows(i + 1)
                } else {
                    Phase::Caret
                }
            }
            Phase::Caret => {
                if self.ensure.is_some_and(|line| {
                    (self.source[1].is_empty() && line == 0)
                        || (self.source[1].ends_with('\n') && line == self.lines[1].len())
                }) {
                    self.emit(LineOp::Keep, true);
                }
                Phase::Excerpt {
                    range: 0,
                    row: 0,
                    header: true,
                }
            }
            Phase::Excerpt { range, row, header } => {
                let Some(span) = self.ranges.get(range) else {
                    return Phase::Done;
                };
                let end = span.end.min(self.rows.len());
                if header {
                    let first = self.rows[span.start];
                    let last = self.rows[end - 1];
                    let old_count = last.old + usize::from(last.has_old()) - first.old;
                    let new_count = last.new + usize::from(last.has_new()) - first.new;
                    self.result.push(Self::plain(format!(
                        "@@ -{},{} +{},{} @@",
                        first.old + usize::from(old_count > 0),
                        old_count,
                        first.new + usize::from(new_count > 0),
                        new_count
                    )));
                    return Phase::Excerpt {
                        range,
                        row: span.start,
                        header: false,
                    };
                }
                if row == end {
                    return Phase::Excerpt {
                        range: range + 1,
                        row: 0,
                        header: true,
                    };
                }
                let r = self.rows[row];
                let (byte, mut end) = if r.synthetic {
                    (0, 0)
                } else {
                    let side = usize::from(r.op != LineOp::Delete);
                    let i = if side == 0 { r.old } else { r.new };
                    let span = &self.lines[side][i];
                    let bytes = self.source[side].as_bytes();
                    let mut end = span.end;
                    if bytes[end - 1] == b'\n' {
                        end -= 1;
                        if end > span.start && bytes[end - 1] == b'\r' {
                            end -= 1;
                        }
                    }
                    (span.start, end)
                };
                // Synthetic rows have no source slice.
                if r.synthetic {
                    end = byte;
                }
                Phase::Copy {
                    range,
                    row,
                    byte,
                    end,
                    text: String::new(),
                }
            }
            Phase::Copy {
                range,
                row,
                mut byte,
                end,
                mut text,
            } => {
                let r = self.rows[row];
                let side = usize::from(r.op != LineOp::Delete);
                if byte < end {
                    let source = &self.source[side];
                    let mut chunk_end = byte.saturating_add(BYTE_CHUNK).min(end);
                    // Retreat at most three continuation bytes. BYTE_CHUNK >= 4
                    // guarantees progress even if the next scalar is four bytes.
                    while !source.is_char_boundary(chunk_end) {
                        chunk_end -= 1;
                    }
                    text.push_str(&source[byte..chunk_end]);
                    byte = chunk_end;
                    return Phase::Copy {
                        range,
                        row,
                        byte,
                        end,
                        text,
                    };
                }
                let old_line_number = r.has_old().then_some(r.old as u32 + 1);
                let new_line_number = r.has_new().then_some(r.new as u32 + 1);
                self.result.push(DiffLine {
                    text,
                    kind: match r.op {
                        LineOp::Keep => K::Context,
                        LineOp::Delete => K::Remove,
                        LineOp::Insert => K::Add,
                    },
                    line_number: new_line_number.or(old_line_number),
                    old_line_number,
                    new_line_number,
                });
                if !r.synthetic {
                    let i = if side == 0 { r.old } else { r.new };
                    if self.source[side].as_bytes()[self.lines[side][i].end - 1] != b'\n'
                    {
                        self.result
                            .push(Self::plain("\\ No newline at end of file".into()));
                    }
                }
                Phase::Excerpt {
                    range,
                    row: row + 1,
                    header: false,
                }
            }
            Phase::Done => Phase::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(
        rows: &[DiffLine],
    ) -> Vec<(K, Option<u32>, Option<u32>, Option<u32>, &str)> {
        rows.iter()
            .map(|r| {
                (
                    r.kind,
                    r.line_number,
                    r.old_line_number,
                    r.new_line_number,
                    r.text.as_str(),
                )
            })
            .collect()
    }

    fn compare(
        base: &str,
        current: &str,
        context: usize,
        ensure: Option<usize>,
        budget: usize,
    ) {
        let expected =
            super::super::reference_review_diff(base, current, context, ensure);
        let mut builder =
            ReviewDiffBuilder::new(base.into(), current.into(), context, ensure);
        assert!(builder.poll(0).is_none());
        for _ in 0..2_000_000 {
            if let Some(actual) = builder.poll(budget) {
                assert_eq!(signature(&actual), signature(&expected),
                    "base={base:?}, current={current:?}, context={context}, ensure={ensure:?}, budget={budget}");
                assert!(
                    builder.poll(1).is_none(),
                    "completion must be delivered once"
                );
                return;
            }
        }
        panic!("builder failed to make bounded progress");
    }

    #[test]
    fn eof_utf8_crlf_and_caret_match_legacy() {
        let documents = [
            "",
            "\n",
            "a",
            "a\n",
            "α\n新",
            "α\r\n新\r\n",
            "a\na\nb\n",
            "\r",
            "\n\n",
        ];
        for base in documents {
            for current in documents {
                for context in [0, 1, 3, usize::MAX] {
                    for ensure in [None, Some(0), Some(1), Some(2), Some(9)] {
                        compare(base, current, context, ensure, 1);
                    }
                }
            }
        }
    }

    #[test]
    fn deterministic_repeated_lines_and_separated_excerpts() {
        let mut seed = 0x6d2b79f5u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let tokens = ["a\n", "b\n", "α\n", "\n", "x\r\n"];
        for case in 0..100 {
            let mut base = String::new();
            let mut current = String::new();
            for _ in 0..(next() % 35) {
                base.push_str(tokens[next() as usize % tokens.len()]);
            }
            for _ in 0..(next() % 35) {
                current.push_str(tokens[next() as usize % tokens.len()]);
            }
            if case % 2 == 0 {
                base.pop();
            }
            if case % 3 == 0 {
                current.pop();
            }
            compare(
                &base,
                &current,
                case % 4,
                Some(case % 30),
                [1, 7, 256][case % 3],
            );
        }
        let base = (0..200).map(|i| format!("line {i}\n")).collect::<String>();
        let current = base
            .replace("line 3\n", "edit 3\n")
            .replace("line 180\n", "edit 180\n");
        compare(&base, &current, 2, Some(90), 1);
    }

    #[test]
    fn capped_myers_fallback_matches_legacy_at_tiny_budgets() {
        let base = format!("prefix\n{}suffix", "old\n".repeat(300));
        let current = format!("prefix\n{}suffix\n", "new\n".repeat(300));
        compare(&base, &current, 1, Some(301), 1);
        compare(&base, &current, 0, None, 256);
    }

    #[test]
    fn indexing_comparison_and_output_of_giant_lines_yield() {
        let text = "λ".repeat(4096);
        let mut builder = ReviewDiffBuilder::new(text.clone(), text.clone(), 0, Some(0));
        assert!(builder.lines[0].is_empty() && builder.lines[1].is_empty());
        assert!(builder.rows.is_empty() && builder.result.is_empty());
        assert!(builder.poll(7).is_none());
        assert!(matches!(
            builder.phase,
            Phase::Index {
                side: 0,
                byte,
                ..
            } if byte == 7 * BYTE_CHUNK
        ));
        assert!(
            builder.lines[0].is_empty(),
            "no eager splitting of a giant line"
        );
        while !matches!(builder.phase, Phase::Prefix(_)) {
            assert!(builder.poll(1).is_none());
        }
        assert!(builder.poll(7).is_none());
        assert!(matches!(builder.phase, Phase::Prefix(byte) if byte == 7 * BYTE_CHUNK));
        while !matches!(builder.phase, Phase::Copy { .. }) {
            assert!(builder.poll(1).is_none());
        }
        assert!(builder.poll(7).is_none());
        match &builder.phase {
            Phase::Copy { byte, text, .. } => {
                assert_eq!(*byte, 7 * BYTE_CHUNK);
                assert_eq!(text.len(), 7 * BYTE_CHUNK);
                assert_eq!(text.chars().count(), 7 * BYTE_CHUNK / 2);
            }
            _ => panic!("output copying must also yield"),
        }
        let actual = loop {
            if let Some(rows) = builder.poll(7) {
                break rows;
            }
        };
        let expected = super::super::reference_review_diff(&text, &text, 0, Some(0));
        assert_eq!(signature(&actual), signature(&expected));
    }

    #[test]
    fn chunk_edges_preserve_utf8_newlines_and_late_mismatches() {
        let line = format!("{}🦀λz", "a".repeat(BYTE_CHUNK - 1));
        let base = format!("{line}\r\n{}\n{line}", "\n".repeat(BYTE_CHUNK));
        let current = base.replace('z', "x");
        compare(&base, &current, 1, Some(2), 1);
        compare(&base, &current, 0, None, 7);

        let text = line.repeat(32);
        let mut builder = ReviewDiffBuilder::new(text.clone(), text.clone(), 0, Some(0));
        while !matches!(builder.phase, Phase::Copy { .. }) {
            assert!(builder.poll(1).is_none());
        }
        let mut saw_boundary_retreat = false;
        while let Phase::Copy { byte, end, .. } = &builder.phase {
            let before = *byte;
            let remaining = end - before;
            assert!(builder.poll(1).is_none());
            if let Phase::Copy {
                byte, text: copied, ..
            } = &builder.phase
            {
                let advanced = *byte - before;
                assert!(advanced > 0 && advanced <= BYTE_CHUNK);
                assert!(text.is_char_boundary(*byte));
                assert_eq!(copied, &text[..*byte]);
                if remaining > BYTE_CHUNK && advanced < BYTE_CHUNK {
                    saw_boundary_retreat = true;
                }
            }
        }
        assert!(
            saw_boundary_retreat,
            "exercise a scalar straddling the chunk edge"
        );
        let actual = loop {
            if let Some(rows) = builder.poll(7) {
                break rows;
            }
        };
        let expected = super::super::reference_review_diff(&text, &text, 0, Some(0));
        assert_eq!(signature(&actual), signature(&expected));
    }

    #[test]
    fn fallback_rows_and_frontier_snapshots_are_budgeted() {
        let mut builder =
            ReviewDiffBuilder::new("a\n".repeat(300), "b\n".repeat(300), 0, None);
        while !matches!(builder.phase, Phase::Snapshot(_)) {
            assert!(builder.poll(1).is_none());
        }
        assert!(builder.poll(7).is_none());
        assert_eq!(
            builder.v.len(),
            7,
            "frontier initialization is not hidden in one poll"
        );
        while !matches!(builder.phase, Phase::Fallback { .. }) {
            assert!(builder.poll(1).is_none());
        }
        let before = builder.rows.len();
        assert!(builder.poll(7).is_none());
        assert_eq!(
            builder.rows.len() - before,
            7,
            "fallback emits one metadata row per unit"
        );
    }
}
