use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use fff_search::{
    has_regex_metacharacters, AiGrepConfig, FuzzySearchOptions, GrepMode,
    GrepSearchOptions, PaginationArgs, QueryParser,
};
use neoism_agent_service_api::{
    DirectorySearchRequest, DirectorySearchResult, FindFilesRequest, FindFilesResult,
    GrepWorkspaceRequest, GrepWorkspaceResult, WorkspaceFileMatch, WorkspaceGrepMatch,
    WorkspaceSearchBounds, WorkspaceSearchMode,
};
use serde_json::{json, Value};

use super::args::{optional_string, required_string, usize_arg};
use super::paths::{
    directory_entries, display_path, existing_project_path, truncate_line,
};
use super::{process, ToolContext, ToolExecutionResult};

const DEFAULT_FFF_TIMEOUT_MS: u64 = 45_000;
const MAX_FFF_TIMEOUT_MS: u64 = 300_000;
pub(super) const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    ".claude/worktrees",
    ".codex",
    ".neoism/cache",
    "target",
    "node_modules",
    "dist",
    ".tmp",
];

pub(crate) fn find_files(
    registry: &crate::picker_registry::PickerRegistry,
    request: &FindFilesRequest,
) -> anyhow::Result<FindFilesResult> {
    let root = &request.root;
    if request.query.trim().is_empty() {
        let entries = directory_entries(root)?;
        let total = entries.len();
        let items = entries.into_iter().skip(request.offset).take(request.limit).map(|path| WorkspaceFileMatch {
            path, score: 0, git_status: None, size: 0, modified: 0,
        }).collect::<Vec<_>>();
        return Ok(FindFilesResult {
            bounds: WorkspaceSearchBounds {
                total: Some(total), total_at_least: total,
                next_cursor: (request.offset.saturating_add(items.len()) < total).then_some(request.offset.saturating_add(items.len())),
                truncated: request.offset.saturating_add(items.len()) < total,
                timed_out: false,
            },
            items,
            engine: Some("directory".to_string()),
            fallback_reason: None,
        });
    }
    if super::streaming_search::root_requires_fallback(root) {
        return super::streaming_search::find_files(request, "indexed search is disabled for home and filesystem roots");
    }
    let started = Instant::now();
    let search = registry.with_picker(root, |picker| {
        let parser = QueryParser::default();
        let mut results = picker.fuzzy_search(
            &parser.parse(&request.query),
            None,
            FuzzySearchOptions {
                max_threads: 0,
                current_file: None,
                project_path: Some(root),
                pagination: PaginationArgs { offset: request.offset, limit: request.limit },
                ..Default::default()
            },
        );
        if results.items.is_empty() {
            if let Some(token) = request.query.split_whitespace().filter(|token| token.len() >= 3).max_by_key(|token| token.len()) {
                if token != request.query {
                    results = picker.fuzzy_search(
                        &parser.parse(token),
                        None,
                        FuzzySearchOptions {
                            max_threads: 0,
                            current_file: None,
                            project_path: Some(root),
                            pagination: PaginationArgs { offset: request.offset, limit: request.limit },
                            ..Default::default()
                        },
                    );
                }
            }
        }
        let items = results.items.iter().zip(results.scores.iter()).map(|(item, score)| WorkspaceFileMatch {
            path: item.relative_path(picker),
            score: score.total,
            git_status: item.git_status.map(git_status_label),
            size: item.size,
            modified: item.modified,
        }).collect::<Vec<_>>();
        (items, results.total_matched)
    });
    match search {
        Ok((items, total)) => Ok(FindFilesResult {
            bounds: WorkspaceSearchBounds {
                total: Some(total),
                total_at_least: total,
                next_cursor: (request.offset.saturating_add(items.len()) < total)
                    .then_some(request.offset.saturating_add(items.len())),
                truncated: request.offset.saturating_add(items.len()) < total,
                timed_out: false,
            },
            items,
            engine: Some("fff".to_string()),
            fallback_reason: None,
        }),
        Err(error) => {
            let remaining = request.control.timeout_ms.saturating_sub(started.elapsed().as_millis() as u64).max(1);
            let mut fallback = request.clone();
            fallback.control.timeout_ms = fallback_timeout_ms(remaining);
            super::streaming_search::find_files(&fallback, &error.to_string())
        }
    }
}

