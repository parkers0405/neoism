use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::lsp::{self, LspDiagnostic};

/// Maximum number of *other* project files whose cached diagnostics are
/// surfaced alongside the file(s) a mutation just touched.
const DIAGNOSTICS_PROJECT_FILE_LIMIT: usize = 8;
/// Upper bound on how many diagnostics are scanned across the project when
/// assembling post-mutation metadata, so a noisy workspace can't balloon the
/// tool result.
const DIAGNOSTICS_PROJECT_SCAN_LIMIT: usize = 200;
/// Matches opencode's per-file cap in the "please fix" report block.
const MAX_REPORTED_ERRORS_PER_FILE: usize = 20;

/// Notify the language servers that `paths` changed on disk, mirroring
/// opencode's `lsp.touchFile(file, "document")` step before it gathers
/// diagnostics.
///
/// NOTE: this performs blocking LSP I/O (it can wait for `publishDiagnostics`).
/// Callers MUST invoke it off the async executor — see [`attach_lsp_diagnostics`].
pub(super) fn touch_paths(
    runtime: &lsp::LspRuntime,
    cwd: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Value {
    let mut entries = Vec::new();
    for path in resolve_existing(cwd, paths) {
        let notified = lsp::touch_document(runtime, cwd, &path, None);
        entries.push(json!({
            "path": display_path(cwd, &path),
            "notified": notified,
        }));
    }
    Value::Array(entries)
}

/// Gather LSP diagnostics for the freshly mutated files (plus a bounded slice of
/// the rest of the project's cached diagnostics), attach them to `metadata`, and
/// return an opencode-style "please fix" block for any *errors* in the touched
/// files so the model is prompted to repair regressions it just introduced.
///
/// This is a pure cache read — the single blocking wait already happened in
/// [`touch_paths`] (opencode's `touchFile(_, "document")`); this mirrors
/// opencode's `diagnostics()`, which just reads each client's published
/// `client.diagnostics`. Callers MUST run `touch_paths` first so the cache is
/// populated. It is cheap, but stays inside the same `spawn_blocking` as
/// `touch_paths` for simplicity.
pub(super) fn attach_lsp_diagnostics(
    runtime: &lsp::LspRuntime,
    cwd: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
    metadata: &mut Value,
) -> Option<String> {
    let touched = resolve_existing(cwd, paths);
    let mut touched_keys = BTreeSet::new();

    let mut entries = Vec::new();
    let mut report_sections = Vec::new();
    let mut total = 0usize;

    for path in &touched {
        touched_keys.insert(path.clone());
        let diagnostics = lsp::cached_diagnostics(runtime, cwd, path);
        total += diagnostics.len();
        let display = display_path(cwd, path);
        if let Some(section) = error_report(&display, &diagnostics) {
            report_sections.push(section);
        }
        entries.push(diagnostic_entry(&display, "touched", diagnostics));
    }

    // Bounded scan of the rest of the project's *cached* diagnostics so the
    // model also sees regressions its edit caused in other open files. opencode
    // returns the full diagnostics record; we cap files/diagnostics to stay
    // cheap and never spawn a server here.
    let mut project_files = 0usize;
    for (path, diagnostics) in lsp::cached_project_diagnostics(runtime, cwd) {
        if touched_keys.contains(&path) || diagnostics.is_empty() {
            continue;
        }
        if project_files >= DIAGNOSTICS_PROJECT_FILE_LIMIT
            || total >= DIAGNOSTICS_PROJECT_SCAN_LIMIT
        {
            break;
        }
        project_files += 1;
        total += diagnostics.len();
        let display = display_path(cwd, &path);
        entries.push(diagnostic_entry(&display, "project", diagnostics));
    }

    if let Some(object) = metadata.as_object_mut() {
        object.insert("diagnostics".to_string(), Value::Array(entries));
        object.insert("diagnosticsCount".to_string(), json!(total));
        object.insert(
            "diagnosticsProjectFileLimit".to_string(),
            json!(DIAGNOSTICS_PROJECT_FILE_LIMIT),
        );
        object.insert(
            "diagnosticsProjectScanLimit".to_string(),
            json!(DIAGNOSTICS_PROJECT_SCAN_LIMIT),
        );
    }

    if report_sections.is_empty() {
        None
    } else {
        Some(format!(
            "LSP errors detected in this file, please fix:\n{}",
            report_sections.join("\n")
        ))
    }
}

fn diagnostic_entry(
    display: &str,
    source: &str,
    diagnostics: Vec<LspDiagnostic>,
) -> Value {
    let errors = diagnostics
        .iter()
        .filter(|item| item.severity == "error")
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|item| item.severity == "warning")
        .count();
    json!({
        "path": display,
        "source": source,
        "errorCount": errors,
        "warningCount": warnings,
        "diagnostics": diagnostics,
    })
}

/// opencode `LSP.Diagnostic.report`: errors only, capped per file, wrapped in a
/// `<diagnostics file="...">` block.
fn error_report(display: &str, diagnostics: &[LspDiagnostic]) -> Option<String> {
    let errors = diagnostics
        .iter()
        .filter(|item| item.severity == "error")
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return None;
    }
    let mut lines = errors
        .iter()
        .take(MAX_REPORTED_ERRORS_PER_FILE)
        .map(|item| pretty(item))
        .collect::<Vec<_>>();
    if errors.len() > MAX_REPORTED_ERRORS_PER_FILE {
        lines.push(format!(
            "... and {} more",
            errors.len() - MAX_REPORTED_ERRORS_PER_FILE
        ));
    }
    Some(format!(
        "<diagnostics file=\"{display}\">\n{}\n</diagnostics>",
        lines.join("\n")
    ))
}

/// opencode `LSP.Diagnostic.pretty`: `SEVERITY [line:col] message` with 1-based
/// coordinates.
fn pretty(diagnostic: &LspDiagnostic) -> String {
    let (line, col) = diagnostic
        .range
        .as_ref()
        .map(|range| (range.start.line + 1, range.start.character + 1))
        .unwrap_or((1, 1));
    let severity = match diagnostic.severity.as_str() {
        "error" => "ERROR",
        "warning" => "WARN",
        "information" => "INFO",
        "hint" => "HINT",
        _ => "ERROR",
    };
    format!("{severity} [{line}:{col}] {}", diagnostic.message)
}

fn resolve_existing(
    cwd: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in paths {
        let query = if path.is_absolute() {
            path
        } else {
            cwd.join(&path)
        };
        if query.is_file() && seen.insert(query.clone()) {
            out.push(query);
        }
    }
    out
}

fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}
