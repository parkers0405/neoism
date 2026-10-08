use anyhow::Context;
use base64::Engine;
use serde_json::{json, Value};
use std::io::{Read, Seek};

#[path = "read_pagination.rs"]
mod read_pagination;

struct WriteMutation {
    display: String,
    content_len: usize,
    previous_len: usize,
    snapshot_before: crate::snapshot::FileState,
}

struct EditMutation {
    display: String,
    count: usize,
    remaining_matches: usize,
    snapshot_before: crate::snapshot::FileState,
}

use super::args::{required_string, usize_arg};
use super::paths::{
    directory_entries, display_path, existing_project_path, project_path_for_write,
};
use super::{diagnostics, edit_match, format, ToolContext, ToolExecutionResult};

const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_MEDIA_READ_BYTES: usize = 20 * 1024 * 1024;

pub(super) fn read_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let raw_path = required_string(&arguments, "filePath")?;
    let path = existing_project_path(&context, raw_path)?;
    let display = display_path(&context.cwd, &path);
    context.ensure_allowed("read", &display)?;
    let check_cancel = || -> anyhow::Result<()> {
        if context
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed))
        {
            anyhow::bail!("read cancelled");
        }
        Ok(())
    };
    check_cancel()?;
    let cursor = match arguments.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) if token.trim().is_empty() => None,
        Some(Value::String(token)) => Some(token.as_str()),
        Some(_) => anyhow::bail!("cursor must be a string"),
    };
    // Model tool calls may include every optional field with empty/default values.
    // A real cursor is authoritative; a blank cursor leaves line offsets intact.
    let offset = if cursor.is_some() {
        1
    } else {
        usize_arg(&arguments, "offset").unwrap_or(1).max(1)
    };
    let limit = usize_arg(&arguments, "limit")
        .unwrap_or(DEFAULT_READ_LIMIT)
        .max(1);

    if path.is_dir() {
        anyhow::ensure!(cursor.is_none(), "directories use offset, not cursor");
        let entries = directory_entries(&path)?;
        check_cancel()?;
        if offset > entries.len().saturating_add(1) {
            anyhow::bail!(
                "offset {offset} is out of range for {} ({} entries)",
                display,
                entries.len()
            );
        }
        let start = offset - 1;
        let output = entries
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = start.saturating_add(limit) < entries.len();
        let suffix = if truncated {
            format!(
                "\n(Showing {} of {} entries. Use offset={} to continue.)",
                output.lines().count(),
                entries.len(),
                offset + output.lines().count()
            )
        } else {
            format!("\n({} entries)", entries.len())
        };
        let output = format!(
            "<path>{}</path>\n<type>directory</type>\n<entries>\n{}{suffix}\n</entries>",
            path.display(),
            output
        );
        return Ok(ToolExecutionResult {
            title: format!("Read {display}"),
            output,
            metadata: Some(json!({
                "path": display,
                "type": "directory",
                "count": entries.len(),
                "offset": offset,
                "limit": limit,
                "truncated": truncated,
                "preview": entries.iter().skip(start).take(20).cloned().collect::<Vec<_>>().join("\n"),
            })),
        });
    }

    let metadata = path
        .metadata()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // Avoid blocking on a FIFO swapped in between inspection and open. Verify
    // the opened handle too, rather than trusting pre-open path metadata.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let version = read_pagination::version(&path, &metadata);
    let position = cursor
        .map(|token| read_pagination::decode(token, &version, metadata.len()))
        .transpose()?;
    let mut sample = vec![0_u8; 64 * 1024];
    let sample_len = file
        .read(&mut sample)
        .with_context(|| format!("failed to sample {}", path.display()))?;
    sample.truncate(sample_len);

    check_cancel()?;
    if let Some(mime) = supported_media_mime(&sample) {
        anyhow::ensure!(cursor.is_none(), "media attachments do not support cursor");
        if metadata.len() > MAX_MEDIA_READ_BYTES as u64 {
            anyhow::bail!(
                "{display} is too large to attach ({} bytes, limit {} bytes)",
                metadata.len(),
                MAX_MEDIA_READ_BYTES
            );
        }
        file.rewind()
            .with_context(|| format!("failed to rewind {}", path.display()))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let mut buffer = [0u8; 8192];
        loop {
            check_cancel()?;
            let remaining = (MAX_MEDIA_READ_BYTES + 1).saturating_sub(bytes.len());
            let count = file
                .read(&mut buffer[..remaining.min(8192)])
                .with_context(|| format!("failed to read {}", path.display()))?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            anyhow::ensure!(
                bytes.len() <= MAX_MEDIA_READ_BYTES,
                "{display} grew beyond the media attachment byte limit"
            );
        }
        anyhow::ensure!(
            read_pagination::version(&path, &file.metadata()?) == version
                && read_pagination::version(&path, &path.metadata()?) == version,
            "{display} changed while being read; retry"
        );
        let loaded = crate::instruction::nearby(&context.cwd, &path);
        let loaded_paths = loaded
            .iter()
            .map(|item| item.filepath.clone())
            .collect::<Vec<_>>();
        let url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );
        let media_kind = if mime == "application/pdf" {
            "PDF"
        } else {
            "Image"
        };
        return Ok(ToolExecutionResult {
            title: format!("Read {display}"),
            output: format!("{media_kind} read successfully"),
            metadata: Some(json!({
                "path": display,
                "type": "file",
                "mime": mime,
                "bytes": bytes.len(),
                "preview": format!("{media_kind} read successfully"),
                "truncated": false,
                "loaded": loaded_paths,
                "attachments": [{
                    "type": "file",
                    "mime": mime,
                    "url": url,
                    "filename": path.file_name().and_then(|name| name.to_str()).unwrap_or("file"),
                }],
            })),
        });
    }
    if appears_binary(&sample) {
        anyhow::bail!("{display} appears to be binary");
    }
    file.rewind()
        .with_context(|| format!("failed to rewind {}", path.display()))?;
    // Instructions are part of the output budget, not an afterthought: the
    // central limiter must not discard a long fragment from an ordinary page.
    let loaded = crate::instruction::nearby(&context.cwd, &path);
    check_cancel()?;
    let reminder = if loaded.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n<system-reminder>\n{}\n</system-reminder>",
            loaded
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    };
    let prefix = format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n",
        path.display()
    );
    let (content_cap, row_cap, instruction_overflow) =
        read_output_budget(&prefix, raw_path, limit, &reminder)?;
    // Keep a handle for post-read version validation even though pagination
    // owns its buffered reader. A changed file must never yield a usable token.
    let verification = file.try_clone()?;
    let page = read_pagination::page(
        file,
        offset,
        limit.min(row_cap),
        position,
        content_cap,
        check_cancel,
    )?;
    anyhow::ensure!(
        read_pagination::version(&path, &verification.metadata()?) == version
            && read_pagination::version(&path, &path.metadata()?) == version,
        "{display} changed while being read; retry without cursor"
    );
    check_cancel()?;
    let next_cursor = page
        .more
        .then(|| read_pagination::encode(&version, &page.next));
    let continuation = next_cursor.as_ref().map(|token| {
        json!({
            "filePath": raw_path,
            "limit": limit,
            "cursor": token,
        })
    });
    let mut output = prefix;
    output.push_str(&page.rendered.join("\n"));
    if let Some(args) = &continuation {
        output.push_str(&read_continuation_footer(
            args,
            page.next.line,
            page.byte_capped,
        ));
    } else {
        output.push_str(&format!(
            "\n\n(End of file - total {} lines)",
            page.total_lines
        ));
    }
    output.push_str("\n</content>");
    // Exceptionally large instruction bundles still go through the central
    // artifact limiter intact; never omit instructions or bypass that limiter.
    output.push_str(&reminder);
    debug_assert!(
        instruction_overflow
            || (output.len() <= super::truncate::MAX_BYTES
                && output.lines().count() <= super::truncate::MAX_LINES)
    );
    let loaded_paths = loaded
        .iter()
        .map(|item| item.filepath.clone())
        .collect::<Vec<_>>();

    Ok(ToolExecutionResult {
        title: format!("Read {display}"),
        output,
        metadata: Some(json!({
            "path": display,
            "type": "file",
            "lines": (!page.more).then_some(page.total_lines),
            "offset": page.start.line,
            "limit": limit,
            "truncated": page.more,
            "byteCapped": page.byte_capped,
            "rowCapped": page.more && !page.byte_capped && page.rendered.len() == row_cap && row_cap < limit,
            "instructionOverflow": instruction_overflow,
            "contentByteBudget": content_cap,
            "contentRowBudget": row_cap,
            "byteStart": page.start.byte,
            "byteEnd": page.next.byte,
            "startLine": page.start.line,
            "startColumnBytes": page.start.column,
            "nextLine": page.next.line,
            "nextColumnBytes": page.next.column,
            "partialLine": page.more && page.next.column > 0,
            "nextCursor": next_cursor,
            "continuation": continuation,
            "preview": page.preview.join("\n"),
            "loaded": loaded_paths,
        })),
    })
}

