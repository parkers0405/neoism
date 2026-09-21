//! Read-only process-owner identity, not authentication/account identity.
use crate::state::AppState;
use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Identity {
    configured_name: Option<String>,
    system_name: Option<String>,
}

fn name(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(32).collect())
}
fn resolve(
    env: Option<&str>,
    configured: Option<&str>,
    user: Option<&str>,
    username: Option<&str>,
) -> Identity {
    Identity {
        configured_name: name(env).or_else(|| name(configured)),
        system_name: name(user).or_else(|| name(username)),
    }
}

pub(crate) async fn get(State(state): State<AppState>) -> Json<Identity> {
    // Product configuration reads may touch disk. Do not block the async worker.
    let configured =
        tokio::task::spawn_blocking(move || state.services().config.display_name())
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
    Json(resolve(
        std::env::var("NEOISM_DISPLAY_NAME").ok().as_deref(),
        configured.as_deref(),
        std::env::var("USER").ok().as_deref(),
        std::env::var("USERNAME").ok().as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn read_endpoint_exposes_only_identity_fields_and_capability() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let root = std::env::temp_dir()
            .join(format!("neoism-identity-route-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::open_database(root.join("agent.sqlite3"))
            .await
            .unwrap();
        let router = crate::app(state);
        let response = router
            .clone()
            .oneshot(Request::get("/v2/identity").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert!(value.get("configuredName").is_some());
        assert!(value.get("systemName").is_some());
        let response = router
            .oneshot(
                Request::get("/v2/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let capabilities: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(capabilities
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "neoism.identity" && c["enabled"] == true));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn priority_and_no_hostname_guessing() {
        let id = resolve(
            Some(" Env "),
            Some("Configured"),
            Some("alice"),
            Some("windows"),
        );
        assert_eq!(id.configured_name.as_deref(), Some("Env"));
        assert_eq!(id.system_name.as_deref(), Some("alice"));
        let id = resolve(Some(" "), Some(" Configured "), Some(" "), Some("windows"));
        assert_eq!(id.configured_name.as_deref(), Some("Configured"));
        assert_eq!(id.system_name.as_deref(), Some("windows"));
        assert!(resolve(None, None, None, None).system_name.is_none());
        assert_eq!(name(Some(&"é".repeat(40))).unwrap().chars().count(), 32);
    }
}