pub(crate) fn search_directories(
    registry: &crate::picker_registry::PickerRegistry,
    request: &DirectorySearchRequest,
) -> anyhow::Result<DirectorySearchResult> {
    registry.with_picker(&request.root, |picker| {
        let parser = QueryParser::new(fff_search::DirSearchConfig);
        let results = picker.fuzzy_search_directories(
            &parser.parse(&request.query),
            FuzzySearchOptions {
                max_threads: 0,
                project_path: Some(&request.root),
                pagination: PaginationArgs { offset: request.offset, limit: request.limit },
                ..Default::default()
            },
        );
        let paths = results.items.iter().map(|item| item.relative_path(picker)).collect::<Vec<_>>();
        DirectorySearchResult {
            bounds: WorkspaceSearchBounds {
                total: Some(results.total_matched),
                total_at_least: results.total_matched,
                next_cursor: (request.offset.saturating_add(paths.len()) < results.total_matched)
                    .then_some(request.offset.saturating_add(paths.len())),
                truncated: request.offset.saturating_add(paths.len()) < results.total_matched,
                timed_out: false,
            },
            paths,
            engine: Some("fff".to_string()),
        }
    })
}

pub(crate) fn grep_workspace(
    registry: &crate::picker_registry::PickerRegistry,
    request: &GrepWorkspaceRequest,
) -> anyhow::Result<GrepWorkspaceResult> {
    let root = request.root.clone();
    let exclude = request.excludes.join(" ");
    let requested_mode = service_grep_mode(request.mode, request.patterns.first().map(String::as_str).unwrap_or(""));
    if super::streaming_search::root_requires_fallback(&root) {
        return super::streaming_search::grep_workspace(request, "indexed search is disabled for home and filesystem roots");
    }
    let query_text = grep_query_text(
        &root,
        &request.path,
        &root,
        request.include.as_deref(),
        Some(&exclude),
        if request.patterns.len() == 1 { &request.patterns[0] } else { "" },
    );
    let parser = QueryParser::<AiGrepConfig>::new(AiGrepConfig);
    let query = parser.parse(&query_text);
    let grep_budget_ms = request.control.timeout_ms.saturating_sub(2_000).max(500);
    let options = |mode| GrepSearchOptions {
        page_limit: request.limit,
        mode,
        smart_case: !request.case_sensitive,
        before_context: request.context_lines,
        after_context: request.context_lines,
        classify_definitions: true,
        trim_whitespace: false,
        time_budget_ms: grep_budget_ms,
        abort_signal: request.control.cancel.clone(),
        ..Default::default()
    };
    let search = registry.with_picker(&root, |picker| {
        let (results, used_mode) = if request.patterns.len() > 1 {
            let refs = request.patterns.iter().map(String::as_str).collect::<Vec<_>>();
            (picker.multi_grep(&refs, &query.constraints, &options(GrepMode::PlainText)), "multi".to_string())
        } else {
            let mut results = picker.grep(&query, &options(requested_mode));
            let mut used = mode_label(requested_mode).to_string();
            if results.matches.is_empty() && requested_mode != GrepMode::Fuzzy && request.patterns.first().is_some_and(|pattern| pattern.len() <= 1024) {
                let fuzzy = picker.grep(&query, &options(GrepMode::Fuzzy));
                if !fuzzy.matches.is_empty() {
                    results = fuzzy;
                    used = "fuzzy".to_string();
                }
            }
            (results, used)
        };
        let items = results.matches.iter().filter_map(|item| {
            let file = results.files.get(item.file_index)?;
            Some(WorkspaceGrepMatch {
                path: file.relative_path(picker),
                line: item.line_number,
                text: truncate_line(&item.line_content),
                definition: item.is_definition,
                fuzzy_score: item.fuzzy_score,
            })
        }).collect::<Vec<_>>();
        (items, results.files_with_matches, results.total_files_searched, results.next_file_offset, used_mode)
    });
    match search {
        Ok((mut items, files_with_matches, total_files_searched, next_file_offset, mode))
            if !items.is_empty() || total_files_searched > 0 => {
                let overflowed = items.len() > request.limit;
                items.truncate(request.limit);
                Ok(GrepWorkspaceResult {
                    bounds: WorkspaceSearchBounds {
                        total: None,
                        total_at_least: items.len(),
                        next_cursor: (next_file_offset != 0).then_some(next_file_offset),
                        truncated: next_file_offset != 0 || overflowed || items.len() >= request.limit,
                        timed_out: false,
                    },
                    items,
                    files_with_matches,
                    total_files_searched,
                    mode,
                    engine: Some("fff".to_string()),
                    fallback_reason: None,
                })
            }
        Ok(_) => super::streaming_search::grep_workspace(request, "indexed search returned no searchable files"),
        Err(error) => super::streaming_search::grep_workspace(request, &error.to_string()),
    }
}

