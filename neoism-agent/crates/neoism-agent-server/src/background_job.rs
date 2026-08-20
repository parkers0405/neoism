use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::Context;
use axum::extract::{Path as AxumPath, State};
use axum::Json;
use neoism_agent_core::{
    create, event_type, EventPayload, Id, IdDirection, IdKind, PermissionRule,
    PromptPart, PromptRequest,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::ApiError;
use crate::state::AppState;
use crate::tool::{self, process, shell_scan};
use crate::{ensure_tool_permission, now_millis};

const BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background shell task completion.";
const DEFAULT_TIMEOUT_MS: u64 = 30 * 60 * 1000;
const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 256 * 1024;
const BACKGROUND_TASK_RESULT_INLINE_CHARS: usize = 32_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackgroundJob {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) description: String,
    pub(crate) command: String,
    pub(crate) cwd: String,
    pub(crate) shell: String,
    pub(crate) status: BackgroundJobStatus,
    pub(crate) started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) signal: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pid: Option<u32>,
    pub(crate) timeout_ms: u64,
    pub(crate) output_limit_bytes: usize,
    pub(crate) output_truncated: bool,
    pub(crate) output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) command_patterns: Vec<String>,
    pub(crate) always_patterns: Vec<String>,
    pub(crate) external_dirs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackgroundJobStatus {
    Running,
    Completed,
    Cancelled,
    Error,
    TimedOut,
}

impl BackgroundJobStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
            Self::TimedOut => "timed_out",
        }
    }

    fn result_tag(&self) -> &'static str {
        match self {
            Self::Completed | Self::Cancelled => "background_task_result",
            Self::Running | Self::Error | Self::TimedOut => "background_task_error",
        }
    }
}

struct JobFinish {
    status: BackgroundJobStatus,
    exit_code: Option<i32>,
    signal: Option<i32>,
    output: String,
    output_truncated: bool,
    error: Option<String>,
}

struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

pub(crate) async fn start_background_task_tool(
    state: &AppState,
    session_id: &Id,
    permissions: &[PermissionRule],
    input: Value,
    env: BTreeMap<String, String>,
) -> Result<tool::ToolExecutionResult, String> {
    let parent = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("session {session_id} not found"))?;
    let command = string_arg(&input, "command")
        .ok_or_else(|| "tool argument command is required".to_string())?;
    let description = string_arg(&input, "description")
        .unwrap_or_else(|| command.chars().take(80).collect::<String>());
    let cwd = resolve_job_cwd(&parent.directory, optional_cwd(&input), permissions)?;
    let project_root =
        crate::windows_process::canonicalize_path(Path::new(&parent.directory))
            .map_err(|error| format!("failed to resolve project directory: {error}"))?;
    let scan = shell_scan::scan(
        &command,
        &cwd,
        &project_root,
        crate::platform_shell::runtime().kind(),
    );
    for dir in &scan.external_dirs {
        ensure_tool_permission(permissions, "external_directory", dir)?;
    }
    if scan.command_patterns.is_empty() {
        ensure_tool_permission(permissions, "bash", command.trim())?;
    } else {
        for pattern in &scan.command_patterns {
            ensure_tool_permission(permissions, "bash", pattern)?;
        }
    }

    let timeout_ms = u64_arg(&input, "timeout")
        .or_else(|| u64_arg(&input, "timeoutMs"))
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .max(1);
    let output_limit_bytes = usize_arg(&input, "outputLimit")
        .or_else(|| usize_arg(&input, "output_limit"))
        .or_else(|| usize_arg(&input, "maxOutputBytes"))
        .unwrap_or(DEFAULT_OUTPUT_LIMIT_BYTES)
        .max(1);
    let runtime = crate::platform_shell::runtime();
    let shell = runtime.program().to_string_lossy().into_owned();
    let mut command_proc = Command::new(&shell);
    runtime.apply_command(&mut command_proc, &command, true);
    command_proc
        .current_dir(&cwd)
        .env("TERM", "xterm-256color")
        .env("NEOISM_TERMINAL", "1")
        .envs(env)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process::set_new_process_group(&mut command_proc);
    let child = command_proc
        .spawn()
        .with_context(|| format!("failed to spawn shell {shell}"))
        .map_err(|error| error.to_string())?;
    let pid = child.id();
    let job_id = create("job", IdDirection::Ascending, None);
    let job = BackgroundJob {
        id: job_id.clone(),
        session_id: session_id.to_string(),
        description: description.clone(),
        command: command.clone(),
        cwd: cwd.to_string_lossy().to_string(),
        shell: shell.clone(),
        status: BackgroundJobStatus::Running,
        started_at: now_millis(),
        finished_at: None,
        exit_code: None,
        signal: None,
        pid,
        timeout_ms,
        output_limit_bytes,
        output_truncated: false,
        output: String::new(),
        error: None,
        command_patterns: scan.command_patterns.iter().cloned().collect(),
        always_patterns: scan.always_patterns.iter().cloned().collect(),
        external_dirs: scan.external_dirs.iter().cloned().collect(),
    };
    state
        .inner
        .background_jobs
        .write()
        .await
        .insert(job_id.clone(), job.clone());

    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    state
        .inner
        .background_job_cancellations
        .write()
        .await
        .insert(job_id.clone(), cancel_tx);
    tokio::spawn(run_background_job(
        state.clone(),
        job.clone(),
        child,
        cancel_rx,
    ));

    Ok(tool::ToolExecutionResult {
        title: description,
        output: background_task_started_output(&job),
        metadata: Some(job_metadata(&job)),
    })
}