fn read_continuation_footer(args: &Value, line: usize, byte_capped: bool) -> String {
    format!(
        "\n\n({} Continue with read arguments: {}. Continuation may resume within line {}; fragments of the same numbered line concatenate without a newline.)",
        if byte_capped { "Page byte budget reached." } else { "More content remains." },
        args, line,
    )
}

/// Reserve the actual escaped path, limit, instruction bundle and framing,
/// plus an upper bound on the cursor/footer (only numeric widths vary). This
/// costs hundreds of bytes, not half the content budget, for ordinary paths.
fn read_output_budget(
    prefix: &str,
    raw_path: &str,
    limit: usize,
    reminder: &str,
) -> anyhow::Result<(usize, usize, bool)> {
    // Base64url cursor characters never need JSON escaping. Reserve its exact
    // fixed length without issuing a dummy capability into the cursor cache.
    let cursor = "x".repeat(read_pagination::CURSOR_LENGTH);
    let args = json!({"filePath": raw_path, "limit": limit, "cursor": cursor});
    let continuation = read_continuation_footer(&args, usize::MAX, true);
    let eof = format!("\n\n(End of file - total {} lines)", usize::MAX);
    let footer = if continuation.len() >= eof.len() {
        continuation
    } else {
        eof
    };
    let framing = format!("{prefix}{footer}\n</content>");
    let minimum_content = usize::MAX.to_string().len() + 2 + 4; // label + one UTF-8 scalar
    let remaining = |suffix: &str| {
        let envelope = format!("{framing}{suffix}");
        // Adding N rendered rows adds at most N lines to the empty envelope.
        (
            super::truncate::MAX_BYTES.saturating_sub(envelope.len()),
            super::truncate::MAX_LINES.saturating_sub(envelope.lines().count()),
        )
    };
    let (bytes, rows) = remaining(reminder);
    if bytes >= minimum_content && rows > 0 {
        return Ok((bytes, rows, false));
    }
    // Nearby instructions can themselves exceed a global budget. Keep them
    // verbatim and let the central limiter create its recoverable artifact.
    let (bytes, rows) = remaining("");
    anyhow::ensure!(
        bytes >= minimum_content && rows > 0,
        "read path/framing exceeds output budget; use a shorter filePath"
    );
    Ok((bytes, rows, !reminder.is_empty()))
}