fn service_grep_mode(mode: WorkspaceSearchMode, pattern: &str) -> GrepMode {
    match mode {
        WorkspaceSearchMode::Regex => GrepMode::Regex,
        WorkspaceSearchMode::Fuzzy => GrepMode::Fuzzy,
        WorkspaceSearchMode::Plain => GrepMode::PlainText,
        WorkspaceSearchMode::Auto if has_regex_metacharacters(pattern) => GrepMode::Regex,
        WorkspaceSearchMode::Auto => GrepMode::PlainText,
    }
}

pub(super) async fn glob_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let timeout_ms = fff_timeout_ms(&arguments);
    let cancel = context.cancel.clone();
    run_fff_blocking("glob", timeout_ms, cancel, move || {
        glob_tool_sync(context, arguments, timeout_ms)
    })
    .await
}

fn glob_tool_sync(
    context: ToolContext,
    arguments: Value,
    timeout_ms: u64,
) -> anyhow::Result<ToolExecutionResult> {
    let query_text = required_string(&arguments, "pattern")?.trim().to_string();
    let raw_path = optional_string(&arguments, "path").unwrap_or_else(|| ".".to_string());
    let path = existing_project_path(&context, &raw_path)?;
    context.ensure_allowed("glob", &display_path(&context.cwd, &path))?;
    if !path.is_dir() {
        anyhow::bail!("glob path must be a directory: {}", path.display());
    }
    let limit = usize_arg(&arguments, "limit").unwrap_or(50).max(1);
    let offset = usize_arg(&arguments, "offset").unwrap_or(0);
    let result = context.services().workspace_search.find_files(&FindFilesRequest {
        root: path.clone(),
        query: query_text.clone(),
        offset,
        limit,
        control: neoism_agent_service_api::WorkspaceSearchRequestControl {
            timeout_ms,
            cancel: context.cancel.clone(),
        },
    }).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let items = result.items;
    let mut output = items
        .iter()
        .map(|item| {
            let status = item
                .git_status
                .as_ref()
                .map(|status| format!(" [{status}]"))
                .unwrap_or_default();
            format!("{}{}", item.path, status)
        })
        .collect::<Vec<_>>();
    if output.is_empty() {
        output.push("No files found".to_string());
    }

    Ok(ToolExecutionResult {
        title: if query_text.is_empty() { "Glob directory".to_string() } else { format!("Glob {query_text}") },
        output: output.join("\n"),
        metadata: Some(json!({
            "query": query_text,
            "offset": offset,
            "limit": limit,
            "total": result.bounds.total,
            "totalAtLeast": result.bounds.total_at_least,
            "count": items.len(),
            "truncated": result.bounds.truncated,
            "timedOut": result.bounds.timed_out,
            "timeout": timeout_ms,
            "engine": result.engine,
            "fallbackReason": result.fallback_reason,
            "path": display_path(&context.cwd, &path),
            "items": workspace_file_items_json(&items),
        })),
    })
}

pub(super) async fn grep_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let timeout_ms = fff_timeout_ms(&arguments);
    let cancel = context.cancel.clone();
    run_fff_blocking("grep", timeout_ms, cancel, move || {
        grep_tool_sync(context, arguments, timeout_ms)
    })
    .await
}

