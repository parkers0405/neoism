use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use neoism_agent_core::{VcsApplyResult, VcsFileDiff, VcsFileStatus, VcsInfo};
use neoism_agent_plugin_api::{
    ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture,
    PluginHostError, PluginManifest, PluginScope, RouteContribution, RouteDescriptor,
    RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};
use neoism_agent_service_api::{AgentServices, ExecutablePurpose, ExecutableRequest};
use serde_json::{json, Value};

pub const ID: &str = "dev.neoism.vcs";

pub struct VcsPlugin {
    services: AgentServices,
}

impl VcsPlugin {
    pub fn new(services: AgentServices) -> Self {
        Self { services }
    }
}

impl PluginDefinition for VcsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Version control".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.vcs".into()],
            requires: Vec::new(),
            event_namespaces: vec!["vcs".into()],
            api_prefix: Some(format!("/v2/plugins/{ID}")),
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        use neoism_agent_plugin_api::HostCapability::*;
        vec![WorkspaceRead, WorkspaceWrite, ProcessSpawn]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        for (operation_id, method, suffix, action) in [
            (
                "v2.plugins.vcs.get",
                RouteMethod::Get,
                "",
                VcsRouteAction::Info,
            ),
            (
                "v2.plugins.vcs.diff",
                RouteMethod::Get,
                "/diff",
                VcsRouteAction::Diff,
            ),
            (
                "v2.plugins.vcs.status",
                RouteMethod::Get,
                "/status",
                VcsRouteAction::Status,
            ),
            (
                "v2.plugins.vcs.diffRaw",
                RouteMethod::Get,
                "/diff/raw",
                VcsRouteAction::DiffRaw,
            ),
            (
                "v2.plugins.vcs.apply",
                RouteMethod::Post,
                "/apply",
                VcsRouteAction::Apply,
            ),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: operation_id.into(),
                    method,
                    path: format!("/v2/plugins/{ID}{suffix}"),
                    scope: RouteScope::Workspace,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(
                    operation_id,
                    ID,
                    PluginScope::Workspace,
                ),
                handler: Arc::new(VcsRoute {
                    services: self.services.clone(),
                    action,
                }),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum VcsRouteAction {
    Info,
    Diff,
    Status,
    DiffRaw,
    Apply,
}

struct VcsRoute {
    services: AgentServices,
    action: VcsRouteAction,
}

impl RouteHandler for VcsRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let directory = request.workspace.unwrap_or_default();
            let directory = directory.to_string_lossy();
            let body = match self.action {
                VcsRouteAction::Info => {
                    serde_json::to_value(info(&self.services, &directory))
                }
                VcsRouteAction::Diff => {
                    serde_json::to_value(diff(&self.services, &directory))
                }
                VcsRouteAction::Status => {
                    serde_json::to_value(status(&self.services, &directory))
                }
                VcsRouteAction::DiffRaw => Ok(serde_json::Value::String(diff_raw(
                    &self.services,
                    &directory,
                ))),
                VcsRouteAction::Apply => serde_json::to_value(apply(
                    &self.services,
                    &directory,
                    patch_from_body(&request.body).unwrap_or_default(),
                )),
            };
            let body = match body {
                Ok(body) => body,
                Err(error) => {
                    return Err(neoism_agent_plugin_api::PluginRuntimeError::new(
                        error.to_string(),
                    ))
                }
            };
            let mut response = RouteResponse::json(200, body);
            if matches!(self.action, VcsRouteAction::DiffRaw) {
                response
                    .headers
                    .insert("content-type".into(), "text/plain; charset=utf-8".into());
            }
            Ok(response)
        })
    }
}

pub fn info(services: &AgentServices, directory: &str) -> VcsInfo {
    VcsInfo {
        branch: git_output(services, directory, &["branch", "--show-current"]),
        default_branch: git_output(
            services,
            directory,
            &["symbolic-ref", "refs/remotes/origin/HEAD"],
        )
        .and_then(|value| value.rsplit('/').next().map(ToOwned::to_owned)),
    }
}

