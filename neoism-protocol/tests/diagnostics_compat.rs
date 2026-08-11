//! Compatibility tests for diagnostics sent by older daemon versions.

use neoism_protocol::diagnostics::{DiagnosticItem, DiagnosticsServerMessage, LspState};

#[test]
fn legacy_diagnostic_defaults_new_range_and_metadata_fields() {
    let item: DiagnosticItem = serde_json::from_str(
        r#"{"line":4,"col":2,"severity":1,"message":"not found","source":"rust-analyzer"}"#,
    )
    .expect("decode legacy diagnostic");

    assert_eq!((item.end_line, item.end_col), (0, 0));
    assert_eq!(item.code, None);
    assert_eq!(item.code_description, None);
    assert!(item.tags.is_empty());
    assert!(item.related_information.is_empty());
}

#[test]
fn empty_optional_metadata_is_omitted_from_json() {
    let json = serde_json::to_string(&DiagnosticItem {
        line: 1,
        col: 2,
        end_line: 1,
        end_col: 5,
        severity: 2,
        message: "unused binding".into(),
        source: None,
        code: None,
        code_description: None,
        tags: Vec::new(),
        related_information: Vec::new(),
    })
    .expect("serialize diagnostic");

    assert!(
        !json.contains("\"code\""),
        "empty code was serialized: {json}"
    );
    assert!(
        !json.contains("code_description"),
        "empty code description was serialized: {json}"
    );
    assert!(!json.contains("tags"), "empty tags were serialized: {json}");
    assert!(
        !json.contains("related_information"),
        "empty related information was serialized: {json}"
    );
}

#[test]
fn every_lsp_state_keeps_its_external_tag() {
    let states = [
        (LspState::Starting, r#""Starting""#),
        (LspState::Ready, r#""Ready""#),
        (LspState::Indexing, r#""Indexing""#),
        (LspState::Stopped, r#""Stopped""#),
        (
            LspState::Failed {
                message: "spawn failed".into(),
            },
            r#"{"Failed":{"message":"spawn failed"}}"#,
        ),
    ];

    for (state, expected) in states {
        let json = serde_json::to_string(&state).expect("serialize LSP state");
        assert_eq!(json, expected);
    }
}

#[test]
fn diagnostics_push_retains_large_route_ids() {
    let message = DiagnosticsServerMessage::DiagnosticsCleared { route_id: u64::MAX };
    let json = serde_json::to_string(&message).expect("serialize diagnostics clear");
    let decoded: DiagnosticsServerMessage =
        serde_json::from_str(&json).expect("deserialize diagnostics clear");

    assert_eq!(decoded, message);
}
