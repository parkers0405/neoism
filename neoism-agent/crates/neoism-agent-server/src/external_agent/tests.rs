use super::*;

#[test]
fn resolves_supported_external_agents() {
    assert!(matches!(
        ExternalRuntime::resolve("opencode"),
        Some(ExternalRuntime::OpenCode)
    ));
    assert!(matches!(
        ExternalRuntime::resolve("codex"),
        Some(ExternalRuntime::Codex)
    ));
    assert!(matches!(
        ExternalRuntime::resolve("claude-code"),
        Some(ExternalRuntime::Claude)
    ));
    assert!(ExternalRuntime::resolve("general").is_none());
}

#[test]
fn external_acp_configs_use_expected_launchers() {
    let services = crate::standard_services();
    let codex = ExternalRuntime::Codex.acp_config("/tmp", &services);
    assert_eq!(
        std::path::Path::new(&codex.command)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("npx")
    );
    assert_eq!(
        codex.args,
        vec!["--yes", "@zed-industries/codex-acp@latest"]
    );

    let claude = ExternalRuntime::Claude.acp_config("/tmp", &services);
    assert_eq!(
        std::path::Path::new(&claude.command)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("npx")
    );
    assert_eq!(
        claude.args,
        vec!["--yes", "@agentclientprotocol/claude-agent-acp@latest"]
    );
}

#[test]
fn maps_neoism_permission_replies_to_acp_options() {
    let options = json!([
        { "kind": "allow_always", "optionId": "allow_always" },
        { "kind": "allow_once", "optionId": "allow" },
        { "kind": "reject_once", "optionId": "reject" }
    ]);
    let ids = vec![
        "allow_always".to_string(),
        "allow".to_string(),
        "reject".to_string(),
    ];

    assert_eq!(
        select_acp_permission_option(&options, &ids, "once").as_deref(),
        Some("allow")
    );
    assert_eq!(
        select_acp_permission_option(&options, &ids, "always").as_deref(),
        Some("allow_always")
    );
    assert_eq!(
        select_acp_permission_option(&options, &ids, "reject").as_deref(),
        Some("reject")
    );
}

#[test]
fn detects_provider_owned_nested_agent_tools() {
    assert!(is_external_nested_agent_tool(
        &json!({ "kind": "think", "title": "Review code" }),
        &json!({ "prompt": "review src/lib.rs", "description": "Review code" })
    ));
    assert!(is_external_nested_agent_tool(
        &json!({ "title": "Task" }),
        &json!({ "prompt": "inspect", "subagent_type": "general" })
    ));
    assert!(!is_external_nested_agent_tool(
        &json!({ "kind": "execute", "title": "cargo test" }),
        &json!({ "command": "cargo test" })
    ));
}
