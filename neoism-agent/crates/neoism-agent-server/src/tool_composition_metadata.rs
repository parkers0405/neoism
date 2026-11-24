//! Dedicated conditional metadata adapter protocol, not ordinary MCP tools/call.
//! Provision a unique shared key per adapter. The trusted adapter verifies the
//! request MAC and expected server name, then compares the definition digest
//! before serving the immutable version operation. It returns 412 on drift.
//! The response MAC covers the request digest and the exact response bytes;
//! a recorded response cannot authenticate a new nonce. This is authentication,
//! not encryption, and the payload is intentionally limited to public metadata.

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use futures::future::BoxFuture;
use hmac::{Hmac, Mac};
use neoism_agent_composition::{ExecutionResult, ToolDescriptor, ToolHost};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::mcp_admission::{check_metadata_candidate, CurrentMetadata, ReviewedMetadata, TOOL};
use crate::{state::AppState, workspace_runtime::PluginGenerationLease};

const ENV: &str = "NEOISM_WORKFLOW_COMPOSE_METADATA";
const PATH: &str = "neoism.provider_metadata";
const DEFINITION: &[u8] = br#"{"name":"provider_metadata","inputSchema":{"type":"object","additionalProperties":false,"properties":{}},"annotations":{"readOnlyHint":true}}"#;
const MAX_BODY: usize = 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    server: String,
    endpoint: String,
    approval_identity: String,
    definition_sha256: String,
    key_hex: String,
}

pub(crate) fn is_metadata(tool_ids: &[String]) -> bool {
    matches!(tool_ids, [id] if id.strip_prefix("mcp__").and_then(|rest| rest.strip_suffix("__provider_metadata")).is_some_and(valid_server))
}

fn valid_server(name: &str) -> bool {
    name.len() <= 64 && name.split('_').all(|part| !part.is_empty()
        && part.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))
}

pub(crate) fn validate(code: &str, ids: &[String]) -> Result<(), String> {
    if !is_metadata(ids) || code.trim().is_empty() || code.contains('\0')
        || code.len() > super::unattended::limits().max_source_bytes
    {
        return Err("invalid fixed metadata composition".into());
    }
    Ok(())
}

pub(crate) fn check_operator(action: &crate::workflow_action::ValidatedDirectAction) -> Result<()> {
    let crate::workflow_action::DirectAction::Compose { tool_ids, .. } = action.action() else {
        anyhow::bail!("expected metadata action");
    };
    let _ = operator_binding(tool_ids, action.approval_identity())?;
    Ok(())
}

fn operator_binding(ids: &[String], identity: &str) -> Result<Binding> {
    let raw = std::env::var(ENV).ok();
    parse_binding(raw.as_deref(), ids, identity)
}

fn parse_binding(raw: Option<&str>, ids: &[String], identity: &str) -> Result<Binding> {
    let raw = raw.filter(|value| value.len() <= 4096).context("metadata adapter not configured")?;
    // Never surface operator configuration or the shared key in API errors.
    let binding: Binding = serde_json::from_str(raw).map_err(|_| anyhow::anyhow!("invalid metadata binding"))?;
    ensure!(is_metadata(ids) && ids[0] == crate::mcp::tool_runtime_id(&binding.server, TOOL)
        && valid_server(&binding.server) && binding.approval_identity == identity,
        "metadata adapter is not approved for this action");
    decode_hex(&binding.key_hex).context("invalid metadata adapter key")?;
    ensure!(binding.definition_sha256 == definition_hash(), "metadata definition is not approved");
    Ok(binding)
}

fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2*i..2*i+2], 16).ok()?;
    }
    Some(bytes)
}

pub(crate) async fn run(
    state: &AppState,
    workspace: &str,
    generation: PluginGenerationLease,
    ids: &[String],
    code: &str,
    identity: &str,
) -> Result<ExecutionResult> {
    validate(code, ids).map_err(anyhow::Error::msg)?;
    let binding = operator_binding(ids, identity)?;
    run_bound(state, workspace, generation, ids, code, binding).await
}

async fn run_bound(state: &AppState, workspace: &str, generation: PluginGenerationLease, ids: &[String], code: &str, binding: Binding) -> Result<ExecutionResult> {
    ensure!(!state.services().hosted && state.services().workspace_worker.is_none(), "metadata requires a local runtime");
    let host = Arc::new(Host { state: state.clone(), workspace: workspace.into(), generation, ids: ids.to_vec(), binding });
    host.check()?;
    let mut limits = super::unattended::limits();
    limits.max_calls = 1;
    Ok(neoism_agent_composition::execute(code, host, limits, Default::default()).await)
}

