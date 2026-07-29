use super::*;

// ---------------------------------------------------------------------------
// shared subprocess progress driver
// ---------------------------------------------------------------------------

pub(super) async fn wait_for_command(
    child: &mut tokio::process::Child,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    progress: &UnboundedSender<ProgressEvent>,
    tool: &str,
) -> Result<(std::process::ExitStatus, Vec<String>), InstallError> {
    use std::sync::{Arc, Mutex};

    let tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout_task = spawn_line_reader(stdout, progress.clone(), None);
    let stderr_task = spawn_line_reader(stderr, progress.clone(), Some(tail.clone()));

    // Completion is driven by the CHILD PROCESS EXITING, not by the output
    // pipes reaching EOF. A grandchild — npm's detached update-notifier, a
    // spawned node-gyp, etc. — can inherit and hold stdout/stderr open long
    // after the tool itself exits; waiting on pipe-EOF then hung us until the
    // timeout (observed: npm reported "still running" for 300s though the
    // install actually finished in ~2s). So we wait on `child.wait()`, emit a
    // liveness heartbeat every 5s, then drain + abort the readers below.
    let started = Instant::now();
    let wait_child = async {
        loop {
            tokio::select! {
                status = child.wait() => return status,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    let _ = progress.send(ProgressEvent::Waiting {
                        status: format!(
                            "{tool} is still running ({}s)",
                            started.elapsed().as_secs()
                        ),
                    });
                }
            }
        }
    };

    let status = match tokio::time::timeout(INSTALL_PROCESS_TIMEOUT, wait_child).await {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => {
            abort_reader(stdout_task);
            abort_reader(stderr_task);
            return Err(InstallError::Io(err));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            abort_reader(stdout_task);
            abort_reader(stderr_task);
            return Err(InstallError::TimedOut {
                tool: tool.to_string(),
                seconds: INSTALL_PROCESS_TIMEOUT.as_secs(),
            });
        }
    };

    // The tool exited: give each reader a brief grace to drain the last
    // buffered lines (so the error tail is complete on a normal failure),
    // then abort so a pipe-holding grandchild can't keep us waiting.
    settle_reader(stdout_task, Duration::from_millis(400)).await;
    settle_reader(stderr_task, Duration::from_millis(400)).await;

    let out = tail.lock().unwrap().clone();
    Ok((status, out))
}

/// Spawn a task that reads a child pipe line-by-line, forwarding each line as
/// a progress event and (for stderr) keeping the last ~20 lines for error
/// reporting. `None` when the pipe handle is absent.
fn spawn_line_reader<R>(
    reader: Option<R>,
    progress: UnboundedSender<ProgressEvent>,
    tail: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
) -> Option<tokio::task::JoinHandle<()>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    reader.map(|r| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(tail) = &tail {
                    let mut t = tail.lock().unwrap();
                    t.push(line.clone());
                    if t.len() > 20 {
                        let drop_n = t.len() - 20;
                        t.drain(0..drop_n);
                    }
                }
                emit_command_line(&progress, line);
            }
        })
    })
}

/// Wait up to `grace` for a reader task to finish draining, then abort it so a
/// grandchild holding the pipe open can't keep us waiting indefinitely.
async fn settle_reader(handle: Option<tokio::task::JoinHandle<()>>, grace: Duration) {
    let Some(mut handle) = handle else {
        return;
    };
    if tokio::time::timeout(grace, &mut handle).await.is_err() {
        handle.abort();
    }
}

fn abort_reader(handle: Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = handle {
        handle.abort();
    }
}

/// Preserve real package-manager percentages, but never invent one from log
/// line counts. A noisy install is not necessarily further along than a quiet
/// one; lines without a percentage are explicitly indeterminate activity.
pub(super) fn emit_command_line(progress: &UnboundedSender<ProgressEvent>, line: String) {
    if let Some(pct) = parse_percent(&line) {
        emit(
            progress,
            ProgressEvent::Progress {
                percent: pct,
                status: line,
            },
        );
    } else {
        emit(progress, ProgressEvent::Waiting { status: line });
    }
}

pub(super) fn parse_percent(line: &str) -> Option<u8> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'%' {
                let n: u32 = line[start..i].parse().ok()?;
                return Some(n.min(100) as u8);
            }
        } else {
            i += 1;
        }
    }
    None
}
