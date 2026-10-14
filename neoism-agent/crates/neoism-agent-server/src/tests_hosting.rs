use super::*;

#[tokio::test]
async fn hosting_http_requires_operator_and_guests_read_same_history() {
    let _guard = env_lock();
    let dir = std::env::temp_dir().join(format!(
        "agent-host-http-{}",
        neoism_agent_core::new_session_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let previous = std::env::var_os("NEOISM_DAEMON_TOKEN");
    std::env::set_var("NEOISM_DAEMON_TOKEN", "hosting-test-signing-key");
    let state = AppState::open_database(dir.join("agent.db")).await.unwrap();
    match previous {
        Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
        None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
    }
    let mut old = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    old.directory = dir.to_string_lossy().into_owned();
    state.inner.store.insert_session(&old).await.unwrap();
    let message = store_test_message(&old.id, now_millis(), "before hosting");
    state
        .inner
        .store
        .append_message(old.id.as_str(), &message)
        .await
        .unwrap();
    let artifact = neoism_agent_core::ArtifactInfo {
        id: "artifact_hosting_history".into(),
        filename: "old.txt".into(),
        media_type: "text/plain".into(),
        size: 3,
        sha256: "test".into(),
        created: now_millis(),
        session_id: Some(old.id.to_string()),
        download_url: String::new(),
    };
    state
        .inner
        .store
        .insert_artifact(&artifact, "local")
        .await
        .unwrap();
    let app = app(state.clone());
    let signed = |subject: &str, method: Method, path: &str| {
        let claims =
            neoism_agent_service_api::daemon_credential::DaemonCredentialClaims::new(
                subject,
                "http-host",
                "workspace:http-host",
                vec![dir.to_string_lossy().into_owned()],
                true,
                (now_millis() / 1000) as i64,
                60,
            )
            .unwrap();
        let token = neoism_agent_service_api::daemon_credential::issue(
            &claims,
            b"hosting-test-signing-key",
        )
        .unwrap();
        let mut req = request(method, path, None);
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        req.headers_mut()
            .insert("x-neoism-directory", dir.to_string_lossy().parse().unwrap());
        req
    };
    let guest_adoption = app
        .clone()
        .oneshot(signed(
            "device:guest",
            Method::POST,
            "/v2/hosting/associate",
        ))
        .await
        .unwrap();
    assert_eq!(guest_adoption.status(), StatusCode::FORBIDDEN);
    let adopted: Value = response_json(
        app.clone()
            .oneshot(signed(
                crate::hosting::HOST_ASSOCIATION_SUBJECT,
                Method::POST,
                "/v2/hosting/associate",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(adopted["workspaceId"], "http-host");
    let mcp_catalog = app
        .clone()
        .oneshot(signed(
            "device:guest",
            Method::GET,
            "/v2/plugins/dev.neoism.mcp/catalog",
        ))
        .await
        .unwrap();
    assert_eq!(mcp_catalog.status(), StatusCode::OK);
    let mcp_catalog: Value = response_json(mcp_catalog).await;
    assert!(mcp_catalog.is_object());
    let mut child_request = signed("device:guest", Method::POST, "/v2/sessions");
    child_request
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    *child_request.body_mut() = Body::from(
        json!({"parentId": old.id, "title": "continued child task"}).to_string(),
    );
    let child: SessionInfo =
        response_json(app.clone().oneshot(child_request).await.unwrap()).await;
    assert_eq!(child.parent_id, Some(old.id.clone()));
    assert_eq!(child.workspace_id.as_deref(), Some("http-host"));
    assert_eq!(
        state
            .inner
            .store
            .associate_host_directory(dir.to_str().unwrap(), "repeat-http-host")
            .await
            .unwrap(),
        "http-host"
    );
    let retrieved: neoism_agent_core::ArtifactInfo = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                Method::GET,
                "/v2/artifacts/artifact_hosting_history",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(retrieved.session_id, Some(old.id.to_string()));
    let attachments: Vec<neoism_agent_core::ArtifactInfo> = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                Method::GET,
                &format!("/v2/artifacts?sessionId={}", old.id),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(attachments.len(), 1);
    let history: neoism_agent_core::Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                Method::GET,
                &format!("/v2/sessions/{}/messages", old.id),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        serde_json::to_value(history.items).unwrap(),
        json!([message])
    );

    // Joined peers must receive the same canonical history cursor as local
    // clients. Authorization remains session/workspace based on every page;
    // the cursor itself grants no access.
    let second = store_test_message(&old.id, now_millis() + 1, "after hosting");
    state
        .inner
        .store
        .append_message(old.id.as_str(), &second)
        .await
        .unwrap();
    let first_page: neoism_agent_core::Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                Method::GET,
                &format!("/v2/sessions/{}/messages?order=desc&limit=1", old.id),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(
        crate::message_id_of(&first_page.items[0]),
        crate::message_id_of(&second)
    );
    let cursor = first_page
        .cursor
        .next
        .expect("full page exposes an older cursor");
    let older_page: neoism_agent_core::Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                Method::GET,
                &format!(
                    "/v2/sessions/{}/messages?order=desc&limit=1&cursor={cursor}",
                    old.id
                ),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(older_page.items.len(), 1);
    assert_eq!(
        crate::message_id_of(&older_page.items[0]),
        crate::message_id_of(&message)
    );
    let list: neoism_agent_core::Page<SessionInfo> = response_json(
        app.clone()
            .oneshot(signed("device:guest", Method::GET, "/v2/sessions"))
            .await
            .unwrap(),
    )
    .await;
    assert!(list.items.iter().any(|session| session.id == old.id));
    state.inner.store.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

fn claims(
    root: &std::path::Path,
    workspace: &str,
    subject: &str,
) -> crate::caller::CallerClaims {
    crate::caller::CallerClaims {
        subject: subject.into(),
        workspace_id: Some(workspace.into()),
        tenant_id: format!("workspace:{workspace}"),
        directory_prefixes: vec![root.to_string_lossy().into_owned()],
        hosted: true,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
        resolved: None,
        worker: None,
    }
}

#[tokio::test]
async fn hosting_preserves_local_family_and_reuses_namespace_without_stealing() {
    let dir = std::env::temp_dir().join(format!(
        "agent-hosting-{}",
        neoism_agent_core::new_session_id()
    ));
    let root = dir.join("repo");
    let other = dir.join("other");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let path = dir.join("agent.db");
    let store = SessionStore::open(path.clone()).await.unwrap();
    let mut old = store_test_session(&neoism_agent_core::new_session_id(), 123);
    old.directory = root.join(".").to_string_lossy().into_owned();
    let mut child = store_test_session(&neoism_agent_core::new_session_id(), 124);
    child.directory = old.directory.clone();
    child.parent_id = Some(old.id.clone());
    let mut unrelated = store_test_session(&neoism_agent_core::new_session_id(), 125);
    unrelated.directory = other.to_string_lossy().into_owned();
    let mut owned = store_test_session(&neoism_agent_core::new_session_id(), 126);
    owned.directory = old.directory.clone();
    owned.workspace_id = Some("different".into());
    owned.extra.insert(
        crate::caller::TENANT_EXTRA_KEY.into(),
        json!("workspace:different"),
    );
    for session in [&old, &child, &unrelated, &owned] {
        store.insert_session(session).await.unwrap();
    }
    let message = store_test_message(&old.id, 123, "original history");
    store
        .append_message(old.id.as_str(), &message)
        .await
        .unwrap();
    assert_eq!(
        store
            .associate_host_directory(root.to_str().unwrap(), "host-one")
            .await
            .unwrap(),
        "host-one"
    );
    let guest = claims(&root, "host-one", "device:guest");
    let host = claims(&root, "host-one", "local-operator");
    let mut local = host.clone();
    local.hosted = false;
    local.workspace_id = None;
    local.tenant_id = "local".into();
    let loaded = store.list_sessions().await.unwrap();
    let page = store
        .list_root_sessions_page(
            crate::state::TenantQueryScope::LocalAll,
            root.to_str(),
            None,
            None,
            None,
            None,
            Some(50),
        )
        .await
        .unwrap();
    assert!(page
        .items
        .iter()
        .any(|session| session.id == old.id
            && crate::caller::allows_session(&guest, session)));
    assert_eq!(loaded.len(), 4);
    for id in [&old.id, &child.id] {
        let session = loaded.iter().find(|s| &s.id == id).unwrap();
        assert!(crate::caller::allows_session(&guest, session));
        assert!(crate::caller::allows_session(&host, session));
        assert!(crate::caller::allows_session(&local, session));
        assert_eq!(session.id, *id);
    }
    assert!(!crate::caller::allows_session(
        &guest,
        loaded.iter().find(|s| s.id == unrelated.id).unwrap()
    ));
    assert!(!crate::caller::allows_session(
        &guest,
        loaded.iter().find(|s| s.id == owned.id).unwrap()
    ));
    // A running adopted child may persist its projected namespace before its
    // parent writes again; repeat hosting must still recognize that family.
    let persisted_child = store.get_session(child.id.as_str()).await.unwrap().unwrap();
    store.update_session(&persisted_child).await.unwrap();
    assert_eq!(
        store
            .associate_host_directory(root.to_str().unwrap(), "host-again")
            .await
            .unwrap(),
        "host-one"
    );
    // A normal metadata write must not fork the history or lose local access.
    let mut saved = store.get_session(old.id.as_str()).await.unwrap().unwrap();
    saved.title = "continued by guest".into();
    store.update_session(&saved).await.unwrap();
    assert_eq!(
        store
            .associate_host_directory(root.join(".").to_str().unwrap(), "host-two")
            .await
            .unwrap(),
        "host-one"
    );
    assert!(crate::caller::allows_session(
        &local,
        &store.get_session(old.id.as_str()).await.unwrap().unwrap()
    ));
    assert_eq!(
        serde_json::to_value(store.list_messages(old.id.as_str()).await.unwrap())
            .unwrap(),
        json!([message])
    );
    assert_eq!(
        store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap()
            .parent_id,
        Some(old.id)
    );
    // Imported/user extras cannot forge the store's local-access marker.
    owned
        .extra
        .insert(crate::caller::HOST_LOCAL_ACCESS_KEY.into(), json!(true));
    store.update_session(&owned).await.unwrap();
    assert!(!crate::caller::allows_session(
        &local,
        &store.get_session(owned.id.as_str()).await.unwrap().unwrap()
    ));
    store.close().await;
    let reopened = SessionStore::open(path).await.unwrap();
    assert_eq!(
        reopened
            .associate_host_directory(root.to_str().unwrap(), "host-three")
            .await
            .unwrap(),
        "host-one"
    );
    reopened.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn hosting_skips_entire_out_of_root_family_and_rehosts_eligible_history() {
    let _guard = env_lock();
    let dir = std::env::temp_dir().join(format!(
        "agent-host-filter-{}",
        neoism_agent_core::new_session_id()
    ));
    let root = dir.join("repo");
    let child_dir = root.join("task");
    std::fs::create_dir_all(&child_dir).unwrap();
    let path = dir.join("agent.db");
    let previous = std::env::var_os("NEOISM_DAEMON_TOKEN");
    std::env::set_var("NEOISM_DAEMON_TOKEN", "hosting-filter-test-key");
    let state = AppState::open_database(path.clone()).await.unwrap();
    match previous {
        Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
        None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
    }
    let store = &state.inner.store;
    let app = app(state.clone());
    let signed = |subject: &str, workspace: &str, method: Method, uri: &str| {
        let credential =
            neoism_agent_service_api::daemon_credential::DaemonCredentialClaims::new(
                subject,
                workspace,
                format!("workspace:{workspace}"),
                vec![root.to_string_lossy().into_owned()],
                true,
                (now_millis() / 1000) as i64,
                60,
            )
            .unwrap();
        let token = neoism_agent_service_api::daemon_credential::issue(
            &credential,
            b"hosting-filter-test-key",
        )
        .unwrap();
        let mut req = request(method, uri, None);
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        req.headers_mut().insert(
            "x-neoism-directory",
            root.to_string_lossy().parse().unwrap(),
        );
        req
    };
    let mut valid = store_test_session(&neoism_agent_core::new_session_id(), 123);
    valid.directory = root.to_string_lossy().into_owned();
    let mut valid_child = store_test_session(&neoism_agent_core::new_session_id(), 124);
    valid_child.parent_id = Some(valid.id.clone());
    valid_child.directory = child_dir.to_string_lossy().into_owned();
    let mut private = store_test_session(&neoism_agent_core::new_session_id(), 125);
    private.directory = valid.directory.clone();
    let mut private_inside =
        store_test_session(&neoism_agent_core::new_session_id(), 126);
    private_inside.parent_id = Some(private.id.clone());
    private_inside.directory = valid.directory.clone();
    let mut private_outside =
        store_test_session(&neoism_agent_core::new_session_id(), 127);
    private_outside.parent_id = Some(private.id.clone());
    // Mirrors a project-root conversation with a historical home-directory task.
    private_outside.directory = dir.to_string_lossy().into_owned();
    for session in [
        &valid,
        &valid_child,
        &private,
        &private_inside,
        &private_outside,
    ] {
        store.insert_session(session).await.unwrap();
    }
    let history = store_test_message(&private.id, 125, "private original history");
    store
        .append_message(private.id.as_str(), &history)
        .await
        .unwrap();
    let before = store.hosting_history_json_for_test().await.unwrap();
    for (id, session_id) in [
        ("eligible-artifact", Some(valid.id.to_string())),
        ("private-artifact", Some(private.id.to_string())),
        ("unattached-artifact", None),
    ] {
        store
            .insert_artifact(
                &neoism_agent_core::ArtifactInfo {
                    id: id.into(),
                    filename: "test.txt".into(),
                    media_type: "text/plain".into(),
                    size: 1,
                    sha256: "test".into(),
                    created: now_millis(),
                    session_id,
                    download_url: String::new(),
                },
                "local",
            )
            .await
            .unwrap();
    }
    let guest = claims(&root, "host-first", "device:guest");
    let mut local = guest.clone();
    local.hosted = false;
    local.workspace_id = None;
    local.tenant_id = "local".into();
    local.directory_prefixes = vec![dir.to_string_lossy().into_owned()];
    for requested in ["host-first", "host-next"] {
        let associated: Value = response_json(
            app.clone()
                .oneshot(signed(
                    crate::hosting::HOST_ASSOCIATION_SUBJECT,
                    requested,
                    Method::POST,
                    "/v2/hosting/associate",
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(associated["workspaceId"], "host-first");
        let artifacts: Vec<neoism_agent_core::ArtifactInfo> = response_json(
            app.clone()
                .oneshot(signed(
                    "device:guest",
                    "host-first",
                    Method::GET,
                    "/v2/artifacts",
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id, "eligible-artifact");
        for session in [&valid, &valid_child] {
            let loaded = store
                .get_session(session.id.as_str())
                .await
                .unwrap()
                .unwrap();
            assert!(crate::caller::allows_session(&guest, &loaded));
            assert_eq!(loaded.workspace_id.as_deref(), Some("host-first"));
            let hosted: SessionInfo = response_json(
                app.clone()
                    .oneshot(signed(
                        "device:guest",
                        "host-first",
                        Method::GET,
                        &format!("/v2/sessions/{}", session.id),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(hosted.id, session.id);
            let wrong_namespace = app
                .clone()
                .oneshot(signed(
                    "device:guest",
                    "unrelated-workspace",
                    Method::GET,
                    &format!("/v2/sessions/{}", session.id),
                ))
                .await
                .unwrap();
            assert_eq!(wrong_namespace.status(), StatusCode::NOT_FOUND);
        }
        for session in [&private, &private_inside, &private_outside] {
            let loaded = store
                .get_session(session.id.as_str())
                .await
                .unwrap()
                .unwrap();
            assert!(!crate::caller::allows_session(&guest, &loaded));
            assert!(crate::caller::allows_session(&local, &loaded));
            assert!(loaded.workspace_id.is_none());
            let denied = app
                .clone()
                .oneshot(signed(
                    "device:guest",
                    "host-first",
                    Method::GET,
                    &format!("/v2/sessions/{}", session.id),
                ))
                .await
                .unwrap();
            assert!(matches!(
                denied.status(),
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ));
            assert_eq!(
                serde_json::to_value(&loaded).unwrap(),
                serde_json::to_value(session).unwrap()
            );
        }
        let after = store.hosting_history_json_for_test().await.unwrap();
        assert_eq!(before, after, "hosting must not rewrite any history JSON");
        assert_eq!(
            serde_json::to_value(store.list_messages(private.id.as_str()).await.unwrap())
                .unwrap(),
            json!([history])
        );
    }
    store.close().await;
    let reopened = SessionStore::open(path).await.unwrap();
    assert_eq!(
        reopened
            .associate_host_directory(root.to_str().unwrap(), "host-after-restart")
            .await
            .unwrap(),
        "host-first"
    );
    assert!(reopened
        .get_session(private.id.as_str())
        .await
        .unwrap()
        .unwrap()
        .workspace_id
        .is_none());
    reopened.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn hosting_revokes_existing_and_persisted_family_grants_after_local_continuation() {
    let _guard = env_lock();
    let dir = std::env::temp_dir().join(format!(
        "agent-host-revoke-{}",
        neoism_agent_core::new_session_id()
    ));
    let root_dir = dir.join("repo");
    std::fs::create_dir_all(&root_dir).unwrap();
    let db = dir.join("agent.db");
    let previous = std::env::var_os("NEOISM_DAEMON_TOKEN");
    std::env::set_var("NEOISM_DAEMON_TOKEN", "hosting-revoke-test-key");
    let state = AppState::open_database(db.clone()).await.unwrap();
    match previous {
        Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
        None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
    }
    let store = &state.inner.store;
    let app = app(state.clone());
    let signed = |subject: &str, workspace: &str, method: Method, uri: &str| {
        let claims =
            neoism_agent_service_api::daemon_credential::DaemonCredentialClaims::new(
                subject,
                workspace,
                format!("workspace:{workspace}"),
                vec![root_dir.to_string_lossy().into_owned()],
                true,
                (now_millis() / 1000) as i64,
                60,
            )
            .unwrap();
        let token = neoism_agent_service_api::daemon_credential::issue(
            &claims,
            b"hosting-revoke-test-key",
        )
        .unwrap();
        let mut req = request(method, uri, None);
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        req.headers_mut().insert(
            "x-neoism-directory",
            root_dir.to_string_lossy().parse().unwrap(),
        );
        req
    };
    let mut root = store_test_session(&neoism_agent_core::new_session_id(), 123);
    root.directory = root_dir.to_string_lossy().into_owned();
    let mut child = store_test_session(&neoism_agent_core::new_session_id(), 124);
    child.parent_id = Some(root.id.clone());
    child.directory = root.directory.clone();
    // A separate eligible family must remain served after the first is revoked.
    let mut safe = store_test_session(&neoism_agent_core::new_session_id(), 125);
    safe.directory = root.directory.clone();
    for session in [&root, &child, &safe] {
        store.insert_session(session).await.unwrap();
    }
    let message = store_test_message(&root.id, 123, "original local history");
    store
        .append_message(root.id.as_str(), &message)
        .await
        .unwrap();
    let adopted: Value = response_json(
        app.clone()
            .oneshot(signed(
                crate::hosting::HOST_ASSOCIATION_SUBJECT,
                "revocation-host",
                Method::POST,
                "/v2/hosting/associate",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(adopted["workspaceId"], "revocation-host");
    let initial: SessionInfo = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                "revocation-host",
                Method::GET,
                &format!("/v2/sessions/{}", root.id),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(initial.id, root.id);
    // Simulate a normal edit saving a hydrated overlay into historical JSON.
    let persisted = store.get_session(root.id.as_str()).await.unwrap().unwrap();
    store.update_session(&persisted).await.unwrap();
    // Exercise the actual local continuation path that inherits the parent's namespace.
    let mut continuation = request(
        Method::POST,
        "/v2/sessions",
        Some(json!({
            "parentId": root.id, "title": "local home-directory continuation",
        })),
    );
    continuation
        .headers_mut()
        .insert("x-neoism-directory", dir.to_string_lossy().parse().unwrap());
    let outside: SessionInfo =
        response_json(app.clone().oneshot(continuation).await.unwrap()).await;
    assert_eq!(outside.directory, dir.to_string_lossy());
    assert_eq!(outside.workspace_id.as_deref(), Some("revocation-host"));
    let children: neoism_agent_core::Page<SessionInfo> = response_json(
        app.clone()
            .oneshot(signed(
                "device:guest",
                "revocation-host",
                Method::GET,
                &format!("/v2/sessions/{}/children", root.id),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        children.items.len(),
        1,
        "each child must be authorized independently even before rehosting"
    );
    assert_eq!(children.items[0].id, child.id);
    for (id, session, tenant) in [
        ("revoked-local-artifact", &root, "local"),
        (
            "revoked-persisted-artifact",
            &root,
            "workspace:revocation-host",
        ),
        ("still-safe-artifact", &safe, "local"),
    ] {
        store
            .insert_artifact(
                &neoism_agent_core::ArtifactInfo {
                    id: id.into(),
                    filename: "original.txt".into(),
                    media_type: "text/plain".into(),
                    size: 1,
                    sha256: "test".into(),
                    created: now_millis(),
                    session_id: Some(session.id.to_string()),
                    download_url: String::new(),
                },
                tenant,
            )
            .await
            .unwrap();
    }
    let before = store.hosting_history_json_for_test().await.unwrap();
    for requested in ["revocation-next", "revocation-again"] {
        let adopted: Value = response_json(
            app.clone()
                .oneshot(signed(
                    crate::hosting::HOST_ASSOCIATION_SUBJECT,
                    requested,
                    Method::POST,
                    "/v2/hosting/associate",
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(adopted["workspaceId"], "revocation-host");
        for session in [&root, &child, &outside] {
            for suffix in ["", "/messages", "/children"] {
                let uri = format!("/v2/sessions/{}{suffix}", session.id);
                let denied = app
                    .clone()
                    .oneshot(signed("device:guest", "revocation-host", Method::GET, &uri))
                    .await
                    .unwrap();
                assert!(
                    matches!(
                        denied.status(),
                        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
                    ),
                    "revoked guest route {uri}"
                );
            }
            let local: SessionInfo = response_json(
                app.clone()
                    .oneshot(request(
                        Method::GET,
                        &format!("/v2/sessions/{}", session.id),
                        None,
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(local.id, session.id);
            assert!(local.workspace_id.is_none());
            assert_eq!(crate::caller::session_tenant(&local), "local");
        }
        let history: neoism_agent_core::Page<MessageWithParts> = response_json(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    &format!("/v2/sessions/{}/messages", root.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            serde_json::to_value(history.items).unwrap(),
            json!([message])
        );
        let local_children: neoism_agent_core::Page<SessionInfo> = response_json(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    &format!("/v2/sessions/{}/children", root.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(local_children.items.len(), 2);
        for id in ["revoked-local-artifact", "revoked-persisted-artifact"] {
            let uri = format!("/v2/artifacts/{id}");
            let denied = app
                .clone()
                .oneshot(signed("device:guest", "revocation-host", Method::GET, &uri))
                .await
                .unwrap();
            assert!(matches!(
                denied.status(),
                StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
            ));
            let local: neoism_agent_core::ArtifactInfo = response_json(
                app.clone()
                    .oneshot(request(Method::GET, &uri, None))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(local.id, id);
        }
        let artifacts: Vec<neoism_agent_core::ArtifactInfo> = response_json(
            app.clone()
                .oneshot(signed(
                    "device:guest",
                    "revocation-host",
                    Method::GET,
                    "/v2/artifacts",
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id, "still-safe-artifact");
        let local_artifacts: Vec<neoism_agent_core::ArtifactInfo> = response_json(
            app.clone()
                .oneshot(request(Method::GET, "/v2/artifacts", None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(local_artifacts.len(), 3);
        let still_safe: SessionInfo = response_json(
            app.clone()
                .oneshot(signed(
                    "device:guest",
                    "revocation-host",
                    Method::GET,
                    &format!("/v2/sessions/{}", safe.id),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(still_safe.id, safe.id);
        assert_eq!(store.hosting_history_json_for_test().await.unwrap(), before);
    }
    store.close().await;
    let previous = std::env::var_os("NEOISM_DAEMON_TOKEN");
    std::env::set_var("NEOISM_DAEMON_TOKEN", "hosting-revoke-test-key");
    let reopened_state = AppState::open_database(db).await.unwrap();
    match previous {
        Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
        None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
    }
    let reopened = &reopened_state.inner.store;
    let reopened_app = crate::app(reopened_state.clone());
    let adopted: Value = response_json(
        reopened_app
            .clone()
            .oneshot(signed(
                crate::hosting::HOST_ASSOCIATION_SUBJECT,
                "after-reopen",
                Method::POST,
                "/v2/hosting/associate",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(adopted["workspaceId"], "revocation-host");
    for session in [&root, &child, &outside] {
        assert!(reopened
            .get_session_for_tenant("workspace:revocation-host", session.id.as_str())
            .await
            .unwrap()
            .is_none());
        let local = reopened
            .get_session(session.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert!(local.workspace_id.is_none());
        assert_eq!(crate::caller::session_tenant(&local), "local");
        for suffix in ["", "/messages", "/children"] {
            let uri = format!("/v2/sessions/{}{suffix}", session.id);
            let denied = reopened_app
                .clone()
                .oneshot(signed("device:guest", "revocation-host", Method::GET, &uri))
                .await
                .unwrap();
            assert!(
                matches!(
                    denied.status(),
                    StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
                ),
                "revoked reopened guest route {uri}"
            );
        }
        let local: SessionInfo = response_json(
            reopened_app
                .clone()
                .oneshot(request(
                    Method::GET,
                    &format!("/v2/sessions/{}", session.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(local.id, session.id);
    }
    for id in ["revoked-local-artifact", "revoked-persisted-artifact"] {
        let uri = format!("/v2/artifacts/{id}");
        let denied = reopened_app
            .clone()
            .oneshot(signed("device:guest", "revocation-host", Method::GET, &uri))
            .await
            .unwrap();
        assert!(matches!(
            denied.status(),
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
        ));
        let local: neoism_agent_core::ArtifactInfo = response_json(
            reopened_app
                .clone()
                .oneshot(request(Method::GET, &uri, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(local.id, id);
    }
    let history: neoism_agent_core::Page<MessageWithParts> = response_json(
        reopened_app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", root.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        serde_json::to_value(history.items).unwrap(),
        json!([message])
    );
    let artifacts: Vec<neoism_agent_core::ArtifactInfo> = response_json(
        reopened_app
            .clone()
            .oneshot(signed(
                "device:guest",
                "revocation-host",
                Method::GET,
                "/v2/artifacts",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, "still-safe-artifact");
    assert_eq!(
        reopened.hosting_history_json_for_test().await.unwrap(),
        before
    );
    reopened.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn hosting_rejects_conflicting_family_atomically_and_guest_cannot_associate() {
    let dir = std::env::temp_dir().join(format!(
        "agent-host-auth-{}",
        neoism_agent_core::new_session_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::open_database(dir.join("agent.db")).await.unwrap();
    let mut root = store_test_session(&neoism_agent_core::new_session_id(), 123);
    root.directory = dir.to_string_lossy().into_owned();
    let mut child = root.clone();
    child.id = neoism_agent_core::new_session_id();
    child.parent_id = Some(root.id.clone());
    child.workspace_id = Some("other-owner".into());
    child.extra.insert(
        crate::caller::TENANT_EXTRA_KEY.into(),
        json!("workspace:other-owner"),
    );
    state.inner.store.insert_session(&root).await.unwrap();
    state.inner.store.insert_session(&child).await.unwrap();
    assert!(state
        .inner
        .store
        .associate_host_directory(dir.to_str().unwrap(), "first")
        .await
        .is_err());
    // Directory ineligibility must not mask a foreign ownership conflict.
    child.directory = std::env::temp_dir().to_string_lossy().into_owned();
    state.inner.store.update_session(&child).await.unwrap();
    assert!(state
        .inner
        .store
        .associate_host_directory(dir.to_str().unwrap(), "first")
        .await
        .is_err());
    assert!(state
        .inner
        .store
        .get_session(root.id.as_str())
        .await
        .unwrap()
        .unwrap()
        .workspace_id
        .is_none());
    for subject in ["device:guest", "trust-local", "local-operator"] {
        let result = crate::hosting::associate(
            axum::extract::State(state.clone()),
            Some(axum::Extension(claims(&dir, "first", subject))),
        )
        .await;
        assert!(result.is_err());
    }
    assert!(
        crate::hosting::associate(axum::extract::State(state.clone()), None)
            .await
            .is_err()
    );
    state
        .inner
        .store
        .delete_session(child.id.as_str())
        .await
        .unwrap();
    // Failed adoption rolled back even the directory reservation.
    assert_eq!(
        state
            .inner
            .store
            .associate_host_directory(dir.to_str().unwrap(), "second")
            .await
            .unwrap(),
        "second"
    );
    state.inner.store.close().await;
    let _ = std::fs::remove_dir_all(dir);
}
