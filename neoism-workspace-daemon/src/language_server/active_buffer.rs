use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use neoism_agent_server::language_server;
use neoism_protocol::{diagnostics::DiagnosticItem, editor::EditorServerMessage};

/// Last authoritative buffer text per file, as synced by the native
/// editor through `OpenBuffer`. Interactive queries (completion) and
/// reference-row previews read this so they reflect unsaved edits
/// instead of stale disk content.
fn live_text_store() -> &'static Mutex<HashMap<PathBuf, String>> {
    static STORE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// The text last synced for `file` (None when the editor never opened
/// it on this daemon).
pub(crate) fn live_buffer_text(file: &Path) -> Option<String> {
    live_text_store()
        .lock()
        .ok()
        .and_then(|store| store.get(file).cloned())
}

/// Queue the native editor's authoritative text into the host LSP —
/// the NON-BLOCKING half of the old `sync_buffer_snapshot`: a cache
/// insert plus a channel send onto the per-document FIFO worker, safe
/// to run inline in the socket loop (a cold server spawn can no longer
/// stall PTY forwarding behind a keystroke sync). Because it runs
/// inline, queue order matches socket order — interactive queries that
/// later `flush_document_sync` are guaranteed to see this text.
pub(crate) fn queue_buffer_sync(runtime: &language_server::LspRuntime, workspace_root: &Path, file: &Path, text: String) {
    if let Ok(mut store) = live_text_store().lock() {
        store.insert(file.to_path_buf(), text.clone());
    }
    super::live_sync::sync_document(runtime, workspace_root, file, text);
}

