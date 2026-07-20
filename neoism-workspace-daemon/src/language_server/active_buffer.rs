use std::path::Path;

use neoism_agent_server::language_server;
use neoism_protocol::{
    diagnostics::{DiagnosticItem, LspState},
    editor::{EditorServerMessage, LspSnapshotServer},
};

use crate::nvim::{DiagnosticsFetch, NvimSessionHandle};

#[derive(Debug, Clone)]
struct LanguageServerSnapshot {
    diagnostics: Vec<DiagnosticItem>,
    states: Vec<(String, LspState)>,
    /// Servers relevant to the active buffer's language, mapped for the
    /// status-bar pill / popup (`EditorServerMessage::LspSnapshot`).
    servers: Vec<LspSnapshotServer>,
    /// The active buffer's language id (empty when no bundled spec claims
    /// its extension).
    filetype: String,
    /// Active buffer path, used as the `file_path` on the desktop
    /// `EditorServerMessage::Diagnostics`.
    file: std::path::PathBuf,
}

/// One poll of the Neoism LSP runtime for the active buffer, producing BOTH
/// the diagnostics fetch (web pill / diagnostics gutter) and the editor
/// `LspSnapshot` message (desktop status-bar pill + popup). Reads the
/// buffer and queries the engine once so the two surfaces stay in sync
/// without a double round-trip. Returns no messages when there is no
/// file-backed active buffer.
pub(crate) async fn poll(
    session: &NvimSessionHandle,
    workspace_root: &Path,
) -> (Option<DiagnosticsFetch>, Vec<EditorServerMessage>) {
    let Some(snapshot) = snapshot(session, workspace_root).await else {
        return (None, Vec::new());
    };
    let snapshot_message = EditorServerMessage::LspSnapshot {
        surface_id: None,
        file_path: Some(snapshot.file.clone()),
        filetype: snapshot.filetype,
        servers: snapshot.servers,
    };
    let diagnostics_message = diagnostics_message(&snapshot.diagnostics, &snapshot.file);
    let fetch =
        DiagnosticsFetch::from_parts(Some(snapshot.diagnostics), Some(snapshot.states));
    // Diagnostics are pushed immediately through `subscribe_diagnostics`, but
    // also replayed here from the active-buffer cache so a missed/early push
    // recovers on the next poll instead of leaving stale inline errors.
    (Some(fetch), vec![snapshot_message, diagnostics_message])
}

