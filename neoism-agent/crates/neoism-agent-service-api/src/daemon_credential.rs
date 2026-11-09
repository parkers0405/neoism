//! Short-lived credentials minted by an authenticated Neoism workspace daemon.
//!
//! The daemon and Agent server already share the daemon authentication key in
//! `NEOISM_DAEMON_TOKEN`; this format deliberately derives from that key rather
//! than introducing another persisted full-access secret.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const ISSUER: &str = "neoism-workspace-daemon";
pub const AUDIENCE: &str = "neoism-agent-server";
pub const PREFIX: &str = "neoism-daemon-v1";
pub const MAX_LIFETIME_SECS: i64 = 120;
pub const AGENT_SIGNING_KEY_ENV: &str = "NEOISM_AGENT_SIGNING_KEY";

/// One trust-root path for the desktop, temporary hosts, and Agent verifier.
/// Product config/data directories are deliberately not involved.
pub fn canonical_daemon_token_path() -> std::path::PathBuf {
    #[cfg(unix)]
    {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            return std::path::PathBuf::from(runtime).join("neoism/daemon-token");
        }
        std::env::temp_dir()
            .join(format!("neoism-{}", unsafe { libc::geteuid() }))
            .join("daemon-token")
    }
    #[cfg(not(unix))]
    {
        windows_daemon_token_path(&std::env::temp_dir(), std::env::var("USERNAME").ok().as_deref())
    }
}

pub fn windows_daemon_token_path(temp: &std::path::Path, username: Option<&str>) -> std::path::PathBuf {
    let user = username.filter(|name| !name.is_empty()).map(|name| {
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>()
    }).unwrap_or_else(|| "default".into());
    temp.join(format!("neoism-{user}")).join("daemon-token")
}

/// A hosted daemon's transport password is not an Agent signing authority.
/// Capture the parent's Agent authority before replacing that password.
pub fn agent_signing_key() -> Option<String> {
    signing_key_from_sources(
        &canonical_daemon_token_path(),
        std::env::var(AGENT_SIGNING_KEY_ENV).ok(),
        std::env::var("NEOISM_DAEMON_TOKEN").ok(),
        std::env::var("NEOISM_FAST_SERVER").as_deref() == Ok("1"),
    )
}

fn signing_key_from_sources(
    path: &std::path::Path,
    shared: Option<String>,
    transport: Option<String>,
    fast: bool,
) -> Option<String> {
    std::fs::read_to_string(path).ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| shared.filter(|key| !key.trim().is_empty()))
        .or_else(|| if fast { None } else { transport.filter(|key| !key.trim().is_empty()) })
        .map(|key| key.trim().to_owned())
}

pub const SYSTEM_WORKFLOW_VERSION: u32 = 1;
pub const SYSTEM_WORKFLOW_SUBJECT: &str = "daemon:system-workflow";
pub const LOCAL_AGENT_SUBJECT: &str = "daemon:local-agent";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DaemonCredentialKind {
    #[default]
    Workspace,
    SystemWorkflow,
    /// Existing unscoped local/paired Agent bridge, explicitly NOT root owner.
    LocalAgent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonCredentialClaims {
    pub issuer: String,
    pub audience: String,
    pub subject: String,
    #[serde(default)]
    pub kind: DaemonCredentialKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub tenant_id: String,
    pub directory_prefixes: Vec<String>,
    pub hosted: bool,
    pub issued_at: i64,
    pub expires_at: i64,
}

impl DaemonCredentialClaims {
    pub fn local_agent(now: i64, lifetime_secs: i64) -> Result<Self, &'static str> {
        let mut claims = Self::system_workflow(now, lifetime_secs)?;
        claims.kind = DaemonCredentialKind::LocalAgent;
        claims.subject = LOCAL_AGENT_SUBJECT.into();
        Ok(claims)
    }
    /// Machine workflow access is intentionally not a workspace or hosted identity.
    pub fn system_workflow(now: i64, lifetime_secs: i64) -> Result<Self, &'static str> {
        let mut claims = Self::new(
            SYSTEM_WORKFLOW_SUBJECT,
            "unused",
            "local",
            vec![],
            false,
            now,
            lifetime_secs,
        )?;
        claims.kind = DaemonCredentialKind::SystemWorkflow;
        claims.workspace_id = None;
        Ok(claims)
    }

    pub fn new(
        subject: impl Into<String>,
        workspace_id: impl Into<String>,
        tenant_id: impl Into<String>,
        directory_prefixes: Vec<String>,
        hosted: bool,
        now: i64,
        lifetime_secs: i64,
    ) -> Result<Self, &'static str> {
        if !(1..=MAX_LIFETIME_SECS).contains(&lifetime_secs) {
            return Err("daemon credential lifetime is out of bounds");
        }
        Ok(Self {
            issuer: ISSUER.into(),
            audience: AUDIENCE.into(),
            subject: subject.into(),
            kind: DaemonCredentialKind::Workspace,
            workspace_id: Some(workspace_id.into()),
            tenant_id: tenant_id.into(),
            directory_prefixes,
            hosted,
            issued_at: now,
            expires_at: now.saturating_add(lifetime_secs),
        })
    }
}

