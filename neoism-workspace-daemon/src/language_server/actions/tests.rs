use super::*;
use serde_json::json;
use tempfile::TempDir;

fn text_edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> serde_json::Value {
    json!({
        "range": {
            "start": {"line": start.0, "character": start.1},
            "end": {"line": end.0, "character": end.1}
        },
        "newText": new_text
    })
}

fn buffer(path: PathBuf, text: &str) -> crate::nvim::BufferText {
    crate::nvim::BufferText {
        path,
        text: text.to_string(),
        cursor_line: 0,
        cursor_col: 0,
    }
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .expect("absolute fixture path")
        .into()
}

#[test]
fn applies_format_edits_from_bottom_to_top() {
    let mut text = "one\ntwo\nthree".to_string();
    apply_lsp_text_edits(
        &mut text,
        &[
            json!({
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}},
                "newText": "ONE"
            }),
            json!({
                "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 5}},
                "newText": "THREE"
            }),
        ],
    )
    .unwrap();

    assert_eq!(text, "ONE\ntwo\nTHREE");
}

#[test]
fn rejects_overlapping_format_edits() {
    let mut text = "abcdef".to_string();
    let original = text.clone();
    let error = apply_lsp_text_edits(
        &mut text,
        &[
            json!({
                "range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 4}},
                "newText": "X"
            }),
            json!({
                "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 5}},
                "newText": "Y"
            }),
        ],
    )
    .unwrap_err();

    assert!(error.contains("overlap"));
    assert_eq!(text, original, "validation must precede mutation");
}

#[test]
fn applies_unicode_byte_columns_without_splitting_codepoints() {
    let mut text = "aé🙂z".to_string();
    apply_lsp_text_edits(
        &mut text,
        &[
            text_edit((0, 1), (0, 3), "E"),
            text_edit((0, 3), (0, 7), "emoji"),
        ],
    )
    .unwrap();

    assert_eq!(text, "aEemojiz");

    let original = "aé".to_string();
    let mut invalid = original.clone();
    let error = apply_lsp_text_edits(&mut invalid, &[text_edit((0, 2), (0, 3), "x")])
        .unwrap_err();
    assert!(error.contains("splits utf-8 codepoint"));
    assert_eq!(invalid, original);
}

#[test]
fn preserves_same_position_insert_order() {
    let mut text = "tail".to_string();
    apply_lsp_text_edits(
        &mut text,
        &[
            text_edit((0, 0), (0, 0), "first-"),
            text_edit((0, 0), (0, 0), "second-"),
        ],
    )
    .unwrap();

    assert_eq!(text, "first-second-tail");
}

#[test]
fn handles_crlf_line_endings_and_rejects_nonexistent_final_line() {
    let mut text = "one\r\ntwo".to_string();
    apply_lsp_text_edits(&mut text, &[text_edit((0, 3), (0, 3), "!")]).unwrap();
    assert_eq!(text, "one!\r\ntwo");

    let error = offset_for_lsp_position(&text, 2, 0).unwrap_err();
    assert!(error.contains("past document end"));
}

#[test]
fn summarizes_code_action_titles() {
    let actions = selectable_code_actions(
        Path::new("/workspace/main.rs"),
        "fn main() {}\n",
        &[json!({
            "language": "fixture-lsp",
            "actions": [
                {"title": "Import foo", "kind": "quickfix", "isPreferred": true},
                {"title": "Create function", "disabled": {"reason": "not available here"}}
            ]
        })],
    );
    let summary = format_code_action_summary(&actions);

    assert_eq!(
        summary,
        "Neoism LSP found 2 code actions: Import foo, Create function"
    );
    assert_eq!(actions[0].server_id, "fixture-lsp");
    assert_eq!(actions[0].kind.as_deref(), Some("quickfix"));
    assert!(actions[0].preferred);
    assert_eq!(
        actions[1].disabled_reason.as_deref(),
        Some("not available here")
    );
    assert_eq!(actions[1].file_path, Path::new("/workspace/main.rs"));
    assert_eq!(
        actions[0].document_revision,
        document_revision("fn main() {}\n")
    );
}

