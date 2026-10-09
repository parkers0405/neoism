//! Internal native process runner. Session admission belongs to the caller, not a provider.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::tool::process;

const MAX_CAPTURE_BYTES_PER_STREAM: usize = 1024 * 1024;

pub(crate) struct ProcessSpec {
    pub(crate) executable: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct ExecResult {
    pub(crate) status: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) truncated: bool,
}

// Dropping a future must not detach pipe readers or leave its process group alive.
struct Cleanup {
    child_id: Option<u32>,
    readers: Vec<tokio::task::AbortHandle>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        process::kill_process_group(self.child_id);
        for reader in &self.readers {
            reader.abort();
        }
    }
}

pub(crate) async fn run(
    spec: ProcessSpec,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<ExecResult> {
    if cancel
        .as_ref()
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
    {
        anyhow::bail!("command aborted");
    }
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .envs(&spec.env)
        .kill_on_drop(true)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    process::set_new_process_group(&mut command);
    let mut child = command.spawn().context("failed to spawn process")?;
    let child_id = child.id();
    let mut stdout =
        process::read_child_output(child.stdout.take(), MAX_CAPTURE_BYTES_PER_STREAM);
    let mut stderr =
        process::read_child_output(child.stderr.take(), MAX_CAPTURE_BYTES_PER_STREAM);
    let stdin = child.stdin.take();
    let mut input = tokio::spawn(async move {
        if let (Some(mut pipe), Some(bytes)) = (stdin, spec.stdin) {
            pipe.write_all(&bytes).await?;
        }
        Ok::<(), std::io::Error>(())
    });
    let cleanup = Cleanup {
        child_id,
        readers: vec![
            stdout.abort_handle(),
            stderr.abort_handle(),
            input.abort_handle(),
        ],
    };
    let timeout_ms = spec.timeout_ms;
    let timeout = async {
        match timeout_ms {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms.max(1))).await,
            None => std::future::pending::<()>().await,
        }
    };
    let result = tokio::select! {
        result = async {
            let status = child.wait().await?;
            (&mut input).await??;
            let stdout = (&mut stdout).await??;
            let stderr = (&mut stderr).await??;
            Ok::<_, anyhow::Error>(ExecResult {
                status: status.code().unwrap_or(-1),
                stdout: stdout.bytes,
                stderr: stderr.bytes,
                truncated: stdout.truncated || stderr.truncated,
            })
        } => result,
        _ = timeout => Err(anyhow::anyhow!("process timed out after {}ms", timeout_ms.unwrap_or(0))),
        _ = process::wait_for_cancel(cancel) => Err(anyhow::anyhow!("command aborted")),
    };
    if result.is_err() {
        process::terminate_child(&mut child, child_id).await;
        // terminate_child may reap an already-exited parent before escalating.
        // Its descendants can still ignore SIGTERM and keep the pipes open.
        process::kill_process_group(child_id);
    }
    drop(cleanup);
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn shell(command: &str) -> ProcessSpec {
        ProcessSpec {
            executable: "sh".into(),
            args: vec!["-c".into(), command.into()],
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout_ms: Some(5_000),
        }
    }

    #[tokio::test]
    async fn captures_bounded_output_and_drains_both_streams() {
        let result = run(
            shell("head -c 1100000 /dev/zero; head -c 1100000 /dev/zero >&2"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.status, 0);
        assert_eq!(result.stdout.len(), MAX_CAPTURE_BYTES_PER_STREAM);
        assert_eq!(result.stderr.len(), MAX_CAPTURE_BYTES_PER_STREAM);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn forwards_stdin_environment_and_exit_status() {
        let mut spec = shell("cat; printf '%s' \"$NEOISM_PROCESS_TEST\" >&2; exit 7");
        spec.stdin = Some(b"input".to_vec());
        spec.env
            .insert("NEOISM_PROCESS_TEST".into(), "environment".into());
        let result = run(spec, None).await.unwrap();
        assert_eq!(result.stdout, b"input");
        assert_eq!(result.stderr, b"environment");
        assert_eq!(result.status, 7);
        assert!(!result.truncated);
    }

    fn pid_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "neoism-process-tree-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ))
    }

    fn descendant(path: &std::path::Path) -> String {
        // The leader exits immediately; a TERM-ignoring descendant retains both pipes.
        format!("sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do sleep 1; done' & exit 0", path.display())
    }

    async fn read_pid(path: &std::path::Path) -> i32 {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(value) = std::fs::read_to_string(path) {
                    if let Ok(pid) = value.trim().parse() {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("descendant did not start")
    }

    async fn assert_terminated(pid: i32) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    break;
                }
                #[cfg(target_os = "linux")]
                if std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .is_some_and(|stat| {
                        stat.split_once(") ")
                            .is_some_and(|(_, rest)| rest.starts_with('Z'))
                    })
                {
                    break; // Reaped by init, not by this process (the leader has exited).
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("descendant survived process cleanup");
    }

    #[tokio::test]
    async fn timeout_kills_descendants_even_after_leader_exits() {
        let path = pid_path();
        let mut spec = shell(&descendant(&path));
        spec.timeout_ms = Some(300);
        let task = tokio::spawn(run(spec, None));
        let pid = read_pid(&path).await;
        let error = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert_terminated(pid).await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn cancellation_kills_tree_and_does_not_wait_for_open_pipes() {
        let path = pid_path();
        let cancel = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(run(shell(&descendant(&path)), Some(cancel.clone())));
        let pid = read_pid(&path).await;
        cancel.store(true, Ordering::SeqCst);
        let error = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("aborted"));
        assert_terminated(pid).await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn dropping_runner_future_terminates_descendants() {
        let path = pid_path();
        let task = tokio::spawn(run(shell(&descendant(&path)), None));
        let pid = read_pid(&path).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_terminated(pid).await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn timeout_also_covers_blocked_stdin_writes() {
        let mut spec = shell("sleep 30");
        spec.stdin = Some(vec![0; 2 * 1024 * 1024]);
        spec.timeout_ms = Some(50);
        let result = tokio::time::timeout(Duration::from_secs(3), run(spec, None))
            .await
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}
