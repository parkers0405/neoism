use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use neoism_agent_service_api::{
    ExecResult, ExecutionLease, ExecutionProcess, ExecutionProvider, ExecutionRequest, ProcessSpec,
    ServiceError, ServiceFuture, WorkspaceCommit,
};
use tokio::process::Command;

const MAX_CAPTURE_BYTES_PER_STREAM: usize = 1024 * 1024;

pub(crate) struct LocalExecutionProvider;

impl ExecutionProvider for LocalExecutionProvider {
    fn backend_name(&self) -> &'static str {
        "local-native"
    }

    fn available(&self) -> bool {
        true
    }

    fn acquire<'a>(
        &'a self,
        request: ExecutionRequest,
    ) -> ServiceFuture<'a, Result<Arc<dyn ExecutionLease>, ServiceError>> {
        Box::pin(async move {
            if request.provider.is_some() {
                return Err(ServiceError::new(
                    "local execution provider cannot satisfy a hosted sandbox policy",
                ));
            }
            let root = request.workspace.local_path.ok_or_else(|| {
                ServiceError::new("local execution requires a local workspace path")
            })?;
            let root = crate::windows_process::canonicalize_path(&root)
                .map_err(|error| ServiceError::new(error.to_string()))?;
            Ok(Arc::new(LocalExecutionLease {
                id: request.scope.execution_id,
                root,
            }) as Arc<dyn ExecutionLease>)
        })
    }
}

struct LocalExecutionLease {
    id: String,
    root: PathBuf,
}

impl ExecutionLease for LocalExecutionLease {
    fn id(&self) -> &str {
        &self.id
    }

    fn backend_name(&self) -> &'static str {
        "local-native"
    }

    fn exec<'a>(
        &'a self,
        spec: ProcessSpec,
    ) -> ServiceFuture<'a, Result<ExecResult, ServiceError>> {
        Box::pin(async move {
            let cwd = spec.cwd.as_deref().unwrap_or(&self.root);
            let cwd = crate::windows_process::canonicalize_path(cwd)
                .map_err(|error| ServiceError::new(error.to_string()))?;
            ensure_within_root(&self.root, &cwd)?;

            let mut command = Command::new(&spec.executable);
            command
                .args(&spec.args)
                .current_dir(cwd)
                .envs(spec.env)
                .kill_on_drop(true)
                .stdin(if spec.stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            crate::tool::process::set_new_process_group(&mut command);
            let mut child = command
                .spawn()
                .map_err(|error| ServiceError::new(format!("failed to spawn process: {error}")))?;
            if let Some(stdin) = spec.stdin {
                use tokio::io::AsyncWriteExt;
                if let Some(mut pipe) = child.stdin.take() {
                    pipe.write_all(&stdin)
                        .await
                        .map_err(|error| ServiceError::new(error.to_string()))?;
                }
            }
            let child_id = child.id();
            let stdout = crate::tool::process::read_child_output(
                child.stdout.take(),
                MAX_CAPTURE_BYTES_PER_STREAM,
            );
            let stderr = crate::tool::process::read_child_output(
                child.stderr.take(),
                MAX_CAPTURE_BYTES_PER_STREAM,
            );
            let wait = async {
                if let Some(timeout_ms) = spec.timeout_ms {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms.max(1)),
                        child.wait(),
                    )
                    .await
                    {
                        Ok(status) => status.map_err(|error| ServiceError::new(error.to_string())),
                        Err(_) => {
                            crate::tool::process::terminate_child(&mut child, child_id).await;
                            Err(ServiceError::new(format!(
                                "process timed out after {timeout_ms}ms"
                            )))
                        }
                    }
                } else {
                    child
                        .wait()
                        .await
                        .map_err(|error| ServiceError::new(error.to_string()))
                }
            };
            let (status, stdout, stderr) = tokio::join!(wait, stdout, stderr);
            let status = status?;
            let stdout = stdout
                .map_err(|error| ServiceError::new(error.to_string()))?
                .map_err(|error| ServiceError::new(error.to_string()))?;
            let stderr = stderr
                .map_err(|error| ServiceError::new(error.to_string()))?
                .map_err(|error| ServiceError::new(error.to_string()))?;
            Ok(ExecResult {
                status: status.code().unwrap_or(-1),
                stdout: stdout.bytes,
                stderr: stderr.bytes,
                truncated: stdout.truncated || stderr.truncated,
            })
        })
    }

    fn spawn<'a>(
        &'a self,
        _spec: ProcessSpec,
        _pty: bool,
    ) -> ServiceFuture<'a, Result<Arc<dyn ExecutionProcess>, ServiceError>> {
        Box::pin(async { Err(ServiceError::new("streaming local execution is not migrated yet")) })
    }

    fn commit_workspace<'a>(
        &'a self,
    ) -> ServiceFuture<'a, Result<WorkspaceCommit, ServiceError>> {
        Box::pin(async {
            Ok(WorkspaceCommit {
                base_revision: None,
                revision: None,
                changed_paths: Vec::new(),
            })
        })
    }

    fn terminate<'a>(&'a self) -> ServiceFuture<'a, Result<(), ServiceError>> {
        Box::pin(async { Ok(()) })
    }
}

fn ensure_within_root(root: &Path, cwd: &Path) -> Result<(), ServiceError> {
    if cwd.starts_with(root) {
        Ok(())
    } else {
        Err(ServiceError::new(format!(
            "process working directory {} is outside workspace {}",
            cwd.display(),
            root.display()
        )))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use neoism_agent_service_api::{
        ExecutionScope, NetworkPolicy, ProcessClass, ResourceLimits, WorkspaceMaterialization,
    };

    #[tokio::test]
    async fn local_provider_executes_only_local_native_requests() {
        let root = std::env::temp_dir().join(format!(
            "neoism-local-execution-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let request = ExecutionRequest {
            scope: ExecutionScope {
                tenant_id: "local".into(),
                subject: "local".into(),
                root_id: "local".into(),
                session_id: "local".into(),
                execution_id: "test".into(),
            },
            provider: None,
            process_class: ProcessClass::Command,
            workspace: WorkspaceMaterialization {
                revision: None,
                local_path: Some(root.clone()),
                remote_locator: None,
            },
            limits: ResourceLimits::default(),
            network: NetworkPolicy::Allow,
            idle_ttl_seconds: 0,
            max_lifetime_seconds: 0,
        };
        let lease = LocalExecutionProvider.acquire(request).await.unwrap();
        let result = lease
            .exec(ProcessSpec {
                executable: "sh".into(),
                args: vec!["-c".into(), "printf brokered".into()],
                cwd: Some(root.clone()),
                env: Default::default(),
                stdin: None,
                timeout_ms: Some(5_000),
            })
            .await
            .unwrap();
        assert_eq!(result.status, 0);
        assert_eq!(result.stdout, b"brokered");

        let hosted = ExecutionRequest {
            scope: ExecutionScope {
                tenant_id: "company".into(),
                subject: "service".into(),
                root_id: "root".into(),
                session_id: "session".into(),
                execution_id: "hosted".into(),
            },
            provider: Some("vercel".into()),
            process_class: ProcessClass::Command,
            workspace: WorkspaceMaterialization {
                revision: None,
                local_path: Some(root.clone()),
                remote_locator: None,
            },
            limits: ResourceLimits::default(),
            network: NetworkPolicy::Deny,
            idle_ttl_seconds: 60,
            max_lifetime_seconds: 300,
        };
        assert!(LocalExecutionProvider.acquire(hosted).await.is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}