//! Host-owned trust records for executable extension artifacts.
//!
//! This store deliberately lives below the application data directory, never
//! below a workspace. A package manifest can request authority, but only this
//! file can grant it.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const NATIVE_ABI_VERSION: u32 = 1;
pub const TREE_SITTER_ABI_MIN: u32 = 13;
pub const TREE_SITTER_ABI_MAX: u32 = 15;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "workspace", rename_all = "snake_case")]
pub enum ApprovalScope {
    User,
    Workspace(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    PermissionRequired,
    Approved,
    Revoked,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionApproval {
    pub plugin_id: String,
    pub revision: String,
    pub package_digest: String,
    pub artifact_digest: String,
    pub abi_version: u32,
    pub capabilities: BTreeSet<String>,
    pub scope: ApprovalScope,
    pub state: ApprovalState,
    pub decided_at_millis: u64,
    pub decided_by: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustAuditRecord {
    pub sequence: u64,
    pub timestamp_millis: u64,
    pub action: String,
    pub plugin_id: String,
    pub revision: String,
    pub artifact_digest: String,
    pub scope: ApprovalScope,
    pub actor: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustDocument {
    #[serde(default = "document_version")]
    version: u32,
    #[serde(default)]
    approvals: Vec<ExtensionApproval>,
    #[serde(default)]
    audit: Vec<TrustAuditRecord>,
}

fn document_version() -> u32 {
    1
}

#[derive(Debug, Error)]
pub enum TrustError {
    #[error("extension trust store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("extension trust store is invalid: {0}")]
    Invalid(String),
    #[error(
        "extension approval does not exactly match the requested owner and artifact"
    )]
    Mismatch,
}

pub struct ExtensionTrustStore {
    path: PathBuf,
    mutation: Arc<Mutex<()>>,
}

impl ExtensionTrustStore {
    pub fn managed() -> Self {
        static MUTATION: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
        Self {
            path: crate::paths::extensions_dir().join("trust-v1.json"),
            mutation: MUTATION.get_or_init(|| Arc::new(Mutex::new(()))).clone(),
        }
    }
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            mutation: Arc::new(Mutex::new(())),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn approvals(&self) -> Result<Vec<ExtensionApproval>, TrustError> {
        Ok(self.read()?.approvals)
    }

    pub fn exact(
        &self,
        plugin_id: &str,
        revision: &str,
        package_digest: &str,
        artifact_digest: &str,
        abi_version: u32,
        capabilities: &BTreeSet<String>,
        scope: &ApprovalScope,
    ) -> Result<Option<ExtensionApproval>, TrustError> {
        validate_identity(
            plugin_id,
            revision,
            package_digest,
            artifact_digest,
            abi_version,
        )?;
        Ok(self.read()?.approvals.into_iter().find(|approval| {
            approval.plugin_id == plugin_id
                && approval.revision == revision
                && approval.package_digest == package_digest
                && approval.artifact_digest == artifact_digest
                && approval.abi_version == abi_version
                && approval.capabilities == *capabilities
                && approval.scope == *scope
        }))
    }

    pub fn approve(&self, mut approval: ExtensionApproval) -> Result<(), TrustError> {
        validate_identity(
            &approval.plugin_id,
            &approval.revision,
            &approval.package_digest,
            &approval.artifact_digest,
            approval.abi_version,
        )?;
        approval.state = ApprovalState::Approved;
        approval.decided_at_millis = now_millis();
        if approval.decided_by.trim().is_empty() {
            approval.decided_by = "native-ui".into();
        }
        self.mutate("approved", approval, None)
    }

    pub fn revoke(
        &self,
        mut approval: ExtensionApproval,
        reason: Option<String>,
    ) -> Result<(), TrustError> {
        validate_identity(
            &approval.plugin_id,
            &approval.revision,
            &approval.package_digest,
            &approval.artifact_digest,
            approval.abi_version,
        )?;
        approval.state = ApprovalState::Revoked;
        approval.decided_at_millis = now_millis();
        approval.reason = reason.clone();
        self.mutate("revoked", approval, reason)
    }

    pub fn record_failure(
        &self,
        mut approval: ExtensionApproval,
        message: String,
    ) -> Result<(), TrustError> {
        approval.state = ApprovalState::Failed;
        approval.decided_at_millis = now_millis();
        approval.reason = Some(message.clone());
        self.mutate("failed", approval, Some(message))
    }

    pub fn audit_event(
        &self,
        action: &str,
        approval: &ExtensionApproval,
        detail: Option<String>,
    ) -> Result<(), TrustError> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| TrustError::Invalid("trust store lock poisoned".into()))?;
        let mut document = self.read()?;
        let sequence = document
            .audit
            .last()
            .map_or(1, |record| record.sequence.saturating_add(1));
        document.audit.push(TrustAuditRecord {
            sequence,
            timestamp_millis: now_millis(),
            action: action.into(),
            plugin_id: approval.plugin_id.clone(),
            revision: approval.revision.clone(),
            artifact_digest: approval.artifact_digest.clone(),
            scope: approval.scope.clone(),
            actor: "extension-host".into(),
            detail,
        });
        if document.audit.len() > 4096 {
            document.audit.drain(..document.audit.len() - 4096);
        }
        write_atomic(&self.path, &document)
    }

