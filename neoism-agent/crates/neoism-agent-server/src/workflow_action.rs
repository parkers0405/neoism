//! Shared validation and approval identity for direct workflow actions. This module
//! does not grant permissions, resolve tool registrations, or execute actions.

use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_SHELL_BYTES: usize = 16 * 1024;
const MAX_COMPOSE_BYTES: usize = 64 * 1024;
const MAX_TOOL_IDS: usize = 32;
const MAX_TOOL_ID_BYTES: usize = 512;
const IDENTITY_DOMAIN: &[u8] = b"neoism.workflow.direct-action.v1\0";

/// Unvalidated definition data. Neither variant implies a shell or tool grant.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum DirectAction {
    /// Literal source text, not a template to expand with trigger or model data.
    Shell { command: String },
    Compose {
        code: String,
        /// Exact registry IDs, not discovery paths, selectors, or patterns.
        #[serde(rename = "toolIds")]
        tool_ids: Vec<String>,
    },
}

/// An immutable snapshot of validated inputs, not an authorization token.
pub(crate) struct ValidatedDirectAction {
    action: DirectAction,
    directory: PathBuf,
    approval_identity: String,
}

impl DirectAction {
    pub(crate) fn validate_workflow_action(&self) -> Result<()> {
        match self {
            Self::Shell { command } => {
                validate_source(command, MAX_SHELL_BYTES, "shell command")
            }
            Self::Compose { code, tool_ids } => {
                if crate::tool_composition::metadata::is_metadata(tool_ids) {
                    crate::tool_composition::metadata::validate(code, tool_ids)
                } else {
                    crate::tool_composition::unattended::validate_inputs(code, tool_ids)
                }
                .map_err(anyhow::Error::msg)
            }
        }
    }

    /// `source_revision` must come from the same server-loaded source snapshot as
    /// this action. `directory` must be the absolute cwd selected by the caller;
    /// the caller must enforce its directory scope on the canonical result.
    pub(crate) fn validate(
        self,
        directory: &Path,
        source_revision: &str,
    ) -> Result<ValidatedDirectAction> {
        ensure!(
            source_revision
                .strip_prefix("sha256:")
                .is_some_and(is_sha256_hex),
            "direct action requires a canonical workflow source revision"
        );

        let mut digest = Sha256::new();
        digest.update(IDENTITY_DOMAIN);
        match &self {
            Self::Shell { command } => {
                validate_source(command, MAX_SHELL_BYTES, "shell command")?;
                digest.update([0]);
                hash_field(&mut digest, command.as_bytes());
            }
            Self::Compose { code, tool_ids } => {
                validate_source(code, MAX_COMPOSE_BYTES, "compose code")?;
                ensure!(
                    (1..=MAX_TOOL_IDS).contains(&tool_ids.len()),
                    "compose requires 1 to {MAX_TOOL_IDS} explicit tool IDs"
                );
                digest.update([1]);
                hash_field(&mut digest, code.as_bytes());
                digest.update((tool_ids.len() as u64).to_be_bytes());
                for (index, id) in tool_ids.iter().enumerate() {
                    ensure!(
                        valid_tool_id(id),
                        "invalid compose tool ID at index {index}"
                    );
                    ensure!(
                        !tool_ids[..index].contains(id),
                        "duplicate compose tool ID at index {index}"
                    );
                    hash_field(&mut digest, id.as_bytes());
                }
            }
        }

        ensure!(
            directory.is_absolute(),
            "direct action cwd must be absolute"
        );
        let directory = crate::windows_process::canonicalize_path(directory)
            .context("could not canonicalize direct action cwd")?;
        ensure!(
            directory
                .metadata()
                .context("could not inspect direct action cwd")?
                .is_dir(),
            "direct action cwd must be a directory"
        );
        let directory_text = directory
            .to_str()
            .context("direct action cwd must be UTF-8")?;
        hash_field(&mut digest, directory_text.as_bytes());
        hash_field(&mut digest, source_revision.as_bytes());

        Ok(ValidatedDirectAction {
            action: self,
            directory,
            approval_identity: format!("direct-action:v1:sha256:{:x}", digest.finalize()),
        })
    }
}