pub(crate) async fn background_task_result_tool(
    state: &AppState,
    session_id: &Id,
    input: Value,
) -> Result<tool::ToolExecutionResult, String> {
    if let Some(job_id) = string_arg(&input, "job_id") {
        let job = state
            .inner
            .background_jobs
            .read()
            .await
            .get(&job_id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "background job {job_id} not found; background task state is only retained during this server lifetime"
                )
            })?;
        ensure_job_belongs_to_session(&job, session_id)?;
        return Ok(tool::ToolExecutionResult {
            title: job.description.clone(),
            output: background_task_result_output(&job),
            metadata: Some(job_metadata(&job)),
        });
    }

    let mut jobs = state
        .inner
        .background_jobs
        .read()
        .await
        .values()
        .filter(|job| job.session_id == session_id.to_string())
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by(|left, right| right.started_at.cmp(&left.started_at));
    if jobs.is_empty() {
        return Ok(tool::ToolExecutionResult {
            title: "Background tasks".to_string(),
            output: "No background tasks exist for this session yet.".to_string(),
            metadata: Some(json!({ "jobs": [] })),
        });
    }

    let mut lines = vec!["Background tasks for this session:".to_string()];
    let mut metadata = Vec::new();
    for job in jobs {
        lines.push(format!(
            "job_id: {} status: {} exit: {} description: {}",
            job.id,
            job.status.as_str(),
            job.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string()),
            job.description
        ));
        metadata.push(job_metadata(&job));
    }
    Ok(tool::ToolExecutionResult {
        title: "Background tasks".to_string(),
        output: lines.join("\n"),
        metadata: Some(json!({ "jobs": metadata })),
    })
}