pub fn status(services: &AgentServices, directory: &str) -> Vec<VcsFileStatus> {
    let Some(output) =
        git_output_raw(services, directory, &["status", "--porcelain=v1", "-z"])
    else {
        return Vec::new();
    };
    let mut stats = diff_stats(services, directory);
    let mut statuses = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let code = String::from_utf8_lossy(&field[..2]).to_string();
        let path = String::from_utf8_lossy(&field[3..]).to_string();
        if code.starts_with('R') || code.starts_with('C') {
            fields.next();
        }
        let (additions, deletions) = stats.remove(&path).unwrap_or_else(|| {
            if normalize_status(&code) == "added"
                && is_untracked(services, directory, &path)
            {
                stat_untracked(directory, &path)
            } else {
                (0, 0)
            }
        });
        statuses.push(VcsFileStatus {
            file: path.clone(),
            path,
            status: normalize_status(&code).into(),
            additions,
            deletions,
        });
    }
    statuses.sort_by(|left, right| left.file.cmp(&right.file));
    statuses
}

pub fn diff(services: &AgentServices, directory: &str) -> Vec<VcsFileDiff> {
    let mut stats = diff_stats(services, directory);
    status(services, directory)
        .into_iter()
        .map(|file| {
            let patch = if file.status == "added"
                && is_untracked(services, directory, &file.path)
            {
                untracked_patch(directory, &file.path)
            } else {
                git_output(services, directory, &["diff", "HEAD", "--", &file.path])
            }
            .unwrap_or_default();
            let (added, removed) = stats
                .remove(&file.path)
                .unwrap_or_else(|| count_patch_lines(&patch));
            let hunks = (!patch.is_empty())
                .then(|| vec![json!({ "patch": patch.clone() })])
                .unwrap_or_default();
            VcsFileDiff {
                file: file.path.clone(),
                path: file.path,
                status: file.status,
                added,
                removed,
                additions: added,
                deletions: removed,
                patch,
                hunks,
            }
        })
        .collect()
}

pub fn diff_raw(services: &AgentServices, directory: &str) -> String {
    git_output(services, directory, &["diff", "HEAD"]).unwrap_or_default()
}

pub fn apply(services: &AgentServices, directory: &str, patch: &str) -> VcsApplyResult {
    if patch.trim().is_empty() {
        return failure("missing patch");
    }
    let git = match resolve_git(services) {
        Ok(path) => path,
        Err(error) => return failure(error.to_string()),
    };
    let mut command = hidden_command(git);
    let mut child = match command
        .args(["apply", "--whitespace=nowarn", "--"])
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return failure(format!("failed to start git apply: {error}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(patch.as_bytes()) {
            return failure(format!("failed to write patch to git apply: {error}"));
        }
    }
    match child.wait_with_output() {
        Ok(output) if output.status.success() => VcsApplyResult {
            success: true,
            error: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            failure(if stderr.is_empty() { stdout } else { stderr })
        }
        Err(error) => failure(format!("failed to wait for git apply: {error}")),
    }
}

pub fn patch_from_body(body: &Value) -> Option<&str> {
    body.get("patch").and_then(Value::as_str)
}

fn failure(error: impl Into<String>) -> VcsApplyResult {
    VcsApplyResult {
        success: false,
        error: Some(error.into()),
    }
}

fn diff_stats(services: &AgentServices, directory: &str) -> BTreeMap<String, (u64, u64)> {
    git_output(services, directory, &["diff", "--numstat", "HEAD"])
        .into_iter()
        .flat_map(|output| {
            output
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, '\t');
                    Some((
                        parts.next()?.parse().ok()?,
                        parts.next()?.parse().ok()?,
                        parts.next()?.to_string(),
                    ))
                })
                .map(|(added, removed, path)| (path, (added, removed)))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn git_output(
    services: &AgentServices,
    directory: &str,
    args: &[&str],
) -> Option<String> {
    let output = git_output_raw(services, directory, args)?;
    let text = String::from_utf8_lossy(&output).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn git_output_raw(
    services: &AgentServices,
    directory: &str,
    args: &[&str],
) -> Option<Vec<u8>> {
    let mut command = hidden_command(resolve_git(services).ok()?);
    command.args(args).current_dir(directory);
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn resolve_git(services: &AgentServices) -> anyhow::Result<PathBuf> {
    services.executables.resolve(&ExecutableRequest::new("git", ExecutablePurpose::VersionControl))
        .map(|result| result.path)
        .map_err(|error| anyhow::anyhow!("Git version-control executable `git` is unavailable: {error}; configure the host executable resolver or install it"))
}

fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
        );
    }
    command
}