pub fn issue(
    claims: &DaemonCredentialClaims,
    daemon_key: &[u8],
) -> Result<String, &'static str> {
    validate_shape(claims)?;
    if daemon_key.is_empty() {
        return Err("daemon credential signing key is empty");
    }
    let payload =
        serde_json::to_vec(claims).map_err(|_| "cannot encode daemon credential")?;
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{PREFIX}.{payload}");
    let signature = hmac_sha256(daemon_key, signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub fn verify(
    token: &str,
    daemon_key: &[u8],
    now: i64,
) -> Result<DaemonCredentialClaims, &'static str> {
    let mut parts = token.split('.');
    let prefix = parts.next().ok_or("malformed daemon credential")?;
    let payload = parts.next().ok_or("malformed daemon credential")?;
    let signature = parts.next().ok_or("malformed daemon credential")?;
    if parts.next().is_some() || prefix != PREFIX || daemon_key.is_empty() {
        return Err("malformed daemon credential");
    }
    let supplied = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "malformed daemon credential signature")?;
    let signing_input = format!("{PREFIX}.{payload}");
    let expected = hmac_sha256(daemon_key, signing_input.as_bytes());
    if supplied.len() != expected.len()
        || supplied.ct_eq(expected.as_slice()).unwrap_u8() != 1
    {
        return Err("invalid daemon credential signature");
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "malformed daemon credential payload")?;
    let claims: DaemonCredentialClaims = serde_json::from_slice(&payload)
        .map_err(|_| "malformed daemon credential claims")?;
    validate_shape(&claims)?;
    if claims.issued_at > now.saturating_add(5) {
        return Err("daemon credential is not yet valid");
    }
    if claims.expires_at <= now {
        return Err("daemon credential has expired");
    }
    if claims.expires_at.saturating_sub(claims.issued_at) > MAX_LIFETIME_SECS {
        return Err("daemon credential lifetime is out of bounds");
    }
    Ok(claims)
}

fn validate_shape(claims: &DaemonCredentialClaims) -> Result<(), &'static str> {
    if claims.issuer != ISSUER {
        return Err("invalid daemon credential issuer");
    }
    if claims.audience != AUDIENCE {
        return Err("invalid daemon credential audience");
    }
    if claims.subject.trim().is_empty() || claims.tenant_id.trim().is_empty() {
        return Err("daemon credential identity is empty");
    }
    match claims.kind {
        DaemonCredentialKind::Workspace => {
            if claims
                .workspace_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
                || matches!(claims.subject.as_str(), SYSTEM_WORKFLOW_SUBJECT | LOCAL_AGENT_SUBJECT)
            {
                return Err("invalid workspace credential scope");
            }
        }
        DaemonCredentialKind::SystemWorkflow | DaemonCredentialKind::LocalAgent => {
            if claims.workspace_id.is_some()
                || claims.hosted
                || claims.tenant_id != "local"
                || !claims.directory_prefixes.is_empty()
                || claims.subject != if claims.kind == DaemonCredentialKind::SystemWorkflow { SYSTEM_WORKFLOW_SUBJECT } else { LOCAL_AGENT_SUBJECT }
            {
                return Err("invalid system workflow credential scope");
            }
        }
    }
    if claims.expires_at <= claims.issued_at {
        return Err("daemon credential expiry is invalid");
    }
    Ok(())
}

