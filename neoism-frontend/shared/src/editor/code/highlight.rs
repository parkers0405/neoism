//! Whole-buffer syntax cache for the code pane.
//!
//! Runs `syntax::highlight_source` over the full text whenever the
//! buffer revision moves, then splits the global byte spans into
//! per-line runs the styled-run feed consumes. This is what makes
//! multi-line constructs (block comments, raw strings, triple-quoted
//! strings) color correctly — the per-line highlighter stays as the
//! fallback for wasm, oversized files, and languages without a parser.
//! Renderer-agnostic: no sugarloaf, plain data out.

use std::time::Duration;

use web_time::Instant;

use crate::syntax::{highlight_source, Lang, SynTok};

use super::types::CodeBuffer;

/// Above this, a full re-highlight per keystroke costs too much on the
/// UI thread; fall back to per-line until incremental parsing lands.
pub const WHOLE_BUFFER_HIGHLIGHT_MAX_BYTES: usize = 512 * 1024;

/// Above this, a revision bump does NOT reparse immediately: the cache
/// keeps serving the previous (stale) spans and defers the whole-buffer
/// pass until the revision has held still for
/// [`HIGHLIGHT_DEBOUNCE_IDLE`]. The painter calls `refresh` every frame
/// while the pane repaints, so polling stands in for a timer — no
/// threads. Small files keep the instant path.
pub const HIGHLIGHT_DEBOUNCE_MIN_BYTES: usize = 64 * 1024;

/// How long a large file's revision must hold still before the deferred
/// reparse runs.
const HIGHLIGHT_DEBOUNCE_IDLE: Duration = Duration::from_millis(120);

/// A buffer revision seen but not yet parsed (large-file debounce).
#[derive(Clone, Copy, Debug)]
struct PendingRefresh {
    revision: u64,
    since: Instant,
}

#[derive(Clone, Debug, Default)]
pub struct CodeHighlightCache {
    revision: Option<u64>,
    lang: Option<Lang>,
    /// Per source line: (token, start, end) in line-local bytes.
    lines: Vec<Vec<(SynTok, usize, usize)>>,
    /// Whole-buffer pass unavailable — callers use the per-line path.
    unavailable: bool,
    /// Large-file debounce state; `Some` while served spans are stale.
    pending: Option<PendingRefresh>,
}

