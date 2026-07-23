use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::{McpContent, McpToolCallResult, McpToolInfo};
use serde_json::{json, Value};

pub(crate) const MEMORY_MCP_ID: &str = "neoism-memory";

const INDEX_FILE: &str = "MEMORY.md";
const PROJECT_MEMORY_DIR: &str = "Memory";
const USER_MEMORY_DIR: &str = "Memory/Personal";
/// Pre-flip user memory location (`Personal/Memory` inside the Default
/// vault). Reads fall back here while the new `Memory/Personal` folder is
/// missing; write paths migrate its files forward on first touch.
const LEGACY_USER_MEMORY_DIR: &str = "Personal/Memory";
const SCOPE_DESCRIPTION: &str = "Scope: auto searches project and user memory; project uses the linked project vault; user uses Default/Memory/Personal; all is the same as auto.";

pub(crate) fn tools() -> Vec<McpToolInfo> {
    vec![
        tool(
            "memory.init",
            "Create Claude-style Neoism memory folders and MEMORY.md indexes",
            json!({
                "type": "object",
                "properties": { "scope": scope_schema() },
                "description": SCOPE_DESCRIPTION
            }),
        ),
        tool(
            "memory.list",
            "List Neoism memory files",
            json!({
                "type": "object",
                "properties": { "scope": scope_schema(), "limit": { "type": "integer" } },
                "description": SCOPE_DESCRIPTION
            }),
        ),
        tool(
            "memory.recall",
            "Search Neoism memory indexes and topic files",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "scope": scope_schema(),
                    "limit": { "type": "integer" }
                },
                "required": ["query"],
                "description": SCOPE_DESCRIPTION
            }),
        ),
        tool(
            "memory.read",
            "Read a memory file by path relative to a memory folder",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "scope": scope_schema()
                },
                "required": ["path"]
            }),
        ),
        tool(
            "memory.write",
            "Write or update a Claude-style Neoism memory topic file and keep MEMORY.md as a compact index",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "type": {
                        "type": "string",
                        "description": "project, feedback, bug, feature, reference, perf, preference, workflow, or personal"
                    },
                    "scope": scope_schema(),
                    "body": { "type": "string" },
                    "content": { "type": "string" },
                    "fileName": { "type": "string" },
                    "created": { "type": "string" },
                    "updated": { "type": "string" },
                    "origin": { "type": "string" }
                },
                "required": ["name", "description"]
            }),
        ),
    ]
}

/// Entry point used by the MCP dispatcher: identical to `call_tool`, but
/// `memory.recall` upgrades to semantic (turso vector) ranking when an
/// embeddings client and a turso-backed store are available. Any semantic
/// failure falls back to the keyword scan — recall never breaks because an
/// embeddings provider is down.
pub(crate) async fn call_tool_with_app_state(
    state: Option<&crate::state::AppState>,
    directory: &str,
    tool: &str,
    arguments: Value,
) -> anyhow::Result<McpToolCallResult> {
    if tool == "memory.recall" {
        if let Some(state) = state {
            match semantic_recall(state, directory, &arguments).await {
                Ok(Some(result)) => return Ok(result),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "semantic memory recall failed; falling back to keyword recall"
                    );
                }
            }
        }
    }
    call_tool(directory, tool, arguments)
}