/// Fail-closed canonical v2 workflow surface, shared by proxy and verifier.
/// No prefix grants: new routes must be deliberately added here.
pub fn system_workflow_allows(method: &str, path: &str) -> bool {
    if !path.starts_with("/v2/") || path.contains('%') || path.contains('\\') {
        return false;
    }
    let parts: Vec<_> = path.split('/').skip(1).collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return false;
    }
    match parts.as_slice() {
        ["v2", "neo", "session"] => method == "POST",
        ["v2", "health" | "meta" | "capabilities" | "providers" | "agents"
        | "commands" | "skills" | "tools" | "session-projects" | "directories" | "events"] => {
            method == "GET"
        }
        ["v2", "config", "defaults"] => method == "GET",
        ["v2", "providers", "configured"] => method == "GET",
        ["v2", "session-catalog", "events"] => method == "GET",
        ["v2", "sessions"] => matches!(method, "GET" | "POST"),
        ["v2", "sessions", "status"] => method == "GET",
        ["v2", "sessions", id] if workflow_id(id) => {
            matches!(method, "GET" | "PATCH" | "DELETE")
        }
        ["v2", "sessions", id, action] if workflow_id(id) => match *action {
            "messages" | "children" | "runtime" | "participants"
            | "directory-options" | "todos" | "diff" | "undo-tree" | "context" => {
                method == "GET"
            }
            "control" => matches!(method, "GET" | "POST" | "DELETE"),
            "queue" => matches!(method, "GET" | "DELETE"),
            // GET negotiates direct execution before a client may submit POST.
            "shell" => matches!(method, "GET" | "POST"),
            "fork" | "prompt" | "prompt-async" | "abort" | "compact" | "wait"
            | "commands" | "revert" | "unrevert" | "undo" | "redo"
            | "summarize" | "pin" => method == "POST",
            _ => false,
        },
        ["v2", "sessions", id, "messages", message]
            if workflow_id(id) && workflow_id(message) =>
        {
            matches!(method, "GET" | "DELETE")
        }
        ["v2", "sessions", id, "queue", "pop"] if workflow_id(id) => method == "POST",
        ["v2", "sessions", id, "jobs", job] if workflow_id(id) && workflow_id(job) => {
            method == "DELETE"
        }
        ["v2", "interactions", "permissions" | "questions"] => method == "GET",
        ["v2", "interactions", "permissions" | "questions", id, "reply"]
            if workflow_id(id) =>
        {
            method == "POST"
        }
        ["v2", "interactions", "questions", id, "reject"] if workflow_id(id) => {
            method == "POST"
        }
        ["v2", "artifacts"] => matches!(method, "GET" | "POST"),
        ["v2", "artifacts", id] if workflow_id(id) => matches!(method, "GET" | "DELETE"),
        ["v2", "artifacts", id, "content"] if workflow_id(id) => method == "GET",
        // Desktop directory/session menus follow /cd and reopened conversations.
        // Keep these concrete operations, not a grant for arbitrary plugins.
        ["v2", "plugins", "dev.neoism.goals", id] if workflow_id(id) => {
            matches!(method, "GET" | "POST" | "DELETE")
        }
        ["v2", "plugins", "dev.neoism.subagents", "sessions", id, "tasks"]
            if workflow_id(id) => method == "GET",
        ["v2", "plugins", "dev.neoism.mcp", "catalog"] => method == "GET",
        ["v2", "plugins", "dev.neoism.mcp", _, action] => match *action {
            "tools" | "resources" | "prompts" => method == "GET",
            "connect" | "disconnect" | "auth" => method == "POST",
            "config" => method == "PATCH",
            _ => false,
        },
        _ => false,
    }
}

