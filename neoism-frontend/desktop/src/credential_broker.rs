//! Desktop-only credential brokerage for trusted plugin generations.
//!
//! Secrets are held by the operating-system credential vault. The JSON file is
//! metadata only: aliases, exact grants and an audit trail.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use neoism_lua::PluginOwner;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "dev.neoism.plugin-credentials";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "workspace", rename_all = "snake_case")]
pub(crate) enum CredentialScope { User, Workspace(String) }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AliasMetadata {
    alias: String,
    vault_account: String,
    scope: CredentialScope,
    #[serde(default)]
    allowed_hosts: BTreeSet<String>,
    #[serde(default = "bearer_header")]
    operation: String,
    created_at_millis: u64,
    revoked_at_millis: Option<u64>,
}
fn bearer_header() -> String { "authorization_bearer".into() }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialGrant {
    owner: PluginOwner,
    alias: String,
    scope: CredentialScope,
    operations: BTreeSet<String>,
    granted_at_millis: u64,
    revoked_at_millis: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditRecord { sequence: u64, timestamp_millis: u64, action: String, plugin_id: String, revision: String, alias: String, scope: CredentialScope, outcome: String }

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerDocument {
    #[serde(default)] aliases: BTreeMap<String, AliasMetadata>,
    #[serde(default)] grants: Vec<CredentialGrant>,
    #[serde(default)] audit: Vec<AuditRecord>,
}

pub(crate) struct CredentialBroker { path: PathBuf, mutation: Mutex<()> }

impl CredentialBroker {
    fn managed() -> Self {
        let path = dirs::data_local_dir().or_else(dirs::data_dir).unwrap_or_else(|| PathBuf::from(".neoism"))
            .join("neoism").join("plugin-credentials-v1.json");
        Self { path, mutation: Mutex::new(()) }
    }

    /// Host UI provisioning API. The raw value goes directly to the platform
    /// vault and is never serialized by Neoism.
    pub(crate) fn provision(&self, alias: &str, secret: &str, scope: CredentialScope, allowed_hosts: BTreeSet<String>) -> Result<(), String> {
        validate_alias(alias)?;
        if secret.is_empty() { return Err("credential secret is empty".into()); }
        let account = format!("{}-{:016x}", alias, unique());
        keyring::Entry::new(SERVICE, &account).map_err(keyring_error)?.set_password(secret).map_err(keyring_error)?;
        let metadata = AliasMetadata { alias: alias.into(), vault_account: account.clone(), scope: scope.clone(), allowed_hosts, operation: bearer_header(), created_at_millis: now(), revoked_at_millis: None };
        if let Err(error) = self.mutate(|document| { document.aliases.insert(alias.into(), metadata); Ok(()) }) {
            let _ = keyring::Entry::new(SERVICE, &account).and_then(|entry| entry.delete_credential());
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn grant(&self, owner: PluginOwner, alias: &str, scope: CredentialScope, operations: BTreeSet<String>) -> Result<(), String> {
        validate_owner(&owner)?; validate_alias(alias)?;
        if operations.is_empty() || !operations.iter().all(|item| item == "network.authorize") { return Err("credential grant contains an unsupported operation".into()); }
        self.mutate(|document| {
            let metadata = document.aliases.get(alias).ok_or("credential alias does not exist")?;
            if metadata.revoked_at_millis.is_some() || metadata.scope != scope { return Err("credential alias scope is unavailable"); }
            document.grants.retain(|grant| !(grant.owner.plugin_id == owner.plugin_id && grant.alias == alias && grant.scope == scope));
            document.grants.push(CredentialGrant { owner: owner.clone(), alias: alias.into(), scope: scope.clone(), operations, granted_at_millis: now(), revoked_at_millis: None });
            audit(document, "grant", &owner, alias, scope, "ok"); Ok(())
        })
    }

    pub(crate) fn revoke_owner(&self, owner: &PluginOwner) -> Result<(), String> {
        validate_owner(owner)?;
        self.mutate(|document| {
            let timestamp = now();
            for grant in document.grants.iter_mut().filter(|grant| &grant.owner == owner && grant.revoked_at_millis.is_none()) { grant.revoked_at_millis = Some(timestamp); }
            audit(document, "revoke_owner", owner, "*", CredentialScope::User, "ok"); Ok(())
        })
    }

    pub(crate) fn available(&self, owner: &PluginOwner, alias: &str, workspace: Option<&str>) -> Result<bool, String> {
        validate_owner(owner)?; validate_alias(alias)?;
        let document = self.read()?;
        let Some(metadata) = document.aliases.get(alias).filter(|item| item.revoked_at_millis.is_none()) else { return Ok(false) };
        let scope = requested_scope(&metadata.scope, workspace)?;
        Ok(document.grants.iter().any(|grant| grant.owner == *owner && grant.alias == alias && grant.scope == scope && grant.revoked_at_millis.is_none() && grant.operations.contains("network.authorize")))
    }

    pub(crate) fn authorize_network(&self, owner: &PluginOwner, alias: &str, workspace: Option<&str>, host: &str) -> Result<String, String> {
        validate_owner(owner)?; validate_alias(alias)?;
        let _guard = self.mutation.lock().map_err(|_| "credential broker lock poisoned")?;
        let mut document = self.read()?;
        let metadata = document.aliases.get(alias).cloned().ok_or("credential alias does not exist")?;
        let scope = requested_scope(&metadata.scope, workspace)?;
        let allowed = metadata.revoked_at_millis.is_none()
            && (metadata.allowed_hosts.is_empty() || metadata.allowed_hosts.contains(&host.to_ascii_lowercase()))
            && document.grants.iter().any(|grant| grant.owner == *owner && grant.alias == alias && grant.scope == scope && grant.revoked_at_millis.is_none() && grant.operations.contains("network.authorize"));
        if !allowed { audit(&mut document, "network.authorize", owner, alias, scope, "denied"); let _ = self.write(&document); return Err("credential alias is not granted to this plugin revision, scope, operation, or host".into()); }
        let secret = keyring::Entry::new(SERVICE, &metadata.vault_account).map_err(keyring_error)?.get_password().map_err(keyring_error)?;
        audit(&mut document, "network.authorize", owner, alias, scope, "ok"); self.write(&document)?;
        Ok(format!("Bearer {secret}"))
    }

    fn mutate<T>(&self, operation: impl FnOnce(&mut BrokerDocument) -> Result<T, &'static str>) -> Result<T, String> {
        let _guard = self.mutation.lock().map_err(|_| "credential broker lock poisoned")?;
        let mut document = self.read()?; let result = operation(&mut document).map_err(str::to_owned)?; self.write(&document)?; Ok(result)
    }
    fn read(&self) -> Result<BrokerDocument, String> { match fs::read(&self.path) { Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BrokerDocument::default()), Err(e) => Err(e.to_string()) } }
    fn write(&self, document: &BrokerDocument) -> Result<(), String> { write_atomic(&self.path, document) }
}

pub(crate) fn broker() -> &'static CredentialBroker { static BROKER: OnceLock<CredentialBroker> = OnceLock::new(); BROKER.get_or_init(CredentialBroker::managed) }

fn requested_scope(scope: &CredentialScope, workspace: Option<&str>) -> Result<CredentialScope, String> { match scope { CredentialScope::User => Ok(CredentialScope::User), CredentialScope::Workspace(expected) if workspace == Some(expected.as_str()) => Ok(scope.clone()), CredentialScope::Workspace(_) => Err("credential is bound to another workspace".into()) } }
fn validate_owner(owner: &PluginOwner) -> Result<(), String> { if owner.plugin_id.is_empty() || owner.revision.0.is_empty() { Err("credential operation requires an exact plugin owner revision".into()) } else { Ok(()) } }
fn validate_alias(alias: &str) -> Result<(), String> { if alias.is_empty() || alias.len() > 64 || !alias.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) { Err("credential alias is invalid".into()) } else { Ok(()) } }
fn keyring_error(error: keyring::Error) -> String { format!("platform credential vault: {error}") }
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }
fn unique() -> u64 { use std::sync::atomic::{AtomicU64, Ordering}; static NEXT: AtomicU64 = AtomicU64::new(1); (now() << 16) ^ NEXT.fetch_add(1, Ordering::Relaxed) }
fn audit(document: &mut BrokerDocument, action: &str, owner: &PluginOwner, alias: &str, scope: CredentialScope, outcome: &str) { let sequence = document.audit.last().map_or(1, |item| item.sequence.saturating_add(1)); document.audit.push(AuditRecord { sequence, timestamp_millis: now(), action: action.into(), plugin_id: owner.plugin_id.clone(), revision: owner.revision.0.clone(), alias: alias.into(), scope, outcome: outcome.into() }); if document.audit.len() > 4096 { document.audit.drain(..document.audit.len()-4096); } }
fn write_atomic(path: &Path, document: &BrokerDocument) -> Result<(), String> { let parent = path.parent().ok_or("credential metadata path has no parent")?; fs::create_dir_all(parent).map_err(|e| e.to_string())?; let temp = path.with_extension("tmp"); let mut options = OpenOptions::new(); options.create(true).truncate(true).write(true); #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); } let mut file = options.open(&temp).map_err(|e| e.to_string())?; file.write_all(&serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?).and_then(|_| file.write_all(b"\n")).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?; fs::rename(temp, path).map_err(|e| e.to_string()) }