#[test]
fn normalizes_command_union_without_forwarding_display_title() {
    assert_eq!(
        code_action_command_params(&json!({
            "title": "Command literal",
            "command": "fixture.literal",
            "arguments": [1, {"value": true}]
        }))
        .unwrap(),
        Some(json!({
            "command": "fixture.literal",
            "arguments": [1, {"value": true}]
        }))
    );
    assert_eq!(
        code_action_command_params(&json!({
            "title": "Code action",
            "command": {
                "title": "Server-only label",
                "command": "fixture.nested"
            }
        }))
        .unwrap(),
        Some(json!({
            "command": "fixture.nested",
            "arguments": []
        }))
    );
}

#[test]
fn resolves_partial_code_actions_but_never_command_literals() {
    assert!(should_resolve_code_action(&json!({
        "title": "Eager command, lazy edit",
        "command": {
            "title": "Finish",
            "command": "fixture.finish"
        },
        "data": { "resolve": 1 }
    })));
    assert!(should_resolve_code_action(&json!({
        "title": "Eager edit, lazy command",
        "edit": { "changes": {} },
        "data": { "resolve": 2 }
    })));
    assert!(should_resolve_code_action(&json!({
        "title": "Fully lazy",
        "data": { "resolve": 3 }
    })));
    assert!(!should_resolve_code_action(&json!({
        "title": "Command literal",
        "command": "fixture.literal",
        "arguments": []
    })));
}

#[test]
fn rejects_malformed_code_action_commands() {
    let error = code_action_command_params(&json!({
        "title": "Bad",
        "command": {"title": "Missing command"}
    }))
    .unwrap_err();
    assert!(error.contains("missing `command`"));

    let error = code_action_command_params(&json!({
        "title": "Bad args",
        "command": "fixture.bad",
        "arguments": "not-an-array"
    }))
    .unwrap_err();
    assert!(error.contains("arguments are not an array"));
}

#[test]
fn document_revision_changes_for_every_live_edit() {
    let before = document_revision("let value = missing;\n");
    let after = document_revision("let value = fixed;\n");

    assert_ne!(before, after);
    assert_eq!(before, document_revision("let value = missing;\n"));
}

#[test]
fn extracts_current_file_code_action_changes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("main.rs");
    let uri = file_uri(&path);
    let edits = workspace_edits(
        &json!({
            "changes": {
                (uri): [
                    {
                        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                        "newText": "use foo;\n"
                    }
                ]
            }
        }),
    )
    .unwrap();

    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].path, path);
    assert_eq!(edits[0].edits.len(), 1);
}

#[test]
fn extracts_cross_file_code_action_changes() {
    let temp = TempDir::new().unwrap();
    let main = temp.path().join("main.rs");
    let other = temp.path().join("other.rs");
    let edits = workspace_edits(&json!({
        "changes": {
            (file_uri(&main)): [],
            (file_uri(&other)): []
        }
    }))
    .unwrap();

    assert!(edits.iter().any(|document| document.path == main));
    assert!(edits.iter().any(|document| document.path == other));
}

#[test]
fn decodes_spaces_unicode_and_literal_percent_in_file_uris() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("space 世界 100%.gd");
    fs::write(&path, "extends Node\n").unwrap();
    let uri = file_uri(&path);

    assert!(uri.contains("%20"));
    assert!(uri.contains("%25"));
    assert_eq!(path_for_lsp_uri(&uri).unwrap(), path);
}

#[test]
fn rejects_non_file_workspace_edit_uris() {
    let error = path_for_lsp_uri("https://example.com/main.rs").unwrap_err();
    assert!(error.contains("only file URIs"));
}

