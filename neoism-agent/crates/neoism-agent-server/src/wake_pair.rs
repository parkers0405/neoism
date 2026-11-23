//! Narrow local-shell delegation. The master credential never crosses this handoff.
use axum::http::Method;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::SocketAddr;

pub(crate) const PREFIX: &str = "neoism-wake-v1.";

// Allowlist the complete wire shape, not today's optional V2PromptRequest
// fields: new general-purpose prompt options must not expand the delegation.
pub(crate) async fn allow_text_prompt(body: &mut axum::body::Body) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Text {
        prompt: String,
    }
    let Ok(bytes) = axum::body::to_bytes(std::mem::take(body), 32 * 1024).await else {
        return false;
    };
    let Ok(text) = serde_json::from_slice::<Text>(&bytes) else {
        return false;
    };
    if text.prompt.trim().is_empty() || text.prompt.len() > 16384 {
        return false;
    }
    *body = bytes.into();
    true
}

// Do not put a bearer in an inherited Windows TEMP ACL. Add a tested
// private-DACL transport before enabling automatic Windows handoffs.
#[cfg(not(unix))]
pub(crate) fn handoff(
    _: &crate::AppState,
    _: &neoism_agent_core::SessionInfo,
) -> anyhow::Result<Option<tempfile::NamedTempFile>> {
    Ok(None)
}

#[cfg(unix)]
pub(crate) fn handoff(
    state: &crate::AppState,
    session: &neoism_agent_core::SessionInfo,
) -> anyhow::Result<Option<tempfile::NamedTempFile>> {
    if crate::caller::session_tenant(session) != "local" {
        return Ok(None);
    }
    let (Some(key), Some(&addr)) = (
        state.inner.caller_policy.local_wake_key(),
        state.inner.wake_listener.get(),
    ) else {
        return Ok(None);
    };
    let Some(token) = credential(key, addr, session.id.as_str()) else {
        return Ok(None);
    };
    let mut file = tempfile::Builder::new().prefix("neoism-wake-").tempfile()?;
    serde_json::to_writer(
        file.as_file_mut(),
        &serde_json::json!({"session": session.id, "origin": format!("http://{addr}"), "workspace": session.directory, "bearer": token}),
    )?;
    Ok(Some(file))
}

pub(crate) fn credential(
    key: &[u8],
    listener: SocketAddr,
    session: &str,
) -> Option<String> {
    let id = session.strip_prefix("ses_")?;
    if !listener.ip().is_loopback()
        || !(1..=128).contains(&id.len())
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(format!("{PREFIX}{listener}/{session}").as_bytes());
    let digest = mac.finalize().into_bytes();
    Some(format!("{PREFIX}{digest:x}"))
}

fn route_session<'a>(method: &Method, path: &'a str) -> Option<&'a str> {
    let tail = path.strip_prefix("/v2/sessions/")?;
    match *method {
        Method::GET => Some(tail),
        Method::POST => tail.strip_suffix("/prompt-if-idle"),
        _ => None,
    }
}

pub(crate) async fn authorize(
    state: &crate::AppState,
    method: &Method,
    uri: &axum::http::Uri,
    token: &str,
) -> Option<crate::caller::CallerClaims> {
    if uri.query().is_some() {
        return None;
    }
    let id = route_session(method, uri.path())?;
    let expected = credential(
        state.inner.caller_policy.local_wake_key()?,
        *state.inner.wake_listener.get()?,
        id,
    )?;
    if !crate::caller::constant_time_eq(token.as_bytes(), expected.as_bytes()) {
        return None;
    }
    let session = state.inner.store.get_session(id).await.ok()??;
    if crate::caller::session_tenant(&session) != "local" {
        return None;
    }
    Some(crate::caller::CallerClaims {
        subject: "neoboard:wake".into(),
        workspace_id: session.workspace_id,
        tenant_id: "local".into(),
        directory_prefixes: vec![session.directory],
        hosted: false,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
        resolved: None,
        worker: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bound_to_authenticated_listener_method_and_conversation() {
        let addr = "127.0.0.1:1234".parse().unwrap();
        let token = |id| credential(b"disposable test key", addr, id);
        assert_ne!(token("ses_one"), token("ses_two"));
        assert_ne!(
            token("ses_one"),
            credential(
                b"disposable test key",
                "127.0.0.1:1235".parse().unwrap(),
                "ses_one"
            )
        );
        for (method, path) in [
            (Method::GET, "/v2/sessions"),
            (Method::GET, "/v2/sessions/ses_one/messages"),
            (Method::POST, "/v2/sessions/ses_one/prompt"),
            (Method::DELETE, "/v2/sessions/ses_one"),
        ] {
            assert!(route_session(&method, path).and_then(token).is_none());
        }
    }
}