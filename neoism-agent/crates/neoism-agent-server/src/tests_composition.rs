use super::*;
use axum::{
    extract::{Path, State},
    Json,
};

async fn setup() -> (
    AppState,
    SessionInfo,
    crate::workspace_runtime::PluginGenerationLease,
) {
    let root = std::env::temp_dir().join(format!(
        "neoism-composition-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("target.txt"), "before\n").unwrap();
    let state = AppState::open_database(root.join("agent.sqlite3"))
        .await
        .unwrap();
    let mut session: SessionInfo = response_json(
        app(state.clone())
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.display()),
                Some(json!({"model":{"providerID":"neoism","id":"stub"}})),
            ))
            .await
            .unwrap(),
    )
    .await;
    session.model = Some(neoism_agent_core::ModelRef {
        id: "stub".into(),
        provider_id: "neoism".into(),
        connection_id: None,
        variant: None,
    });
    state.inner.store.update_session(&session).await.unwrap();
    let snapshot = state
        .workspace_runtime(&session.directory)
        .await
        .unwrap()
        .snapshot();
    (state, session, snapshot)
}
fn allow() -> Vec<PermissionRule> {
    vec![PermissionRule {
        permission: "*".into(),
        pattern: "*".into(),
        action: PermissionAction::Allow,
    }]
}
async fn compose(
    state: &AppState,
    session: &SessionInfo,
    snapshot: crate::workspace_runtime::PluginGenerationLease,
    permissions: Vec<PermissionRule>,
    code: &str,
) -> crate::tool::ToolExecutionResult {
    crate::tool_runtime::execute_tool_call_in_generation(
        state,
        &session.id,
        &Id::ascending(IdKind::Message),
        &session.directory,
        permissions,
        "parent-compose",
        "compose",
        json!({"code":code}),
        snapshot,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn composition_builtin_native_schema_and_partial_effects() {
    let (state, session, snapshot) = setup().await;
    assert!(snapshot.runtime_tools.contains_key("compose"));
    let result = compose(
        &state,
        &session,
        snapshot.clone(),
        allow(),
        "const r = await tools.neoism.read({filePath: 'target.txt'}); return r.output;",
    )
    .await;
    assert!(result.metadata.as_ref().unwrap()["composition"]["ok"]
        .as_bool()
        .unwrap());
    assert!(result.output.contains("before"));
    assert!(!result.output.contains("toolCalls"));
    let paged = compose(&state, &session, snapshot.clone(), allow(),
        "const r = await tools.neoism.read({filePath:'target.txt',offset:1,limit:7}); return r.metadata.limit;").await;
    assert_eq!(
        paged.metadata.as_ref().unwrap()["composition"]["value"].as_f64(),
        Some(7.0),
        "JS integers must survive native argument parsing"
    );
    let invalid = compose(
        &state,
        &session,
        snapshot.clone(),
        allow(),
        "return await tools.neoism.read({filePath: 3});",
    )
    .await;
    assert_eq!(
        invalid.metadata.as_ref().unwrap()["composition"]["ok"],
        false
    );
    assert!(invalid.output.contains("invalid input"));
    let failed = compose(&state, &session, snapshot, allow(),
        "await tools.neoism.edit({filePath:'target.txt',oldString:'before',newString:'after'}); throw 'after effect';").await;
    assert_eq!(
        std::fs::read_to_string(
            std::path::Path::new(&session.directory).join("target.txt")
        )
        .unwrap(),
        "after\n"
    );
    assert_eq!(
        failed.metadata.as_ref().unwrap()["composition"]["ok"],
        false
    );
    assert!(failed
        .metadata
        .as_ref()
        .unwrap()
        .get("snapshotArtifact")
        .is_some());
    assert!(failed.is_error());
    let (live, _, part_id) = live_parent(&session);
    let mut message = live.lock().await;
    crate::message_part_mutation::set_tool_execution_result(
        &mut message.parts,
        part_id.as_str(),
        failed,
    );
    let Part::Tool(part) = &message.parts[0] else {
        panic!("expected parent")
    };
    assert!(matches!(part.state, ToolState::Error { .. }));
    assert_eq!(
        part.metadata.as_ref().unwrap()["toolResult"]["isError"],
        true
    );
    let snapshots = crate::snapshot::collect_from_revert_items_hydrated(
        &state,
        "local",
        session.id.as_str(),
        &[],
        &message.parts,
    )
    .await
    .unwrap();
    crate::snapshot::apply(
        &session.directory,
        &snapshots,
        crate::snapshot::SnapshotDirection::Revert,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(
            std::path::Path::new(&session.directory).join("target.txt")
        )
        .unwrap(),
        "before\n"
    );
    assert!(
        state
            .inner
            .store
            .list_messages(session.id.as_str())
            .await
            .unwrap()
            .is_empty(),
        "children must not create provider parts"
    );
}
#[tokio::test]
async fn composition_catalog_exclusion_deny_and_model_eligibility() {
    let (state, session, snapshot) = setup().await;
    let denied = compose(&state, &session, snapshot.clone(), vec![
        PermissionRule { permission: "*".into(), pattern: "*".into(), action: PermissionAction::Allow },
        PermissionRule { permission: "edit".into(), pattern: "*".into(), action: PermissionAction::Deny },
    ], "return await tools.neoism.edit({filePath:'target.txt',oldString:'before',newString:'after'});").await;
    assert_eq!(
        denied.metadata.as_ref().unwrap()["composition"]["ok"],
        false
    );
    let excluded = compose(
        &state,
        &session,
        snapshot.clone(),
        allow(),
        "return await tools.neoism.move_chat({directory:'child'});",
    )
    .await;
    assert_eq!(
        excluded.metadata.as_ref().unwrap()["composition"]["ok"],
        false
    );
    let auth =
        crate::mcp_auth::McpAuthStore::for_session(state.services(), &session).unwrap();
    let tools = crate::agent_tool_registry::composition_tools_for_agent(
        &state,
        &session.directory,
        &snapshot,
        &allow(),
        "gpt-5",
        &crate::caller::session_execution_policy(state.services(), &session),
        &auth,
    )
    .await
    .unwrap();
    assert!(!tools
        .iter()
        .any(|tool| matches!(tool.id.as_str(), "edit" | "write")));
    assert!(tools.iter().any(|tool| tool.id == "apply_patch"));
    let mut patched_session = session.clone();
    patched_session.model.as_mut().unwrap().id = "gpt-5".into();
    state
        .inner
        .store
        .update_session(&patched_session)
        .await
        .unwrap();
    let model_denied = compose(&state, &patched_session, snapshot, allow(),
        "return await tools.neoism.edit({filePath:'target.txt',oldString:'before',newString:'after'});").await;
    assert_eq!(
        model_denied.metadata.as_ref().unwrap()["composition"]["ok"],
        false
    );
}
#[tokio::test]
async fn composition_child_ask_does_not_replay_script() {
    let (state, session, snapshot) = setup().await;
    let task_state = state.clone();
    let task_session = session.clone();
    let task = tokio::spawn(async move {
        compose(&task_state, &task_session, snapshot, vec![
            PermissionRule { permission:"*".into(), pattern:"*".into(), action:PermissionAction::Allow },
            PermissionRule { permission:"edit".into(), pattern:"*".into(), action:PermissionAction::Ask },
        ], "console.log('once'); await tools.neoism.edit({filePath:'target.txt',oldString:'before',newString:'after'}); return 'done';").await
    });
    let pending_id = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(id) = state.inner.permissions.read().await.keys().next().cloned()
            {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pending = state
        .inner
        .permission_waiters
        .write()
        .await
        .remove(&pending_id)
        .unwrap();
    assert_ne!(
        pending.request.tool.as_ref().unwrap()["callID"],
        "parent-compose"
    );
    pending
        .sender
        .send(Ok(crate::permission_runtime::permission_grants(
            &pending.request,
            false,
        )))
        .unwrap();
    state.inner.permissions.write().await.remove(&pending_id);
    let result = task.await.unwrap();
    assert_eq!(
        result.metadata.as_ref().unwrap()["composition"]["logs"],
        json!(["once"])
    );
    assert_eq!(
        result.metadata.as_ref().unwrap()["composition"]["toolCalls"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

fn live_parent(
    session: &SessionInfo,
) -> (Arc<tokio::sync::Mutex<MessageWithParts>>, Id, Id) {
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let message = MessageWithParts {
        info: MessageInfo::User(neoism_agent_core::UserMessage {
            id: message_id.clone(),
            session_id: session.id.clone(),
            time: neoism_agent_core::CreatedTime {
                created: now_millis(),
            },
            agent: "build".into(),
            model: UserModel {
                provider_id: "neoism".into(),
                model_id: "stub".into(),
                connection_id: None,
                variant: None,
            },
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Tool(ToolPart {
            id: part_id.clone(),
            session_id: session.id.clone(),
            message_id: message_id.clone(),
            tool: "compose".into(),
            call_id: "parent-compose".into(),
            state: ToolState::Running {
                input: json!({}),
                time: PartTime {
                    start: now_millis(),
                    end: None,
                },
            },
            metadata: Some(json!({"sentinel":"preserved"})),
        })],
    };
    (
        Arc::new(tokio::sync::Mutex::new(message)),
        message_id,
        part_id,
    )
}
#[tokio::test]
async fn composition_parent_live_progress_and_local_cancel_clean_permission() {
    let (state, session, snapshot) = setup().await;
    let (live, message_id, part_id) = live_parent(&session);
    let mut events = state.subscribe();
    let cancel = Arc::new(AtomicBool::new(false));
    let task_cancel = cancel.clone();
    let task_live = live.clone();
    let task_state = state.clone();
    let task_session = session.clone();
    let task_part = part_id.clone();
    let task = tokio::spawn(async move {
        crate::tool_runtime::execute_tool_call_scoped(&task_state, &task_session.id, &message_id,
            &task_session.directory, vec![
                PermissionRule { permission:"*".into(), pattern:"*".into(), action:PermissionAction::Allow },
                PermissionRule { permission:"edit".into(), pattern:"*".into(), action:PermissionAction::Ask },
            ], "parent-compose", "compose", json!({"code":"await tools.neoism.read({filePath:'target.txt'}); return await tools.neoism.edit({filePath:'target.txt',oldString:'before',newString:'after'});"}), snapshot,
            crate::tool_composition::CallScope { cancel:Some(task_cancel),
                progress:Some(crate::tool_composition::ProgressSink {message:task_live,part_id:task_part.to_string()}), ..Default::default() }).await.unwrap()
    });
    let pending_id = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(id) = state.inner.permissions.read().await.keys().next().cloned()
            {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    {
        let message = live.lock().await;
        assert_eq!(message.parts.len(), 1);
        let Part::Tool(part) = &message.parts[0] else {
            panic!("expected parent")
        };
        let metadata = part.metadata.as_ref().unwrap();
        assert_eq!(metadata["sentinel"], "preserved");
        let calls = metadata["composition"]["toolCalls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["status"], "completed");
        assert_eq!(calls[0]["presentation"]["tool"], "read");
        assert!(calls[0]["presentation"]["output"]
            .as_str()
            .unwrap()
            .contains("before"));
        assert_eq!(calls[1]["status"], "running");
        assert_eq!(
            metadata["composition"]["code"],
            "await tools.neoism.read({filePath:'target.txt'}); return await tools.neoism.edit({filePath:'target.txt',oldString:'before',newString:'after'});"
        );
        assert_ne!(
            metadata["composition"]["code"],
            serde_json::to_value(&part.state).unwrap()["input"]["code"]
        );
    }
    cancel.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    let replay: Value = serde_json::from_str(&result.output).unwrap();
    assert!(replay.get("code").is_none());
    assert!(replay.get("toolCalls").is_none());
    assert!(result.metadata.as_ref().unwrap()["composition"]["code"].is_string());
    assert_eq!(
        result.metadata.as_ref().unwrap()["composition"]["ok"],
        false
    );
    assert!(!state
        .inner
        .permission_waiters
        .read()
        .await
        .contains_key(&pending_id));
    assert!(!state
        .inner
        .permissions
        .read()
        .await
        .contains_key(&pending_id));
    let _ = crate::interaction::permission_reply(
        State(state.clone()),
        Path(pending_id),
        Json(crate::interaction::PermissionReplyRequest {
            reply: Some("always".into()),
            response: None,
            message: None,
        }),
    )
    .await
    .unwrap();
    assert!(state.inner.permission_approvals.read().await.is_empty());
    let mut statuses = std::collections::BTreeSet::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == event_type::MESSAGE_PART_UPDATED {
            assert_eq!(event.properties["part"]["id"], part_id.to_string());
            for call in event.properties["part"]["metadata"]["composition"]["toolCalls"]
                .as_array()
                .unwrap()
            {
                statuses.insert(call["status"].as_str().unwrap().to_string());
            }
        }
    }
    assert_eq!(
        statuses,
        ["running", "completed", "error"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
    assert_eq!(
        std::fs::read_to_string(
            std::path::Path::new(&session.directory).join("target.txt")
        )
        .unwrap(),
        "before\n"
    );
    let mut message = live.lock().await;
    crate::message_part_mutation::set_tool_execution_result(
        &mut message.parts,
        part_id.as_str(),
        result,
    );
    let Part::Tool(part) = &message.parts[0] else {
        panic!("expected parent")
    };
    assert_eq!(part.metadata.as_ref().unwrap()["composition"]["ok"], false);
}

#[tokio::test]
async fn composition_pinned_generation_survives_refresh() {
    let (state, session, pinned) = setup().await;
    let config_dir = std::path::Path::new(&session.directory).join(".agent");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("agent.json"),
        r#"{"dangerouslySkipPermissions":true}"#,
    )
    .unwrap();
    let runtime = state.workspace_runtime(&session.directory).await.unwrap();
    crate::workspace_runtime::refresh_plugins(&runtime, &state)
        .await
        .unwrap();
    assert_ne!(runtime.snapshot().generation, pinned.generation);
    let result = compose(
        &state,
        &session,
        pinned,
        allow(),
        "return (await tools.neoism.read({filePath:'target.txt'})).output;",
    )
    .await;
    assert_eq!(result.metadata.as_ref().unwrap()["composition"]["ok"], true);
}

#[tokio::test]
async fn composition_dispatcher_bounds_noncooperative_native_leaf_preserving_media() {
    struct Fixture {
        definition: neoism_agent_plugin_api::PluginToolDefinition,
        calls: AtomicUsize,
        dropped: Arc<AtomicBool>,
    }
    impl neoism_agent_plugin_api::RuntimeTool for Fixture {
        fn definition(&self) -> neoism_agent_plugin_api::PluginToolDefinition {
            self.definition.clone()
        }
        fn execute<'a>(
            &'a self,
            _: neoism_agent_plugin_api::PluginToolInvocation,
        ) -> neoism_agent_plugin_api::PluginFuture<
            'a,
            neoism_agent_plugin_api::PluginToolResult,
        > {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Ok(neoism_agent_plugin_api::PluginToolResult {
                        title: "Recovery".into(),
                        output: "known media".into(),
                        metadata: Some(
                            json!({"attachments":[{"mime":"image/png","url":"data:image/png;base64,YWJj"}],"generatedFiles":[{"name":"known.png"}]}),
                        ),
                    });
                }
                struct Flag(Arc<AtomicBool>);
                impl Drop for Flag {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::SeqCst);
                    }
                }
                let _flag = Flag(self.dropped.clone());
                std::future::pending().await // explicitly ignores invocation.cancel
            })
        }
    }
    let (state, session, snapshot) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let fixture = Arc::new(Fixture {
        definition: snapshot.runtime_tools["read"].definition(),
        calls: AtomicUsize::new(0),
        dropped: dropped.clone(),
    });
    let snapshot = snapshot.test_with_runtime_tool("read", fixture.clone());
    // The wall-clock limit also pays for store lookups, cold catalog/schema
    // compilation and interpreter startup. 100 ms can expire before either
    // leaf is dispatched when the suite runs in parallel. Give startup room,
    // but keep the uncooperative leaf's cleanup grace at just 20 ms.
    let wall_clock = Duration::from_secs(1);
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        wall_clock + Duration::from_secs(1),
        crate::tool_composition::run_with_limits(
            &state,
            &session.id,
            &Id::ascending(IdKind::Message),
            &session.directory,
            &allow(),
            "await tools.neoism.read({filePath:'target.txt'}); await tools.neoism.read({filePath:'target.txt'});",
            snapshot,
            None,
            None,
            neoism_agent_composition::Limits {
                wall_clock,
                cleanup_timeout: Duration::from_millis(20),
                ..Default::default()
            },
        ),
    )
    .await
    .expect("noncooperative native leaf must not hold the parent indefinitely")
    .unwrap();
    assert!(started.elapsed() < wall_clock + Duration::from_secs(1));
    assert!(result.is_error());
    let metadata = result.metadata.as_ref().unwrap();
    assert_eq!(
        metadata["composition"]["error"]["kind"],
        "timeout",
        "must hit the wall-clock deadline, not an unrelated failure: {metadata}"
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2, "{metadata}");
    assert_eq!(
        metadata["composition"]["toolCalls"].as_array().unwrap().len(),
        2,
        "both fixture leaves must run before the deadline: {metadata}"
    );
    assert_eq!(
        metadata["composition"]["toolCalls"][0]["status"],
        "completed"
    );
    assert_eq!(
        metadata["composition"]["toolCalls"][1]["status"],
        "partial_unknown"
    );
    assert_eq!(metadata["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(metadata["generatedFiles"][0]["name"], "known.png");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn composition_real_mcp_gateway_structured_data_and_error_media() {
    async fn handler(Json(request): Json<Value>) -> Json<Value> {
        if request["method"] == "tools/call"
            && request["params"]["arguments"]["hang"] == true
        {
            return std::future::pending::<Json<Value>>().await; // ignores host cancellation
        }
        let result = match request["method"].as_str().unwrap_or_default() {
            "initialize" => {
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
            }
            "tools/list" => {
                json!({"tools":[
                    {"name":"echo","description":"Echo structured data","inputSchema":{"type":"object","properties":{"fail":{"type":"boolean"},"hang":{"type":"boolean"}},"required":["fail"]}},
                    {"name":"invalid","description":"Unrelated malformed integration","inputSchema":{"type":"not-a-type"}},
                    {"name":"oversized","description":"x".repeat(8193),"inputSchema":{"type":"object"}},
                    {"name":"large_schema","description":"Unrelated oversized schema","inputSchema":{"type":"object","description":"x".repeat(300_000)}}
                ]})
            }
            "tools/call" => {
                json!({"content":[{"type":"text","text":"formatted text, not JSON"},{"type":"image","mimeType":"image/png","data":"YWJj"}],
                "structuredContent":{"count":7},"isError":request["params"]["arguments"]["fail"]})
            }
            _ => json!({}),
        };
        Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route("/mcp", axum::routing::post(handler)),
        )
        .await
        .unwrap();
    });
    let (state, session, _) = setup().await;
    let config_dir = std::path::Path::new(&session.directory).join(".agent");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("agent.json"), json!({"mcp":{"fixture":{"type":"remote","url":format!("http://{address}/mcp"),"oauth":false}}}).to_string()).unwrap();
    let runtime = state.workspace_runtime(&session.directory).await.unwrap();
    crate::workspace_runtime::refresh_plugins(&runtime, &state)
        .await
        .unwrap();
    let pinned = runtime.snapshot();
    let native = compose(
        &state,
        &session,
        pinned.clone(),
        allow(),
        "return (await tools.neoism.read({filePath:'target.txt'})).output;",
    )
    .await;
    assert!(
        !native.is_error(),
        "unrelated malformed MCP must not suppress native read"
    );
    assert!(native.output.contains("before"));
    assert_eq!(
        native.metadata.as_ref().unwrap()["composition"]["warnings"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(
        native.output.contains("Omitted MCP tool"),
        "catalog warnings must reach model replay"
    );
    let (parent, _, part_id) = live_parent(&session);
    {
        let mut message = parent.lock().await;
        crate::message_part_mutation::set_tool_execution_result(
            &mut message.parts,
            part_id.as_str(),
            native,
        );
        let Part::Tool(part) = &message.parts[0] else {
            panic!("expected parent")
        };
        assert_eq!(
            part.metadata.as_ref().unwrap()["composition"]["warnings"]
                .as_array()
                .unwrap()
                .len(),
            3,
            "catalog warnings must reach parent UI metadata"
        );
    }
    let structured = compose(
        &state,
        &session,
        pinned.clone(),
        allow(),
        "const r = await tools.fixture.echo({fail:false}); return r.data;",
    )
    .await;
    assert_eq!(
        structured.metadata.as_ref().unwrap()["composition"]["value"]["count"].as_f64(),
        Some(7.0)
    );
    let aliased = compose(
        &state,
        &session,
        pinned.clone(),
        allow(),
        "const r = await tools.fixture.echo({fail:false}); r.data.count = 8; return {data:r.data, structured:r.structuredContent, canonical:r.metadata.mcp.result.structuredContent};",
    )
    .await;
    let selected = &aliased.metadata.as_ref().unwrap()["composition"]["value"];
    assert_eq!(selected["data"], json!({"count":8}));
    assert_eq!(selected["structured"], selected["data"]);
    assert_eq!(selected["canonical"], selected["data"]);
    let failure = compose(
        &state,
        &session,
        pinned.clone(),
        allow(),
        "try { await tools.fixture.echo({fail:true}); } catch (e) { return 'caught'; }",
    )
    .await;
    let metadata = failure.metadata.as_ref().unwrap();
    assert_eq!(metadata["composition"]["value"], "caught");
    assert_eq!(
        metadata["composition"]["toolCalls"][0]["path"],
        "fixture.echo"
    );
    assert_eq!(metadata["composition"]["toolCalls"][0]["status"], "error");
    let presentation = &metadata["composition"]["toolCalls"][0]["presentation"];
    assert_eq!(presentation["tool"], "execute");
    assert_eq!(presentation["isError"], true);
    assert_eq!(presentation["metadata"]["mcp"]["client"], "fixture");
    assert!(presentation["metadata"]["mcp"].get("result").is_none());
    assert!(presentation["output"]
        .as_str()
        .unwrap()
        .contains("formatted text"));
    assert!(metadata["attachments"][0]["artifactID"].as_str().is_some());
    let started = std::time::Instant::now();
    let interrupted = crate::tool_composition::run_with_limits(&state, &session.id,
        &Id::ascending(IdKind::Message), &session.directory, &allow(),
        "await tools.fixture.echo({fail:false}); await tools.fixture.echo({fail:false,hang:true});", pinned, None, None,
        neoism_agent_composition::Limits { wall_clock:Duration::from_millis(300),cleanup_timeout:Duration::from_millis(50), ..Default::default() }).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "noncooperative MCP child must not hold the parent indefinitely"
    );
    assert!(interrupted.is_error());
    let metadata = interrupted.metadata.as_ref().unwrap();
    assert_eq!(
        metadata["composition"]["toolCalls"][0]["status"],
        "completed"
    );
    let status = metadata["composition"]["toolCalls"][1]["status"]
        .as_str()
        .unwrap();
    // The real MCP transport may honor cancellation itself; otherwise the
    // supervised leaf deadline reports uncertainty. Both are bounded.
    assert!(matches!(status, "error" | "partial_unknown"), "{metadata}");
    if status == "partial_unknown" {
        assert!(metadata["composition"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("partial_unknown")));
    }
    assert!(
        !metadata["attachments"].as_array().unwrap().is_empty(),
        "completed recovery media must survive aborted later work"
    );
    let (live, _, part_id) = live_parent(&session);
    let mut message = live.lock().await;
    crate::message_part_mutation::set_tool_execution_result(
        &mut message.parts,
        part_id.as_str(),
        interrupted,
    );
    let Part::Tool(part) = &message.parts[0] else {
        panic!("expected parent")
    };
    assert!(matches!(part.state, ToolState::Error { .. }));
    assert!(
        crate::message_model::tool_state_metadata(part).unwrap()["attachments"]
            .as_array()
            .is_some()
    );
    server.abort();
}
