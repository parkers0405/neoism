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
        .list_root_sessions_page(root.to_str(), None, None, None, None, Some(50))
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