/// Only the server operator's process environment may populate this list.
pub(crate) const SHELL_APPROVALS_ENV: &str = "NEOISM_WORKFLOW_SHELL_APPROVALS";
pub(crate) const COMPOSE_APPROVALS_ENV: &str = "NEOISM_WORKFLOW_COMPOSE_APPROVALS";
pub(crate) const COMPOSE_DOCS_PERMISSION_ENV: &str =
    "NEOISM_WORKFLOW_COMPOSE_DOCS_PERMISSION";

pub(crate) fn require_shell_approval(
    action: &ValidatedDirectAction,
    configured: Option<&str>,
) -> Result<()> {
    ensure!(
        matches!(action.action(), DirectAction::Shell { .. }),
        "expected a shell action"
    );
    require_configured_approval(action, configured)
}

pub(crate) fn require_compose_approval(
    action: &ValidatedDirectAction,
    configured: Option<&str>,
) -> Result<()> {
    ensure!(
        matches!(action.action(), DirectAction::Compose { .. }),
        "expected a compose action"
    );
    require_configured_approval(action, configured)
}

fn require_configured_approval(
    action: &ValidatedDirectAction,
    configured: Option<&str>,
) -> Result<()> {
    const PREFIX: &str = "direct-action:v1:sha256:";
    const MAX_APPROVALS: usize = 32;
    let configured = configured.unwrap_or_default();
    ensure!(
        configured.len() <= MAX_APPROVALS * (PREFIX.len() + 65),
        "workflow action approval list is too large"
    );
    let mut approved = None;
    if !configured.is_empty() {
        for (index, entry) in configured.split(',').enumerate() {
            ensure!(
                index < MAX_APPROVALS
                    && entry.strip_prefix(PREFIX).is_some_and(is_sha256_hex),
                "invalid workflow action approval list"
            );
            if entry == action.approval_identity() {
                approved = Some(entry);
            }
        }
    }
    action.require_approval(approved)
}

