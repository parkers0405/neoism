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
use std::collections::{BTreeMap, BTreeSet};

use super::types::*;

/// Diagnostic severity carried on a run. Mapped from the wire's
/// `DiagnosticSeverity` by the host — the shared feed stays
/// protocol-independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// One document-level diagnostic retained by a pane for exact status-pill
/// counts and popup rows. Visible geometry lives in `CodeLineDiagnostic`;
/// this compact summary avoids a process-global raw diagnostics store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeDiagnosticSummary {
    pub line: usize,
    pub byte: usize,
    pub severity: CodeDiagnosticSeverity,
    pub message: String,
}

/// A diagnostic pinned into the CRDT document with sticky anchors
/// (Zed-Anchor semantics): the range endpoints survive local AND
/// remote edits by anchoring to CRDT block identity instead of
/// line/col numbers. Built by the host at diagnostics-publish time
/// when the pane is doc-bound; re-resolved into per-line spans on
/// every buffer revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeDiagAnchor {
    pub start: crate::editor::crdt::CrdtStickyAnchor,
    pub end: crate::editor::crdt::CrdtStickyAnchor,
    pub severity: CodeDiagnosticSeverity,
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
    pub plugin_foreground: Option<[u8; 4]>,
    pub plugin_background: Option<[u8; 4]>,
    pub plugin_underline: Option<[u8; 4]>,
    pub concealed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CodeLinePluginSpan {
    pub start: usize,
    pub end: usize,
    pub foreground: Option<[u8; 4]>,
    pub background: Option<[u8; 4]>,
    pub underline: Option<[u8; 4]>,
    pub concealed: bool,
    pub severity: Option<CodeDiagnosticSeverity>,
}

#[derive(Clone, Debug, Default)]
pub struct CodePluginRenderSnapshot {
    pub revision: u64,
    pub by_line: BTreeMap<usize, Vec<CodeLinePluginDecoration>>,
    pub hidden_lines: BTreeSet<usize>,
    pub virtual_lines: BTreeMap<usize, Vec<CodePluginVirtualLine>>,
    pub spans_by_line: BTreeMap<usize, Vec<CodeLinePluginSpan>>,
}

#[derive(Clone, Debug)]
pub struct CodePluginVirtualLine {
    pub id: neoism_lua::PluginResourceId,
    pub owner: neoism_lua::PluginOwner,
    pub text: String,
    pub style: neoism_lua::ResolvedDecorationStyle,
    pub actions: Vec<neoism_lua::PluginDiagnosticAction>,
}

#[derive(Clone, Debug)]
pub struct CodePluginHitRegion {
    pub rect: [f32; 4],
    pub owner: neoism_lua::PluginOwner,
    pub resource: neoism_lua::PluginResourceId,
    pub action: neoism_lua::PluginDiagnosticAction,
}

#[derive(Clone, Debug)]
pub struct CodeLinePluginDecoration {
    pub id: neoism_lua::PluginResourceId,
    pub owner: neoism_lua::PluginOwner,
    pub start: usize,
    pub end: usize,
    pub layer: neoism_lua::DecorationLayer,
    pub text: Option<String>,
    pub icon: Option<String>,
    pub severity: Option<CodeDiagnosticSeverity>,
    pub style: neoism_lua::ResolvedDecorationStyle,
    pub actions: Vec<neoism_lua::PluginDiagnosticAction>,
}