/// Semantic memory.recall: sync the vector index for every root in scope
/// (embed new/edited files, prune deleted ones), embed the query, and rank
/// by cosine distance. Returns Ok(None) when semantic search is unavailable
/// (no embeddings client, non-turso store, or an empty query) so the caller
/// falls back to the keyword scan.
async fn semantic_recall(
    state: &crate::state::AppState,
    directory: &str,
    arguments: &Value,
) -> anyhow::Result<Option<McpToolCallResult>> {
    let Some(client) = state.inner.semantic.clone() else {
        return Ok(None);
    };
    if !state.inner.store.semantic_search_supported() {
        return Ok(None);
    }
    let query = required_string(arguments, "query")?;
    if query.trim().is_empty() {
        return Ok(None);
    }
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(40)
        .max(1) as usize;
    let cwd = PathBuf::from(directory);
    let roots = roots_for_scope(&cwd, scope_arg(arguments), false)?;

    for root in &roots {
        sync_memory_embeddings(state, &client, root).await?;
    }

    let mut query_vectors = client.embed(&[query.clone()]).await?;
    let query_vector = match query_vectors.pop() {
        Some(vector) if !vector.is_empty() => vector,
        _ => return Ok(None),
    };
    let root_keys: Vec<String> = roots
        .iter()
        .map(|root| root.path.to_string_lossy().to_string())
        .collect();
    let ranked = state
        .inner
        .store
        .memory_semantic_search(
            &root_keys,
            &crate::semantic::vector_json(&query_vector),
            &client.model_spec,
            limit,
        )
        .await?;

    let mut hits = Vec::new();
    for (path, distance) in ranked {
        let absolute = PathBuf::from(&path);
        let Ok(text) = std::fs::read_to_string(&absolute) else {
            // File was deleted since indexing; drop the stale row.
            let _ = state.inner.store.delete_memory_embedding(&path).await;
            continue;
        };
        let Some(root) = roots.iter().find(|root| absolute.starts_with(&root.path))
        else {
            continue;
        };
        hits.push(json!({
            "scope": root.scope,
            "vault": root.label,
            "path": relative_label(root, &absolute),
            "description": frontmatter_value(&text, "description"),
            "type": frontmatter_value(&text, "type"),
            "snippet": snippet(&text, 0),
            "distance": distance,
        }));
    }
    let output = json!({
        "operation": "recall",
        "query": query,
        "mode": "semantic",
        "hits": hits,
    });
    Ok(Some(text_result(serde_json::to_string_pretty(&output)?)))
}

/// Bring the vector index for one memory root up to date with the files on
/// disk: embed new/changed markdown (keyed by content hash), delete rows for
/// removed files. Memory stores are tens of files, so this runs inline at
/// recall time instead of needing a background indexer.
async fn sync_memory_embeddings(
    state: &crate::state::AppState,
    client: &crate::semantic::EmbeddingsClient,
    root: &MemoryRoot,
) -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};

    /// Matches the embeddings client's input cap.
    const MAX_EMBED_CHARS: usize = 8_000;
    const EMBED_BATCH: usize = 16;

    let root_key = root.path.to_string_lossy().to_string();
    let indexed: std::collections::HashMap<String, String> = state
        .inner
        .store
        .memory_embedding_hashes(&root_key, &client.model_spec)
        .await?
        .into_iter()
        .collect();
    let mut on_disk = std::collections::HashSet::new();
    let mut stale: Vec<(String, String, String)> = Vec::new();
    for path in memory_files(root)? {
        let path_key = path.to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let description = frontmatter_value(&text, "description").unwrap_or_default();
        let embed_text: String = format!("{name}\n{description}\n{text}")
            .chars()
            .take(MAX_EMBED_CHARS)
            .collect();
        let hash = format!("{:x}", Sha256::digest(embed_text.as_bytes()));
        on_disk.insert(path_key.clone());
        if indexed.get(&path_key) == Some(&hash) {
            continue;
        }
        stale.push((path_key, hash, embed_text));
    }
    for path in indexed.keys() {
        if !on_disk.contains(path) {
            let _ = state.inner.store.delete_memory_embedding(path).await;
        }
    }
    let now = crate::now_millis() as i64;
    for chunk in stale.chunks(EMBED_BATCH) {
        let inputs: Vec<String> = chunk.iter().map(|(_, _, text)| text.clone()).collect();
        let vectors = client.embed(&inputs).await?;
        for ((path, hash, _), vector) in chunk.iter().zip(vectors) {
            if vector.is_empty() {
                continue;
            }
            state
                .inner
                .store
                .upsert_memory_embedding(
                    path,
                    &root_key,
                    hash,
                    &client.model_spec,
                    now,
                    &crate::semantic::vector_json(&vector),
                )
                .await?;
        }
    }
    Ok(())
}