struct Host {
    state: AppState,
    workspace: String,
    generation: PluginGenerationLease,
    ids: Vec<String>,
    binding: Binding,
}

impl Host {
    fn check(&self) -> Result<[u8; 32]> {
        self.generation.ensure_active()?;
        // Read current policy without refreshing plugins or connecting MCP.
        // Also require the admitted generation to agree, so unpublished grants
        // never broaden an already loaded catalog.
        let config = neoism_agent_builtins::plugin::config::load(self.state.services(), &self.workspace)
            .map_err(|_| anyhow::anyhow!("metadata workspace policy unavailable"))?.0;
        let server = config.mcp.get(&self.binding.server).context("metadata server not configured in workspace")?;
        let permissions = crate::permission::from_config_map(&config.permission);
        let key = decode_hex(&self.binding.key_hex).context("invalid metadata adapter key")?;
        // This is the immutable protocol contract. Authentication is completed
        // by verifying the response MAC before a result is ever exposed.
        let pin = Sha256::digest(key).into();
        let review = ReviewedMetadata {
            server: &self.binding.server, endpoint: &self.binding.endpoint,
            provider_pin: pin, definition_sha256: decode_hex(&self.binding.definition_sha256)
                .context("invalid metadata definition pin")?,
        };
        check_metadata_candidate(Some(&review), CurrentMetadata {
            server: &self.binding.server, config: server, provider_pin: Some(pin),
            raw_definition: DEFINITION, permissions: &permissions,
        }, &self.ids, &json!({}))?;
        let loaded = self.generation.config();
        let loaded_server = loaded.mcp.get(&self.binding.server).context("metadata server not in the loaded workspace catalog")?;
        check_metadata_candidate(Some(&review), CurrentMetadata {
            server: &self.binding.server, config: loaded_server, provider_pin: Some(pin),
            raw_definition: DEFINITION, permissions: &crate::permission::from_config_map(&loaded.permission),
        }, &self.ids, &json!({}))?;
        Ok(key)
    }
}

impl ToolHost for Host {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        if self.check().is_err() { return vec![]; }
        vec![ToolDescriptor {
            path: PATH.into(), description: "Return the configured provider adapter version.".into(),
            signature: "neoism.provider_metadata(input: {}): Promise<{ version: string }>".into(),
        }]
    }

    fn call(&self, path: String, input: Value, cancel: Arc<AtomicBool>) -> BoxFuture<'static, Result<Value, String>> {
        if path != PATH || input.as_object().is_none_or(|args| !args.is_empty()) || cancel.load(Ordering::SeqCst) {
            return Box::pin(async { Err("metadata capability or arguments not admitted".into()) });
        }
        let key = self.check().map_err(|error| error.to_string());
        let endpoint = self.binding.endpoint.clone();
        let server = self.binding.server.clone();
        Box::pin(async move {
            let value = fetch(&endpoint, &server, &key?).await.map_err(|_| "metadata adapter failed authentication or contract validation".to_string())?;
            if cancel.load(Ordering::SeqCst) { return Err("metadata call cancelled".into()); }
            Ok(value)
        })
    }
}

const REQUEST_DOMAIN: &[u8] = b"neoism.metadata.request.v1\0";
const RESPONSE_DOMAIN: &[u8] = b"neoism.metadata.response.v1\0";
const MAC_HEADER: &str = "x-neoism-metadata-mac";

fn definition_hash() -> String {
    format!("{:x}", Sha256::digest(DEFINITION))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    server: &'a str,
    nonce: String,
    definition_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: String,
}

fn mac(key: &[u8; 32], domain: &[u8], bytes: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a 32-byte key");
    mac.update(domain);
    mac.update(bytes);
    mac
}

