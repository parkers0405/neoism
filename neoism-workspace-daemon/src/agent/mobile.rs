//! Explicit chat capability list. Never calls the general WS agent dispatcher.

use super::{events, handlers::*, http::{http_get_json, split_model_ref}, AgentInner, AgentReplySender, AgentSession};
use neoism_protocol::agent::{
    AgentClientMessage as Request, AgentServerMessage as Reply, Attachment,
    MOBILE_PHOTO_MAX_BYTES, MOBILE_PHOTO_MAX_COUNT, mobile_photo_filename, mobile_photo_mime,
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::task::JoinSet;

const DENIED: &str = "Not allowed on mobile chat";
const BUSY: &str = "Mobile chat is busy; retry shortly";

pub(crate) fn local_endpoint(input: &str) -> bool {
    url::Url::parse(input).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && match url.host() {
                Some(url::Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            }
            && url.username().is_empty() && url.password().is_none() && url.path() == "/"
            && url.query().is_none() && url.fragment().is_none()
    })
}

// These identifiers go into URL path segments in the shared chat handlers.
fn safe_segment(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn safe_prompt(text: &str) -> bool {
    let text = text.trim_start();
    !text.is_empty() && !text.starts_with(['/', '!'])
}

fn validate_photos(attachments: &[Attachment]) -> Result<(), &'static str> {
    if attachments.len() > MOBILE_PHOTO_MAX_COUNT
        || attachments.iter().map(|a| a.bytes.len()).sum::<usize>() > MOBILE_PHOTO_MAX_BYTES
    {
        return Err("Mobile prompts allow up to four photos and 4 MiB total");
    }
    for attachment in attachments {
        if !mobile_photo_mime(&attachment.kind)
            || attachment.path.as_deref().is_some_and(|name| !mobile_photo_filename(name))
        {
            return Err("Mobile attachments must be inline photos with no file paths or URLs");
        }
        // Match the declared MIME to the actual format and read the image header
        // without decoding or allocating a pixel buffer in the socket task.
        let valid_image = image_rs::guess_format(&attachment.bytes).is_ok_and(|format| {
            format.to_mime_type() == attachment.kind
                && image_rs::ImageReader::with_format(std::io::Cursor::new(&attachment.bytes), format)
                    .into_dimensions().is_ok_and(|(width, height)| width > 0 && height > 0)
        });
        if !valid_image { return Err("Mobile attachment data does not match its image type"); }
    }
    Ok(())
}

pub(crate) struct MobileAgent {
    agent: AgentSession,
    jobs: JoinSet<()>,
    label: String,
}

impl MobileAgent {
    pub(crate) fn new(endpoint: String, label: String, tx: AgentReplySender) -> Self {
        Self { agent: AgentSession::with_endpoint(None, String::new(), tx, endpoint), jobs: JoinSet::new(), label }
    }

    pub(crate) fn has_jobs(&self) -> bool { !self.jobs.is_empty() }

    pub(crate) async fn reap(&mut self) { let _ = self.jobs.join_next().await; }