fn supported_media_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else {
        None
    }
}

fn appears_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    let controls = bytes
        .iter()
        .filter(|byte| matches!(byte, 0x01..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f))
        .count();
    !bytes.is_empty() && controls.saturating_mul(100) > bytes.len().saturating_mul(10)
}

pub(super) async fn write_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let path =
        project_path_for_write(&context, required_string(&arguments, "filePath")?)?;
    let display = display_path(&context.cwd, &path);
    context.ensure_allowed("edit", &display)?;
    let _lock = context.utilities().file_locks.lock_file(&path).await;

    let locked_path = path.clone();
    let mutation = tokio::task::spawn_blocking(move || {
        let result = write_tool_locked(arguments, locked_path, display.clone());
        result
    })
    .await
    .with_context(|| "write tool task panicked")??;
    drop(_lock);

    // LSP diagnostics + formatting do blocking I/O (each diagnostic query can
    // wait for the server). Run them off the async executor so the agent
    // response never freezes while waiting on a language server.
    tokio::task::spawn_blocking(move || write_tool_metadata(context, path, mutation))
        .await
        .with_context(|| "write tool metadata task panicked")?
}

fn write_tool_locked(
    arguments: Value,
    path: std::path::PathBuf,
    display: String,
) -> anyhow::Result<WriteMutation> {
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tool argument content is required"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create directory {}", parent.display())
        })?;
    }
    let snapshot_before = crate::snapshot::FileState::from_path(&path)?;
    let previous_bytes = std::fs::read(&path).ok();
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    let previous_len = previous_bytes
        .as_ref()
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    Ok(WriteMutation {
        display,
        content_len: content.len(),
        previous_len,
        snapshot_before,
    })
}

fn write_tool_metadata(
    context: ToolContext,
    path: std::path::PathBuf,
    mutation: WriteMutation,
) -> anyhow::Result<ToolExecutionResult> {
    let formatted = format::format_paths(
        &context.services(),
        &context.cwd,
        context.formatter(),
        [path.clone()],
    );
    let lsp_runtime = context.lsp_runtime();
    let lsp_touch = lsp_runtime
        .as_ref()
        .ok()
        .map(|runtime| diagnostics::touch_paths(runtime, &context.cwd, [path.clone()]));

    let mut metadata = json!({
        "path": mutation.display,
        "bytes": mutation.content_len,
        "previousBytes": mutation.previous_len,
        "lspTouch": lsp_touch,
    });
    if let Err(error) = &lsp_runtime {
        metadata["lspUnavailable"] = json!(error.to_string());
    }
    match crate::snapshot::file_change(&context.cwd, &path, mutation.snapshot_before) {
        Ok(Some(snapshot)) => {
            crate::snapshot::add_metadata_snapshots(&mut metadata, vec![snapshot])
        }
        Ok(None) => {}
        Err(error) => metadata["snapshotError"] = json!(error.to_string()),
    }
    format::attach_formatted(&mut metadata, &formatted);
    let report = lsp_runtime.as_ref().ok().and_then(|runtime| {
        diagnostics::attach_lsp_diagnostics(
            runtime,
            &context.cwd,
            [path.clone()],
            &mut metadata,
        )
    });

    let mut output = format!(
        "Wrote {} bytes to {} (previously {} bytes)",
        mutation.content_len, mutation.display, mutation.previous_len
    );
    if let Some(report) = report {
        output.push_str("\n\n");
        output.push_str(&report);
    }

    Ok(ToolExecutionResult {
        title: format!("Write {}", mutation.display),
        output,
        metadata: Some(metadata),
    })
}