impl CodePluginRenderSnapshot {
    pub fn try_from_contract(snapshot: &neoism_lua::PluginDecorationSnapshot, lines: &[String]) -> Result<Self, String> {
        const MAX_DECORATIONS: usize = 50_000;
        const MAX_VIRTUAL_TEXT_BYTES: usize = 64 * 1024;
        if snapshot.decorations.len() > MAX_DECORATIONS {
            return Err(format!("plugin decoration snapshot exceeds {MAX_DECORATIONS} items"));
        }
        let mut output = Self { revision: snapshot.revision, ..Self::default() };
        for decoration in &snapshot.decorations {
            let first = decoration.start_position.line as usize;
            let last = decoration.end_position.line as usize;
            if first >= lines.len() || last >= lines.len() || first > last {
                return Err("plugin decoration has invalid resolved line geometry".into());
            }
            let first_len = lines[first].len();
            let last_len = lines[last].len();
            let first_col = decoration.start_position.character as usize;
            let last_col = decoration.end_position.character as usize;
            if first_col > first_len || last_col > last_len
                || !lines[first].is_char_boundary(first_col)
                || !lines[last].is_char_boundary(last_col)
            {
                return Err("plugin decoration has invalid resolved UTF-8 geometry".into());
            }
            if decoration.text.as_ref().is_some_and(|text| text.len() > MAX_VIRTUAL_TEXT_BYTES) {
                return Err("plugin virtual text exceeds the immutable snapshot limit".into());
            }
            if decoration.layer == neoism_lua::DecorationLayer::VirtualLine {
                output.virtual_lines.entry(first).or_default().push(CodePluginVirtualLine {
                    id: decoration.id,
                    owner: decoration.owner.clone(),
                    text: decoration.text.clone().unwrap_or_default(),
                    style: decoration.resolved_style,
                    actions: decoration.actions.clone(),
                });
                continue;
            }
            if decoration.layer == neoism_lua::DecorationLayer::Fold && last > first {
                output.hidden_lines.extend((first + 1)..=last.min(lines.len().saturating_sub(1)));
            }
            for line in first..=last.min(lines.len().saturating_sub(1)) {
                let line_len = lines.get(line).map_or(0, String::len);
                let start = if line == first { decoration.start_position.character as usize } else { 0 }.min(line_len);
                let end = if line == last { decoration.end_position.character as usize } else { line_len }.min(line_len);
                output.by_line.entry(line).or_default().push(CodeLinePluginDecoration {
                    id: decoration.id,
                    owner: decoration.owner.clone(),
                    start,
                    end,
                    layer: decoration.layer,
                    text: decoration.text.clone(),
                    icon: decoration.style.icon.clone(),
                    severity: decoration.severity.map(|severity| match severity {
                        neoism_lua::PluginDiagnosticSeverity::Error => CodeDiagnosticSeverity::Error,
                        neoism_lua::PluginDiagnosticSeverity::Warning => CodeDiagnosticSeverity::Warn,
                        neoism_lua::PluginDiagnosticSeverity::Information => CodeDiagnosticSeverity::Info,
                        neoism_lua::PluginDiagnosticSeverity::Hint => CodeDiagnosticSeverity::Hint,
                    }),
                    style: decoration.resolved_style,
                    actions: decoration.actions.clone(),
                });
            }
        }
        for (line, decorations) in &output.by_line {
            let spans = decorations.iter().filter_map(|decoration| {
                matches!(decoration.layer, neoism_lua::DecorationLayer::Highlight | neoism_lua::DecorationLayer::Conceal | neoism_lua::DecorationLayer::Diagnostic)
                    .then_some(CodeLinePluginSpan {
                        start: decoration.start,
                        end: decoration.end,
                        foreground: decoration.style.foreground,
                        background: decoration.style.background,
                        underline: decoration.style.underline,
                        concealed: decoration.layer == neoism_lua::DecorationLayer::Conceal,
                        severity: decoration.severity,
                    })
            }).collect::<Vec<_>>();
            if !spans.is_empty() {
                output.spans_by_line.insert(*line, spans);
            }
        }
        Ok(output)
    }

    pub fn from_contract(snapshot: &neoism_lua::PluginDecorationSnapshot, lines: &[String]) -> Self {
        Self::try_from_contract(snapshot, lines).unwrap_or_default()
    }

    pub fn spans_for_line(&self, line: usize) -> &[CodeLinePluginSpan] {
        self.spans_by_line.get(&line).map(Vec::as_slice).unwrap_or(&[])
    }
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
    styled_runs_with_syntax(line, None, lang, selection, diagnostics, &[])
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
    plugin_spans: &[CodeLinePluginSpan],
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
    for span in plugin_spans {
        cuts.push(span.start.min(line.len()));
        cuts.push(span.end.min(line.len()));
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
        let selected =
            selection.is_some_and(|(s, e)| s <= start && end <= e.min(line.len()));
        let severity = diagnostics
            .iter()
            .filter(|diag| diag.start <= start && end <= diag.end.min(line.len()))
            .map(|diag| diag.severity)
            .max()
            .max(plugin_spans.iter()
                .filter(|span| span.start <= start && end <= span.end.min(line.len()))
                .filter_map(|span| span.severity)
                .max());
        let plugin = plugin_spans.iter().rev()
            .find(|span| span.start <= start && end <= span.end.min(line.len()));
        let plugin_foreground = plugin.and_then(|span| span.foreground);
        let plugin_background = plugin.and_then(|span| span.background);
        let plugin_underline = plugin.and_then(|span| span.underline);
        let concealed = plugin.is_some_and(|span| span.concealed);
        let merged = runs.last_mut().filter(|prev| {
            prev.end == start
                && prev.token == token
                && prev.selected == selected
                && prev.severity == severity
                && prev.plugin_foreground == plugin_foreground
                && prev.plugin_background == plugin_background
                && prev.plugin_underline == plugin_underline
                && prev.concealed == concealed
        });
        match merged {
            Some(prev) => prev.end = end,
            None => runs.push(CodeStyledRun {
                start,
                end,
                token,
                selected,
                severity,
                plugin_foreground,
                plugin_background,
                plugin_underline,
                concealed,
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
