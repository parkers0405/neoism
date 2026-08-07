use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::Context;
use serde_json::{json, Value};

use super::args::patch_text_arg;
use super::paths::{display_path, project_path_for_write};
use super::{diagnostics, format, locks, patch, ToolContext, ToolExecutionResult};

struct PatchMutation {
    touched: Vec<String>,
    diagnostic_paths: Vec<std::path::PathBuf>,
    before_states: BTreeMap<std::path::PathBuf, crate::snapshot::FileState>,
}

enum PlannedFile {
    Write(String),
    Delete,
}

pub(super) async fn apply_patch_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let patch = patch_text_arg(&arguments)
        .ok_or_else(|| anyhow::anyhow!("tool argument patch is required"))?;
    if patch.trim().is_empty() {
        anyhow::bail!("tool argument patch must not be empty");
    }

    if patch.contains("*** Begin Patch") {
        return apply_v4a_patch(context, patch).await;
    }

    let paths = patch::paths(patch);
    if paths.is_empty() {
        anyhow::bail!(
            "patch had no parseable file headers — expected unified diff (--- a/path / +++ b/path) or the V4A envelope (*** Begin Patch ... *** End Patch)"
        );
    }
    let mut targets = Vec::new();
    for path in &paths {
        let target = project_path_for_write(&context, path)?;
        context.ensure_allowed("edit", &display_path(&context.cwd, &target))?;
        targets.push(target);
    }
    tracing::info!(paths = ?paths, "apply_patch waiting for file locks");
    let _locks = locks::lock_files(targets.clone()).await;
    tracing::info!(paths = ?paths, "apply_patch acquired file locks");
    let mut before_states = BTreeMap::new();
    for target in &targets {
        before_states.insert(
            target.clone(),
            crate::snapshot::FileState::from_path(&target)?,
        );
    }

    let mut child = Command::new("git")
        .args(["apply", "--whitespace=nowarn", "--"])
        .current_dir(&context.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| "failed to start git apply")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(patch.as_bytes())
            .with_context(|| "failed to send patch to git apply")?;
    }
    let output = tokio::task::spawn_blocking(move || {
        tracing::info!("apply_patch git apply wait start");
        child
            .wait_with_output()
            .with_context(|| "failed to wait for git apply")
    })
    .await
    .with_context(|| "git apply task panicked")??;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        anyhow::bail!(
            "patch failed: {}",
            if stderr.is_empty() { stdout } else { stderr }
        );
    }

    // Snapshots, formatting and LSP diagnostics all do blocking I/O (diagnostics
    // wait on the language server). Run them off the async executor so the agent
    // response never freezes after applying a patch.
    let diagnostic_paths = targets;
    tokio::task::spawn_blocking(move || {
        let formatted = format::format_paths(
            &context.cwd,
            context.formatter(),
            before_states.keys().cloned().collect::<Vec<_>>(),
        );
        let mut metadata = json!({ "paths": paths });
        let mut snapshots = Vec::new();
        for (path, before) in before_states {
            if let Some(snapshot) =
                crate::snapshot::file_change(&context.cwd, &path, before)?
            {
                snapshots.push(snapshot);
            }
        }
        crate::snapshot::add_metadata_snapshots(&mut metadata, snapshots);
        format::attach_formatted(&mut metadata, &formatted);
        // Touch first (the single blocking diagnostics wait + cache), then read
        // the cache — mirrors opencode's touchFile-then-diagnostics.
        metadata["lspTouch"] =
            diagnostics::touch_paths(&context.cwd, diagnostic_paths.clone());
        let report = diagnostics::attach_lsp_diagnostics(
            &context.cwd,
            diagnostic_paths,
            &mut metadata,
        );

        let mut output = if stdout.is_empty() {
            format!("Applied patch to:\n{}", paths.join("\n"))
        } else {
            stdout
        };
        if let Some(report) = report {
            output.push_str("\n\n");
            output.push_str(&report);
        }

        Ok(ToolExecutionResult {
            title: format!("Applied patch to {} file(s)", paths.len()),
            output,
            metadata: Some(metadata),
        })
    })
    .await
    .with_context(|| "apply_patch metadata task panicked")?
}