    pub(crate) fn admit(&mut self, message: Request) -> Result<(), &'static str> {
        let inner = self.agent.inner.clone();
        match message {
            Request::ResumeStream { session_id } if safe_segment(&session_id) => {
                let handles = inner.stream_handles.lock();
                if handles.len() >= 17 && !handles.contains_key(&session_id) { return Err(BUSY); }
                drop(handles);
                events::start_event_stream(&inner, &session_id);
                return Ok(());
            }
            Request::StopStream { session_id } if safe_segment(&session_id) => {
                events::stop_event_stream(&inner, &session_id);
                return Ok(());
            }
            Request::Ping => {
                let _ = inner.tx.send(Reply::Pong);
                return Ok(());
            }
            _ => (),
        }
        if self.jobs.len() >= 16 { return Err(BUSY); }
        let author = self.label.clone();
        // Reject unknown capabilities before spawning anything.
        match &message {
            Request::ListSessionProjects { .. } | Request::ListProjectSessions { .. }
            | Request::ListThreads { .. } | Request::SearchThreads { .. }
            | Request::ListProviders | Request::ListModelCatalog { .. } | Request::GetConfigDefaults { .. }
            | Request::ConnectListProviders { .. } => (),
            Request::ConnectListConnections { provider_id } if safe_segment(provider_id) => (),
            Request::GetHistory { session_id, .. } | Request::SwitchThread { session_id }
                if safe_segment(session_id) => (),
            Request::CreateThread { directory, .. }
                if directory.as_deref().is_none_or(|path| std::path::Path::new(path).is_absolute()) => (),
            Request::SubmitPrompt { session_id, message_id, text, attachments, .. }
                if safe_segment(session_id) && safe_segment(message_id)
                    && (safe_prompt(text) || (!attachments.is_empty() && text.trim().is_empty())) => {
                validate_photos(attachments)?;
            }
            _ => return Err(DENIED),
        }
        match &message {
            Request::ListSessionProjects { .. } => events::start_catalog_event_stream(&inner, None),
            Request::SwitchThread { session_id } => {
                let handles = inner.stream_handles.lock();
                if handles.len() >= 17 && !handles.contains_key(session_id) { return Err(BUSY); }
                drop(handles);
                events::start_event_stream(&inner, session_id);
            }
            _ => (),
        }
        self.jobs.spawn(async move {
            let tx = inner.tx.clone();
            if tokio::time::timeout(Duration::from_secs(30), async move {
                match message {
                    Request::ListSessionProjects { catalog_identity, generation } =>
                        handle_session_projects(inner, catalog_identity, generation).await,
                    Request::ListProjectSessions { catalog_identity, directory, cursor, generation } =>
                        handle_project_sessions(inner, catalog_identity, directory, cursor, generation).await,
                    Request::ListThreads { directory, limit, cursor } =>
                        handle_list_threads(inner, directory, limit, cursor).await,
                    Request::SearchThreads { query, generation, directory } =>
                        handle_search_threads(inner, query, generation, directory).await,
                    Request::GetHistory { session_id, cursor, limit } =>
                        handle_get_history(inner, session_id, cursor, limit).await,
                    Request::SwitchThread { session_id } => handle_switch_thread(inner, session_id, false).await,
                    Request::SubmitPrompt { session_id, message_id, text, attachments, delivery, model, connection_id, thinking, .. } => {
                        if let Some(choice) = validated_selection(&inner, model, connection_id, thinking, Some(&session_id)).await {
                            handle_submit_prompt(inner, session_id, message_id, text, Some(author), attachments, None, choice.model, choice.connection_id, choice.thinking, delivery).await;
                        }
                    }
                    Request::CreateThread { title, directory, model, connection_id, thinking, .. } => {
                        // Only the default is resolved by the host; explicit paths must be known projects.
                        let directory = match directory {
                            None => dirs::home_dir().filter(|path| path.is_absolute() && path.parent().is_some()).and_then(|path| path.into_os_string().into_string().ok()),
                            Some(directory) => http_get_json(&inner, "/v2/session-projects").await.ok().is_some_and(|value| {
                                value["items"].as_array().is_some_and(|items| items.iter().any(|item| item["directory"].as_str() == Some(directory.as_str())))
                            }).then_some(directory),
                        };
                        if let Some(directory) = directory {
                            if let Some(choice) = validated_selection(&inner, model, connection_id, thinking, None).await {
                                handle_create_thread(inner, title, Some(directory), None, choice.model, choice.connection_id, choice.thinking, None, false).await;
                            }
                        } else {
                            let _ = inner.tx.send(Reply::Error { message: "Choose an existing chat project".into() });
                        }
                    }
                    Request::ListProviders => handle_list_providers(inner).await,
                    Request::ListModelCatalog { request_id, .. } => handle_model_catalog(inner, Some(request_id), None).await,
                    // Use the host-wide default; do not resolve a client-supplied config directory.
                    Request::GetConfigDefaults { .. } => handle_get_config_defaults(inner, None).await,
                    Request::ConnectListConnections { provider_id } => handle_connect_list_connections(inner, provider_id).await,
                    Request::ConnectListProviders { request_id, .. } => {
                        let reply = match http_get_json(&inner, "/v2/providers").await {
                            Ok(value) => Reply::ConnectProviderCatalog { providers: provider_status(&value), auth: json!({}), request_id },
                            Err(_) => match request_id {
                                Some(request_id) => Reply::ConnectProviderCatalogFailed { request_id, error: "Provider status unavailable".into() },
                                None => Reply::Error { message: "Provider status unavailable".into() },
                            },
                        };
                        let _ = inner.tx.send(reply);
                    }
                    _ => unreachable!("checked mobile capability"),
                }
            }).await.is_err() {
                let _ = tx.send(Reply::Error { message: "Agent request timed out".into() });
            }
        });
        Ok(())
    }
}

