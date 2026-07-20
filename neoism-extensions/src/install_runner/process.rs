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
    let run = async {
        let tail = drive_command_progress(stdout, stderr, progress, tool).await;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, tail))
    };
    match tokio::time::timeout(INSTALL_PROCESS_TIMEOUT, run).await {
        Ok(result) => result.map_err(InstallError::Io),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(InstallError::TimedOut {
                tool: tool.to_string(),
                seconds: INSTALL_PROCESS_TIMEOUT.as_secs(),
            })
        }
    }
}

/// Read stdout + stderr line-by-line, emit progress events, return the last
/// ~20 stderr lines for error reporting.
pub(super) async fn drive_command_progress(
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    progress: &UnboundedSender<ProgressEvent>,
    tool: &str,
) -> Vec<String> {
    use std::sync::{Arc, Mutex};

    let tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout_task = stdout.map(|s| {
        let progress = progress.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                emit_command_line(&progress, line);
            }
        })
    });

    let stderr_task = stderr.map(|s| {
        let progress = progress.clone();
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                {
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
    });

    let readers = async {
        if let Some(t) = stdout_task {
            let _ = t.await;
        }
        if let Some(t) = stderr_task {
            let _ = t.await;
        }
        let lock = tail.lock().unwrap();
        lock.clone()
    };
    tokio::pin!(readers);
    let started = Instant::now();
    loop {
        tokio::select! {
            tail = &mut readers => return tail,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                let _ = progress.send(ProgressEvent::Waiting {
                    status: format!(
                        "{tool} is still running ({}s; waiting for output)",
                        started.elapsed().as_secs()
                    ),
                });
            }
        }
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