fn workflow_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        && !matches!(id, "import" | "export" | "external" | "status")
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    outer.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neo_root_and_local_peer_capabilities_are_distinct_and_fail_closed() {
        let peer = DaemonCredentialClaims::local_agent(100, 60).unwrap();
        let signed = issue(&peer, b"key").unwrap();
        assert_eq!(verify(&signed, b"key", 110).unwrap(), peer);
        assert_eq!(peer.kind, DaemonCredentialKind::LocalAgent);
        assert_eq!(peer.subject, LOCAL_AGENT_SUBJECT);
        assert_ne!(peer.subject, SYSTEM_WORKFLOW_SUBJECT);
        let mut forged = peer.clone();
        forged.kind = DaemonCredentialKind::SystemWorkflow;
        assert!(issue(&forged, b"key").is_err());
        forged = peer;
        forged.workspace_id = Some("workspace".into());
        assert!(issue(&forged, b"key").is_err());
        assert!(system_workflow_allows("POST", "/v2/neo/session"));
        assert!(!system_workflow_allows("GET", "/v2/neo/session"));
        assert!(!system_workflow_allows("POST", "/v2/neo/session/extra"));
    }

    #[test]
    fn fast_host_keeps_agent_authority_separate_from_transport_password() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon-token");
        let shared = Some("shared-agent-key".to_owned());
        let transport = Some("private-host-password".to_owned());
        let key = signing_key_from_sources(&path, shared, transport.clone(), true).unwrap();
        let claims = DaemonCredentialClaims::new(
            "neoism:host-chat-association", "host", "workspace:host",
            vec![temp.path().to_string_lossy().into_owned()], true, 100, 60,
        ).unwrap();
        let credential = issue(&claims,key.as_bytes()).unwrap();
        assert_eq!(verify(&credential,b"shared-agent-key",110).unwrap(),claims);
        assert!(verify(&credential,b"private-host-password",110).is_err());
        assert!(signing_key_from_sources(&path,None,transport.clone(),true).is_none());
        // Ordinary home daemons may have an operator-supplied, nonpersisted key.
        assert_eq!(signing_key_from_sources(&path,None,transport.clone(),false),transport);
        // Keep following live key-file rotations instead of a captured old key.
        std::fs::write(&path,"rotated-agent-key\n").unwrap();
        assert_eq!(signing_key_from_sources(&path,Some(key),transport,true).as_deref(),Some("rotated-agent-key"));
    }

    #[test]
    fn windows_agent_key_is_independent_of_roaming_product_config() {
        let temp = std::path::Path::new("C:/Users/test/AppData/Local/Temp");
        assert_eq!(windows_daemon_token_path(temp,Some("chandler")),temp.join("neoism-chandler/daemon-token"));
        assert_eq!(windows_daemon_token_path(temp,Some("User Name!")),temp.join("neoism-User-Name-/daemon-token"));
        assert_eq!(windows_daemon_token_path(temp,None),temp.join("neoism-default/daemon-token"));
    }

    #[test]
    fn system_workflow_is_explicit_unscoped_short_lived_and_fail_closed() {
        let claims = DaemonCredentialClaims::system_workflow(100, 60).unwrap();
        let token = issue(&claims, b"key").unwrap();
        assert_eq!(verify(&token, b"key", 110).unwrap(), claims);
        let json = serde_json::to_value(&claims).unwrap();
        assert_eq!(json["kind"], "system-workflow");
        assert!(json.get("workspaceId").is_none());
        let mut invalid = claims.clone();
        invalid.workspace_id = Some("spoof".into());
        assert!(issue(&invalid, b"key").is_err());
        invalid = claims.clone();
        invalid.hosted = true;
        assert!(issue(&invalid, b"key").is_err());
        assert!(verify(&token, b"key", 160).is_err());
        for path in [
            "/v2/session-projects",
            "/v2/directories",
            "/v2/providers",
            "/v2/providers/configured",
            "/v2/agents",
            "/v2/commands",
            "/v2/skills",
            "/v2/config/defaults",
            "/v2/plugins/dev.neoism.mcp/catalog",
            "/v2/sessions/ses_test/messages",
        ] {
            assert!(system_workflow_allows("GET", path), "{path}");
            assert!(!system_workflow_allows("PUT", path), "{path}");
        }
        for path in [
            "/v2/config",
            "/v2/management/agents",
            "/v2/hosting/associate",
            "/v2/providers/openai/auth",
            "/v2/providers/openai/oauth/authorize",
            "/v2/providers/openai/connections",
            "/v2/sessions/../config",
            "/v2/sessions/%2e%2e",
            "/v2/sessions/import",
        ] {
            for method in ["GET", "POST", "PUT", "DELETE"] {
                assert!(!system_workflow_allows(method, path), "{method} {path}");
            }
        }
    }

    #[test]
    fn desktop_directory_menus_have_exact_method_and_route_grants() {
        for (path, allowed) in [
            ("/v2/config/defaults", &["GET"][..]),
            ("/v2/skills", &["GET"][..]),
            ("/v2/plugins/dev.neoism.mcp/catalog", &["GET"][..]),
            ("/v2/plugins/dev.neoism.mcp/browser.tools/tools", &["GET"][..]),
            ("/v2/plugins/dev.neoism.mcp/local/resources", &["GET"][..]),
            ("/v2/plugins/dev.neoism.mcp/local/prompts", &["GET"][..]),
            ("/v2/plugins/dev.neoism.mcp/local/connect", &["POST"][..]),
            ("/v2/plugins/dev.neoism.mcp/local/disconnect", &["POST"][..]),
            ("/v2/plugins/dev.neoism.mcp/local/auth", &["POST"][..]),
            ("/v2/plugins/dev.neoism.mcp/local/config", &["PATCH"][..]),
            ("/v2/plugins/dev.neoism.goals/ses_test", &["GET", "POST", "DELETE"][..]),
            ("/v2/plugins/dev.neoism.subagents/sessions/ses_test/tasks", &["GET"][..]),
        ] {
            for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] {
                assert_eq!(system_workflow_allows(method, path), allowed.contains(&method), "{method} {path}");
            }
        }
        for path in [
            "/v2/config", "/v2/skills/install", "/v2/plugins/dev.neoism.mcp",
            "/v2/plugins/dev.neoism.mcp/local/tools/execute",
            "/v2/plugins/dev.neoism.mcp/local/config/extra",
            "/v2/plugins/dev.neoism.mcp/../config",
            "/v2/plugins/dev.neoism.mcp/%2e%2e/config",
            "/v2/plugins/unknown/ses_test",
            "/v2/plugins/dev.neoism.goals/ses_test/research",
            "/v2/plugins/dev.neoism.subagents/sessions/ses_test/tasks/stop",
            "/v2/providers/openai/connections", "/v2/providers/openai/oauth/authorize",
        ] {
            for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                assert!(!system_workflow_allows(method, path), "{method} {path}");
            }
        }
    }

    #[test]
    fn direct_shell_probe_and_execution_are_allowed_without_a_prefix_grant() {
        let path = "/v2/sessions/ses_test/shell";
        assert!(system_workflow_allows("GET", path));
        assert!(system_workflow_allows("POST", path));
        for method in ["PUT", "PATCH", "DELETE", "HEAD"] {
            assert!(!system_workflow_allows(method, path));
        }
        for path in ["/v2/sessions/ses_test/shell/extra", "/v2/sessions/../shell", "/v2/sessions/%2e%2e/shell"] {
            assert!(!system_workflow_allows("GET", path));
            assert!(!system_workflow_allows("POST", path));
        }
    }

    #[test]
    fn rejects_forged_expired_and_wrong_audience_credentials() {
        let key = b"daemon-key";
        let valid = DaemonCredentialClaims::new(
            "device:a",
            "workspace-a",
            "tenant:a",
            vec!["/repo".into()],
            true,
            100,
            60,
        )
        .unwrap();
        let token = issue(&valid, key).unwrap();
        // Old workspace payloads had no kind tag; they remain workspace-only.
        let mut legacy = serde_json::to_value(&valid).unwrap();
        legacy.as_object_mut().unwrap().remove("kind");
        let legacy: DaemonCredentialClaims = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.kind, DaemonCredentialKind::Workspace);
        assert_eq!(legacy.workspace_id.as_deref(), Some("workspace-a"));
        assert_eq!(verify(&token, key, 120).unwrap(), valid);
        assert!(verify(&token, b"wrong-key", 120).is_err());
        assert!(verify(&token, key, 161).is_err());

        let mut wrong_audience = valid;
        wrong_audience.audience = "other-service".into();
        // Sign the altered claims directly to prove audience validation is
        // independent of signature validation.
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&wrong_audience).unwrap());
        let input = format!("{PREFIX}.{payload}");
        let token = format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(hmac_sha256(key, input.as_bytes()))
        );
        assert_eq!(
            verify(&token, key, 120),
            Err("invalid daemon credential audience")
        );
    }
}
