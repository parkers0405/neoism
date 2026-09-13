//! Session-executor -> persisted Error part -> provider media regression.
//! Only the native desktop backend is mocked; no desktop input/capture occurs.
use super::*;

#[tokio::test]
async fn failed_computer_batch_recovery_image_reaches_provider_with_error_status() {
    use neoism_agent_service_api::{BuiltinMcpCallResult,BuiltinMcpContent};
    let _revocation=crate::computer_use::TEST_REVOCATION_LOCK.lock().await;
    let root=std::env::temp_dir().join(format!("neoism-error-media-{}",Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(root.join(".agent/agent.json"),r#"{"mcp":{"computer":{"type":"local","command":["builtin","computer"],"enabled":true}}}"#).unwrap();
    let state=AppState::open_database_with_services(root.join("state.db"),crate::standard_services()).await.unwrap();
    let session_id=Id::ascending(IdKind::Session);
    let mut session=store_test_session(&session_id,now_millis());
    session.directory=root.to_string_lossy().into_owned();
    state.inner.store.insert_session(&session).await.unwrap();
    // Failed image result, plain failed result, and successful image result all
    // traverse the same real gateway, generation hooks, truncation and setter.
    for (failed,image) in [(true,true),(true,false),(false,true)] {
        let [_,mut message]=test_compaction_pair(&session_id,None,"");
        message.parts.clear();
        let MessageInfo::Assistant(info)=&mut message.info else { unreachable!() };
        info.mode="build".into(); info.finish=Some("tool-calls".into());
        let message_id=info.id.clone();
        let part_id=Id::ascending(IdKind::Part);
        let input=json!({"action":"call","tool":"computer.batch","arguments":{"actions":[{"action":"text","text":"mock only"}],"screenshot":"display"}});
        set_tool_running(&mut message.parts,part_id.clone(),&session_id,&message_id,"call-recovery".into(),"execute".into(),input.clone());
        state.inner.store.append_message(session_id.as_str(),&message).await.unwrap();
        let result=crate::computer_use::with_test_backend(std::sync::Arc::new(move |tool,_| {
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
        assert_eq!(result.is_error(),failed);
        assert_eq!(result.metadata.as_ref().unwrap()["mcp"]["result"]["isError"],failed);
        // This exact terminal-state helper is used by apply_queued_tool_result.
        crate::message_part_mutation::set_tool_execution_result(&mut message.parts,part_id.as_str(),result).unwrap();
        state.inner.store.update_message(session_id.as_str(),&message).await.unwrap();
        let stored=state.inner.store.list_messages(session_id.as_str()).await.unwrap().into_iter().find(|m|message_id_of(m)==message_id.as_str()).unwrap();
        let Part::Tool(part)=&stored.parts[0] else { panic!("expected persisted tool") };
        assert_eq!(matches!(part.state,ToolState::Error {..}),failed);
        assert_eq!(matches!(part.state,ToolState::Completed {..}),!failed);
        let provider=crate::message_model::provider_messages(std::slice::from_ref(&stored));
        let tool=provider.iter().find(|m|matches!(m.role,neoism_agent_core::ProviderRole::Tool)).unwrap();
        assert_eq!(tool.tool_error,failed.then_some(true));
        assert!(tool.content.contains("recovery-frame"));
        assert_eq!(tool.attachments.len(),usize::from(image));
        if image {
            let media=provider.iter().find(|m|matches!(m.role,neoism_agent_core::ProviderRole::User)).unwrap();
            assert_eq!(media.attachments.len(),1);
            assert_eq!(media.attachments[0].mime,"image/png");
            assert!(media.attachments[0].url.starts_with("data:image/png;base64,iVBOR"));
        }
        let compacted=crate::message_model::compaction_provider_messages(&[stored]);
        assert!(compacted.iter().all(|m|m.attachments.is_empty()));
        assert_eq!(compacted.iter().find(|m|matches!(m.role,neoism_agent_core::ProviderRole::Tool)).unwrap().tool_error,failed.then_some(true));
    }
    state.inner.store.close().await;
    let _=std::fs::remove_dir_all(root);
}
