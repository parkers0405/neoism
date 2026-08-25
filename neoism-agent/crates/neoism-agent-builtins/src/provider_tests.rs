use super::provider_openai_stream::{
    handle_tool_call_deltas, openai_key, parse_stream_line,
};
use std::collections::BTreeMap;

use neoism_agent_core::{AuthInfo, ProviderStreamEvent};

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
