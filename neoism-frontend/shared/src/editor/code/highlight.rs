//! Whole-buffer syntax cache for the code pane.
//!
//! Runs `syntax::highlight_source` over the full text whenever the
//! buffer revision moves, then splits the global byte spans into
//! per-line runs the styled-run feed consumes. This is what makes
//! multi-line constructs (block comments, raw strings, triple-quoted
//! strings) color correctly — the per-line highlighter stays as the
//! fallback for wasm, oversized files, and languages without a parser.
//! Renderer-agnostic: no sugarloaf, plain data out.

use crate::syntax::{highlight_source, Lang, SynTok};

use super::types::CodeBuffer;

/// Above this, a full re-highlight per keystroke costs too much on the
/// UI thread; fall back to per-line until incremental parsing lands.
pub const WHOLE_BUFFER_HIGHLIGHT_MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Default)]
pub struct CodeHighlightCache {
    revision: Option<u64>,
    lang: Option<Lang>,
    /// Per source line: (token, start, end) in line-local bytes.
    lines: Vec<Vec<(SynTok, usize, usize)>>,
    /// Whole-buffer pass unavailable — callers use the per-line path.
    unavailable: bool,
}

impl CodeHighlightCache {
    /// Recompute if the buffer or language changed since the last call.
    pub fn refresh(&mut self, buffer: &CodeBuffer, lang: Lang) {
        if self.revision == Some(buffer.revision) && self.lang == Some(lang) {
            return;
        }
        self.revision = Some(buffer.revision);
        self.lang = Some(lang);
        self.lines.clear();
        self.unavailable = true;

        let total_bytes: usize = buffer.lines.iter().map(|line| line.len() + 1).sum();
        if total_bytes > WHOLE_BUFFER_HIGHLIGHT_MAX_BYTES {
            return;
        }
        let source = buffer.text();
        let Some(spans) = highlight_source(&source, lang) else {
            return;
        };
        self.unavailable = false;

        let mut line_starts: Vec<usize> = Vec::with_capacity(buffer.lines.len());
        let mut acc = 0usize;
        for line in &buffer.lines {
            line_starts.push(acc);
            acc += line.len() + 1;
        }
        self.lines = vec![Vec::new(); buffer.lines.len()];
        for (token, start, end) in spans {
            if token == SynTok::Plain {
                continue;
            }
            let mut line_ix = line_starts
                .partition_point(|line_start| *line_start <= start)
                .saturating_sub(1);
            let mut span_start = start;
            while span_start < end && line_ix < buffer.lines.len() {
                let line_start = line_starts[line_ix];
                let line_end = line_start + buffer.lines[line_ix].len();
                let seg_start = span_start.max(line_start);
                let seg_end = end.min(line_end);
                if seg_start < seg_end {
                    self.lines[line_ix].push((
                        token,
                        seg_start - line_start,
                        seg_end - line_start,
                    ));
                }
                line_ix += 1;
                span_start = line_end + 1;
            }
        }
    }

    /// Whole-buffer runs for one line, or None when the caller should
    /// fall back to the per-line highlighter.
    pub fn line_runs(&self, line: usize) -> Option<&[(SynTok, usize, usize)]> {
        if self.unavailable {
            return None;
        }
        self.lines.get(line).map(|runs| runs.as_slice())
    }
}