async fn apply_v4a_patch(
    context: ToolContext,
    patch_text: &str,
) -> anyhow::Result<ToolExecutionResult> {
    let hunks = patch::parse_v4a_patch(patch_text)?;
    if hunks.is_empty() {
        anyhow::bail!("V4A patch had no file operations");
    }
    let mut lock_paths = Vec::new();
    for hunk in &hunks {
        let path_str = match hunk {
            patch::V4AHunk::Add { path, .. }
            | patch::V4AHunk::Delete { path }
            | patch::V4AHunk::Update { path, .. } => path.clone(),
        };
        let target = project_path_for_write(&context, &path_str)?;
        context.ensure_allowed("edit", &display_path(&context.cwd, &target))?;
        lock_paths.push(target.clone());
        if let patch::V4AHunk::Update {
            move_path: Some(new_path),
            ..
        } = hunk
        {
            let new_target = project_path_for_write(&context, new_path)?;
            context.ensure_allowed("edit", &display_path(&context.cwd, &new_target))?;
            lock_paths.push(new_target.clone());
        }
    }

    tracing::info!(paths = ?lock_paths, "V4A apply_patch waiting for file locks");
    let _locks = locks::lock_files(lock_paths).await;
    tracing::info!("V4A apply_patch acquired file locks");

    let mutation = tokio::task::spawn_blocking({
        let context = context.clone();
        move || apply_v4a_patch_locked(context, hunks)
    })
    .await
    .with_context(|| "V4A patch task panicked")??;
    drop(_locks);

    // Diagnostics + formatting block on language servers; keep them off the
    // async executor so the agent response can't freeze after a patch.
    tokio::task::spawn_blocking(move || apply_v4a_patch_metadata(context, mutation))
        .await
        .with_context(|| "V4A patch metadata task panicked")?
}

