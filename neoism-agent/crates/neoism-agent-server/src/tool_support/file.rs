use anyhow::Context;
use base64::Engine;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Seek};

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
    truncate_line,
};
use super::{diagnostics, edit_match, format, ToolContext, ToolExecutionResult};

const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_READ_BYTES: usize = 50 * 1024;
const MAX_MEDIA_READ_BYTES: usize = 20 * 1024 * 1024;

pub(super) fn read_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let raw_path = required_string(&arguments, "filePath")?;
    let path = existing_project_path(&context, raw_path)?;
    let display = display_path(&context.cwd, &path);
    context.ensure_allowed("read", &display)?;
    let offset = usize_arg(&arguments, "offset").unwrap_or(1).max(1);
    let limit = usize_arg(&arguments, "limit")
        .unwrap_or(DEFAULT_READ_LIMIT)
        .max(1);

    if path.is_dir() {
        let entries = directory_entries(&path)?;
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
    let mut file = std::fs::File::open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut sample = vec![0_u8; 64 * 1024];
    let sample_len = file
        .read(&mut sample)
        .with_context(|| format!("failed to sample {}", path.display()))?;
    sample.truncate(sample_len);

    if let Some(mime) = supported_media_mime(&sample) {
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
        file.read_to_end(&mut bytes)
            .with_context(|| format!("failed to read {}", path.display()))?;
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
    let mut reader = BufReader::new(file);
    let mut rendered = Vec::new();
    let mut preview = Vec::new();
    let mut rendered_bytes = 0usize;
    let mut byte_capped = false;
    let mut line_number = 0usize;
    let mut has_more = false;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        let read = reader
            .read_until(b'\n', &mut raw)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        line_number += 1;
        if line_number < offset {
            continue;
        }
        if rendered.len() >= limit {
            has_more = true;
            break;
        }
        while raw.last().is_some_and(|byte| matches!(byte, b'\n' | b'\r')) {
            raw.pop();
        }
        let text = std::str::from_utf8(&raw).with_context(|| {
            format!("{display} is not valid UTF-8 near line {line_number}")
        })?;
        let text = truncate_line(&text);
        let line = format!("{line_number}: {text}");
        let size = line.len() + usize::from(!rendered.is_empty());
        if rendered_bytes.saturating_add(size) > MAX_READ_BYTES {
            byte_capped = true;
            has_more = true;
            break;
        }
        rendered_bytes += size;
        if preview.len() < 20 {
            preview.push(text);
        }
        rendered.push(line);
    }
    if rendered.is_empty() && offset > line_number.saturating_add(1) {
        anyhow::bail!(
            "offset {offset} is out of range for {display} ({line_number} lines)"
        );
    }
    let truncated = has_more;
    let last = if rendered.is_empty() {
        offset.saturating_sub(1)
    } else {
        offset + rendered.len() - 1
    };
    let next = last + 1;
    let mut output = format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n{}",
        path.display(),
        rendered.join("\n")
    );
    if byte_capped {
        output.push_str(&format!(
            "\n\n(Output capped at 50 KB. Showing lines {offset}-{last}. Use offset={next} to continue.)"
        ));
    } else if has_more {
        output.push_str(&format!(
            "\n\n(Showing lines {offset}-{last}. Use offset={next} to continue.)"
        ));
    } else {
        output.push_str(&format!("\n\n(End of file - total {line_number} lines)"));
    }
    output.push_str("\n</content>");
    let loaded = crate::instruction::nearby(&context.cwd, &path);
    if !loaded.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("<system-reminder>\n");
        output.push_str(
            &loaded
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        output.push_str("\n</system-reminder>");
    }
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
            "lines": (!has_more).then_some(line_number),
            "offset": offset,
            "limit": limit,
            "truncated": truncated,
            "preview": preview.join("\n"),
            "loaded": loaded_paths,
        })),
    })
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
    let formatted =
        format::format_paths(&context.services(), &context.cwd, context.formatter(), [path.clone()]);
    let lsp_touch = diagnostics::touch_paths(&context.lsp_runtime(), &context.cwd, [path.clone()]);

    let mut metadata = json!({
        "path": mutation.display,
        "bytes": mutation.content_len,
        "previousBytes": mutation.previous_len,
        "lspTouch": lsp_touch,
    });
    if let Some(snapshot) =
        crate::snapshot::file_change(&context.cwd, &path, mutation.snapshot_before)?
    {
        crate::snapshot::add_metadata_snapshots(&mut metadata, vec![snapshot]);
    }
    format::attach_formatted(&mut metadata, &formatted);
    let report =
        diagnostics::attach_lsp_diagnostics(&context.lsp_runtime(), &context.cwd, [path.clone()], &mut metadata);

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
    let formatted =
        format::format_paths(&context.services(), &context.cwd, context.formatter(), [path.clone()]);
    let lsp_touch = diagnostics::touch_paths(&context.lsp_runtime(), &context.cwd, [path.clone()]);

    let mut metadata = json!({
        "path": mutation.display,
        "replaced": mutation.count,
        "remainingMatches": mutation.remaining_matches,
        "lspTouch": lsp_touch,
    });
    if let Some(snapshot) =
        crate::snapshot::file_change(&context.cwd, &path, mutation.snapshot_before)?
    {
        crate::snapshot::add_metadata_snapshots(&mut metadata, vec![snapshot]);
    }
    format::attach_formatted(&mut metadata, &formatted);
    let report =
        diagnostics::attach_lsp_diagnostics(&context.lsp_runtime(), &context.cwd, [path.clone()], &mut metadata);

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
