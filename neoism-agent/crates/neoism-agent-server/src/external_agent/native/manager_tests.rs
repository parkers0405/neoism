use super::*;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn managed_turns_keep_identity_and_abort_closes_idle_native_background_work() {
    for runtime in [ExternalRuntime::Codex, ExternalRuntime::Claude, ExternalRuntime::OpenCode] {
        let root = std::env::temp_dir().join(format!("native-manager-{}", Id::ascending(IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let mut services = crate::standard_services();
        let node = services.executables.resolve(&neoism_agent_service_api::ExecutableRequest::new(
            "node", neoism_agent_service_api::ExecutablePurpose::ExternalAgent,
        )).unwrap().path;
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/external_agent/native/connection_fixture.mjs");
        let wrapper = root.join("node-host");
        std::fs::write(&wrapper, format!("#!/bin/sh\nexec '{}' '{}'\n", node.display(), fixture.display())).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        services.executables = Arc::new(crate::executable::test_support::FakeExecutableService::with("node", wrapper)
            .with_path(runtime.provider_id(), &node).with_path("npm", &node));
        let state = AppState::open_database_with_services(root.join("agent.sqlite3"), services).await.unwrap();
        let session = crate::session_routes::create_session_in_directory(&state, root.to_str().unwrap(), neoism_agent_core::CreateSessionRequest {
            parent_id: None, title: None, agent: None, model: None, permission: Some(vec![PermissionRule {
                permission: "read".into(), pattern: "*".into(), action: PermissionAction::Allow,
            }]), workspace_id: None, external_provider: Some(runtime.provider_id().into()), external_options: None,
        }, BTreeMap::new()).await.unwrap();
        let model = external_model(runtime);
        for prompt in ["first", "background"] {
            let user_id = Id::ascending(IdKind::Message);
            append_external_user_message(&state, &session, user_id.clone(), prompt, runtime, &model, None).await.unwrap();
            let step = start_assistant_step(&state, &session.id, session.id.as_str(), &user_id, &session.directory, now_millis(),
                "build".into(), runtime.agent_name().into(), model.model_id.clone(), model.provider_id.clone(), false).await.unwrap();
            let result = run_native_prompt(&state, &session, prompt, runtime, &step, &model, Arc::new(AtomicBool::new(false))).await.unwrap();
            assert_eq!(result.provider_response.text, "early");
            finish_provider_stream_success(&state, &session.id, session.id.as_str(), &step.assistant_id, &step.text_part_id,
                &step.live_message, &model, result.provider_response, Default::default(), true).await.unwrap();
        }
        let stored = state.inner.store.get_session(session.id.as_str()).await.unwrap().unwrap();
        assert_eq!(stored.extra["externalAgent"]["runtime"], runtime.native_driver_id());
        assert_eq!(stored.extra["externalAgent"]["nativeSessionId"], format!("native-{}", runtime.provider_id()));
        assert_eq!(std::fs::read_to_string(root.join("host-initializations")).unwrap().lines().count(), 1);
        assert!(crate::session_actions::abort_session_run(&state, session.id.as_str()).await);
        assert!(!close_session(&state, session.id.as_str()).await);
        let children = state.inner.store.list_child_sessions(session.id.as_str()).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].extra["externalAgent"]["status"], "interrupted");
        assert_eq!(children[0].extra["externalAgent"]["nativeCapabilities"]["resume"], false);
        state.shutdown().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}