fn grep_tool_sync(
    context: ToolContext,
    arguments: Value,
    timeout_ms: u64,
) -> anyhow::Result<ToolExecutionResult> {
    if arguments.get("pattern").is_some_and(Value::is_array) {
        return multi_grep_tool_sync(context, arguments, timeout_ms);
    }
    let pattern = required_string(&arguments, "pattern")?.to_string();
    let limit = usize_arg(&arguments, "limit").unwrap_or(100).max(1);
    let raw_path = optional_string(&arguments, "path").unwrap_or_else(|| ".".to_string());
    let path = existing_project_path(&context, &raw_path)?;
    context.ensure_allowed("grep", &display_path(&context.cwd, &path))?;
    let include = optional_string(&arguments, "include");
    let exclude = merge_exclude(optional_string(&arguments, "exclude").as_deref());
    let context_lines = usize_arg(&arguments, "context").unwrap_or(0);
    let case_sensitive = arguments
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mode = grep_mode(&arguments, &pattern);
    // fff's fuzzy scorer computes `needle.len() as u16 * 16` (fuzzy_grep.rs)
    // and OVERFLOWS once the needle passes ~4095 bytes — panicking on an
    // INTERNAL fff worker thread that our `catch_unwind` cannot intercept, so
    // it hangs the tool instead of erroring. Never hand fuzzy an oversized
    // needle: skip the fuzzy fallback for long patterns and downgrade an
    // explicit fuzzy request to a plain literal search.
    const MAX_FUZZY_NEEDLE: usize = 1024;
    // fff's fuzzy needle = the query's joined non-constraint text tokens
    // (`FFFQuery::grep_text()`): our `!exclude` globs are constraints, so the
    // needle is the pattern plus any positive `include` glob. Bound on both.
    let fuzzy_safe =
        pattern.len() + include.as_deref().map_or(0, str::len) <= MAX_FUZZY_NEEDLE;
    let mode = if mode == GrepMode::Fuzzy && !fuzzy_safe {
        GrepMode::PlainText
    } else {
        mode
    };
    let alternation = literal_alternation_terms(&pattern);
    let patterns = alternation.clone().unwrap_or_else(|| vec![pattern.clone()]);
    let service_mode = if alternation.is_some() {
        WorkspaceSearchMode::Plain
    } else {
        workspace_mode(mode)
    };
    let result = context.services().workspace_search.grep(&GrepWorkspaceRequest {
        root: context.cwd.clone(),
        path: path.clone(),
        patterns,
        include: include.clone(),
        excludes: exclude.split([',', ' ']).map(str::trim).filter(|item| !item.is_empty()).map(|item| item.trim_start_matches('!').to_string()).collect(),
        context_lines,
        case_sensitive,
        mode: service_mode,
        limit,
        control: neoism_agent_service_api::WorkspaceSearchRequestControl {
            timeout_ms,
            cancel: context.cancel.clone(),
        },
    }).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let output = render_workspace_grep_output("Grep", &result.items, result.files_with_matches, limit);
    return Ok(ToolExecutionResult {
        title: format!("Grep {pattern}"),
        output,
        metadata: Some(json!({
            "pattern": pattern,
            "include": include,
            "exclude": exclude,
            "mode": result.mode,
            "engine": result.engine,
            "matches": result.items.len(),
            "filesWithMatches": result.files_with_matches,
            "totalFilesSearched": result.total_files_searched,
            "nextFileOffset": result.bounds.next_cursor.unwrap_or(0),
            "truncated": result.bounds.truncated,
            "timedOut": result.bounds.timed_out,
            "timeout": timeout_ms,
            "fallbackReason": result.fallback_reason,
            "items": workspace_grep_items_json(&result.items),
        })),
    })
}

fn multi_grep_tool_sync(
    context: ToolContext,
    arguments: Value,
    timeout_ms: u64,
) -> anyhow::Result<ToolExecutionResult> {
    let patterns = patterns_arg(&arguments)?;
    let raw_path = optional_string(&arguments, "path").unwrap_or_else(|| ".".to_string());
    let path = existing_project_path(&context, &raw_path)?;
    context.ensure_allowed("grep", &display_path(&context.cwd, &path))?;
    let limit = usize_arg(&arguments, "limit").unwrap_or(100).max(1);
    let context_lines = usize_arg(&arguments, "context").unwrap_or(0);
    let exclude = merge_exclude(optional_string(&arguments, "exclude").as_deref());
    let include = optional_string(&arguments, "include");
    let result = context.services().workspace_search.grep(&GrepWorkspaceRequest {
        root: context.cwd.clone(),
        path: path.clone(),
        patterns: patterns.clone(),
        include: include.clone(),
        excludes: exclude.split([',', ' ']).map(str::trim).filter(|item| !item.is_empty()).map(|item| item.trim_start_matches('!').to_string()).collect(),
        context_lines,
        case_sensitive: false,
        mode: WorkspaceSearchMode::Plain,
        limit,
        control: neoism_agent_service_api::WorkspaceSearchRequestControl {
            timeout_ms,
            cancel: context.cancel.clone(),
        },
    }).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let output = render_workspace_grep_output("Grep", &result.items, result.files_with_matches, limit);
    Ok(ToolExecutionResult {
        title: format!("Grep {}", patterns.join(", ")),
        output,
        metadata: Some(json!({
            "patterns": patterns,
            "include": include,
            "exclude": exclude,
            "mode": result.mode,
            "engine": result.engine,
            "matches": result.items.len(),
            "filesWithMatches": result.files_with_matches,
            "totalFilesSearched": result.total_files_searched,
            "nextFileOffset": result.bounds.next_cursor.unwrap_or(0),
            "truncated": result.bounds.truncated,
            "timedOut": result.bounds.timed_out,
            "timeout": timeout_ms,
            "fallbackReason": result.fallback_reason,
            "items": workspace_grep_items_json(&result.items),
        })),
    })
}