pub(crate) fn call_tool(
    directory: &str,
    tool: &str,
    arguments: Value,
) -> anyhow::Result<McpToolCallResult> {
    let cwd = PathBuf::from(directory);
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(40)
        .max(1) as usize;
    let scope = scope_arg(&arguments);

    let output = match tool {
        "memory.init" => {
            let roots = roots_for_scope(&cwd, scope, true)?;
            for root in &roots {
                ensure_memory_root(root)?;
            }
            json!({
                "operation": "init",
                "roots": roots.iter().map(root_json).collect::<Vec<_>>()
            })
        }
        "memory.list" => {
            let roots = roots_for_scope(&cwd, scope, false)?;
            let entries = collect_scoped(&roots, |root| list_entries(root, limit))?;
            json!({ "operation": "list", "entries": entries })
        }
        "memory.recall" => {
            let query = required_string(&arguments, "query")?;
            let roots = roots_for_scope(&cwd, scope, false)?;
            let hits =
                collect_scoped(&roots, |root| recall_entries(root, &query, limit))?;
            json!({ "operation": "recall", "query": query, "hits": hits })
        }
        "memory.read" => {
            let path = required_string(&arguments, "path")?;
            let roots = roots_for_scope(&cwd, scope, false)?;
            let root = roots
                .first()
                .ok_or_else(|| anyhow::anyhow!("no memory root available"))?;
            let absolute = safe_memory_path(root, &path)?;
            let text = std::fs::read_to_string(&absolute)
                .with_context(|| format!("failed to read {}", absolute.display()))?;
            json!({
                "operation": "read",
                "scope": root.scope,
                "path": relative_label(root, &absolute),
                "absolutePath": absolute,
                "text": text
            })
        }
        "memory.write" => {
            let name = required_string(&arguments, "name")?;
            let description = required_string(&arguments, "description")?;
            let kind = optional_string(&arguments, "type")
                .unwrap_or_else(|| "project".to_string());
            let body = optional_string(&arguments, "body")
                .or_else(|| optional_string(&arguments, "content"))
                .unwrap_or_default();
            let root = write_root_for_scope(&cwd, scope, &kind)?;
            ensure_memory_root(&root)?;
            let file_name = optional_string(&arguments, "fileName")
                .map(|value| safe_file_name(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| memory_file_name(&kind, &name));
            let path = root.path.join(file_name);
            // Reject near-duplicate memories: if an existing topic file in
            // this root already covers the same fact, point the model at it
            // instead of writing a sibling duplicate.
            if let Some(existing) =
                find_similar_memory(&root, &path, &name, &description)?
            {
                let output = json!({
                    "operation": "write",
                    "status": "duplicate",
                    "scope": root.scope,
                    "existingPath": relative_label(&root, &existing),
                    "absolutePath": existing,
                    "hint": "a memory covering this already exists; read it and update that file (pass its fileName) instead of creating a duplicate"
                });
                return Ok(text_result(serde_json::to_string_pretty(&output)?));
            }
            let today = today_utc();
            let created =
                optional_string(&arguments, "created").unwrap_or_else(|| today.clone());
            let updated = optional_string(&arguments, "updated").unwrap_or(today);
            let origin = optional_string(&arguments, "origin")
                .unwrap_or_else(|| "neoism-agent".to_string());
            let source = render_memory_file(
                &name,
                &description,
                &kind,
                root.scope,
                &origin,
                &created,
                &updated,
                &body,
            );
            std::fs::write(&path, source)
                .with_context(|| format!("failed to write {}", path.display()))?;
            update_index(&root, &path, &name, &description)?;
            reindex_root(&root)?;
            json!({
                "operation": "write",
                "scope": root.scope,
                "path": relative_label(&root, &path),
                "absolutePath": path
            })
        }
        other => anyhow::bail!("unknown Neoism Memory MCP tool {other}"),
    };

    Ok(text_result(serde_json::to_string_pretty(&output)?))
}

/// Cap on how much of a single MEMORY.md index gets injected into the
/// system prompt. Indexes are compact by design; this only guards against
/// a runaway hand-edited file.
const MAX_INJECTED_INDEX_CHARS: usize = 8_000;

/// Compact MEMORY.md indexes for the session system prompt, so the model is
/// briefed on durable memory automatically instead of having to remember to
/// call memory.recall first. Returns one section per scope that actually has
/// indexed memories; failures and empty boilerplate indexes are skipped.
pub(crate) fn system_memory_indexes(directory: &str) -> Vec<String> {
    let cwd = Path::new(directory);
    let Ok(roots) = roots_for_scope(cwd, "auto", false) else {
        return Vec::new();
    };
    let mut sections = Vec::new();
    for root in roots {
        let index = root.path.join(INDEX_FILE);
        let Ok(text) = std::fs::read_to_string(&index) else {
            continue;
        };
        if !text
            .lines()
            .any(|line| line.trim_start().starts_with("- ["))
        {
            continue;
        }
        let text = truncate_at_char_boundary(text.trim(), MAX_INJECTED_INDEX_CHARS);
        sections.push(format!(
            "Persistent {} memory index (vault {}). Before repeating project discovery, read the linked topic files with the {} MCP memory.read tool (paths are relative to the memory folder); memory.recall ranks memories semantically (natural-language queries work), falling back to keyword matching when embeddings are unavailable. Save new durable facts with memory.write.\n{}",
            root.scope, root.label, MEMORY_MCP_ID, text
        ));
    }
    sections
}

fn truncate_at_char_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Clone)]
struct MemoryRoot {
    scope: &'static str,
    label: String,
    path: PathBuf,
    workspace: neoism_workspace_index::config::NeoismWorkspace,
}