impl CodeHighlightCache {
    /// Recompute if the buffer or language changed since the last call.
    pub fn refresh(&mut self, buffer: &CodeBuffer, lang: Lang) {
        if self.revision == Some(buffer.revision) && self.lang == Some(lang) {
            self.pending = None;
            return;
        }

        let total_bytes: usize = buffer.lines.iter().map(|line| line.len() + 1).sum();

        // Typing-burst debounce: on large files a whole-buffer reparse
        // per keystroke janks the UI thread, so keep serving the
        // previous spans and only reparse once the revision has been
        // stable for the idle window. The first parse (nothing to serve
        // yet) and language switches (stale spans would be the wrong
        // grammar) stay immediate.
        if total_bytes > HIGHLIGHT_DEBOUNCE_MIN_BYTES
            && self.revision.is_some()
            && self.lang == Some(lang)
        {
            match self.pending {
                Some(pending) if pending.revision == buffer.revision => {
                    if pending.since.elapsed() < HIGHLIGHT_DEBOUNCE_IDLE {
                        return;
                    }
                }
                _ => {
                    // New keystroke in the burst: (re)arm the idle timer
                    // and drop stale runs the edit made unsafe to paint.
                    self.pending = Some(PendingRefresh {
                        revision: buffer.revision,
                        since: Instant::now(),
                    });
                    self.sanitize_stale_runs(buffer);
                    return;
                }
            }
        }
        self.pending = None;

        self.revision = Some(buffer.revision);
        self.lang = Some(lang);
        self.lines.clear();
        self.unavailable = true;

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

    /// While stale spans are served (debounce window), the painter still
    /// slices the LIVE line text by our byte offsets — drop any run that
    /// no longer fits its line or lands inside a UTF-8 sequence so a
    /// mid-burst paint can't slice out of bounds. Out-of-range line
    /// indexes are handled by `line_runs` returning `None`.
    fn sanitize_stale_runs(&mut self, buffer: &CodeBuffer) {
        for (runs, line) in self.lines.iter_mut().zip(&buffer.lines) {
            runs.retain(|(_, start, end)| {
                *end <= line.len()
                    && line.is_char_boundary(*start)
                    && line.is_char_boundary(*end)
            });
        }
    }

    /// Whole-buffer runs for one line, or None when the caller should
    /// fall back to the per-line highlighter. During a large-file
    /// typing burst these are the previous revision's (stale) runs;
    /// lines past the cached range return None so the painter can index
    /// by live line numbers safely.
    pub fn line_runs(&self, line: usize) -> Option<&[(SynTok, usize, usize)]> {
        if self.unavailable {
            return None;
        }
        self.lines.get(line).map(|runs| runs.as_slice())
    }
}

// Tests need real parses; `highlight_source` is a `None` stub on wasm.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn has_kind(runs: Option<&[(SynTok, usize, usize)]>, kind: SynTok) -> bool {
        runs.is_some_and(|runs| runs.iter().any(|(tok, ..)| *tok == kind))
    }

    #[test]
    fn small_files_reparse_instantly() {
        let mut buffer = CodeBuffer::from_text("fn main() {}\n");
        let mut cache = CodeHighlightCache::default();
        cache.refresh(&buffer, Lang::Rust);
        assert!(has_kind(cache.line_runs(0), SynTok::Keyword));

        buffer.lines[0] = "let value = \"text\";".to_string();
        buffer.revision += 1;
        cache.refresh(&buffer, Lang::Rust);
        assert!(
            has_kind(cache.line_runs(0), SynTok::String),
            "small files must reflect an edit on the very next refresh"
        );
        assert!(cache.line_runs(10).is_none());
    }

    #[test]
    fn large_files_debounce_reparse_and_serve_stale_spans() {
        // Comfortably past HIGHLIGHT_DEBOUNCE_MIN_BYTES (so the edits
        // below can't shrink the buffer back under the threshold) while
        // staying under WHOLE_BUFFER_HIGHLIGHT_MAX_BYTES.
        let mut source = String::new();
        while source.len() <= HIGHLIGHT_DEBOUNCE_MIN_BYTES + 1024 {
            source.push_str("const K: usize = 1; // padding\n");
        }
        let mut buffer = CodeBuffer::from_text(&source);
        let mut cache = CodeHighlightCache::default();

        // The first parse is immediate even on a large file.
        cache.refresh(&buffer, Lang::Rust);
        assert!(has_kind(cache.line_runs(0), SynTok::Keyword));

        // A keystroke keeps serving the previous spans: the edit below
        // is not reflected yet.
        buffer.lines[1] = "\"now a string\"".to_string();
        buffer.revision += 1;
        cache.refresh(&buffer, Lang::Rust);
        assert!(
            !has_kind(cache.line_runs(1), SynTok::String),
            "stale spans must not include the unparsed edit"
        );

        // Once the revision holds still past the idle window, the next
        // refresh reparses and the edit shows up.
        std::thread::sleep(HIGHLIGHT_DEBOUNCE_IDLE + Duration::from_millis(40));
        cache.refresh(&buffer, Lang::Rust);
        assert!(has_kind(cache.line_runs(1), SynTok::String));
    }

    #[test]
    fn stale_runs_stay_inside_the_live_lines() {
        let mut source = String::from("let alpha = \"long string literal\";\n");
        while source.len() <= HIGHLIGHT_DEBOUNCE_MIN_BYTES + 1024 {
            source.push_str("const K: usize = 1;\n");
        }
        let mut buffer = CodeBuffer::from_text(&source);
        let mut cache = CodeHighlightCache::default();
        cache.refresh(&buffer, Lang::Rust);
        assert!(has_kind(cache.line_runs(0), SynTok::String));

        // Shrink line 0 onto a multi-byte char: every surviving stale
        // run must still slice the LIVE text safely.
        buffer.lines[0] = "é".to_string();
        buffer.revision += 1;
        cache.refresh(&buffer, Lang::Rust);
        let line = &buffer.lines[0];
        for (_, start, end) in cache.line_runs(0).unwrap_or(&[]) {
            assert!(
                *end <= line.len()
                    && line.is_char_boundary(*start)
                    && line.is_char_boundary(*end),
                "stale run {start}..{end} would slice {line:?} unsafely"
            );
        }

        // Line indexes past the cached range fall back gracefully.
        assert!(cache.line_runs(buffer.lines.len() + 10_000).is_none());
    }
}