pub(crate) async fn stop_background_task(
    State(state): State<AppState>,
    AxumPath((session_id, job_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let jobs = state.inner.background_jobs.read().await;
    let job = jobs.get(&job_id).ok_or_else(|| {
        ApiError::not_found(format!("background job {job_id} not found"))
    })?;
    if job.session_id != session_id {
        return Err(ApiError::not_found(format!(
            "background job {job_id} not found in session {session_id}"
        )));
    }
    if job.status != BackgroundJobStatus::Running {
        return Err(ApiError::conflict(format!(
            "background job {job_id} is already {}",
            job.status.as_str()
        )));
    }
    drop(jobs);
    let cancel = state
        .inner
        .background_job_cancellations
        .write()
        .await
        .remove(&job_id)
        .ok_or_else(|| {
            ApiError::conflict(format!("background job {job_id} is already finishing"))
        })?;
    cancel.send(()).map_err(|_| {
        ApiError::conflict(format!("background job {job_id} is already finishing"))
    })?;
    Ok(Json(json!({ "jobId": job_id, "status": "stopping" })))
}

async fn run_background_job(
    state: AppState,
    mut job: BackgroundJob,
    mut child: tokio::process::Child,
    cancel_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let finish = wait_for_background_job(&mut child, &job, cancel_rx).await;
    state
        .inner
        .background_job_cancellations
        .write()
        .await
        .remove(&job.id);
    job.status = finish.status;
    job.finished_at = Some(now_millis());
    job.exit_code = finish.exit_code;
    job.signal = finish.signal;
    job.output = finish.output;
    job.output_truncated = finish.output_truncated;
    job.error = finish.error;
    finish_background_job(&state, job).await;
}

async fn wait_for_background_job(
    child: &mut tokio::process::Child,
    job: &BackgroundJob,
    mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
) -> JobFinish {
    let child_id = child.id();
    let stdout_task =
        read_limited_child_output(child.stdout.take(), job.output_limit_bytes);
    let stderr_task =
        read_limited_child_output(child.stderr.take(), job.output_limit_bytes);
    let timeout = tokio::time::sleep(Duration::from_millis(job.timeout_ms));
    tokio::pin!(timeout);

    let mut timed_out = false;
    let mut cancelled = false;
    let wait_result: anyhow::Result<ExitStatus> = tokio::select! {
        biased;
        _ = &mut cancel_rx => {
            cancelled = true;
            process::terminate_child(child, child_id).await;
            Err(anyhow::anyhow!("background task cancelled"))
        }
        status = child.wait() => {
            status.with_context(|| format!("failed to wait for shell {}", job.shell))
        }
        _ = &mut timeout => {
            timed_out = true;
            process::terminate_child(child, child_id).await;
            Err(anyhow::anyhow!(
                "background task timed out after {}ms",
                job.timeout_ms
            ))
        }
    };

    let stdout = join_limited_output(stdout_task).await;
    let stderr = join_limited_output(stderr_task).await;
    let output_truncated = stdout.truncated || stderr.truncated;
    let mut error = None;
    let mut exit_code = None;
    let mut signal = None;
    let status = match wait_result {
        Ok(status) if status.success() => {
            exit_code = status.code();
            signal = exit_signal(&status);
            BackgroundJobStatus::Completed
        }
        Ok(status) => {
            exit_code = status.code();
            signal = exit_signal(&status);
            BackgroundJobStatus::Error
        }
        Err(wait_error) => {
            if cancelled {
                BackgroundJobStatus::Cancelled
            } else if timed_out {
                error = Some(wait_error.to_string());
                BackgroundJobStatus::TimedOut
            } else {
                error = Some(wait_error.to_string());
                BackgroundJobStatus::Error
            }
        }
    };
    let mut output = render_job_output(stdout.bytes, stderr.bytes);
    if output_truncated {
        output.push_str(&format!(
            "\n... output truncated after {} bytes.",
            job.output_limit_bytes
        ));
    }
    if let Some(message) = error.as_ref() {
        if output == "(no output)" {
            output.clear();
        } else {
            output.push('\n');
        }
        output.push_str(message);
    }

    JobFinish {
        status,
        exit_code,
        signal,
        output,
        output_truncated,
        error,
    }
}

async fn finish_background_job(state: &AppState, job: BackgroundJob) {
    state
        .inner
        .background_jobs
        .write()
        .await
        .insert(job.id.clone(), job.clone());
    state.publish(EventPayload::new(
        event_type::SESSION_BACKGROUND_TASK_COMPLETED,
        json!({
            "sessionID": job.session_id.clone(),
            "parentSessionID": job.session_id.clone(),
            "jobID": job.id.clone(),
            "taskID": job.id.clone(),
            "status": job.status.as_str(),
            "title": job.description.clone(),
            "command": job.command.clone(),
            "cwd": job.cwd.clone(),
            "exitCode": job.exit_code,
            "result": job.output.clone(),
        }),
    ));
    if let Err(error) =
        enqueue_parent_background_task_completion_prompt(state, &job).await
    {
        tracing::warn!(
            session_id = %job.session_id,
            job_id = %job.id,
            error = %error,
            "failed to notify session about completed background task"
        );
    }
}

async fn enqueue_parent_background_task_completion_prompt(
    state: &AppState,
    job: &BackgroundJob,
) -> Result<(), ApiError> {
    if state
        .inner
        .store
        .get_session(job.session_id.as_str())
        .await?
        .is_none()
    {
        return Ok(());
    }
    let request = background_task_completion_request(job);
    let event_request = request.clone();
    let (start_worker, queue_len) =
        crate::session_queue::enqueue_prompt_request_with_delivery(
            state,
            job.session_id.as_str(),
            request,
            "continue",
        )
        .await?;
    crate::session_queue::publish_prompt_queue_changed(
        state,
        job.session_id.as_str(),
        "enqueue",
        Some(&event_request),
        Some("continue"),
        0,
    )
    .await;
    crate::session_queue::publish_prompt_queue_status(
        state,
        job.session_id.as_str(),
        queue_len,
    )
    .await;
    if start_worker {
        crate::session_queue::spawn_drain_prompt_queue(
            state.clone(),
            job.session_id.clone(),
        );
    }
    Ok(())
}

fn background_task_completion_request(job: &BackgroundJob) -> PromptRequest {
    PromptRequest {
        message_id: Some(
            Id::parse(
                IdKind::Message,
                format!("msg_background_completion_{}", job.id),
            )
            .expect("runtime completion message id"),
        ),
        model: None,
        agent: None,
        no_reply: false,
        system: Some(background_task_completion_system()),
        tools: None,
        author: None,
        parts: vec![PromptPart::Text {
            text: background_task_completion_prompt(job),
        }],
    }
}

fn background_task_completion_system() -> String {
    [
        BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER.to_string(),
        "This message is generated by the runtime, not by the user. Treat it as session state."
            .to_string(),
        "A background shell task has finished.".to_string(),
        "The job_id in the paired message is the in-memory handle for this server lifetime."
            .to_string(),
        "The paired message includes captured process output as system context, not a user request."
            .to_string(),
        "Call background_task_result with that job_id if you need to reread the retained output later."
            .to_string(),
    ]
    .join("\n")
}

fn background_task_completion_prompt(job: &BackgroundJob) -> String {
    let tag = job.status.result_tag();
    let mut lines = vec![
        "Background shell task finished.".to_string(),
        format!("job_id: {}", job.id),
        format!("description: {}", job.description),
        format!("status: {}", job.status.as_str()),
        format!("exit_code: {}", option_i32(job.exit_code)),
        format!("cwd: {}", job.cwd),
        format!("command: {}", job.command),
        String::new(),
        "The captured process output is included below as runtime system context.".to_string(),
        "Call background_task_result with this job_id to reread retained output during this server lifetime."
            .to_string(),
        String::new(),
        format!("<{tag}>"),
        background_task_result_inline(job),
        format!("</{tag}>"),
    ];
    if job.output_truncated {
        lines.insert(
            8,
            format!(
                "Captured output was truncated after {} bytes.",
                job.output_limit_bytes
            ),
        );
    }
    lines.join("\n")
}

fn background_task_result_inline(job: &BackgroundJob) -> String {
    let trimmed = job.output.trim();
    let output = if trimmed.is_empty() {
        "(background task produced no output)".to_string()
    } else if trimmed.chars().count() <= BACKGROUND_TASK_RESULT_INLINE_CHARS {
        trimmed.to_string()
    } else {
        let mut preview = trimmed
            .chars()
            .take(BACKGROUND_TASK_RESULT_INLINE_CHARS)
            .collect::<String>();
        preview.push_str(
            "\n... result truncated in notification; call background_task_result for retained output.",
        );
        preview
    };
    if let Some(error) = job.error.as_ref() {
        format!("{output}\nerror: {error}")
    } else {
        output
    }
}

fn background_task_started_output(job: &BackgroundJob) -> String {
    [
        format!("job_id: {} (use this with background_task_result)", job.id),
        "status: running".to_string(),
        format!("description: {}", job.description),
        format!("cwd: {}", job.cwd),
        format!("command: {}", job.command),
        String::new(),
        "The shell task is running in the background. The main session can keep working. A runtime notification will be queued when it exits."
            .to_string(),
    ]
    .join("\n")
}

fn background_task_result_output(job: &BackgroundJob) -> String {
    if job.status == BackgroundJobStatus::Running {
        return [
            format!("job_id: {}", job.id),
            "status: running".to_string(),
            format!("description: {}", job.description),
            format!("cwd: {}", job.cwd),
            format!("command: {}", job.command),
            String::new(),
            "The background task is still running. Keep working in the main session or call background_task_result later."
                .to_string(),
        ]
        .join("\n");
    }
    let tag = job.status.result_tag();
    let mut lines = vec![
        format!("job_id: {}", job.id),
        format!("status: {}", job.status.as_str()),
        format!("exit_code: {}", option_i32(job.exit_code)),
        format!("cwd: {}", job.cwd),
        format!("command: {}", job.command),
        String::new(),
    ];
    if job.output_truncated {
        lines.push(format!(
            "Captured output was truncated after {} bytes.",
            job.output_limit_bytes
        ));
        lines.push(String::new());
    }
    lines.extend([
        format!("<{tag}>"),
        background_task_result_inline(job),
        format!("</{tag}>"),
    ]);
    lines.join("\n")
}

fn job_metadata(job: &BackgroundJob) -> Value {
    json!({
        "jobId": &job.id,
        "sessionId": &job.session_id,
        "description": &job.description,
        "command": &job.command,
        "cwd": &job.cwd,
        "shell": &job.shell,
        "status": job.status.as_str(),
        "startedAt": job.started_at,
        "finishedAt": job.finished_at,
        "exitCode": job.exit_code,
        "signal": job.signal,
        "pid": job.pid,
        "timeout": job.timeout_ms,
        "outputLimit": job.output_limit_bytes,
        "outputTruncated": job.output_truncated,
        "commandPatterns": &job.command_patterns,
        "alwaysPatterns": &job.always_patterns,
        "externalDirectories": &job.external_dirs,
    })
}

fn ensure_job_belongs_to_session(
    job: &BackgroundJob,
    session_id: &Id,
) -> Result<(), String> {
    if job.session_id == session_id.to_string() {
        return Ok(());
    }
    Err(format!(
        "job_id {} is not a background task for session {}",
        job.id, session_id
    ))
}

fn resolve_job_cwd(
    project_directory: &str,
    cwd_arg: Option<String>,
    permissions: &[PermissionRule],
) -> Result<PathBuf, String> {
    let project_root =
        crate::windows_process::canonicalize_path(Path::new(project_directory))
            .map_err(|error| format!("failed to resolve project directory: {error}"))?;
    let candidate = match cwd_arg {
        Some(raw) if Path::new(&raw).is_absolute() => PathBuf::from(raw),
        Some(raw) => project_root.join(raw),
        None => project_root.clone(),
    };
    let cwd = crate::windows_process::canonicalize_path(&candidate).map_err(|error| {
        format!("failed to resolve cwd {}: {error}", candidate.display())
    })?;
    if !cwd.is_dir() {
        return Err(format!("cwd {} is not a directory", cwd.display()));
    }
    if !cwd.starts_with(&project_root) {
        ensure_tool_permission(
            permissions,
            "external_directory",
            &external_directory_pattern(&cwd),
        )?;
    }
    Ok(cwd)
}

fn external_directory_pattern(path: &Path) -> String {
    format!("{}/*", path.display())
}

fn optional_cwd(input: &Value) -> Option<String> {
    string_arg(input, "cwd")
}

fn string_arg(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn u64_arg(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(Value::as_u64)
}

fn usize_arg(input: &Value, key: &str) -> Option<usize> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn option_i32(value: Option<i32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn render_job_output(stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    let mut rendered = String::new();
    if !stdout.is_empty() {
        rendered.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !rendered.is_empty() && !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str(&stderr);
    }
    if rendered.is_empty() {
        "(no output)".to_string()
    } else {
        rendered
    }
}

fn read_limited_child_output<T>(
    pipe: Option<T>,
    limit: usize,
) -> tokio::task::JoinHandle<anyhow::Result<LimitedOutput>>
where
    T: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let Some(mut pipe) = pipe else {
            return Ok(LimitedOutput { bytes, truncated });
        };
        let mut buffer = [0_u8; 8192];
        loop {
            let count = pipe.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let remaining = limit.saturating_sub(bytes.len());
            if remaining == 0 {
                truncated = true;
                continue;
            }
            let keep = remaining.min(count);
            bytes.extend_from_slice(&buffer[..keep]);
            if keep < count {
                truncated = true;
            }
        }
        Ok(LimitedOutput { bytes, truncated })
    })
}

async fn join_limited_output(
    task: tokio::task::JoinHandle<anyhow::Result<LimitedOutput>>,
) -> LimitedOutput {
    match task.await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => LimitedOutput {
            bytes: format!("failed to read process output: {error}").into_bytes(),
            truncated: false,
        },
        Err(error) => LimitedOutput {
            bytes: format!("failed to join process output reader: {error}").into_bytes(),
            truncated: false,
        },
    }
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_job(status: BackgroundJobStatus, output: &str) -> BackgroundJob {
        BackgroundJob {
            id: "job_test".to_string(),
            session_id: "ses_test".to_string(),
            description: "Build project".to_string(),
            command: "cargo build".to_string(),
            cwd: "/tmp/project".to_string(),
            shell: "/bin/sh".to_string(),
            status,
            started_at: 1,
            finished_at: Some(2),
            exit_code: Some(0),
            signal: None,
            pid: Some(123),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
            output_truncated: false,
            output: output.to_string(),
            error: None,
            command_patterns: vec!["cargo build".to_string()],
            always_patterns: vec!["cargo *".to_string()],
            external_dirs: Vec::new(),
        }
    }

    #[test]
    fn completion_request_is_runtime_system_notification() {
        let job = test_job(BackgroundJobStatus::Completed, "finished");
        let request = background_task_completion_request(&job);

        assert!(request.message_id.is_some());
        let system = request.system.as_deref().expect("system notification");
        assert!(system.contains(BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER));
        assert!(system.contains("runtime, not by the user"));
        assert!(system.contains("background_task_result"));

        let PromptPart::Text { text } = &request.parts[0] else {
            panic!("expected text notification");
        };
        assert!(text.contains("Background shell task finished."));
        assert!(text.contains("job_id: job_test"));
        assert!(text.contains("<background_task_result>"));
        assert!(text.contains("finished"));
    }

    #[test]
    fn failed_completion_uses_error_result_tag() {
        let mut job = test_job(BackgroundJobStatus::Error, "failed");
        job.exit_code = Some(2);
        let output = background_task_result_output(&job);

        assert!(output.contains("status: error"));
        assert!(output.contains("<background_task_error>"));
        assert!(output.contains("failed"));
    }
}