#[test]
fn parses_ordered_text_document_changes_and_rejects_resource_operations() {
    let temp = TempDir::new().unwrap();
    let uri = file_uri(&temp.path().join("main.rs"));
    let edits = workspace_edits(&json!({
        "documentChanges": [
            {
                "textDocument": {"uri": uri, "version": 7},
                "edits": [text_edit((0, 0), (0, 0), "one")]
            },
            {
                "textDocument": {"uri": uri, "version": 8},
                "edits": [text_edit((0, 3), (0, 3), "two")]
            }
        ]
    }))
    .unwrap();

    assert_eq!(edits.len(), 2, "ordered edits must not be flattened");

    let error = workspace_edits(&json!({
        "documentChanges": [{"kind": "create", "uri": "file:///tmp/new.rs"}]
    }))
    .unwrap_err();
    assert!(error.contains("resource operation `create` is not supported"));
}

#[test]
fn prepares_document_changes_sequentially() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("main.rs");
    fs::write(&path, "tail").unwrap();
    let buffer = buffer(path.clone(), "tail");
    let edits = vec![
        DocumentEdit {
            path: path.clone(),
            edits: vec![text_edit((0, 0), (0, 0), "one")],
        },
        DocumentEdit {
            path: path.clone(),
            // This offset is valid only after the first TextDocumentEdit.
            edits: vec![text_edit((0, 3), (0, 3), "two")],
        },
    ];

    let prepared = prepare_workspace_edits(temp.path(), &buffer, edits).unwrap();
    let document = prepared.documents.values().next().unwrap();
    assert!(document.active);
    assert_eq!(document.text, "onetwotail");
    assert_eq!(prepared.edit_count, 2);
    assert_eq!(fs::read_to_string(path).unwrap(), "tail");
}

#[test]
fn active_buffer_text_is_authoritative_during_preparation() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("main.rs");
    fs::write(&path, "stale-on-disk").unwrap();
    let buffer = buffer(path.clone(), "unsaved-buffer");
    let edits = vec![DocumentEdit {
        path: path.clone(),
        edits: vec![text_edit((0, 0), (0, 7), "saved")],
    }];

    let prepared = prepare_workspace_edits(temp.path(), &buffer, edits).unwrap();
    let document = prepared.documents.values().next().unwrap();
    assert!(document.active);
    assert_eq!(document.text, "saved-buffer");
    assert_eq!(fs::read_to_string(path).unwrap(), "stale-on-disk");
}

#[test]
fn supports_an_unsaved_active_file_without_allowing_other_file_creation() {
    let temp = TempDir::new().unwrap();
    let active_path = temp.path().join("new-file.rs");
    let buffer = buffer(active_path.clone(), "unsaved");
    let prepared = prepare_workspace_edits(
        temp.path(),
        &buffer,
        vec![DocumentEdit {
            path: active_path.clone(),
            edits: vec![text_edit((0, 0), (0, 7), "edited")],
        }],
    )
    .unwrap();

    let document = prepared.documents.values().next().unwrap();
    assert!(document.active);
    assert_eq!(document.text, "edited");
    assert!(!active_path.exists());

    let other_path = temp.path().join("server-created.rs");
    let error = prepare_workspace_edits(
        temp.path(),
        &buffer,
        vec![DocumentEdit {
            path: other_path,
            edits: vec![text_edit((0, 0), (0, 0), "not allowed")],
        }],
    )
    .unwrap_err();
    assert!(error.contains("does not exist and is not the active buffer"));
}

#[test]
fn validates_all_files_before_external_mutation() {
    let temp = TempDir::new().unwrap();
    let active_path = temp.path().join("active.rs");
    let other_path = temp.path().join("other.rs");
    fs::write(&active_path, "active").unwrap();
    fs::write(&other_path, "other").unwrap();
    let buffer = buffer(active_path.clone(), "active");
    let edits = vec![
        DocumentEdit {
            path: active_path.clone(),
            edits: vec![text_edit((0, 0), (0, 6), "changed")],
        },
        DocumentEdit {
            path: other_path.clone(),
            edits: vec![text_edit((0, 99), (0, 99), "invalid")],
        },
    ];

    let error = prepare_workspace_edits(temp.path(), &buffer, edits).unwrap_err();
    assert!(error.contains("past line end"));
    assert_eq!(fs::read_to_string(active_path).unwrap(), "active");
    assert_eq!(fs::read_to_string(other_path).unwrap(), "other");
}