/// Subscribe to the engine's real-time `publishDiagnostics` bus. The socket
/// loop drains this and forwards to the editor with zero polling.
pub(crate) fn subscribe_diagnostics(
) -> tokio::sync::broadcast::Receiver<language_server::DiagnosticsEvent> {
    language_server::subscribe_diagnostics()
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

pub(super) async fn read_active_file_buffer(
    session: &NvimSessionHandle,
) -> Result<crate::nvim::BufferText, String> {
    match session.read_active_buffer().await {
        Ok(Some(buffer)) if !buffer.path.as_os_str().is_empty() => Ok(buffer),
        Ok(Some(_)) | Ok(None) => {
            Err("Neoism LSP needs a file-backed active buffer".to_string())
        }
        Err(error) => Err(format!("Neoism LSP active buffer read failed: {error}")),
    }
}

async fn snapshot(
    session: &NvimSessionHandle,
    workspace_root: &Path,
) -> Option<LanguageServerSnapshot> {
    let buffer = match session.poll_active_buffer().await {
        Ok(Some(buffer)) => buffer,
        Ok(None) => return None,
        Err(error) => {
            tracing::debug!(error = %error, "language-server active buffer read failed");
            return None;
        }
    };
    if buffer.path.as_os_str().is_empty() {
        return None;
    }

    // didOpen/didChange are fed directly by OpenBuffer + nvim's on_lines/CRDT
    // event stream. This poll is only a status/cache recovery snapshot; it
    // never reads or transmits document text and is not part of edit latency.
    let diagnostics = if buffer.too_large {
        Vec::new()
    } else {
        language_server::cached_diagnostics(workspace_root, &buffer.path)
    };
    let statuses = language_server::status(workspace_root, Some(buffer.path.as_path()));
    let filetype = language_server::language_id_for_path_in(workspace_root, &buffer.path)
        .unwrap_or_default();

    // States feed the diagnostics-gutter/web pill and cover every
    // workspace server; the popup `servers` are narrowed to the ones that
    // handle the open buffer's language so the pill reflects "active for
    // this file".
    let states = statuses
        .iter()
        .map(|status| (status.id.clone(), map_lsp_state(status.status.clone())))
        .collect();
    let mut servers: Vec<LspSnapshotServer> = statuses
        .iter()
        .filter(|status| filetype.is_empty() || status.language == filetype)
        .map(map_snapshot_server)
        .collect();
    if buffer.too_large {
        let limit_mib = crate::nvim::MAX_LSP_DOCUMENT_BYTES / (1024 * 1024);
        let size_mib = buffer.byte_len as f64 / (1024.0 * 1024.0);
        for server in &mut servers {
            server.state = "disabled".to_string();
            server.message = Some(format!(
                "Large-file mode: {:.1} MiB document exceeds the {limit_mib} MiB LSP limit",
                size_mib
            ));
        }
    }

    let diagnostics: Vec<DiagnosticItem> =
        diagnostics.into_iter().map(map_diagnostic).collect();
    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
        eprintln!(
            "neoism::lsp snapshot: file={} filetype={} bytes={} large={} servers={} diagnostics={} states={:?}",
            buffer.path.display(),
            filetype,
            buffer.byte_len,
            buffer.too_large,
            servers.len(),
            diagnostics.len(),
            servers.iter().map(|s| format!("{}:{}", s.name, s.state)).collect::<Vec<_>>(),
        );
    }

    Some(LanguageServerSnapshot {
        diagnostics,
        states,
        servers,
        filetype,
        file: buffer.path.clone(),
    })
}

/// Map an engine `LspStatus` onto the wire `LspSnapshotServer` the
/// status-bar popup renders, carrying the resolution source
/// (extension/config/path/missing) so the popup can show where the binary
/// came from.
fn map_snapshot_server(status: &language_server::LspStatus) -> LspSnapshotServer {
    LspSnapshotServer {
        name: status.name.clone(),
        binary: status.command.first().cloned().unwrap_or_default(),
        filetype: status.language.clone(),
        state: snapshot_state_label(&status.status, &status.command_source).to_string(),
        source: Some(command_source_label(&status.command_source).to_string()),
        message: status.detected.message.clone(),
        level: None,
    }
}

/// Popup state label (mirrors `lsp_popup::LspServerState::from_str`). The
/// engine status already identifies the exact workspace/project/server key;
/// never infer attachment from another client's matching language id.
fn snapshot_state_label(
    state: &language_server::LspServerState,
    source: &language_server::LspCommandSource,
) -> &'static str {
    use language_server::{LspCommandSource as C, LspServerState as S};
    match (state, source) {
        (S::Error, _) => "error",
        (_, C::Missing) => "missing",
        (S::Connected, _) => "attached",
        (S::Available, _) => "available",
    }
}

fn command_source_label(source: &language_server::LspCommandSource) -> &'static str {
    match source {
        language_server::LspCommandSource::BuiltIn => "built-in/socket",
        language_server::LspCommandSource::Extension => "extension",
        language_server::LspCommandSource::Config => "config",
        language_server::LspCommandSource::Path => "path",
        language_server::LspCommandSource::Missing => "missing",
    }
}

fn map_lsp_state(state: language_server::LspServerState) -> LspState {
    match state {
        language_server::LspServerState::Available
        | language_server::LspServerState::Connected => LspState::Ready,
        language_server::LspServerState::Error => LspState::Failed {
            message: "language server unavailable".to_string(),
        },
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
