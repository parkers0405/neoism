use super::provider_openai_stream::{
    handle_tool_call_deltas, openai_key, parse_stream_line,
};
use std::collections::BTreeMap;

use neoism_agent_core::{AuthInfo, ProviderStreamEvent};

#[test]
fn local_store_uses_host_credentials_for_matching_workspace_delegation() {
    let scope = super::generation_credential_scope(
        Some("workspace:workspace-1"),
        Some("workspace-1"),
        false,
    )
    .unwrap();

    assert_eq!(scope.tenant_id, "local");
    assert_eq!(scope.workspace_id, None);
}

#[test]
fn local_store_rejects_non_workspace_hosted_scopes() {
    assert!(super::generation_credential_scope(Some("tenant-a"), None, false).is_err());
    assert!(super::generation_credential_scope(
        Some("workspace:workspace-1"),
        Some("workspace-2"),
        false,
    )
    .is_err());
}

#[test]
fn hosted_store_preserves_workspace_scope() {
    let scope = super::generation_credential_scope(
        Some("workspace:workspace-1"),
        Some("workspace-1"),
        true,
    )
    .unwrap();

    assert_eq!(scope.tenant_id, "workspace:workspace-1");
    assert_eq!(scope.workspace_id.as_deref(), Some("workspace-1"));
}

#[test]
fn stored_openai_auth_precedes_environment_keys() {
    std::env::set_var("NEOISM_AGENT_OPENAI_API_KEY", "env-key");
    let auth = AuthInfo::Api {
        key: "stored-key".to_string(),
        metadata: None,
    };

    assert_eq!(openai_key(Some(&auth)).as_deref(), Some("stored-key"));

    std::env::remove_var("NEOISM_AGENT_OPENAI_API_KEY");
}

#[test]
fn provider_adapter_accepts_openai_compatible_catalog_families() {
    assert!(super::is_openai_compatible_npm("@ai-sdk/openai-compatible"));
    assert!(super::is_openai_compatible_npm(
        "@openrouter/ai-sdk-provider"
    ));
    assert!(super::is_openai_compatible_npm("@ai-sdk/azure"));
    assert!(super::is_openai_compatible_npm("@ai-sdk/mistral"));
    assert!(!super::is_openai_compatible_npm("@ai-sdk/anthropic"));
    assert!(!super::is_openai_compatible_npm("@ai-sdk/google"));
    assert!(super::is_anthropic_npm("@ai-sdk/anthropic"));
    assert!(!super::is_anthropic_npm("@ai-sdk/amazon-bedrock"));
}

#[tokio::test]
async fn parses_openai_streaming_chunks() {
    let first = parse_stream_line(
            br#"data: {"choices":[{"delta":{"content":"Hel","reasoning_content":"think"},"finish_reason":null}],"usage":null}"#,
        )
        .unwrap();
    assert!(!first.done);
    assert_eq!(first.deltas, vec!["Hel"]);
    assert_eq!(first.reasoning_deltas, vec!["think"]);

    let second = parse_stream_line(
            br#"data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"completion_tokens_details":{"reasoning_tokens":1}}}"#,
        )
        .unwrap();
    assert_eq!(second.deltas, vec!["lo"]);
    assert_eq!(second.finish.as_deref(), Some("stop"));
    assert_eq!(second.input_tokens, Some(3));
    assert_eq!(second.output_tokens, Some(2));
    assert_eq!(second.reasoning_tokens, Some(1));

    let done = parse_stream_line(b"data: [DONE]").unwrap();

    assert!(done.done);
}

#[test]
fn parses_openai_usage_only_and_metadata_chunks() {
    let usage = parse_stream_line(
        br#"data: {"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16,"prompt_tokens_details":{"cached_tokens":7},"completion_tokens_details":{"reasoning_tokens":2}}}"#,
    )
    .unwrap();
    assert!(usage.deltas.is_empty());
    assert_eq!(usage.total_tokens, Some(16));
    assert_eq!(usage.input_tokens, Some(12));
    assert_eq!(usage.output_tokens, Some(4));
    assert_eq!(usage.reasoning_tokens, Some(2));
    assert_eq!(usage.cache_read_tokens, Some(7));

    let metadata = parse_stream_line(
        br#"data: {"id":"chatcmpl-1","model":"provider-model","choices":[]}"#,
    )
    .unwrap();
    assert!(metadata.deltas.is_empty());
    assert!(metadata.finish.is_none());
}

#[test]
fn surfaces_openai_stream_provider_errors() {
    let error = parse_stream_line(
        br#"data: {"error":{"message":"quota exceeded","type":"rate_limit_error","code":"rate_limit"}}"#,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("quota exceeded"));
    assert!(error.contains("rate_limit_error"));
    assert!(!error.contains("missing field `choices`"));
}

#[test]
fn rejects_openai_stream_chunks_without_choices_or_error() {
    let error =
        parse_stream_line(br#"data: {"id":"chatcmpl-1","model":"provider-model"}"#)
            .unwrap_err()
            .to_string();

    assert!(error.contains("failed to decode OpenAI-compatible streaming chunk"));
}

#[test]
fn parses_openai_tool_call_deltas() {
    let first = parse_stream_line(
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":"}}]},"finish_reason":null}],"usage":null}"#,
        )
        .unwrap();
    assert_eq!(first.tool_calls.len(), 1);
    assert_eq!(first.tool_calls[0].id.as_deref(), Some("call_1"));
    assert_eq!(first.tool_calls[0].name.as_deref(), Some("read"));

    let second = parse_stream_line(
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"src/lib.rs\"}"}}]},"finish_reason":null}],"usage":null}"#,
        )
        .unwrap();
    let mut calls = BTreeMap::new();
    let first_events = handle_tool_call_deltas(&mut calls, first.tool_calls).unwrap();
    assert!(matches!(
        first_events[0],
        ProviderStreamEvent::ToolInputStart { .. }
    ));
    assert!(matches!(
        first_events[1],
        ProviderStreamEvent::ToolInputDelta { .. }
    ));
    let second_events = handle_tool_call_deltas(&mut calls, second.tool_calls).unwrap();
    assert!(matches!(
        second_events[0],
        ProviderStreamEvent::ToolInputDelta { .. }
    ));
    assert!(matches!(
        second_events[1],
        ProviderStreamEvent::ToolInputEnd { .. }
    ));
    let ProviderStreamEvent::ToolCall { id, name, input } = &second_events[2] else {
        panic!("expected tool call event")
    };
    assert_eq!(id, "call_1");
    assert_eq!(name, "read");
    assert_eq!(input["path"], "src/lib.rs");
}