/// The BLOCKING half: wait for the queued sync to reach the engine,
/// then build the `LspSnapshot` status message. Run on a blocking
/// task. Returns `None` when the per-file snapshot throttle says the
/// last one is fresh enough (`language_server::status` walks the
/// workspace — desktop throttles the same way in `refresh_lsp_pill`).
pub(crate) fn buffer_snapshot_message(
    runtime: &language_server::LspRuntime,
    workspace_root: &Path,
    file: &Path,
    surface_id: Option<String>,
) -> Option<EditorServerMessage> {
    {
        static LAST_SNAPSHOT: OnceLock<Mutex<HashMap<PathBuf, std::time::Instant>>> =
            OnceLock::new();
        let mut last = match LAST_SNAPSHOT.get_or_init(Default::default).lock() {
            Ok(last) => last,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = std::time::Instant::now();
        match last.get(file) {
            Some(at) if now.duration_since(*at).as_secs_f32() < 3.0 => return None,
            _ => {
                last.insert(file.to_path_buf(), now);
            }
        }
    }
    super::live_sync::flush_document_sync(runtime, workspace_root, file);
    let statuses = language_server::status(runtime, workspace_root, Some(file));
    let filetype = language_server::language_id_for_path_in(runtime, workspace_root, file)
        .unwrap_or_default();
    let servers = statuses
        .into_iter()
        .map(|status| {
            use neoism_agent_server::language_server::{
                LspCommandSource, LspServerState,
            };
            neoism_protocol::editor::LspSnapshotServer {
                name: status.name,
                binary: status.command.first().cloned().unwrap_or_default(),
                filetype: status.language,
                state: match status.status {
                    LspServerState::Connected => "connected",
                    LspServerState::Available => "available",
                    LspServerState::Error => "error",
                }
                .to_string(),
                source: Some(
                    match status.command_source {
                        LspCommandSource::BuiltIn => "built-in",
                        LspCommandSource::Extension => "managed",
                        LspCommandSource::Config => "config",
                        LspCommandSource::Path => "path",
                        LspCommandSource::Missing => "missing",
                    }
                    .to_string(),
                ),
                message: None,
                level: None,
            }
        })
        .collect();
    Some(EditorServerMessage::LspSnapshot {
        surface_id,
        file_path: Some(file.to_path_buf()),
        filetype,
        servers,
    })
}

// The active-buffer snapshot poll (`poll`/`read_active_file_buffer`) was fed
// by the embedded nvim session and was deleted with it. The engine's
// event-driven `publishDiagnostics` bus below is editor-agnostic and remains
// the live diagnostics path; the active-buffer text source returns with the
// native editor.

/// Subscribe to the engine's real-time `publishDiagnostics` bus. The socket
/// loop drains this and forwards to the editor with zero polling.
pub(crate) fn subscribe_diagnostics(
    runtime: &language_server::LspRuntime,
) -> tokio::sync::broadcast::Receiver<language_server::DiagnosticsEvent> {
    language_server::subscribe_diagnostics(runtime)
}

/// Convert an engine diagnostics push into the editor message.
pub(crate) fn diagnostics_event_message(
    event: language_server::DiagnosticsEvent,
) -> EditorServerMessage {
    let diagnostics: Vec<DiagnosticItem> =
        event.diagnostics.into_iter().map(map_diagnostic).collect();
    diagnostics_message(&diagnostics, std::path::Path::new(&event.file))
}

/// The file a diagnostics push is for (so the socket loop can drop pushes for
/// buffers other than the active one).
pub(crate) fn diagnostics_event_file(event: &language_server::DiagnosticsEvent) -> &str {
    &event.file
}

/// Build the desktop inline-diagnostics message (`EditorServerMessage::
/// Diagnostics`) from the engine's diagnostics for the active buffer,
/// tallying severities the way nvim's `rio_diagnostics` used to.
fn diagnostics_message(
    diagnostics: &[DiagnosticItem],
    file: &Path,
) -> EditorServerMessage {
    use neoism_protocol::editor::{
        DiagnosticItem as EditorDiagnostic, DiagnosticSeverity,
    };
    let (mut error, mut warn, mut info, mut hint) = (0u64, 0u64, 0u64, 0u64);
    let items = diagnostics
        .iter()
        .map(|diagnostic| {
            match diagnostic.severity {
                1 => error += 1,
                2 => warn += 1,
                3 => info += 1,
                _ => hint += 1,
            }
            EditorDiagnostic {
                severity: DiagnosticSeverity::from_u8(diagnostic.severity),
                message: diagnostic.message.clone(),
                source: diagnostic.source.clone(),
                line: diagnostic.line,
                col: diagnostic.col,
                end_line: diagnostic.end_line,
                end_col: diagnostic.end_col,
                lnum: diagnostic.line.saturating_add(1),
                code: diagnostic.code.clone(),
                code_description: diagnostic.code_description.clone(),
                tags: diagnostic.tags.clone(),
                related_information: diagnostic.related_information.clone(),
            }
        })
        .collect();
    EditorServerMessage::Diagnostics {
        surface_id: None,
        error,
        warn,
        info,
        hint,
        file_path: Some(file.to_path_buf()),
        items,
    }
}

fn map_diagnostic(diagnostic: language_server::LspDiagnostic) -> DiagnosticItem {
    let range = diagnostic.range.unwrap_or(language_server::LspRange {
        start: language_server::LspPosition {
            line: 0,
            character: 0,
        },
        end: language_server::LspPosition {
            line: 0,
            character: 0,
        },
    });
    DiagnosticItem {
        // Public LSP positions are normalized to 1-based display coordinates
        // by `parse_lsp_position`. `DiagnosticItem` is the daemon's internal
        // zero-based representation; `diagnostics_message` adds one exactly
        // once when it fills the legacy `lnum` field consumed by the desktop.
        // Keeping the conversion here prevents popup/inline rows drifting one
        // line below the server's actual diagnostic.
        line: range.start.line.saturating_sub(1),
        col: range.start.character.saturating_sub(1),
        end_line: range.end.line.saturating_sub(1),
        end_col: range.end.character.saturating_sub(1),
        severity: match diagnostic.severity.as_str() {
            "error" => 1,
            "warning" => 2,
            "information" => 3,
            "hint" => 4,
            _ => 2,
        },
        message: diagnostic.message,
        source: diagnostic.source,
        code: diagnostic.code,
        code_description: diagnostic.code_description,
        tags: diagnostic.tags,
        related_information: diagnostic
            .related_information
            .into_iter()
            .map(|related| {
                let range = related.range.unwrap_or(language_server::LspRange {
                    start: language_server::LspPosition {
                        line: 1,
                        character: 1,
                    },
                    end: language_server::LspPosition {
                        line: 1,
                        character: 1,
                    },
                });
                neoism_protocol::diagnostics::DiagnosticRelatedInformation {
                    path: related.path,
                    line: range.start.line.saturating_sub(1),
                    col: range.start.character.saturating_sub(1),
                    end_line: range.end.line.saturating_sub(1),
                    end_col: range.end.character.saturating_sub(1),
                    message: related.message,
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_display_position_is_converted_to_internal_zero_based_row_once() {
        let mapped = map_diagnostic(language_server::LspDiagnostic {
            path: "/workspace/main.rs".to_string(),
            range: Some(language_server::LspRange {
                start: language_server::LspPosition {
                    line: 50,
                    character: 9,
                },
                end: language_server::LspPosition {
                    line: 50,
                    character: 16,
                },
            }),
            severity: "error".to_string(),
            code: None,
            code_description: None,
            source: Some("fixture".to_string()),
            message: "broken".to_string(),
            tags: Vec::new(),
            related_information: Vec::new(),
            data: None,
            language: Some("fixture".to_string()),
        });

        assert_eq!(mapped.line, 49);
        assert_eq!(mapped.col, 8);

        let message = diagnostics_message(&[mapped], Path::new("/workspace/main.rs"));
        let EditorServerMessage::Diagnostics { items, .. } = message else {
            panic!("expected diagnostics message");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].line, 49);
        assert_eq!(items[0].col, 8);
        assert_eq!(items[0].lnum, 50);
    }

    #[test]
    fn empty_publication_emits_an_explicit_zero_count_clear_for_the_file() {
        let file = Path::new("/workspace/src/main.rs");
        let message = diagnostics_event_message(language_server::DiagnosticsEvent {
            root: Path::new("/workspace").to_path_buf(),
            server_id: "fixture-lsp".to_string(),
            language: "fixture".to_string(),
            file: file.to_string_lossy().into_owned(),
            diagnostics: Vec::new(),
        });

        let EditorServerMessage::Diagnostics {
            error,
            warn,
            info,
            hint,
            file_path,
            items,
            ..
        } = message
        else {
            panic!("expected diagnostics clear message");
        };
        assert_eq!((error, warn, info, hint), (0, 0, 0, 0));
        assert_eq!(file_path.as_deref(), Some(file));
        assert!(items.is_empty());
    }
}