fn roots_for_scope(
    cwd: &Path,
    scope: &str,
    include_missing: bool,
) -> anyhow::Result<Vec<MemoryRoot>> {
    let mut roots = match scope {
        "project" => vec![project_root(cwd)?],
        "user" => vec![user_root(cwd)?],
        "all" | "auto" | "" => vec![project_root(cwd)?, user_root(cwd)?],
        other => anyhow::bail!("unknown memory scope {other}"),
    };
    if !include_missing {
        for root in &mut roots {
            // Read-side fallback: user memory that has not been migrated
            // yet still lives at the legacy `Personal/Memory` location.
            if !root.path.is_dir() {
                if let Some(legacy) = legacy_user_memory_path(root) {
                    if legacy.is_dir() {
                        root.path = legacy;
                    }
                }
            }
        }
        roots.retain(|root| root.path.is_dir());
    }
    Ok(roots)
}

fn write_root_for_scope(
    cwd: &Path,
    scope: &str,
    kind: &str,
) -> anyhow::Result<MemoryRoot> {
    match scope {
        "user" => user_root(cwd),
        "project" => project_root(cwd),
        "all" => project_root(cwd),
        "auto" | "" => {
            if matches!(kind, "personal" | "preference" | "workflow") {
                user_root(cwd)
            } else {
                project_root(cwd)
            }
        }
        other => anyhow::bail!("unknown memory scope {other}"),
    }
}

fn project_root(cwd: &Path) -> anyhow::Result<MemoryRoot> {
    let workspace = neoism_workspace_index::linked_project_for_code_dir(cwd)?
        .or_else(|| neoism_workspace_index::load_workspace(cwd).ok().flatten())
        .unwrap_or_else(|| neoism_workspace_index::config::NeoismWorkspace {
            root: cwd.to_path_buf(),
            config: neoism_workspace_index::config::WorkspaceConfig::new(cwd),
        });
    let path = workspace.notes_workspace_dir().join(PROJECT_MEMORY_DIR);
    Ok(MemoryRoot {
        scope: "project",
        label: workspace.config.notes.workspace.clone(),
        path,
        workspace,
    })
}

fn user_root(cwd: &Path) -> anyhow::Result<MemoryRoot> {
    let mut config = neoism_workspace_index::config::WorkspaceConfig::new(cwd);
    config.notes.workspace =
        neoism_workspace_index::config::DEFAULT_NOTES_WORKSPACE.to_string();
    let workspace = neoism_workspace_index::config::NeoismWorkspace {
        root: cwd.to_path_buf(),
        config,
    };
    let path = workspace.notes_workspace_dir().join(USER_MEMORY_DIR);
    Ok(MemoryRoot {
        scope: "user",
        label: "Default/Memory/Personal".to_string(),
        path,
        workspace,
    })
}

