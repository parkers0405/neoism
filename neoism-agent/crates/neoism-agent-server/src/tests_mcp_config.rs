use super::*;

#[tokio::test]
async fn mcp_routed_patch_publishes_before_immediate_get() {
    let root = std::env::temp_dir()
        .join(format!("mcp-publish-{}", Id::ascending(IdKind::Event)));
    let user = root.join("user");
    let workspace = root.join("workspace");
    let other = root.join("other");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(other.join(".agent")).unwrap();
    std::fs::write(user.join("agent.json"), r#"{"mcp":{"alpha":{"type":"remote","url":"https://alpha.invalid","enabled":false},"beta":{"type":"remote","url":"https://beta.invalid","enabled":false}},"theme":"keep"}"#).unwrap();
    let override_path = other.join(".agent/agent.json");
    let override_text = r#"{"mcp":{"alpha":{"type":"remote","url":"https://override.invalid","enabled":false}},"theme":"other"}"#;
    std::fs::write(&override_path, override_text).unwrap();
    let services = neoism_agent_service_api::AgentServices::new(
        std::sync::Arc::new(neoism_agent_service_api::StandardExecutableService),
        crate::standard_workspace_search(),
    )
    .with_config(std::sync::Arc::new(
        neoism_agent_service_api::StandardConfigSourceService::new(&user)
            .with_memory_layer(
                "opaque-deployment-policy",
                json!({"mcp":{"locked":{
                    "type":"remote", "url":"https://locked.invalid", "enabled":false
                }}}),
            ),
    ));
    let state = AppState::open_database_with_services(root.join("agent.db"), services)
        .await
        .unwrap();
    let router = app(state.clone());
    let directory = workspace.to_string_lossy();
    let other_directory = other.to_string_lossy();
    let catalog_uri = format!("/v2/plugins/dev.neoism.mcp/catalog?directory={directory}");
    let other_uri =
        format!("/v2/plugins/dev.neoism.mcp/catalog?directory={other_directory}");
    let initial: Value = response_json(
        router
            .clone()
            .oneshot(request(Method::GET, &catalog_uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(initial["alpha"]["enabled"], false);
    assert_eq!(initial["alpha"]["configScope"], "global");
    assert_eq!(initial["locked"]["configScope"], "global");
    assert_eq!(initial["locked"]["configWritable"], false);
    let other_initial: Value = response_json(
        router
            .clone()
            .oneshot(request(Method::GET, &other_uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(other_initial["alpha"]["configScope"], "workspace");
    assert_eq!(other_initial["beta"]["configScope"], "global");
    let pinned = state.plugin_snapshot(&directory).await;
    let other_pinned = state.plugin_snapshot(&other_directory).await;
    for enabled in [true, false] {
        let patched: Value = response_json(
            router
                .clone()
                .oneshot(request(
                    Method::PATCH,
                    &format!(
                        "/v2/plugins/dev.neoism.mcp/alpha/config?directory={directory}"
                    ),
                    Some(json!({"enabled":enabled})),
                ))
                .await
                .unwrap(),
        )
        .await;
        // No sleep: neither response is allowed to wait for the registry's refresh throttle.
        let current: Value = response_json(
            router
                .clone()
                .oneshot(request(Method::GET, &catalog_uri, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            current["alpha"]["enabled"], enabled,
            "immediate GET is stale; PATCH={patched}"
        );
        assert_eq!(patched["enabled"], enabled, "PATCH response is stale");
        assert_eq!(patched["configScope"], "global");
        assert_eq!(current["beta"], initial["beta"]);
        assert_eq!(current["locked"], initial["locked"]);
        if !enabled {
            assert_eq!(current["alpha"]["status"]["status"], "disabled");
        }
        let persisted: Value =
            serde_json::from_slice(&std::fs::read(user.join("agent.json")).unwrap())
                .unwrap();
        assert_eq!(persisted["mcp"]["alpha"]["enabled"], enabled);
        assert_eq!(persisted["mcp"]["beta"]["enabled"], false);
        assert_eq!(persisted["theme"], "keep");
        assert!(!workspace.join(".agent/agent.json").exists());
        assert_eq!(
            std::fs::read_to_string(&override_path).unwrap(),
            override_text
        );
        let other_current: Value = response_json(
            router
                .clone()
                .oneshot(request(Method::GET, &other_uri, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(other_current, other_initial);
        // Mutations publish a new generation, not a rewrite of an in-flight pin.
        crate::workspace_runtime::scope_generation(pinned.clone(), async {
            let still_pinned = state.refreshed_plugin_snapshot(&directory).await;
            assert!(still_pinned.ptr_eq(&pinned));
            assert_eq!(
                serde_json::to_value(&still_pinned.config().mcp["alpha"]).unwrap()
                    ["enabled"],
                false
            );
        })
        .await;
        assert!(state
            .plugin_snapshot(&other_directory)
            .await
            .ptr_eq(&other_pinned));
    }
    // A workspace override must write its owner, not the same-named global entry.
    let global_before = std::fs::read(user.join("agent.json")).unwrap();
    let patched: Value = response_json(
        router
            .clone()
            .oneshot(request(
                Method::PATCH,
                &format!(
                    "/v2/plugins/dev.neoism.mcp/alpha/config?directory={other_directory}"
                ),
                Some(json!({"enabled":true})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(patched["enabled"], true);
    assert_eq!(patched["configScope"], "workspace");
    let other_current: Value = response_json(
        router
            .clone()
            .oneshot(request(Method::GET, &other_uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(other_current["alpha"]["enabled"], true);
    assert_eq!(other_current["beta"], other_initial["beta"]);
    assert_eq!(
        std::fs::read(user.join("agent.json")).unwrap(),
        global_before
    );
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(&override_path).unwrap()).unwrap();
    assert_eq!(persisted["mcp"]["alpha"]["url"], "https://override.invalid");
    assert_eq!(persisted["theme"], "other");
    let current: Value = response_json(
        router
            .clone()
            .oneshot(request(Method::GET, &catalog_uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(current["alpha"]["enabled"], false);
    // Read-only ownership is labelled and still enforced by the mutation route.
    let rejected = router
        .oneshot(request(
            Method::PATCH,
            &format!("/v2/plugins/dev.neoism.mcp/locked/config?directory={directory}"),
            Some(json!({"enabled":true})),
        ))
        .await
        .unwrap();
    // Plugin dispatch currently wraps handler errors as HTTP 500. Verify the
    // owner-policy rejection itself without changing that separate API behavior.
    assert!(!rejected.status().is_success());
    let rejection = axum::body::to_bytes(rejected.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&rejection).contains("read-only config source"));
    assert_eq!(
        std::fs::read(user.join("agent.json")).unwrap(),
        global_before
    );
    drop((pinned, other_pinned));
    state.inner.store.close().await;
    let _ = std::fs::remove_dir_all(root);
}