async fn fetch(endpoint: &str, server: &str, key: &[u8; 32]) -> Result<Value> {
    let client = reqwest::Client::builder().no_proxy()
        .redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(2)).build()?;
    let request = serde_json::to_vec(&Request {
        server, nonce: format!("{:032x}", rand::random::<u128>()),
        definition_sha256: definition_hash(),
    })?;
    let signature = mac(key, REQUEST_DOMAIN, &request).finalize().into_bytes();
    let mut response = client.post(endpoint)
        .header("content-type", "application/json").header(MAC_HEADER, format!("{signature:x}"))
        .body(request.clone()).send().await?;
    ensure!(response.status() == reqwest::StatusCode::OK, "metadata precondition failed");
    let signature = response.headers().get(MAC_HEADER).and_then(|value| value.to_str().ok())
        .and_then(decode_hex).context("metadata response signature missing")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(chunk.len() <= MAX_BODY - bytes.len(), "metadata response too large");
        bytes.extend_from_slice(&chunk);
    }
    // Bind the response to all request bytes (including nonce and schema pin).
    let mut verifier = mac(key, RESPONSE_DOMAIN, &Sha256::digest(request));
    verifier.update(&bytes);
    verifier.verify_slice(&signature).map_err(|_| anyhow::anyhow!("metadata response signature invalid"))?;
    let response: Response = serde_json::from_slice(&bytes)?;
    ensure!((1..=64).contains(&response.version.len()) && response.version.bytes().all(|b|
        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+')), "invalid provider version");
    Ok(json!({"version": response.version}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Bytes, http::{HeaderMap, StatusCode}, response::IntoResponse};

    #[tokio::test]
    async fn metadata_compose_uses_only_the_authenticated_conditional_adapter() {
        let key = [7; 32];
        let mode = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fixture = {
            let mode = mode.clone();
            let calls = calls.clone();
            move |headers: HeaderMap, body: Bytes| {
                let mode = mode.clone();
                let calls = calls.clone();
                async move {
                    let signature = headers.get(MAC_HEADER).and_then(|h| h.to_str().ok()).and_then(decode_hex).unwrap();
                    assert!(mac(&key, REQUEST_DOMAIN, &body).verify_slice(&signature).is_ok());
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(request["server"], "fixture");
                    assert_eq!(request["nonce"].as_str().unwrap().len(), 32);
                    let expected = if mode.load(Ordering::SeqCst) == 1 { "00".repeat(32) } else { definition_hash() };
                    if request["definitionSha256"] != expected {
                        return StatusCode::PRECONDITION_FAILED.into_response();
                    }
                    calls.fetch_add(1, Ordering::SeqCst);
                    let payload = br#"{"version":"1.2.3"}"#;
                    let response_key = if mode.load(Ordering::SeqCst) == 2 { [6; 32] } else { key };
                    let mut tag = mac(&response_key, RESPONSE_DOMAIN, &Sha256::digest(&body));
                    tag.update(payload);
                    ([(MAC_HEADER, format!("{:x}", tag.finalize().into_bytes()))], payload.as_slice()).into_response()
                }
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/metadata", listener.local_addr().unwrap());
        let router = axum::Router::new().route("/metadata", axum::routing::post(fixture));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        std::fs::create_dir(dir.path().join(".agent")).unwrap();
        let id = "mcp__fixture__provider_metadata";
        let ids = [id.to_owned()];
        let config = |permission| json!({
            "mcp": {"fixture": {"type":"remote", "url":endpoint,"enabled":true,"oauth":false}},
            "permission": {"mcp": {id: permission}}
        });
        let path = dir.path().join(".agent/agent.json");
        std::fs::write(&path, config("allow").to_string()).unwrap();
        let state = crate::state::AppState::open_database(dir.path().join("state.db")).await.unwrap();
        let generation = state.refreshed_plugin_snapshot(root).await;
        let raw = json!({"server":"fixture","endpoint":endpoint,"approvalIdentity":"approved","definitionSha256":definition_hash(),"keyHex":"07".repeat(32)}).to_string();
        assert!(parse_binding(Some(&raw), &ids, "stale").is_err());
        let binding = || parse_binding(Some(&raw), &ids, "approved").unwrap();
        let invoke = |code| run_bound(&state, root, generation.clone(), &ids, code, binding());
        let code = "return await tools.neoism.provider_metadata({});";
        let success = invoke(code).await.unwrap();
        assert!(success.ok, "{:?}", success.error);
        assert_eq!(success.value, json!({"version":"1.2.3"}));
        assert!(!invoke("return await tools.neoism.provider_metadata({extra: true});").await.unwrap().ok);
        assert!(!invoke("return await tools.neoism.bash({});").await.unwrap().ok);
        mode.store(1, Ordering::SeqCst);
        assert!(!invoke(code).await.unwrap().ok);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        mode.store(2, Ordering::SeqCst);
        assert!(!invoke(code).await.unwrap().ok);
        std::fs::write(path, config("deny").to_string()).unwrap();
        assert!(invoke(code).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }
}