/// The legacy pre-flip location (`Personal/Memory`) of the user memory
/// folder in the same Default vault as `root`. None for non-user roots.
fn legacy_user_memory_path(root: &MemoryRoot) -> Option<PathBuf> {
    if root.scope != "user" {
        return None;
    }
    Some(
        root.workspace
            .notes_workspace_dir()
            .join(LEGACY_USER_MEMORY_DIR),
    )
}

/// One-way migration from the legacy `Personal/Memory` layout: move every
/// entry the legacy folder still holds into the new `Memory/Personal`
/// folder. Moves only — nothing is ever deleted, and entries that already
/// exist at the destination stay behind at the legacy location. Runs from
/// the write path (memory.init / memory.write) so the flip happens on the
/// first touch that mutates memory.
fn migrate_legacy_user_memory(root: &MemoryRoot) -> anyhow::Result<()> {
    let Some(legacy) = legacy_user_memory_path(root) else {
        return Ok(());
    };
    if legacy == root.path || !legacy.is_dir() {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return Ok(());
    };
    std::fs::create_dir_all(&root.path)
        .with_context(|| format!("failed to create {}", root.path.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let source = entry.path();
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = root.path.join(name);
        if target.exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(&source, &target) {
            tracing::warn!(
                %error,
                source = %source.display(),
                target = %target.display(),
                "failed to migrate legacy user memory entry"
            );
        }
    }
    Ok(())
}

fn ensure_memory_root(root: &MemoryRoot) -> anyhow::Result<()> {
    migrate_legacy_user_memory(root)?;
    std::fs::create_dir_all(&root.path)
        .with_context(|| format!("failed to create {}", root.path.display()))?;
    let index = root.path.join(INDEX_FILE);
    if !index.exists() {
        std::fs::write(&index, initial_index(root.scope))
            .with_context(|| format!("failed to write {}", index.display()))?;
    }
    reindex_root(root)?;
    Ok(())
}

fn initial_index(scope: &str) -> String {
    format!(
        "# Memory\n\nCompact {scope} memory index for Neoism Agent. Keep this file short: one link per memory plus a one-line recall summary. Put details in topic files next to this file and read them on demand.\n\n"
    )
}

fn list_entries(root: &MemoryRoot, limit: usize) -> anyhow::Result<Vec<Value>> {
    let mut entries = memory_files(root)?;
    entries.sort();
    Ok(entries
        .into_iter()
        .take(limit)
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            json!({
                "scope": root.scope,
                "vault": root.label,
                "path": relative_label(root, &path),
                "description": frontmatter_value(&text, "description"),
                "type": frontmatter_value(&text, "type"),
            })
        })
        .collect())
}

fn recall_entries(
    root: &MemoryRoot,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return list_entries(root, limit);
    }
    // Tokenized matching: any query word can hit, so multi-word queries like
    // "cargo build release" recall files that only mention "cargo". Ranking
    // prefers files matching more tokens, then whole-phrase matches, then
    // filename hits over description hits over body hits.
    let tokens: Vec<String> = needle.split_whitespace().map(str::to_string).collect();
    let mut hits = Vec::new();
    for path in memory_files(root)? {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let description = frontmatter_value(&text, "description")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let haystack =
            format!("{file_name}\n{description}\n{}", text.to_ascii_lowercase());
        let mut missed = 0usize;
        let mut tier_sum = 0usize;
        let mut first_pos: Option<usize> = None;
        for token in &tokens {
            let tier = if file_name.contains(token.as_str()) {
                0
            } else if description.contains(token.as_str()) {
                1
            } else if haystack.contains(token.as_str()) {
                2
            } else {
                missed += 1;
                continue;
            };
            tier_sum += tier;
            if first_pos.is_none() {
                first_pos = haystack.find(token.as_str());
            }
        }
        if missed == tokens.len() {
            continue;
        }
        let phrase_miss = usize::from(!haystack.contains(&needle));
        hits.push((
            (missed, phrase_miss, tier_sum),
            path.clone(),
            json!({
                "scope": root.scope,
                "vault": root.label,
                "path": relative_label(root, &path),
                "description": frontmatter_value(&text, "description"),
                "type": frontmatter_value(&text, "type"),
                "snippet": snippet(&haystack, first_pos.unwrap_or(0)),
            }),
        ));
    }
    hits.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(hits
        .into_iter()
        .take(limit)
        .map(|(_, _, hit)| hit)
        .collect())
}

