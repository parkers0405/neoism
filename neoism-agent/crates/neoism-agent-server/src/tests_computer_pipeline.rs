//! Session-executor -> persisted Error part -> provider media regression.
//! Only the native desktop backend is mocked; no desktop input/capture occurs.
use super::*;

#[tokio::test]
async fn failed_computer_batch_recovery_image_reaches_provider_with_error_status() {
    use neoism_agent_service_api::{BuiltinMcpCallResult, BuiltinMcpContent};
    let _revocation = crate::computer_use::TEST_REVOCATION_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "neoism-error-media-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(root.join(".agent/agent.json"),r#"{"mcp":{"computer":{"type":"local","command":["builtin","computer"],"enabled":true}}}"#).unwrap();
    let state = AppState::open_database_with_services(
        root.join("state.db"),
        crate::standard_services(),
    )
    .await
    .unwrap();
    let session_id = Id::ascending(IdKind::Session);
    let mut session = store_test_session(&session_id, now_millis());
    session.directory = root.to_string_lossy().into_owned();
    state.inner.store.insert_session(&session).await.unwrap();
    // Failed image result, plain failed result, and successful image result all
    // traverse the same real gateway, generation hooks, truncation and setter.
    for (failed, image, preflight) in [
        (true, true, false),
        (true, false, false),
        (false, true, false),
        (true, false, true),
    ] {
        let [_, mut message] = test_compaction_pair(&session_id, None, "");
        message.parts.clear();
        let MessageInfo::Assistant(info) = &mut message.info else {
            unreachable!()
        };
        info.mode = "build".into();
        info.finish = Some("tool-calls".into());
        let message_id = info.id.clone();
        let part_id = Id::ascending(IdKind::Part);
        let mut input = json!({"action":"call","tool":"computer.batch","arguments":{"actions":[{"action":"text","text":"mock only"}],"target":"fixture","screenshot":"display"}});
        if preflight {
            input["arguments"]["actions"] = json!([{"action":"click","frame":"fixture","x":0,"y":0},{"action":"type","text":"must not leak\n","method":"auto"}]);
        }
        set_tool_running(
            &mut message.parts,
            part_id.clone(),
            &session_id,
            &message_id,
            "call-recovery".into(),
            "execute".into(),
            input.clone(),
        );
        state
            .inner
            .store
            .append_message(session_id.as_str(), &message)
            .await
            .unwrap();
        let result=crate::computer_use::with_test_backend(std::sync::Arc::new(move |tool,_| {
            assert!(!preflight,"real whole-batch validation must reject before native backend");
            assert_eq!(tool,"batch");
            let mut content=vec![BuiltinMcpContent::Text { text:json!({"completed":1,"total":2,"failedIndex":if failed {Some(1)}else{None},"frame":"recovery-frame"}).to_string(),annotations:None }];
            if image {
                content.push(BuiltinMcpContent::Image { data:"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aMioAAAAASUVORK5CYII=".into(),mime_type:"image/png".into(),annotations:None });
            }
            Ok(BuiltinMcpCallResult {content,is_error:Some(failed)})
        }),crate::tool_runtime::execute_tool_call_with_permission_wait(
            &state,&session_id,&message_id,&session.directory,
            vec![PermissionRule {permission:"computer_use".into(),pattern:"*".into(),action:PermissionAction::Allow}],
            "call-recovery","execute",input,
        )).await.unwrap();
        assert_eq!(result.is_error(), failed);
        assert_eq!(
            result.metadata.as_ref().unwrap()["mcp"]["result"]["isError"],
            failed
        );
        if preflight {
            let payload =
                &result.metadata.as_ref().unwrap()["mcp"]["result"]["content"][0]["text"];
            let value: Value = serde_json::from_str(payload.as_str().unwrap()).unwrap();
            assert_eq!(value["status"], "failed");
            assert_eq!(value["completed"], 0);
            assert_eq!(value["clipboard"], "unchanged");
            assert_eq!(value["partialActionPossible"], false);
            assert!(!result.output.contains("must not leak"));
        }
        // This exact terminal-state helper is used by apply_queued_tool_result.
        crate::message_part_mutation::set_tool_execution_result(
            &mut message.parts,
            part_id.as_str(),
            result,
        )
        .unwrap();
        state
            .inner
            .store
            .update_message(session_id.as_str(), &message)
            .await
            .unwrap();
        let stored = state
            .inner
            .store
            .list_messages(session_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .find(|m| message_id_of(m) == message_id.as_str())
            .unwrap();
        let Part::Tool(part) = &stored.parts[0] else {
            panic!("expected persisted tool")
        };
        assert_eq!(matches!(part.state, ToolState::Error { .. }), failed);
        assert_eq!(matches!(part.state, ToolState::Completed { .. }), !failed);
        let provider =
            crate::message_model::provider_messages(std::slice::from_ref(&stored));
        let tool = provider
            .iter()
            .find(|m| matches!(m.role, neoism_agent_core::ProviderRole::Tool))
            .unwrap();
        assert_eq!(tool.tool_error, failed.then_some(true));
        assert!(tool.content.contains(if preflight {
            "no C0/C1"
        } else {
            "recovery-frame"
        }));
        assert_eq!(tool.attachments.len(), usize::from(image));
        if image {
            let media = provider
                .iter()
                .find(|m| matches!(m.role, neoism_agent_core::ProviderRole::User))
                .unwrap();
            assert_eq!(media.attachments.len(), 1);
            assert_eq!(media.attachments[0].mime, "image/png");
            assert!(media.attachments[0]
                .url
                .starts_with("data:image/png;base64,iVBOR"));
        }
        let compacted = crate::message_model::compaction_provider_messages(&[stored]);
        assert!(compacted.iter().all(|m| m.attachments.is_empty()));
        assert_eq!(
            compacted
                .iter()
                .find(|m| matches!(m.role, neoism_agent_core::ProviderRole::Tool))
                .unwrap()
                .tool_error,
            failed.then_some(true)
        );
    }
    state.inner.store.close().await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn clipboard_consent_reaches_worker_only_through_explicit_registry_grant() {
    use neoism_agent_service_api::{BuiltinMcpCallResult, BuiltinMcpContent};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let _revocation = crate::computer_use::TEST_REVOCATION_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "neoism-clipboard-consent-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(root.join(".agent/agent.json"),r#"{"mcp":{"computer":{"type":"local","command":["builtin","computer"],"enabled":true}}}"#).unwrap();
    let state = AppState::open_database_with_services(
        root.join("state.db"),
        crate::standard_services(),
    )
    .await
    .unwrap();
    let directory = root.to_str().unwrap();
    let snapshot = state.refreshed_plugin_snapshot(directory).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let native_calls = calls.clone();
    crate::computer_use::with_test_backend(Arc::new(move |tool,_| {
        assert_eq!(tool,"batch");native_calls.fetch_add(1,Ordering::SeqCst);
        Ok(BuiltinMcpCallResult{content:vec![BuiltinMcpContent::Text{text:json!({"status":"dispatched_unverified","applicationVerified":false,"mockNative":true}).to_string(),annotations:None}],is_error:None})
    }),async {
        let arguments=json!({"target":"fixture","actions":[{"action":"click","frame":"fixture","x":0,"y":0},{"action":"type","text":"private","method":"paste","clipboard_policy":"replace"}]});
        let base=vec![PermissionRule{permission:"*".into(),pattern:"*".into(),action:PermissionAction::Allow},PermissionRule{permission:"computer_use".into(),pattern:"*".into(),action:PermissionAction::Allow}];
        let execution=neoism_agent_service_api::ExecutionPolicy::NativeLocal;
        let mcp_auth=crate::mcp_auth::McpAuthStore::local(state.services());
        let missing=crate::agent_tool_registry::execute_mcp_tool_by_runtime_id(directory,"mcp__computer__batch",arguments.clone(),&base,None,Some(state.clone()),&snapshot,&execution,&mcp_auth).await.err().unwrap();
        let challenge=crate::permission_runtime::parse_permission_required_error(&missing.to_string()).unwrap();
        assert_eq!(challenge.0,"computer_clipboard");assert_eq!(calls.load(Ordering::SeqCst),0);
        let mut denied=base.clone();denied.push(PermissionRule{permission:"computer_clipboard".into(),pattern:"*".into(),action:PermissionAction::Deny});
        let denied=crate::agent_tool_registry::execute_mcp_tool_by_runtime_id(directory,"mcp__computer__batch",arguments.clone(),&denied,None,Some(state.clone()),&snapshot,&execution,&mcp_auth).await.err().unwrap();
        assert!(denied.to_string().contains("denied"));assert!(crate::permission_runtime::parse_permission_required_error(&denied.to_string()).is_none());assert_eq!(calls.load(Ordering::SeqCst),0);
        let mut allowed=base;allowed.push(PermissionRule{permission:"computer_clipboard".into(),pattern:"*".into(),action:PermissionAction::Allow});
        let result=crate::agent_tool_registry::execute_mcp_tool_by_runtime_id(directory,"mcp__computer__batch",arguments.clone(),&allowed,None,Some(state.clone()),&snapshot,&execution,&mcp_auth).await.unwrap().unwrap();
        assert!(!result.is_error());assert_eq!(calls.load(Ordering::SeqCst),1);
        // Even internal direct session calls cannot manufacture the trusted permit
        // merely by passing session_authorized=true or model replacement policy.
        let store=crate::mcp_auth::McpAuthStore::new(root.join("auth.json"));
        let direct=crate::mcp::call_tool_in_session(directory,"computer","batch",arguments,&store,state.clone(),&snapshot,true,Arc::new(std::sync::atomic::AtomicBool::new(false))).await.unwrap();
        assert_eq!(direct.is_error,Some(true));assert_eq!(calls.load(Ordering::SeqCst),1);
        assert!(crate::mcp::tool_result_text(&direct).contains("computer_clipboard"));
    }).await;
    state.inner.store.close().await;
    let _ = std::fs::remove_dir_all(root);
}