async fn run_fff_blocking<F>(
    tool: &'static str,
    timeout_ms: u64,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    operation: F,
) -> anyhow::Result<ToolExecutionResult>
where
    F: FnOnce() -> anyhow::Result<ToolExecutionResult> + Send + 'static,
{
    if cancel
        .as_ref()
        .is_some_and(|cancel| cancel.load(Ordering::SeqCst))
    {
        anyhow::bail!("{tool} aborted before start");
    }
    let started = Instant::now();
    if fff_perf_logging_enabled() {
        tracing::info!(tool, timeout_ms, "fff tool start");
    }
    let join = tokio::task::spawn_blocking(operation);
    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let result = tokio::select! {
        result = join => {
            result.with_context(|| format!("{tool} worker panicked"))?
        }
        _ = &mut timeout => {
            anyhow::bail!("{tool} timed out after {timeout_ms}ms; narrow the path/exclude pattern, lower the limit, or retry with a higher timeout")
        }
        _ = process::wait_for_cancel(cancel) => {
            anyhow::bail!("{tool} aborted")
        }
    };
    if fff_perf_logging_enabled() {
        match &result {
            Ok(output) => tracing::info!(
                tool,
                elapsed_ms = started.elapsed().as_millis() as u64,
                output_bytes = output.output.len(),
                title = %output.title,
                "fff tool finish"
            ),
            Err(error) => tracing::warn!(
                tool,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %error,
                "fff tool failed"
            ),
        }
    }
    result
}

fn fff_timeout_ms(arguments: &Value) -> u64 {
    usize_arg(arguments, "timeout")
        .map(|timeout| timeout as u64)
        .unwrap_or(DEFAULT_FFF_TIMEOUT_MS)
        .clamp(1_000, MAX_FFF_TIMEOUT_MS)
}

fn fallback_timeout_ms(outer_timeout_ms: u64) -> u64 {
    // The fallback runs inside the same spawn_blocking/timeout envelope as
    // FFF. Finish slightly before that outer deadline so partial results win
    // the race instead of being replaced by a generic timeout error.
    outer_timeout_ms.saturating_sub(500).max(1)
}

fn fff_perf_logging_enabled() -> bool {
    std::env::var_os("NEOISM_AGENT_PERF_LOG")
        .as_deref()
        .is_some_and(|value| {
            matches!(
                value.to_string_lossy().as_ref(),
                "1" | "true" | "TRUE" | "yes" | "YES"
            )
        })
}

fn merge_exclude(existing: Option<&str>) -> String {
    let mut parts = DEFAULT_EXCLUDES
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    if let Some(existing) = existing {
        parts.extend(
            existing
                .split([',', ' '])
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| item.trim_start_matches('!').to_string()),
        );
    }
    parts.join(" ")
}

fn grep_query_text(
    _cwd: &Path,
    path: &Path,
    root: &Path,
    include: Option<&str>,
    exclude: Option<&str>,
    pattern: &str,
) -> String {
    let mut parts = Vec::new();
    if path != root {
        let path_constraint = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if !path_constraint.is_empty() {
            parts.push(path_constraint);
        }
    }
    if let Some(include) = include {
        parts.push(include.to_string());
    }
    if let Some(exclude) = exclude {
        for item in exclude
            .split([',', ' '])
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if item.starts_with('!') {
                parts.push(item.to_string());
            } else {
                parts.push(format!("!{item}"));
            }
        }
    }
    let pattern = pattern.trim();
    if !pattern.is_empty() {
        parts.push(pattern.to_string());
    }
    parts.join(" ")
}