pub(super) async fn edit_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let path_arg = required_string(&arguments, "filePath")?;
    let path = existing_project_path(&context, path_arg)?;
    let display = display_path(&context.cwd, &path);
    context.ensure_allowed("edit", &display)?;
    let _lock = context.utilities().file_locks.lock_file(&path).await;

    let locked_path = path.clone();
    let mutation = tokio::task::spawn_blocking(move || {
        let result = edit_tool_locked(arguments, locked_path, display.clone());
        result
    })
    .await
    .with_context(|| "edit tool task panicked")??;
    drop(_lock);

    // LSP diagnostics + formatting do blocking I/O (each diagnostic query can
    // wait for the server). Run them off the async executor so the agent
    // response never freezes while waiting on a language server.
    tokio::task::spawn_blocking(move || edit_tool_metadata(context, path, mutation))
        .await
        .with_context(|| "edit tool metadata task panicked")?
}

fn edit_tool_locked(
    arguments: Value,
    path: std::path::PathBuf,
    display: String,
) -> anyhow::Result<EditMutation> {
    let old = required_string(&arguments, "oldString")?;
    let new = arguments
        .get("newString")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tool argument newString is required"))?;
    let replace_all = arguments
        .get("replaceAll")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let raw_content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let (had_bom, content) = super::patch::split_bom(&raw_content);

    let normalized_content = content.replace("\r\n", "\n");
    let normalized_old = old.replace("\r\n", "\n");
    let normalized_new = new.replace("\r\n", "\n");

    let snapshot_before = crate::snapshot::FileState::from_path(&path)?;
    let (updated, count, remaining_matches) = edit_match::replace(
        &normalized_content,
        &normalized_old,
        &normalized_new,
        replace_all,
    )
    .with_context(|| format!("failed to edit {display}"))?;

    let final_content = if content.contains("\r\n") {
        updated.replace('\n', "\r\n")
    } else {
        updated
    };
    let current_content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to verify {} before writing", path.display()))?;
    if current_content != raw_content {
        anyhow::bail!(
            "refusing to edit {display}: the file changed after it was read; read it again and retry"
        );
    }
    std::fs::write(&path, super::patch::join_bom(&final_content, had_bom))
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(EditMutation {
        display,
        count,
        remaining_matches,
        snapshot_before,
    })
}

fn edit_tool_metadata(
    context: ToolContext,
    path: std::path::PathBuf,
    mutation: EditMutation,
) -> anyhow::Result<ToolExecutionResult> {
    let formatted = format::format_paths(
        &context.services(),
        &context.cwd,
        context.formatter(),
        [path.clone()],
    );
    let lsp_runtime = context.lsp_runtime();
    let lsp_touch = lsp_runtime
        .as_ref()
        .ok()
        .map(|runtime| diagnostics::touch_paths(runtime, &context.cwd, [path.clone()]));

    let mut metadata = json!({
        "path": mutation.display,
        "replaced": mutation.count,
        "remainingMatches": mutation.remaining_matches,
        "lspTouch": lsp_touch,
    });
    if let Err(error) = &lsp_runtime {
        metadata["lspUnavailable"] = json!(error.to_string());
    }
    match crate::snapshot::file_change(&context.cwd, &path, mutation.snapshot_before) {
        Ok(Some(snapshot)) => {
            crate::snapshot::add_metadata_snapshots(&mut metadata, vec![snapshot])
        }
        Ok(None) => {}
        Err(error) => metadata["snapshotError"] = json!(error.to_string()),
    }
    format::attach_formatted(&mut metadata, &formatted);
    let report = lsp_runtime.as_ref().ok().and_then(|runtime| {
        diagnostics::attach_lsp_diagnostics(
            runtime,
            &context.cwd,
            [path.clone()],
            &mut metadata,
        )
    });

    let mut output = format!(
        "Replaced {} occurrence(s) in {}",
        mutation.count, mutation.display
    );
    if let Some(report) = report {
        output.push_str("\n\n");
        output.push_str(&report);
    }

    Ok(ToolExecutionResult {
        title: format!("Edit {}", mutation.display),
        output,
        metadata: Some(metadata),
    })
}