fn memory_files(root: &MemoryRoot) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root.path) else {
        return Ok(files);
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("md")
            && path.file_name().and_then(|name| name.to_str()) != Some(INDEX_FILE)
        {
            files.push(path);
        }
    }
    Ok(files)
}

fn render_memory_file(
    name: &str,
    description: &str,
    kind: &str,
    scope: &str,
    origin: &str,
    created: &str,
    updated: &str,
    body: &str,
) -> String {
    format!(
        "---\nname: {}\ndescription: {}\ntype: {}\nscope: {}\norigin: {}\ncreated: {}\nupdated: {}\n---\n\n{}\n",
        yaml_string(name),
        yaml_string(description),
        yaml_string(kind),
        yaml_string(scope),
        yaml_string(origin),
        yaml_string(created),
        yaml_string(updated),
        body.trim()
    )
}

fn update_index(
    root: &MemoryRoot,
    path: &Path,
    name: &str,
    description: &str,
) -> anyhow::Result<()> {
    let index = root.path.join(INDEX_FILE);
    let rel = relative_label(root, path);
    let mut lines = std::fs::read_to_string(&index)
        .unwrap_or_else(|_| initial_index(root.scope))
        .lines()
        .filter(|line| !line.contains(&format!("]({rel})")))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !lines.iter().any(|line| line.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.push(format!(
        "- [{}]({}) - {}",
        name.trim(),
        rel,
        description.trim()
    ));
    let mut source = lines.join("\n");
    if !source.ends_with('\n') {
        source.push('\n');
    }
    std::fs::write(&index, source)
        .with_context(|| format!("failed to write {}", index.display()))
}

fn safe_memory_path(root: &MemoryRoot, raw: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(raw);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("memory path must be relative to {}", root.path.display());
    }
    Ok(root.path.join(path))
}

fn reindex_root(root: &MemoryRoot) -> anyhow::Result<()> {
    let _ = neoism_workspace_index::NoteGraph::from_workspace(root.workspace.clone())?;
    Ok(())
}

fn collect_scoped<F>(roots: &[MemoryRoot], mut f: F) -> anyhow::Result<Vec<Value>>
where
    F: FnMut(&MemoryRoot) -> anyhow::Result<Vec<Value>>,
{
    let mut out = Vec::new();
    for root in roots {
        out.push(json!({
            "scope": root.scope,
            "vault": root.label,
            "memoryRoot": root.path,
            "result": f(root)?,
        }));
    }
    Ok(out)
}

fn root_json(root: &MemoryRoot) -> Value {
    json!({
        "scope": root.scope,
        "vault": root.label,
        "memoryRoot": root.path,
        "index": root.path.join(INDEX_FILE),
    })
}

fn relative_label(root: &MemoryRoot, path: &Path) -> String {
    path.strip_prefix(&root.path)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_string),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn memory_file_name(kind: &str, name: &str) -> String {
    let kind = slug(kind);
    let name = slug(name);
    let stem = if kind.is_empty() {
        name
    } else if name.starts_with(&format!("{kind}_")) {
        name
    } else {
        format!("{kind}_{name}")
    };
    format!("{}.md", stem.trim_matches('_'))
}

fn safe_file_name(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_suffix(".md").unwrap_or(raw);
    let file = slug(raw);
    if file.is_empty() {
        String::new()
    } else {
        format!("{file}.md")
    }
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_sep = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_sep = false;
        } else if !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