#[test]
fn rejects_percent_encoded_parent_escape() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let active_path = root.join("active.rs");
    let outside_path = parent.path().join("outside.rs");
    fs::write(&active_path, "active").unwrap();
    fs::write(&outside_path, "outside").unwrap();
    let buffer = buffer(active_path, "active");
    let root_uri = file_uri(&root);
    let uri = format!("{}/%2e%2e/outside.rs", root_uri.trim_end_matches('/'));
    let edits = workspace_edits(&json!({
        "changes": {(uri): [text_edit((0, 0), (0, 0), "escape")]}
    }))
    .unwrap();

    let error = prepare_workspace_edits(&root, &buffer, edits).unwrap_err();
    assert!(error.contains("escapes workspace root"));
    assert_eq!(fs::read_to_string(outside_path).unwrap(), "outside");
}

#[cfg(unix)]
#[test]
fn rejects_symlink_target_that_escapes_workspace() {
    use std::os::unix::fs::symlink;

    let parent = TempDir::new().unwrap();
    let root = parent.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let active_path = root.join("active.rs");
    let outside_path = parent.path().join("outside.rs");
    let link_path = root.join("linked.rs");
    fs::write(&active_path, "active").unwrap();
    fs::write(&outside_path, "outside").unwrap();
    symlink(&outside_path, &link_path).unwrap();
    let buffer = buffer(active_path, "active");
    let edits = vec![DocumentEdit {
        path: link_path,
        edits: vec![text_edit((0, 0), (0, 0), "escape")],
    }];

    let error = prepare_workspace_edits(&root, &buffer, edits).unwrap_err();
    assert!(error.contains("escapes workspace root"));
    assert_eq!(fs::read_to_string(outside_path).unwrap(), "outside");
}

#[test]
fn rename_requires_non_empty_name() {
    let error = rename_name(Some("   ")).unwrap_err();

    assert_eq!(error, "missing new name");
}

fn symbol(
    name: &str,
    kind: &str,
    line: u32,
    children: Vec<language_server::LspDocumentSymbol>,
) -> language_server::LspDocumentSymbol {
    language_server::LspDocumentSymbol {
        name: name.to_string(),
        kind: kind.to_string(),
        detail: None,
        path: String::new(),
        range: None,
        selection_range: Some(language_server::LspRange {
            start: language_server::LspPosition { line, character: 4 },
            end: language_server::LspPosition { line, character: 8 },
        }),
        children,
        language: None,
    }
}

#[test]
fn flattens_symbol_tree_depth_first_with_indentation_depth() {
    let tree = vec![
        symbol(
            "Config",
            "struct",
            10,
            vec![symbol("new", "method", 12, Vec::new())],
        ),
        symbol("main", "function", 30, Vec::new()),
    ];

    let flat = flatten_document_symbols(&tree, Path::new("/tmp/main.rs"), 0);

    let shape: Vec<(&str, u32, u32)> = flat
        .iter()
        .map(|s| (s.name.as_str(), s.depth, s.line))
        .collect();
    assert_eq!(
        shape,
        vec![("Config", 0, 10), ("new", 1, 12), ("main", 0, 30)]
    );
    // Missing per-symbol path falls back to the active buffer path so
    // the picker can still open the location.
    assert_eq!(flat[0].uri, "/tmp/main.rs");
    // Jump target is the selection range start (the symbol name), not
    // column 0 of the enclosing block.
    assert_eq!(flat[0].character, 4);
}

fn completion_item(path: &Path, text: &str) -> EditorLspCompletionItem {
    EditorLspCompletionItem {
        server_id: Some("fixture-lsp".to_string()),
        file_path: path.to_path_buf(),
        document_revision: document_revision(text),
        label: "details".to_string(),
        kind: "property".to_string(),
        detail: None,
        documentation: None,
        insert_text: "details".to_string(),
        filter_text: None,
        sort_text: None,
        preselect: false,
        payload: None,
    }
}

