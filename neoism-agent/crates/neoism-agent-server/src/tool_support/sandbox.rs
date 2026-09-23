use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use neoism_agent_core::{ArtifactInfo, Id, IdKind};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::args::{optional_string, required_string, usize_arg};
use super::{process, ToolContext, ToolExecutionResult};

const INLINE_OUTPUT_BYTES: usize = 64 * 1024;

pub(super) async fn sandbox_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let command = required_string(&arguments, "command")?.to_string();
    let description = optional_string(&arguments, "description").unwrap_or_else(|| command.clone());
    let timeout_ms = usize_arg(&arguments, "timeout")
        .unwrap_or(120_000)
        .clamp(1, 1_800_000) as u64;
    let cwd = optional_string(&arguments, "workdir")
        .map(|path| sandbox_relative_path(&path))
        .transpose()?;
    let request = context
        .execution_request(
            neoism_agent_service_api::ProcessClass::Command,
            Some(timeout_ms),
        )
        .await?;
    let tenant_id = request.scope.tenant_id.clone();
    let session_id = request.scope.session_id.clone();
    let root_id = request.scope.root_id.clone();
    let expected_revision = request.workspace.revision.clone();
    let services = context.services();
    if !services.execution.available() {
        anyhow::bail!("sandbox execution provider is unavailable");
    }
    let lease = services.execution.acquire(request).await?;
    let result = tokio::select! {
        result = lease.exec(neoism_agent_service_api::ProcessSpec {
            executable: "sh".into(),
            args: vec!["-lc".into(), command.clone()],
            cwd,
            env: BTreeMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("NEOISM_TERMINAL".into(), "1".into()),
            ]),
            stdin: None,
            timeout_ms: Some(timeout_ms),
        }) => result?,
        _ = process::wait_for_cancel(context.cancel.clone()) => {
            let _ = lease.terminate().await;
            anyhow::bail!("sandbox command aborted");
        }
    };
    let commit = lease.commit_workspace().await?;
    if commit.base_revision != expected_revision {
        anyhow::bail!(
            "sandbox workspace base revision changed during execution (expected {:?}, provider returned {:?})",
            expected_revision,
            commit.base_revision
        );
    }
    if !services.execution.external_workspace_revisions() {
        if let Some(revision) = commit.revision.as_deref() {
            let state = context
                .state()
                .ok_or_else(|| anyhow::anyhow!("sandbox execution requires session state"))?;
            if !state
                .inner
                .store
                .commit_workspace_revision(
                    &tenant_id,
                    &root_id,
                    expected_revision.as_deref(),
                    revision,
                )
                .await?
            {
                anyhow::bail!("sandbox workspace revision conflict; retry from the latest revision");
            }
        }
    }
    let mut output = result.stdout;
    if !result.stderr.is_empty() {
        if !output.is_empty() {
            output.push(b'\n');
        }
        output.extend_from_slice(&result.stderr);
    }
    if output.is_empty() {
        output.extend_from_slice(b"(no output)");
    }
    let artifact = if output.len() > INLINE_OUTPUT_BYTES {
        Some(
            persist_output_artifact(&context, &tenant_id, &session_id, &output)
                .await?,
        )
    } else {
        None
    };
    let rendered = if let Some(artifact) = artifact.as_ref() {
        let prefix = String::from_utf8_lossy(&output[..INLINE_OUTPUT_BYTES]);
        format!(
            "{prefix}\n\n[output truncated; full output: artifact://{}]",
            artifact.id
        )
    } else {
        String::from_utf8_lossy(&output).into_owned()
    };
    if result.status != 0 {
        anyhow::bail!("sandbox command failed with status {}\n{}", result.status, rendered);
    }
    Ok(ToolExecutionResult {
        title: description,
        output: rendered,
        metadata: Some(json!({
            "command": command,
            "status": result.status,
            "timeout": timeout_ms,
            "leaseId": lease.id(),
            "provider": lease.backend_name(),
            "captureTruncated": result.truncated,
            "workspace": {
                "baseRevision": commit.base_revision,
                "revision": commit.revision,
                "changedPaths": commit.changed_paths,
            },
            "artifact": artifact,
        })),
    })
}

fn sandbox_relative_path(path: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        anyhow::bail!("sandbox workdir must be a relative path within the workspace");
    }
    Ok(path.to_path_buf())
}

async fn persist_output_artifact(
    context: &ToolContext,
    tenant_id: &str,
    session_id: &str,
    output: &[u8],
) -> anyhow::Result<ArtifactInfo> {
    let state = context
        .state()
        .ok_or_else(|| anyhow::anyhow!("sandbox artifacts require session state"))?;
    let session = state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("session not found"))?;
    let quotas = session
        .extra
        .get(crate::caller::QUOTAS_EXTRA_KEY)
        .cloned()
        .and_then(|value| {
            serde_json::from_value::<neoism_agent_service_api::TenantQuotas>(value).ok()
        })
        .unwrap_or_default();
    if quotas
        .max_artifact_bytes
        .is_some_and(|limit| output.len() > limit)
    {
        anyhow::bail!("sandbox output exceeds the tenant artifact byte quota");
    }
    if let Some(limit) = quotas.max_artifacts {
        let count = state
            .inner
            .store
            .list_artifacts(None, Some(tenant_id))
            .await?
            .len();
        if count >= limit {
            anyhow::bail!("tenant artifact count quota exceeded");
        }
    }
    let id = Id::ascending(IdKind::Artifact).to_string();
    state.put_artifact_blob(tenant_id, &id, output).await?;
    let artifact = ArtifactInfo {
        id: id.clone(),
        filename: "sandbox-output.txt".into(),
        media_type: "text/plain; charset=utf-8".into(),
        size: output.len() as u64,
        sha256: Sha256::digest(output)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        created: crate::now_millis(),
        session_id: Some(session_id.to_string()),
        download_url: format!("/v2/artifacts/{id}/content"),
    };
    if let Err(error) = state.inner.store.insert_artifact(&artifact, tenant_id).await {
        let _ = state.delete_artifact_blob(tenant_id, &id).await;
        return Err(error);
    }
    Ok(artifact)
}