fn yaml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Find an existing memory file whose name/description substantially overlap
/// the incoming memory, so writes reconcile into the existing topic file
/// instead of accumulating near-duplicates.
fn find_similar_memory(
    root: &MemoryRoot,
    target: &Path,
    name: &str,
    description: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let incoming = overlap_tokens(&format!("{name} {description}"));
    if incoming.len() < 3 {
        return Ok(None);
    }
    for path in memory_files(root)? {
        if path == target {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let existing_name = frontmatter_value(&text, "name").unwrap_or_default();
        let existing_description =
            frontmatter_value(&text, "description").unwrap_or_default();
        let existing = overlap_tokens(&format!("{existing_name} {existing_description}"));
        if existing.is_empty() {
            continue;
        }
        let shared = incoming.intersection(&existing).count();
        let smaller = incoming.len().min(existing.len());
        // 70% overlap of the smaller token set: catches "Don't run cargo
        // build release" vs "Don't run cargo build --release" style dupes
        // without blocking genuinely distinct memories that share a few words.
        if smaller > 0 && shared * 10 >= smaller * 7 {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn overlap_tokens(source: &str) -> std::collections::BTreeSet<String> {
    source
        .to_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() > 2)
        .map(str::to_string)
        .collect()
}

fn frontmatter_value(source: &str, key: &str) -> Option<String> {
    let mut lines = source.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            break;
        }
        let Some((candidate, value)) = line.split_once(':') else {
            continue;
        };
        if candidate.trim() == key {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn snippet(source: &str, pos: usize) -> String {
    let char_pos = source[..pos].chars().count();
    let start = char_pos.saturating_sub(80);
    source
        .chars()
        .skip(start)
        .take(240)
        .collect::<String>()
        .replace('\n', " ")
}

fn scope_arg(arguments: &Value) -> &str {
    arguments
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("auto")
}

fn scope_schema() -> Value {
    json!({ "type": "string", "enum": ["auto", "project", "user", "all"] })
}

fn required_string(arguments: &Value, key: &str) -> anyhow::Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn today_utc() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn tool(
    name: &'static str,
    description: &'static str,
    input_schema: Value,
) -> McpToolInfo {
    McpToolInfo {
        name: name.to_string(),
        description: Some(description.to_string()),
        input_schema,
        client: MEMORY_MCP_ID.to_string(),
        annotations: None,
    }
}

fn text_result(text: String) -> McpToolCallResult {
    McpToolCallResult {
        content: vec![McpContent::Text {
            text,
            annotations: None,
        }],
        is_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.as_ref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn tempdir(label: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "neoism-memory-{label}-{}-{}",
            std::process::id(),
            nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn nanos() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{n:x}")
    }

    fn result_json(result: McpToolCallResult) -> Value {
        let Some(McpContent::Text { text, .. }) = result.content.first() else {
            panic!("expected text MCP result")
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn writes_project_and_user_memory_in_notes_vaults() {
        let _lock = env_lock().lock().unwrap();
        let cwd = tempdir("cwd");
        let notes_home = tempdir("notes");
        let _env = EnvGuard::set("NEOISM_NOTES_HOME", &notes_home);
        let directory = cwd.to_str().unwrap();

        let init = result_json(
            call_tool(directory, "memory.init", json!({ "scope": "all" })).unwrap(),
        );
        assert_eq!(init["operation"], "init");
        assert!(notes_home.join("Default/Memory/MEMORY.md").is_file());
        assert!(notes_home
            .join("Default/Memory/Personal/MEMORY.md")
            .is_file());

        let project = result_json(
            call_tool(
                directory,
                "memory.write",
                json!({
                    "name": "User likes Claude-style memory",
                    "description": "Use a compact MEMORY.md index and typed topic files",
                    "type": "feedback",
                    "body": "Keep the detailed durable fact in a topic file, not the index.",
                    "created": "2026-06-30",
                    "updated": "2026-06-30"
                }),
            )
            .unwrap(),
        );
        assert_eq!(project["scope"], "project");
        let project_file =
            notes_home.join("Default/Memory/feedback_user_likes_claude_style_memory.md");
        assert!(project_file.is_file());

        let user = result_json(
            call_tool(
                directory,
                "memory.write",
                json!({
                    "name": "Parker prefers notes memory",
                    "description": "Store personal memory in Default/Memory/Personal",
                    "type": "personal",
                    "body": "Personal durable facts should stay in the Default vault.",
                    "created": "2026-06-30",
                    "updated": "2026-06-30"
                }),
            )
            .unwrap(),
        );
        assert_eq!(user["scope"], "user");
        assert!(notes_home
            .join("Default/Memory/Personal/personal_parker_prefers_notes_memory.md")
            .is_file());

        let index =
            std::fs::read_to_string(notes_home.join("Default/Memory/MEMORY.md")).unwrap();
        assert!(index.contains("[User likes Claude-style memory](feedback_user_likes_claude_style_memory.md)"));
        assert!(index.contains("Use a compact MEMORY.md index and typed topic files"));
        assert!(!index.contains("Keep the detailed durable fact"));

        let recall = result_json(
            call_tool(
                directory,
                "memory.recall",
                json!({ "query": "Claude-style", "scope": "project" }),
            )
            .unwrap(),
        );
        let hits = recall["hits"][0]["result"].as_array().unwrap();
        assert_eq!(
            hits[0]["path"],
            "feedback_user_likes_claude_style_memory.md"
        );

        let read = result_json(
            call_tool(
                directory,
                "memory.read",
                json!({
                    "path": "feedback_user_likes_claude_style_memory.md",
                    "scope": "project"
                }),
            )
            .unwrap(),
        );
        let text = read["text"].as_str().unwrap();
        assert!(text.contains("type: \"feedback\""));
        assert!(text.contains("origin: \"neoism-agent\""));

        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(notes_home);
    }

    #[test]
    fn legacy_user_memory_reads_fall_back_and_writes_migrate() {
        let _lock = env_lock().lock().unwrap();
        let cwd = tempdir("cwd-legacy");
        let notes_home = tempdir("notes-legacy");
        let _env = EnvGuard::set("NEOISM_NOTES_HOME", &notes_home);
        let directory = cwd.to_str().unwrap();

        let legacy = notes_home.join("Default/Personal/Memory");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("personal_legacy_fact.md"),
            "---\nname: \"Legacy fact\"\ndescription: \"Recorded before the layout flip\"\ntype: \"personal\"\n---\n\nLegacy body.\n",
        )
        .unwrap();
        std::fs::write(
            legacy.join("MEMORY.md"),
            "# Memory\n\n- [Legacy fact](personal_legacy_fact.md) - Recorded before the layout flip\n",
        )
        .unwrap();

        // Read paths fall back to the legacy folder while Memory/Personal
        // does not exist yet — and reading migrates nothing.
        let recall = result_json(
            call_tool(
                directory,
                "memory.recall",
                json!({ "query": "legacy", "scope": "user" }),
            )
            .unwrap(),
        );
        let hits = recall["hits"][0]["result"].as_array().unwrap();
        assert_eq!(hits[0]["path"], "personal_legacy_fact.md");
        assert!(!notes_home.join("Default/Memory/Personal").exists());

        // First write touch moves the legacy files into Memory/Personal.
        let user = result_json(
            call_tool(
                directory,
                "memory.write",
                json!({
                    "name": "Fresh user note",
                    "description": "Written after the new hierarchy shipped",
                    "type": "personal",
                    "body": "New body."
                }),
            )
            .unwrap(),
        );
        assert_eq!(user["scope"], "user");
        let migrated = notes_home.join("Default/Memory/Personal");
        assert!(migrated.join("personal_legacy_fact.md").is_file());
        assert!(migrated.join("MEMORY.md").is_file());
        assert!(migrated.join("personal_fresh_user_note.md").is_file());
        assert!(!legacy.join("personal_legacy_fact.md").exists());
        assert!(!legacy.join("MEMORY.md").exists());

        let index = std::fs::read_to_string(migrated.join("MEMORY.md")).unwrap();
        assert!(index.contains("[Legacy fact](personal_legacy_fact.md)"));
        assert!(index.contains("[Fresh user note](personal_fresh_user_note.md)"));

        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(notes_home);
    }

    #[test]
    fn civil_date_conversion_matches_unix_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_270), (2025, 7, 1));
    }
}
