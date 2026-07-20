// POD translation helpers between the backend `neoism_backend::performer::nvim`
// notification shapes and the shared `neoism_ui::editor_snapshot` POD shapes
// the lifted chrome panels consume.
//
// These live at the native frontend's edge so the shared `neoism-ui` crate
// stays backend-free. Web frontends do the same conversion off the wire
// payload — same destination shape, different source.

use neoism_backend::performer::nvim::{
    DiagnosticItem as NvimDiagnosticItem, MinimapGitChange as NvimMinimapGitChange,
    MinimapNotification,
};
use neoism_ui::editor_snapshot::{
    DiagnosticItem as SnapshotDiagnosticItem, DiagnosticSeverity, MinimapData,
    MinimapGitChange as SnapshotMinimapGitChange,
};

/// Lift a backend `MinimapNotification` into the POD `MinimapData` the
/// shared minimap panel expects.
pub fn minimap_data_from_notification(n: MinimapNotification) -> MinimapData {
    MinimapData {
        path: n.path,
        changedtick: n.changedtick,
        total_lines: n.total_lines,
        top_line: n.top_line,
        bottom_line: n.bottom_line,
        cursor_line: n.cursor_line,
        sample_stride: n.sample_stride,
        lines: n.lines,
        git_changes: n
            .git_changes
            .into_iter()
            .map(minimap_git_change_from_nvim)
            .collect(),
    }
}

/// Lift a single backend `MinimapGitChange` into the POD shape.
pub fn minimap_git_change_from_nvim(c: NvimMinimapGitChange) -> SnapshotMinimapGitChange {
    SnapshotMinimapGitChange {
        line: c.line,
        kind: c.kind,
    }
}

/// Lift a backend `DiagnosticItem` into the POD shape the shared
/// diagnostics popup consumes. The backend item carries only
/// `lnum: u64`, `severity: u8`, `message: String`, and optional
/// source. The rest of the snapshot POD fields are filled in with
/// sensible defaults.
pub fn diagnostic_item_from_nvim(d: &NvimDiagnosticItem) -> SnapshotDiagnosticItem {
    SnapshotDiagnosticItem {
        severity: DiagnosticSeverity::from_u8(d.severity),
        message: d.message.clone(),
        source: d.source.clone(),
        // The shared POD shape uses `u32`; the popup's `From<&_>` widens
        // back to `u64` at the next boundary. `line` (0-based) is best-
        // effort derived from `lnum` (1-based).
        line: d.lnum.saturating_sub(1).min(u32::MAX as u64) as u32,
        col: d.col.min(u32::MAX as u64) as u32,
        end_line: d.end_line.min(u32::MAX as u64) as u32,
        end_col: d.end_col.min(u32::MAX as u64) as u32,
        lnum: d.lnum.min(u32::MAX as u64) as u32,
        code: d.code.clone(),
        code_description: d.code_description.clone(),
        tags: d.tags.clone(),
        related_information: d
            .related_information
            .iter()
            .map(|related| {
                neoism_ui::editor_snapshot::DiagnosticRelatedInformation {
                    path: related.path.clone(),
                    line: related.line,
                    col: related.col,
                    end_line: related.end_line,
                    end_col: related.end_col,
                    message: related.message.clone(),
                }
            })
            .collect(),
    }
}