fn grep_mode(arguments: &Value, pattern: &str) -> GrepMode {
    match optional_string(arguments, "mode")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "regex" => GrepMode::Regex,
        "fuzzy" => GrepMode::Fuzzy,
        "plain" | "literal" | "text" => GrepMode::PlainText,
        _ if has_regex_metacharacters(pattern) => GrepMode::Regex,
        _ => GrepMode::PlainText,
    }
}

/// Split a pure literal alternation (`foo|bar|baz`) into its terms. Returns
/// `None` when the pattern isn't a multi-branch alternation or any branch
/// carries regex metacharacters — then it's a real regex and must stay on the
/// regex engine. A literal OR is routed to multi-pattern search, which avoids
/// the wide-regex "multiply with overflow" panic and is the faster engine.
fn literal_alternation_terms(pattern: &str) -> Option<Vec<String>> {
    if !pattern.contains('|') {
        return None;
    }
    let terms: Vec<String> = pattern
        .split('|')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect();
    if terms.len() < 2 || terms.iter().any(|term| has_regex_metacharacters(term)) {
        return None;
    }
    Some(terms)
}

fn mode_label(mode: GrepMode) -> &'static str {
    match mode {
        GrepMode::PlainText => "plain",
        GrepMode::Regex => "regex",
        GrepMode::Fuzzy => "fuzzy",
    }
}

fn workspace_mode(mode: GrepMode) -> WorkspaceSearchMode {
    match mode {
        GrepMode::PlainText => WorkspaceSearchMode::Plain,
        GrepMode::Regex => WorkspaceSearchMode::Regex,
        GrepMode::Fuzzy => WorkspaceSearchMode::Fuzzy,
    }
}

fn workspace_grep_items_json(items: &[WorkspaceGrepMatch]) -> Value {
    Value::Array(items.iter().map(|item| json!({
        "path": item.path,
        "line": item.line,
        "text": item.text,
        "definition": item.definition,
        "fuzzyScore": item.fuzzy_score,
    })).collect())
}

fn workspace_file_items_json(items: &[WorkspaceFileMatch]) -> Value {
    Value::Array(items.iter().map(|item| json!({
        "path": item.path,
        "score": item.score,
        "gitStatus": item.git_status,
        "size": item.size,
        "modified": item.modified,
    })).collect())
}

fn render_workspace_grep_output(
    label: &str,
    items: &[WorkspaceGrepMatch],
    files_with_matches: usize,
    limit: usize,
) -> String {
    if items.is_empty() {
        return "No files found".to_string();
    }
    let mut output = vec![format!("{label}: Found {} matches in {files_with_matches} files", items.len())];
    let mut current = "";
    for item in items {
        if current != item.path {
            if !current.is_empty() { output.push(String::new()); }
            current = &item.path;
            output.push(format!("{}:", item.path));
        }
        let marker = if item.definition { " [def]" } else { "" };
        output.push(format!("  Line {}{marker}: {}", item.line, item.text));
    }
    if items.len() >= limit {
        output.push(String::new());
        output.push(format!("(Results may be truncated: showing first {limit} matches. Narrow the query or use nextFileOffset metadata.)"));
    }
    output.join("\n")
}

fn patterns_arg(arguments: &Value) -> anyhow::Result<Vec<String>> {
    let Some(raw) = arguments.get("pattern") else {
        anyhow::bail!("tool argument pattern is required");
    };
    let patterns = if let Some(array) = raw.as_array() {
        array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else if let Some(s) = raw.as_str() {
        s.split([',', '\n'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if patterns.is_empty() {
        anyhow::bail!("tool argument pattern must contain at least one pattern");
    }
    Ok(patterns)
}

fn git_status_label(status: git2::Status) -> String {
    if status.is_wt_new() {
        "untracked"
    } else if status.is_wt_modified() || status.is_index_modified() {
        "modified"
    } else if status.is_index_new() {
        "staged"
    } else if status.is_wt_deleted() || status.is_index_deleted() {
        "deleted"
    } else if status.is_index_renamed() || status.is_wt_renamed() {
        "renamed"
    } else {
        "tracked"
    }
    .to_string()
}