    fn mutate(
        &self,
        action: &str,
        approval: ExtensionApproval,
        detail: Option<String>,
    ) -> Result<(), TrustError> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| TrustError::Invalid("trust store lock poisoned".into()))?;
        let mut document = self.read()?;
        document.approvals.retain(|item| {
            !(item.plugin_id == approval.plugin_id
                && item.revision == approval.revision
                && item.artifact_digest == approval.artifact_digest
                && item.abi_version == approval.abi_version
                && item.capabilities == approval.capabilities
                && item.scope == approval.scope)
        });
        let sequence = document
            .audit
            .last()
            .map_or(1, |record| record.sequence.saturating_add(1));
        document.audit.push(TrustAuditRecord {
            sequence,
            timestamp_millis: now_millis(),
            action: action.into(),
            plugin_id: approval.plugin_id.clone(),
            revision: approval.revision.clone(),
            artifact_digest: approval.artifact_digest.clone(),
            scope: approval.scope.clone(),
            actor: approval.decided_by.clone(),
            detail,
        });
        // Audit metadata is bounded; approvals remain authoritative.
        if document.audit.len() > 4096 {
            document.audit.drain(..document.audit.len() - 4096);
        }
        document.approvals.push(approval);
        write_atomic(&self.path, &document)
    }

    fn read(&self) -> Result<TrustDocument, TrustError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TrustDocument {
                    version: 1,
                    ..Default::default()
                })
            }
            Err(source) => {
                return Err(TrustError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let document: TrustDocument = serde_json::from_slice(&bytes)
            .map_err(|error| TrustError::Invalid(error.to_string()))?;
        if document.version != 1 {
            return Err(TrustError::Invalid(format!(
                "unsupported version {}",
                document.version
            )));
        }
        Ok(document)
    }
}

pub fn sha256_file(path: &Path) -> Result<String, TrustError> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).map_err(|source| TrustError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn verify_artifact(path: &Path, digest: &str) -> Result<(), TrustError> {
    validate_digest(digest)?;
    if sha256_file(path)? != digest.to_ascii_lowercase() {
        return Err(TrustError::Mismatch);
    }
    Ok(())
}

fn validate_identity(
    plugin_id: &str,
    revision: &str,
    package: &str,
    artifact: &str,
    abi: u32,
) -> Result<(), TrustError> {
    if plugin_id.is_empty() || revision.is_empty() || abi == 0 {
        return Err(TrustError::Invalid(
            "approval identity is incomplete".into(),
        ));
    }
    validate_digest(package)?;
    validate_digest(artifact)
}
fn validate_digest(value: &str) -> Result<(), TrustError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TrustError::Invalid(
            "digest must be a SHA-256 hex string".into(),
        ));
    }
    Ok(())
}
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn write_atomic(path: &Path, document: &TrustDocument) -> Result<(), TrustError> {
    let parent = path
        .parent()
        .ok_or_else(|| TrustError::Invalid("trust path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|source| TrustError::Io {
        path: parent.into(),
        source,
    })?;
    let temp = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|source| TrustError::Io {
        path: temp.clone(),
        source,
    })?;
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| TrustError::Invalid(error.to_string()))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|source| TrustError::Io {
            path: temp.clone(),
            source,
        })?;
    fs::rename(&temp, path).map_err(|source| TrustError::Io {
        path: path.into(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_is_exact_and_revocable() {
        let root =
            std::env::temp_dir().join(format!("neoism-trust-{}", std::process::id()));
        let store = ExtensionTrustStore::new(root.join("trust.json"));
        let digest = "a".repeat(64);
        let capabilities = BTreeSet::from(["network.execute".into()]);
        let approval = ExtensionApproval {
            plugin_id: "dev.neoism.test".into(),
            revision: "r1".into(),
            package_digest: digest.clone(),
            artifact_digest: digest.clone(),
            abi_version: 1,
            capabilities: capabilities.clone(),
            scope: ApprovalScope::User,
            state: ApprovalState::PermissionRequired,
            decided_at_millis: 0,
            decided_by: "test".into(),
            reason: None,
        };
        store.approve(approval.clone()).unwrap();
        assert_eq!(
            store
                .exact(
                    "dev.neoism.test",
                    "r1",
                    &digest,
                    &digest,
                    1,
                    &capabilities,
                    &ApprovalScope::User
                )
                .unwrap()
                .unwrap()
                .state,
            ApprovalState::Approved
        );
        store.revoke(approval, Some("test".into())).unwrap();
        assert_eq!(
            store
                .exact(
                    "dev.neoism.test",
                    "r1",
                    &digest,
                    &digest,
                    1,
                    &capabilities,
                    &ApprovalScope::User
                )
                .unwrap()
                .unwrap()
                .state,
            ApprovalState::Revoked
        );
        let _ = fs::remove_dir_all(root);
    }
}
