use super::*;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

async fn request(
    router: &axum::Router,
    claims: Option<&CallerClaims>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(
            body.map(|value| Body::from(value.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    if let Some(claims) = claims {
        request.extensions_mut().insert(claims.clone());
    }
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn global_skills_are_root_bound_not_project_bound() {
    let temp =
        std::env::temp_dir().join(format!("agent-global-skills-{}", crate::slug()));
    let project = temp.join("project");
    // Deliberately beneath the project: adapter-declared scope, not starts_with,
    // determines installation identity.
    let global = project.join("configured-global");
    fs::create_dir_all(&project).unwrap();
    let mut services = crate::standard_services();
    services.config = Arc::new(
        neoism_agent_service_api::StandardConfigSourceService::new(&global),
    );
    let state = AppState::open_database_with_services_and_management(
        temp.join("state.db"),
        services,
        ManagementPolicy::enabled(),
    )
    .await
    .unwrap();
    let router = axum::Router::new()
        .route("/skills", axum::routing::get(list_skills))
        .route(
            "/skills/:id",
            axum::routing::get(get_skill)
                .post(create_skill)
                .put(update_skill)
                .delete(delete_skill),
        )
        .route(
            "/skills/:id/versions",
            axum::routing::get(list_skill_versions),
        )
        .route(
            "/skills/:id/versions/:version",
            axum::routing::get(get_skill_version),
        )
        .route(
            "/skills/:id/versions/:version/restore",
            axum::routing::post(restore_skill_version),
        )
        .with_state(state.clone());
    let claims = CallerClaims {
        subject: "operator".into(),
        workspace_id: None,
        tenant_id: "local".into(),
        directory_prefixes: vec![project.display().to_string()],
        hosted: false,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
    };
    let body = json!({"scope":"installation", "name":"Global tool", "description":"Global instructions", "content":"Instructions", "compatibility":false, "metadata":{"zero":0}, "files":{"references/help.md":"Complete support file"}});
    let uri = "/skills/tool?scope=installation";
    assert_eq!(
        request(&router, None, "POST", uri, Some(body.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, created) =
        request(&router, Some(&claims), "POST", uri, Some(body.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(global.join("skills/tool/SKILL.md").is_file());
    assert!(!project.join(".agent").exists());
    assert_eq!(created["scope"], "installation");
    let (status, versions) = request(
        &router,
        Some(&claims),
        "GET",
        "/skills/tool/versions?scope=installation",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let version = versions[0]["id"].as_str().unwrap();
    assert!(version.starts_with(&skill_version_root_prefix(&global)));
    let (_, bundle) = request(
        &router,
        Some(&claims),
        "GET",
        &format!("/skills/tool/versions/{version}?scope=installation"),
        None,
    )
    .await;
    assert_eq!(bundle["bundle"]["files"], body["files"]);
    let (_, read) = request(
        &router,
        Some(&claims),
        "GET",
        &format!("{uri}&directory={}", project.display()),
        None,
    )
    .await;
    assert_eq!(read["definition"]["bundle"]["compatibility"], false);
    assert_eq!(read["definition"]["bundle"]["files"], body["files"]);
    let mut edited = body.clone();
    edited["content"] = json!("Edited");
    edited["expectedRevision"] = created["revision"].clone();
    let (status, updated) =
        request(&router, Some(&claims), "PUT", uri, Some(edited)).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let (status, restored) = request(&router, Some(&claims), "POST", &format!("/skills/tool/versions/{version}/restore?scope=installation&expectedRevision={}", updated["revision"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert!(fs::read_to_string(global.join("skills/tool/SKILL.md"))
        .unwrap()
        .contains("Instructions"));
    let (status, _) = request(
        &router,
        Some(&claims),
        "POST",
        &format!(
            "/skills/tool/versions/{version}/restore?scope=workspace&directory={}",
            project.display()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut forbidden = claims.clone();
    forbidden.directory_prefixes = vec![temp.join("other").display().to_string()];
    for suffix in [
        "?scope=installation".to_string(),
        format!("/versions/{version}?scope=installation"),
        format!("/versions/{version}/restore?scope=installation"),
    ] {
        let method = if suffix.contains("/restore") {
            "POST"
        } else {
            "GET"
        };
        assert_eq!(
            request(
                &router,
                Some(&forbidden),
                method,
                &format!("/skills/tool{suffix}"),
                None
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &router,
            Some(&claims),
            "DELETE",
            &format!(
                "{uri}&expectedRevision={}",
                restored["revision"].as_str().unwrap()
            ),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(!global.join("skills/tool").exists());
    assert!(!project.join(".agent").exists());
    state.shutdown().await.unwrap();
    fs::remove_dir_all(temp).unwrap();
}
