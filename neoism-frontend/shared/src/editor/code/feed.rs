//! The styled-run feed: the renderer-agnostic contract every host
//! paints from (Zed's `chunks()` idea at line granularity).
//!
//! For one source line it merges three inputs — syntax highlight
//! spans, the local selection, and diagnostic ranges — into a flat,
//! ordered list of byte-range runs, each carrying a composite style.
//! The GUI shell maps runs to sugarloaf spans (squiggle decorations
//! from `severity`), a tty host maps the same runs to terminal cells.
//! Nothing in here may touch pixels or sugarloaf.

use crate::syntax::{highlight_line, Lang, SynTok};

use super::types::*;

/// Diagnostic severity carried on a run. Mapped from the wire's
/// `DiagnosticSeverity` by the host — the shared feed stays
/// protocol-independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodeDiagnosticSeverity {
    Hint,
    Info,
    Warn,
    Error,
}

/// A diagnostic span on one line, in byte columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeLineDiagnostic {
    pub start: usize,
    pub end: usize,
    pub severity: CodeDiagnosticSeverity,
    /// Diagnostic message for the inline virtual text; populated only
    /// on the diagnostic's FIRST line (continuation-line spans carry
    /// an empty message so multi-line diagnostics print once).
    pub message: String,
}

/// One styled run: `line[start..end]` drawn with `token` color,
/// optionally selected and/or underlined by the strongest overlapping
/// diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeStyledRun {
    pub start: usize,
    pub end: usize,
    pub token: SynTok,
    pub selected: bool,
    pub severity: Option<CodeDiagnosticSeverity>,
}

/// Merge syntax + selection + diagnostics for one line into ordered,
/// non-overlapping runs covering `0..line.len()` (empty lines yield no
/// runs). `selection` is a normalized byte range on this line.
/// Per-line highlighter path (fallback / tests).
pub fn styled_runs_for_line(
    line: &str,
    lang: Lang,
    selection: Option<(usize, usize)>,
    diagnostics: &[CodeLineDiagnostic],
) -> Vec<CodeStyledRun> {
    styled_runs_with_syntax(line, None, lang, selection, diagnostics)
}

/// Like `styled_runs_for_line`, but with precomputed whole-buffer
/// syntax runs for this line (from `CodeHighlightCache`); passes
/// `None` to fall back to the per-line highlighter.
pub fn styled_runs_with_syntax(
    line: &str,
    precomputed: Option<&[(SynTok, usize, usize)]>,
    lang: Lang,
    selection: Option<(usize, usize)>,
    diagnostics: &[CodeLineDiagnostic],
) -> Vec<CodeStyledRun> {
    if line.is_empty() {
        return Vec::new();
    }

    let mut syntax: Vec<(usize, usize, SynTok)> = Vec::new();
    match precomputed {
        Some(runs) => {
            for (token, start, end) in runs {
                let start = (*start).min(line.len());
                let end = (*end).min(line.len());
                if start < end {
                    syntax.push((start, end, *token));
                }
            }
        }
        None => {
            // Syntax spans arrive as consecutive slices; recover offsets.
            let mut offset = 0usize;
            for (token, slice) in highlight_line(line, lang) {
                let end = offset + slice.len();
                if !slice.is_empty() {
                    syntax.push((offset, end, token));
                }
                offset = end;
            }
        }
    }
    if syntax.is_empty() {
        syntax.push((0, line.len(), SynTok::Plain));
    }

    // Every style-change point becomes a run boundary.
    let mut cuts: Vec<usize> = vec![0, line.len()];
    for (start, end, _) in &syntax {
        cuts.push(*start);
        cuts.push(*end);
    }
    if let Some((start, end)) = selection {
        cuts.push(start.min(line.len()));
        cuts.push(end.min(line.len()));
    }
    for diag in diagnostics {
        cuts.push(diag.start.min(line.len()));
        cuts.push(diag.end.min(line.len()));
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs: Vec<CodeStyledRun> = Vec::new();
    for window in cuts.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start >= end {
            continue;
        }
        let token = syntax
            .iter()
            .find(|(s, e, _)| *s <= start && end <= *e)
            .map(|(_, _, token)| *token)
            .unwrap_or(SynTok::Plain);
        let selected = selection
            .is_some_and(|(s, e)| s <= start && end <= e.min(line.len()));
        let severity = diagnostics
            .iter()
            .filter(|diag| diag.start <= start && end <= diag.end.min(line.len()))
            .map(|diag| diag.severity)
            .max();
        let merged = runs.last_mut().filter(|prev| {
            prev.end == start
                && prev.token == token
                && prev.selected == selected
                && prev.severity == severity
        });
        match merged {
            Some(prev) => prev.end = end,
            None => runs.push(CodeStyledRun {
                start,
                end,
                token,
                selected,
                severity,
            }),
        }
    }
    runs
}

impl CodeBuffer {
    /// The selection's byte range on `line`, normalized, if any part of
    /// the selection touches it — feeds `styled_runs_for_line`.
    pub fn selection_on_line(&self, line: usize) -> Option<(usize, usize)> {
        let (start, end) = self.selection_range()?;
        if line < start.line || line > end.line {
            return None;
        }
        let text_len = self.lines.get(line)?.len();
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line { end.col } else { text_len };
        Some((from.min(text_len), to.min(text_len)))
    }
}
