use std::path::Path;

use neoism_agent_server::language_server;
use neoism_protocol::{diagnostics::DiagnosticItem, editor::EditorServerMessage};

// The active-buffer snapshot poll (`poll`/`read_active_file_buffer`) was fed
// by the embedded nvim session and was deleted with it. The engine's
// event-driven `publishDiagnostics` bus below is editor-agnostic and remains
// the live diagnostics path; the active-buffer text source returns with the
// native editor.

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