#[derive(Default)]
struct Selection {
    model: Option<String>,
    connection_id: Option<String>,
    thinking: Option<String>,
}

async fn validated_selection(inner: &AgentInner, model: Option<String>, connection_id: Option<String>, thinking: Option<String>, session_id: Option<&str>) -> Option<Selection> {
    let selection = validate_selection(inner, model, connection_id, thinking, session_id).await;
    if selection.is_none() {
        let _ = inner.tx.send(Reply::Error { message: "Model, reasoning level, or connection is not available on this host".into() });
    }
    selection
}

async fn validate_selection(inner: &AgentInner, model: Option<String>, connection_id: Option<String>, thinking: Option<String>, session_id: Option<&str>) -> Option<Selection> {
    let mut model = model.filter(|m| !m.is_empty());
    let connection_id = connection_id.filter(|id| !id.is_empty());
    let thinking = thinking.filter(|t| !t.is_empty());
    if model.is_none() && connection_id.is_none() && thinking.is_none() {
        return Some(Selection::default());
    }
    if model.is_none() {
        // The caller can override reasoning/account for an existing session
        // without restating its model. New chats must name one explicitly.
        let session = http_get_json(inner, &format!("/v2/sessions/{}", session_id?)).await.ok()?;
        let current = &session["model"];
        model = Some(format!("{}/{}", current["providerId"].as_str()?, current["id"].as_str().or_else(|| current["modelId"].as_str())?));
    }
    let (provider, id) = split_model_ref(model.as_deref()?)?;
    if !safe_segment(&provider) || id.len() > 256 || id.chars().any(char::is_control) {
        return None;
    }
    let catalog = http_get_json(inner, "/v2/providers/configured").await.ok()?;
    let models = catalog["providers"].as_array()?.iter().find(|p| p["id"].as_str() == Some(provider.as_str()))?["models"].as_object()?;
    let model_info = models.iter().find_map(|(key, entry)| (entry["id"].as_str().unwrap_or(key) == id).then_some(entry))?;
    if thinking.as_deref().is_some_and(|value| !supports_reasoning(model_info, value)) {
        return None;
    }
    if let Some(connection_id) = connection_id.as_deref() {
        if !safe_segment(connection_id) { return None; }
        let accounts = http_get_json(inner, &format!("/v2/providers/{provider}/connections")).await.ok()?;
        if !accounts.as_array()?.iter().any(|account| account["providerId"].as_str() == Some(provider.as_str())
            && account["connectionId"].as_str() == Some(connection_id)) { return None; }
    }
    Some(Selection { model, connection_id, thinking })
}

fn supports_reasoning(model: &Value, value: &str) -> bool {
    if !safe_segment(value) { return false; }
    if let Some(variants) = model["variants"].as_object().filter(|variants| !variants.is_empty()) {
        return variants.contains_key(value);
    }
    // Some catalog sources publish capability without named levels. Accept
    // only Rune's standard levels there; the provider still resolves them.
    model["reasoning"].as_bool().or_else(|| model["capabilities"]["reasoning"].as_bool()) == Some(true)
        && matches!(value, "minimal" | "low" | "medium" | "high" | "xhigh" | "max")
}

impl Drop for MobileAgent {
    fn drop(&mut self) {
        self.jobs.abort_all();
        for (_, handle) in self.agent.inner.stream_handles.lock().drain() { handle.abort(); }
    }
}