fn normalize_status(code: &str) -> &'static str {
    if code == "??" || code.contains('A') {
        "added"
    } else if code.contains('D') {
        "deleted"
    } else {
        "modified"
    }
}

fn is_untracked(services: &AgentServices, directory: &str, path: &str) -> bool {
    git_output(
        services,
        directory,
        &["ls-files", "--others", "--exclude-standard", "--", path],
    )
    .is_some_and(|output| output.lines().any(|line| line == path))
}

fn untracked_patch(directory: &str, path: &str) -> Option<String> {
    let content = std::fs::read_to_string(PathBuf::from(directory).join(path)).ok()?;
    let lines = content.lines().collect::<Vec<_>>();
    let mut patch = format!("diff --git a/{0} b/{0}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n", patch_path(path), lines.len());
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if !content.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
    }
    Some(patch)
}

fn stat_untracked(directory: &str, path: &str) -> (u64, u64) {
    let Ok(bytes) = std::fs::read(PathBuf::from(directory).join(path)) else {
        return (0, 0);
    };
    if bytes.contains(&0) {
        return (0, 0);
    }
    let text = String::from_utf8_lossy(&bytes);
    (
        (if text.is_empty() {
            0
        } else {
            text.lines().count()
        }) as u64,
        0,
    )
}

fn patch_path(path: &str) -> String {
    Path::new(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn count_patch_lines(patch: &str) -> (u64, u64) {
    patch.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with("+++") || line.starts_with("---") {
            (added, removed)
        } else if line.starts_with('+') {
            (added + 1, removed)
        } else if line.starts_with('-') {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_canonical_patch_body_name() {
        assert_eq!(patch_from_body(&json!({"patch": "x"})), Some("x"));
        assert_eq!(patch_from_body(&json!({"diff": "x"})), None);
        assert_eq!(patch_from_body(&json!({"content": "x"})), None);
    }

    #[test]
    fn counts_only_patch_content() {
        assert_eq!(count_patch_lines("--- a/x\n+++ b/x\n-old\n+new\n"), (1, 1));
    }

    #[test]
    fn plugin_registers_canonical_route() {
        let plugin = VcsPlugin::new(test_services());
        assert_eq!(plugin.manifest().id, ID);
        plugin
            .contributions(&mut PluginContributions::default())
            .unwrap();
    }

    fn test_services() -> AgentServices {
        use std::sync::Arc;
        AgentServices::new(
            Arc::new(neoism_agent_service_api::StandardExecutableService),
            Arc::new(NoSearch),
        )
    }

    struct NoSearch;
    impl neoism_agent_service_api::WorkspaceSearchService for NoSearch {
        fn warm(&self, _: &Path) -> Result<(), neoism_agent_service_api::ServiceError> {
            Ok(())
        }
        fn pin_root(
            &self,
            _: &Path,
        ) -> Result<
            Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>,
            neoism_agent_service_api::ServiceError,
        > {
            Err(neoism_agent_service_api::ServiceError::new("unused"))
        }
        fn find_files(
            &self,
            _: &neoism_agent_service_api::FindFilesRequest,
        ) -> Result<
            neoism_agent_service_api::FindFilesResult,
            neoism_agent_service_api::ServiceError,
        > {
            Err(neoism_agent_service_api::ServiceError::new("unused"))
        }
        fn grep(
            &self,
            _: &neoism_agent_service_api::GrepWorkspaceRequest,
        ) -> Result<
            neoism_agent_service_api::GrepWorkspaceResult,
            neoism_agent_service_api::ServiceError,
        > {
            Err(neoism_agent_service_api::ServiceError::new("unused"))
        }
        fn search_directories(
            &self,
            _: &neoism_agent_service_api::DirectorySearchRequest,
        ) -> Result<
            neoism_agent_service_api::DirectorySearchResult,
            neoism_agent_service_api::ServiceError,
        > {
            Err(neoism_agent_service_api::ServiceError::new("unused"))
        }
    }
}