fn is_sha256_hex(hex: &str) -> bool {
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl ValidatedDirectAction {
    pub(crate) fn action(&self) -> &DirectAction {
        &self.action
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn approval_identity(&self) -> &str {
        &self.approval_identity
    }

    /// Accept only an exact grant from trusted, correctly scoped approval state;
    /// an identity supplied by an invocation is not evidence of approval.
    pub(crate) fn require_approval(&self, stored_identity: Option<&str>) -> Result<()> {
        ensure!(
            stored_identity == Some(self.approval_identity()),
            "direct action requires approval for its current revision and inputs"
        );
        Ok(())
    }
}

fn validate_source(text: &str, limit: usize, label: &str) -> Result<()> {
    ensure!(text.len() <= limit, "{label} exceeds {limit} bytes");
    ensure!(!text.trim().is_empty(), "{label} must not be blank");
    ensure!(
        !text.as_bytes().contains(&0),
        "{label} must not contain NUL"
    );
    Ok(())
}

fn valid_tool_id(id: &str) -> bool {
    fn segment(text: &str) -> bool {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    }

    // Match the registry's sanitized MCP segments, without accepting aliases.
    fn mcp_segment(text: &str) -> bool {
        !text.is_empty()
            && !text.starts_with('_')
            && !text.ends_with('_')
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    id.len() <= MAX_TOOL_ID_BYTES
        && if let Some(rest) = id.strip_prefix("mcp__") {
            rest.split_once("__")
                .is_some_and(|(server, tool)| mcp_segment(server) && mcp_segment(tool))
        } else {
            id.split('.').all(segment)
        }
}

fn hash_field(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const REV: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW_REV: &str =
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn shell(command: &str) -> DirectAction {
        DirectAction::Shell {
            command: command.into(),
        }
    }

    fn compose(code: &str, tool_ids: &[&str]) -> DirectAction {
        DirectAction::Compose {
            code: code.into(),
            tool_ids: tool_ids.iter().map(|id| (*id).into()).collect(),
        }
    }

    #[test]
    fn action_shape_requires_exact_variant_fields() {
        for value in [
            json!({"kind":"shell","command":"pwd","toolIds":["read"]}),
            json!({"kind":"shell","command":"pwd","template":true}),
            json!({"kind":"shell","command":["pwd"]}),
            json!({"kind":"compose","code":"return 1;"}),
            json!({"kind":"compose","code":"return 1;","toolIds":["read"],"command":"pwd"}),
            json!({"kind":"prompt","command":"pwd"}),
        ] {
            assert!(serde_json::from_value::<DirectAction>(value).is_err());
        }
        assert!(serde_json::from_value::<DirectAction>(
            json!({"kind":"compose","code":"return 1;","toolIds":["read"]})
        )
        .is_ok());
    }

    #[test]
    fn source_text_is_bounded_in_bytes_and_not_normalized() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path();
        for text in ["", " \n\t", "ok\0no"] {
            assert!(shell(text).validate(cwd, REV).is_err());
            assert!(compose(text, &["read"]).validate(cwd, REV).is_err());
        }
        let text = "é".repeat(MAX_SHELL_BYTES / 2);
        let validated = shell(&text).validate(cwd, REV).unwrap();
        assert!(
            matches!(validated.action(), DirectAction::Shell { command } if command == &text)
        );
        assert!(shell(&(text + "x")).validate(cwd, REV).is_err());
        let code = "x".repeat(MAX_COMPOSE_BYTES);
        assert!(compose(&code, &["read"]).validate(cwd, REV).is_ok());
        assert!(compose(&(code + "x"), &["read"])
            .validate(cwd, REV)
            .is_err());
    }

    #[test]
    fn compose_tool_ids_are_explicit_bounded_and_unique() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path();
        for ids in [
            &[][..],
            &["read", "read"],
            &[""],
            &[" read"],
            &["*"],
            &["mcp__git__*"],
            &["mcp____status"],
            &["mcp__git__"],
            &["mcp__git__list-refs"],
            &["read/other"],
            &["read..other"],
        ] {
            assert!(compose("return 1;", ids).validate(cwd, REV).is_err());
        }
        assert!(compose("return 1;", &["read", "mcp__git__list_refs"])
            .validate(cwd, REV)
            .is_ok());
        assert!(compose("return 1;", &[&"x".repeat(MAX_TOOL_ID_BYTES + 1)])
            .validate(cwd, REV)
            .is_err());
        let ids: Vec<_> = (0..=MAX_TOOL_IDS).map(|i| format!("tool_{i}")).collect();
        let ids: Vec<_> = ids.iter().map(String::as_str).collect();
        assert!(compose("return 1;", &ids[..MAX_TOOL_IDS])
            .validate(cwd, REV)
            .is_ok());
        assert!(compose("return 1;", &ids).validate(cwd, REV).is_err());
    }

    #[test]
    fn cwd_and_revision_must_resolve_without_fallback() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path();
        let file = cwd.join("file");
        std::fs::write(&file, "file").unwrap();
        for invalid in [Path::new("."), &file, &cwd.join("missing")] {
            assert!(shell("pwd").validate(invalid, REV).is_err());
        }
        for invalid in ["", "sha256:abc", &REV.to_uppercase(), &format!(" {REV}")] {
            assert!(shell("pwd").validate(cwd, invalid).is_err());
        }
    }

    #[test]
    fn approval_is_exact_and_bound_to_kind_bytes_tools_cwd_and_revision() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let cwd = root.path();
        let original = compose("pwd", &["read", "glob"])
            .validate(cwd, REV)
            .unwrap();
        let stored = Some(original.approval_identity());
        let alias = compose("pwd", &["read", "glob"])
            .validate(&cwd.join("."), REV)
            .unwrap();
        assert_eq!(alias.directory(), original.directory());
        assert!(alias.require_approval(stored).is_ok());
        assert!(original.require_approval(None).is_err());
        assert!(original.require_approval(Some("*")).is_err());
        for changed in [
            shell("pwd").validate(cwd, REV),
            compose("pwd\n", &["read", "glob"]).validate(cwd, REV),
            compose("pwd", &["read"]).validate(cwd, REV),
            compose("pwd", &["glob", "read"]).validate(cwd, REV),
            compose("pwd", &["read", "glob"]).validate(other.path(), REV),
            compose("pwd", &["read", "glob"]).validate(cwd, NEW_REV),
        ] {
            assert!(changed.unwrap().require_approval(stored).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_match_but_retargeting_requires_new_approval() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        let link = root.path().join("link");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        std::os::unix::fs::symlink(&first, &link).unwrap();
        let direct = shell("pwd").validate(&first, REV).unwrap();
        let stored = Some(direct.approval_identity());
        assert!(shell("pwd")
            .validate(&link, REV)
            .unwrap()
            .require_approval(stored)
            .is_ok());
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&second, &link).unwrap();
        assert!(shell("pwd")
            .validate(&link, REV)
            .unwrap()
            .require_approval(stored)
            .is_err());
    }
}