fn apply_v4a_patch_locked(
    context: ToolContext,
    hunks: Vec<patch::V4AHunk>,
) -> anyhow::Result<PatchMutation> {
    let mut before_states = BTreeMap::new();
    let mut touched: Vec<String> = Vec::new();
    let mut diagnostic_paths = Vec::new();
    let mut virtual_files: BTreeMap<std::path::PathBuf, Option<String>> = BTreeMap::new();
    let mut planned_files: BTreeMap<std::path::PathBuf, PlannedFile> = BTreeMap::new();

    for hunk in &hunks {
        let path_str = match hunk {
            patch::V4AHunk::Add { path, .. }
            | patch::V4AHunk::Delete { path }
            | patch::V4AHunk::Update { path, .. } => path.clone(),
        };
        let target = project_path_for_write(&context, &path_str)?;
        before_states
            .entry(target.clone())
            .or_insert(crate::snapshot::FileState::from_path(&target)?);
        if let patch::V4AHunk::Update {
            move_path: Some(new_path),
            ..
        } = hunk
        {
            let new_target = project_path_for_write(&context, new_path)?;
            before_states
                .entry(new_target.clone())
                .or_insert(crate::snapshot::FileState::from_path(&new_target)?);
        }
        match hunk {
            patch::V4AHunk::Add { path, contents } => {
                let current = virtual_file(&mut virtual_files, &target)?;
                if current.is_some() {
                    anyhow::bail!("cannot add {path}: file already exists");
                }
                virtual_files.insert(target.clone(), Some(contents.clone()));
                planned_files
                    .insert(target.clone(), PlannedFile::Write(contents.clone()));
                diagnostic_paths.push(target.clone());
                touched.push(path.clone());
            }
            patch::V4AHunk::Delete { path } => {
                if virtual_file(&mut virtual_files, &target)?.is_none() {
                    anyhow::bail!("cannot delete {path}: file does not exist");
                }
                virtual_files.insert(target.clone(), None);
                planned_files.insert(target.clone(), PlannedFile::Delete);
                touched.push(path.clone());
            }
            patch::V4AHunk::Update {
                path,
                move_path,
                chunks,
            } => {
                let original =
                    virtual_file(&mut virtual_files, &target)?.ok_or_else(|| {
                        anyhow::anyhow!("cannot update {path}: file does not exist")
                    })?;
                let patched =
                    patch::apply_chunks(&original, chunks).map_err(|error| {
                        anyhow::anyhow!("failed to apply V4A chunks to {path}: {error:#}")
                    })?;
                let current = patch::join_bom(&patched.text, patched.bom);
                if let Some(new_path) = move_path {
                    let new_target = project_path_for_write(&context, new_path)?;
                    context.ensure_allowed(
                        "edit",
                        &display_path(&context.cwd, &new_target),
                    )?;
                    if virtual_file(&mut virtual_files, &new_target)?.is_some() {
                        anyhow::bail!(
                            "cannot move {path} to {new_path}: target already exists"
                        );
                    }
                    virtual_files.insert(target.clone(), None);
                    virtual_files.insert(new_target.clone(), Some(current.clone()));
                    planned_files.insert(target.clone(), PlannedFile::Delete);
                    planned_files.insert(new_target.clone(), PlannedFile::Write(current));
                    diagnostic_paths.push(new_target.clone());
                    touched.push(format!("{path} -> {new_path}"));
                } else {
                    virtual_files.insert(target.clone(), Some(current.clone()));
                    planned_files.insert(target.clone(), PlannedFile::Write(current));
                    diagnostic_paths.push(target.clone());
                    touched.push(path.clone());
                }
            }
        }
    }

    // Do not touch disk until every chunk in every file has resolved. A stale
    // hunk therefore cannot leave an earlier file partially patched.
    let write_result: anyhow::Result<()> = (|| {
        for (target, planned) in planned_files {
            match planned {
                PlannedFile::Write(contents) => {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).with_context(|| {
                            format!(
                                "failed to create parent directory for {}",
                                target.display()
                            )
                        })?;
                    }
                    std::fs::write(&target, contents.as_bytes()).with_context(|| {
                        format!("failed to write {}", target.display())
                    })?;
                }
                PlannedFile::Delete => {
                    if target.exists() {
                        std::fs::remove_file(&target).with_context(|| {
                            format!("failed to delete {}", target.display())
                        })?;
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = write_result {
        let rollback_errors = before_states
            .iter()
            .filter_map(|(path, state)| {
                crate::snapshot::write_state(path, state)
                    .err()
                    .map(|rollback| format!("{}: {rollback:#}", path.display()))
            })
            .collect::<Vec<_>>();
        if rollback_errors.is_empty() {
            return Err(error)
                .context("patch commit failed; all touched files were restored");
        }
        anyhow::bail!(
            "patch commit failed: {error:#}; rollback also failed for: {}",
            rollback_errors.join("; ")
        );
    }

    Ok(PatchMutation {
        touched,
        diagnostic_paths,
        before_states,
    })
}

fn apply_v4a_patch_metadata(
    context: ToolContext,
    mutation: PatchMutation,
) -> anyhow::Result<ToolExecutionResult> {
    let formatted = format::format_paths(
        &context.cwd,
        context.formatter(),
        mutation.diagnostic_paths.clone(),
    );
    let lsp_touch =
        diagnostics::touch_paths(&context.cwd, mutation.diagnostic_paths.clone());
    let mut metadata = json!({ "paths": mutation.touched });
    metadata["lspTouch"] = lsp_touch;
    let mut snapshots = Vec::new();
    for (path, before) in mutation.before_states {
        if let Some(snapshot) = crate::snapshot::file_change(&context.cwd, &path, before)?
        {
            snapshots.push(snapshot);
        }
    }
    crate::snapshot::add_metadata_snapshots(&mut metadata, snapshots);
    format::attach_formatted(&mut metadata, &formatted);
    let report = diagnostics::attach_lsp_diagnostics(
        &context.cwd,
        mutation.diagnostic_paths,
        &mut metadata,
    );

    let mut output = format!("Applied patch to:\n{}", mutation.touched.join("\n"));
    if let Some(report) = report {
        output.push_str("\n\n");
        output.push_str(&report);
    }

    Ok(ToolExecutionResult {
        title: format!("Applied patch to {} file(s)", mutation.touched.len()),
        output,
        metadata: Some(metadata),
    })
}

fn virtual_file(
    files: &mut BTreeMap<std::path::PathBuf, Option<String>>,
    path: &std::path::Path,
) -> anyhow::Result<Option<String>> {
    if let Some(contents) = files.get(path) {
        return Ok(contents.clone());
    }
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", path.display()))
        }
    };
    files.insert(path.to_path_buf(), contents.clone());
    Ok(contents)
}
