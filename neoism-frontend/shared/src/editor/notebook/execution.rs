use super::*;

pub(crate) fn next_execution_count(cells: &[NotebookCell]) -> u32 {
    cells
        .iter()
        .filter_map(|cell| cell.execution_count)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(crate) fn command_for_language(
    language: &str,
) -> Result<std::process::Command, String> {
    let normalized = language.trim().to_ascii_lowercase();
    let command = if is_shell_language(&normalized) {
        let shell = match normalized.as_str() {
            "bash" => "bash",
            "zsh" => "zsh",
            "fish" => "fish",
            _ => "sh",
        };
        let mut command = std::process::Command::new(shell);
        command.arg("-");
        command
    } else if normalized == "python"
        || normalized == "python3"
        || normalized == "ipython"
        || normalized == "ipython3"
    {
        let mut command = std::process::Command::new("python3");
        command.arg("-");
        command
    } else {
        return Err(format!(
            "No notebook executor for `{language}` yet (supported: python, sh, bash, zsh, fish)"
        ));
    };
    Ok(command)
}

pub(crate) fn run_execution_job(
    job: &NotebookExecutionJob,
) -> Result<(String, String, bool), String> {
    let mut command = command_for_language(&job.language)?;
    if let Some(parent) = job
        .path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        command.current_dir(parent);
    }
    let output = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(job.fallback_script.as_bytes())?;
            }
            child.wait_with_output()
        })
        .map_err(|err| err.to_string())?;
    Ok((
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    ))
}

pub(crate) fn run_execution_job_streaming(
    job: &NotebookExecutionJob,
    send: &impl Fn(NotebookExecutionEvent),
) -> Result<(Vec<Value>, bool), String> {
    let mut command = command_for_language(&job.language)?;
    if let Some(parent) = job
        .path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        command.current_dir(parent);
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(job.fallback_script.as_bytes())
            .map_err(|err| err.to_string())?;
    }

    let (tx, rx) = std::sync::mpsc::channel::<NotebookExecutionChunk>();
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        let cell_index = job.cell_index;
        let cell_id = job.cell_id.clone();
        let run_id = job.run_id;
        readers.push(std::thread::spawn(move || {
            stream_reader(
                stdout,
                cell_index,
                cell_id,
                run_id,
                NotebookOutputStream::Stdout,
                tx,
            );
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let cell_index = job.cell_index;
        let cell_id = job.cell_id.clone();
        let run_id = job.run_id;
        readers.push(std::thread::spawn(move || {
            stream_reader(
                stderr,
                cell_index,
                cell_id,
                run_id,
                NotebookOutputStream::Stderr,
                tx,
            );
        }));
    }
    drop(tx);

    let mut outputs = Vec::new();
    for chunk in rx {
        append_stream_output(&mut outputs, chunk.stream, &chunk.text);
        send(NotebookExecutionEvent::Output(chunk));
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    if outputs.is_empty() {
        append_stream_output(&mut outputs, NotebookOutputStream::Stdout, "");
    }
    Ok((outputs, status.success()))
}

pub(crate) fn stream_reader<R: std::io::Read + Send + 'static>(
    mut reader: R,
    cell_index: usize,
    cell_id: String,
    run_id: u64,
    stream: NotebookOutputStream,
    tx: std::sync::mpsc::Sender<NotebookExecutionChunk>,
) {
    let mut buf = [0_u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = tx.send(NotebookExecutionChunk {
                    cell_index,
                    cell_id: cell_id.clone(),
                    run_id,
                    stream,
                    text,
                });
            }
            Err(_) => break,
        }
    }
}

pub(crate) fn outputs_from_process(stdout: String, stderr: String) -> Vec<Value> {
    let mut outputs = Vec::new();
    if !stdout.is_empty() {
        outputs.push(serde_json::json!({
            "output_type": "stream",
            "name": "stdout",
            "text": stdout,
        }));
    }
    if !stderr.is_empty() {
        outputs.push(serde_json::json!({
            "output_type": "stream",
            "name": "stderr",
            "text": stderr,
        }));
    }
    if outputs.is_empty() {
        outputs.push(serde_json::json!({
            "output_type": "stream",
            "name": "stdout",
            "text": "",
        }));
    }
    outputs
}

pub(crate) fn append_stream_output(
    outputs: &mut Vec<Value>,
    stream: NotebookOutputStream,
    text: &str,
) {
    let name = match stream {
        NotebookOutputStream::Stdout => "stdout",
        NotebookOutputStream::Stderr => "stderr",
    };
    if let Some(existing) = outputs.iter_mut().rev().find(|output| {
        output.get("output_type").and_then(Value::as_str) == Some("stream")
            && output.get("name").and_then(Value::as_str) == Some(name)
    }) {
        if let Some(Value::String(current)) = existing.get_mut("text") {
            current.push_str(text);
            return;
        }
        let current = value_text(existing.get("text")).unwrap_or_default();
        existing["text"] = Value::String(format!("{current}{text}"));
        return;
    }
    outputs.push(serde_json::json!({
        "output_type": "stream",
        "name": name,
        "text": text,
    }));
}

pub(crate) fn is_shell_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "sh" | "shell" | "bash" | "zsh" | "fish"
    )
}