#[test]
fn completion_insert_replace_edit_uses_insert_range_and_tracks_cursor_through_import() {
    let path = PathBuf::from("/tmp/main.ts");
    let text = "import z;\nconst d = obj.detSuffix;";
    let buffer = crate::nvim::BufferText {
        path: path.clone(),
        text: text.to_string(),
        cursor_line: 1,
        cursor_col: 17,
    };
    let selected = completion_item(&path, text);
    let payload = json!({
        "label": "details",
        "textEdit": {
            "insert": {
                "start": {"line": 1, "character": 14},
                "end": {"line": 1, "character": 17}
            },
            "replace": {
                "start": {"line": 1, "character": 14},
                "end": {"line": 1, "character": 23}
            },
            "newText": "details"
        },
        "additionalTextEdits": [{
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 0}
            },
            "newText": "import x;\n"
        }]
    });
    let marker = completion_cursor_marker(text, &payload);
    let mut edits = completion_text_edits(&buffer, &selected, "det", &payload, &marker)
        .expect("valid completion edit");
    edits.extend(
        payload["additionalTextEdits"]
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    let mut result = text.to_string();
    apply_lsp_text_edits(&mut result, &edits).unwrap();
    let offset = result.find(&marker).unwrap();
    result.replace_range(offset..offset + marker.len(), "");

    assert_eq!(result, "import x;\nimport z;\nconst d = obj.detailsSuffix;");
    assert_eq!(lsp_position_for_offset(&result, offset).unwrap(), (2, 21));
}

#[test]
fn completion_plain_insert_replaces_exact_utf8_prefix() {
    let path = PathBuf::from("/tmp/main.rs");
    let text = "é det";
    let buffer = crate::nvim::BufferText {
        path: path.clone(),
        text: text.to_string(),
        cursor_line: 0,
        cursor_col: 6,
    };
    let selected = completion_item(&path, text);
    let payload = json!({"label": "details", "insertText": "details"});
    let marker = completion_cursor_marker(text, &payload);
    let edits = completion_text_edits(&buffer, &selected, "det", &payload, &marker)
        .expect("valid byte prefix");
    let mut result = text.to_string();
    apply_lsp_text_edits(&mut result, &edits).unwrap();
    let offset = result.find(&marker).unwrap();
    result.replace_range(offset..offset + marker.len(), "");

    assert_eq!(result, "é details");
    assert_eq!(lsp_position_for_offset(&result, offset).unwrap(), (0, 10));
}

#[test]
fn completion_rejects_unadvertised_snippet_and_indent_modes() {
    assert!(reject_unsupported_completion_modes(&json!({
        "insertTextFormat": 2
    }))
    .unwrap_err()
    .contains("snippet"));
    assert!(reject_unsupported_completion_modes(&json!({
        "insertTextMode": 2
    }))
    .unwrap_err()
    .contains("adjustIndentation"));
}

#[test]
fn completion_skips_only_additional_edits_overlapping_primary() {
    let text = "fo\nbar";
    let primary = text_edit((0, 0), (0, 2), "foo");
    let additional = vec![
        text_edit((0, 1), (0, 2), "XXX"),
        text_edit((1, 0), (1, 3), "baz"),
    ];

    let mut edits =
        non_overlapping_completion_edits(text, &primary, &additional).unwrap();
    edits.push(primary);
    let mut result = text.to_string();
    apply_lsp_text_edits(&mut result, &edits).unwrap();

    assert_eq!(result, "foo\nbaz");
}

#[test]
fn completion_file_start_auto_import_precedes_primary_insert() {
    let text = "";
    let primary = text_edit((0, 0), (0, 0), "foo");
    let additional = vec![text_edit((0, 0), (0, 0), "import { foo };\n")];

    let mut edits =
        non_overlapping_completion_edits(text, &primary, &additional).unwrap();
    edits.push(primary);
    let mut result = text.to_string();
    apply_lsp_text_edits(&mut result, &edits).unwrap();

    assert_eq!(result, "import { foo };\nfoo");
}