fn provider_status(value: &Value) -> Value {
    let all: Vec<_> = value["all"].as_array().into_iter().flatten().filter_map(|p| {
        Some(json!({ "id": p["id"].as_str()?, "name": p["name"].as_str()? }))
    }).collect();
    let connected: Vec<_> = value["connected"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    json!({"all": all, "connected": connected, "default": {}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::{get, post}, Json, Router};
    use tokio::{net::TcpListener, sync::mpsc, time::timeout};

    #[tokio::test]
    async fn mobile_model_and_connection_are_validated_and_forwarded_without_secrets() {
        let project = if cfg!(windows) { r"C:\project" } else { "/project" };
        let (requests, mut recorded) = mpsc::unbounded_channel();
        let prompt_requests = requests.clone();
        let app = Router::new()
            .route("/v2/health", get(|| async { Json(json!({"healthy":true,"version":"test","providerCredentialStore":"test"})) }))
            .route("/v2/session-projects", get(move || async move { Json(json!({"items":[{"directory":project,"updatedMs":1}]})) }))
            .route("/v2/providers/configured", get(|| async { Json(json!({"providers":[{"id":"openai","models":{"gpt-test":{"variants":{"high":{}}}}}]})) }))
            .route("/v2/providers/openai/connections", get(|| async { Json(json!([{
                "providerId":"openai","connectionId":"acct-1","label":"Host account","authType":"api","isDefault":true,
                "key":"never-serialize-this","refreshToken":"nor-this"
            }])) }))
            .route("/v2/sessions", post(move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>, Json(body): Json<Value>| async move {
                requests.send(json!({"directory": query.get("directory"), "body": body})).unwrap();
                Json(json!({"id":"test-session"}))
            }))
            .route("/v2/sessions/test-session/prompt", post(move |Json(body): Json<Value>| async move {
                prompt_requests.send(body).unwrap();
                Json(json!({}))
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let (tx, mut rx) = super::super::reply_queue::channel();
        let mut agent = MobileAgent::new(endpoint, "My phone".into(), tx);
        let admit = |agent: &mut MobileAgent, wire| agent.admit(serde_json::from_value(wire).unwrap());
        assert!(admit(&mut agent, json!({"ConnectListConnections":{"provider_id":"openai/../../settings"}})).is_err());
        admit(&mut agent, json!({"ConnectListConnections":{"provider_id":"openai"}})).unwrap();
        let status = timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        let status = serde_json::to_value(status).unwrap();
        assert_eq!(status["ConnectConnections"]["connections"][0], json!({
            "providerId":"openai","connectionId":"acct-1","label":"Host account","authType":"api","isDefault":true
        }));
        admit(&mut agent, json!({"CreateThread":{"directory":project,"model":"openai/gpt-test","connection_id":"acct-1","thinking":"high"}})).unwrap();
        let created = timeout(Duration::from_secs(5), recorded.recv()).await.unwrap().unwrap();
        assert_eq!(created["directory"], project);
        assert_eq!(created["body"]["model"], json!({"providerId":"openai","id":"gpt-test","connectionId":"acct-1","variant":"high"}));
        let _ = rx.recv().await; // ThreadCreated and ProviderState
        let _ = rx.recv().await;
        let home = dirs::home_dir().unwrap();
        admit(&mut agent, json!({"CreateThread":{"directory":null,"model":"openai/gpt-test"}})).unwrap();
        let created = timeout(Duration::from_secs(5), recorded.recv()).await.unwrap().unwrap();
        assert_eq!(created["directory"].as_str(), home.to_str());
        let _ = rx.recv().await;
        let _ = rx.recv().await;
        let prompt = |connection: &str, variant: &str| json!({"SubmitPrompt":{
            "session_id":"test-session","message_id":"msg-1","text":"Hello","author":"spoofed", "mode":"override", "delivery":"queue",
            "model":"openai/gpt-test","connection_id":connection,"thinking":variant
        }});
        for invalid in [prompt("missing", "high"), prompt("acct-1", "unsupported")] {
            admit(&mut agent, invalid).unwrap();
            assert!(matches!(timeout(Duration::from_secs(5), rx.recv()).await.unwrap(), Some(Reply::Error { .. })));
            assert!(recorded.try_recv().is_err());
        }
        admit(&mut agent, prompt("acct-1", "high")).unwrap();
        let submitted = timeout(Duration::from_secs(5), recorded.recv()).await.unwrap().unwrap();
        assert_eq!(submitted["model"], json!({"providerId":"openai","modelId":"gpt-test","connectionId":"acct-1","variant":"high"}));
        assert_eq!(submitted["author"], "My phone");
        assert!(submitted.get("agent").is_none());
        server.abort();
    }
}