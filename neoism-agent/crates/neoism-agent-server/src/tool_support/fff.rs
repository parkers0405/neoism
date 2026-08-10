use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use fff_search::{
    has_regex_metacharacters, AiGrepConfig, FilePicker, FuzzySearchOptions, GrepMode,
    GrepSearchOptions, PaginationArgs, QueryParser,
};
use serde::Serialize;
use serde_json::{json, Value};

use super::args::{optional_string, required_string, usize_arg};
use super::paths::{
    directory_entries, display_path, existing_project_path, truncate_line,
};
use super::{process, ToolContext, ToolExecutionResult};

const DEFAULT_FFF_TIMEOUT_MS: u64 = 45_000;
const MAX_FFF_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_EXCLUDES: &[&str] = &[
    ".claude/worktrees",
    ".codex",
    ".neoism/cache",
    "target",
    "node_modules",
    "dist",
    ".tmp",
];

pub(super) fn warm(root: &Path) {
    if let Err(error) = crate::picker_registry::warm(root) {
        tracing::warn!(
            target: "neoism_agent::fff",
            root = %root.display(),
            %error,
            "failed to warm shared FFF picker"
        );
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FffFindItem {
    path: String,
    score: i32,
    git_status: Option<String>,
    size: u64,
    modified: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FffGrepItem {
    path: String,
    line: u64,
    text: String,
    definition: bool,
    fuzzy_score: Option<u16>,
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
    if query_text.is_empty() {
        return glob_directory_fallback(&context, &path, limit, offset, timeout_ms);
    }
    let (items, total_matched) = with_picker(&path, |picker| {
        let parser = QueryParser::default();
        let mut results = picker.fuzzy_search(
            &parser.parse(&query_text),
            None,
            FuzzySearchOptions {
                max_threads: 0,
                current_file: None,
                project_path: Some(&path),
                pagination: PaginationArgs { offset, limit },
                ..Default::default()
            },
        );
        // An oversized / natural-language query fuzzy-matches no file PATH
        // (paths don't contain sentences), so it came back empty. Retry on
        // the single most distinctive token so we still surface the relevant
        // files instead of returning nothing.
        if results.items.is_empty() {
            if let Some(token) = query_text
                .split_whitespace()
                .filter(|token| token.len() >= 3)
                .max_by_key(|token| token.len())
            {
                if token != query_text.as_str() {
                    results = picker.fuzzy_search(
                        &parser.parse(token),
                        None,
                        FuzzySearchOptions {
                            max_threads: 0,
                            current_file: None,
                            project_path: Some(&path),
                            pagination: PaginationArgs { offset, limit },
                            ..Default::default()
                        },
                    );
                }
            }
        }
        let items = results
            .items
            .iter()
            .zip(results.scores.iter())
            .map(|(item, score)| FffFindItem {
                path: item.relative_path(picker),
                score: score.total,
                git_status: item.git_status.map(git_status_label),
                size: item.size,
                modified: item.modified,
            })
            .collect::<Vec<_>>();
        (items, results.total_matched)
    })?;
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
        title: format!("Glob {query_text}"),
        output: output.join("\n"),
        metadata: Some(json!({
            "query": query_text,
            "offset": offset,
            "limit": limit,
            "total": total_matched,
            "count": items.len(),
            "truncated": offset.saturating_add(items.len()) < total_matched,
            "timeout": timeout_ms,
            "engine": "fff",
            "items": items,
        })),
    })
}

fn glob_directory_fallback(
    context: &ToolContext,
    path: &Path,
    limit: usize,
    offset: usize,
    timeout_ms: u64,
) -> anyhow::Result<ToolExecutionResult> {
    let entries = directory_entries(path)?;
    let items = entries
        .iter()
        .skip(offset)
        .take(limit)
        .map(|entry| FffFindItem {
            path: entry.clone(),
            score: 0,
            git_status: None,
            size: 0,
            modified: 0,
        })
        .collect::<Vec<_>>();
    let mut output = items
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    if output.is_empty() {
        output.push("No files found".to_string());
    }
    let total = entries.len();
    Ok(ToolExecutionResult {
        title: "Glob directory".to_string(),
        output: output.join("\n"),
        metadata: Some(json!({
            "query": "",
            "offset": offset,
            "limit": limit,
            "total": total,
            "count": items.len(),
            "truncated": offset.saturating_add(items.len()) < total,
            "timeout": timeout_ms,
            "engine": "directory",
            "path": display_path(&context.cwd, path),
            "items": items,
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
    let root = grep_root(&path);
    // A pure literal alternation (`a|b|c`) is an OR search. Route it through
    // multi-pattern (Aho-Corasick) instead of compiling one wide regex: fff
    // can hit "attempt to multiply with overflow" building/scoring a broad
    // regex alternation (~35 branches), and multi-pattern is the correct and
    // faster engine for a literal OR anyway.
    let alternation = literal_alternation_terms(&pattern);
    // Constraint-only query (path + include + exclude, empty search term) for
    // the multi-pattern route; the regex/plain route appends the pattern.
    let constraint_text = grep_query_text(
        &context.cwd,
        &path,
        &root,
        include.as_deref(),
        Some(exclude.as_str()),
        "",
    );
    let query_text = grep_query_text(
        &context.cwd,
        &path,
        &root,
        include.as_deref(),
        Some(exclude.as_str()),
        &pattern,
    );
    // Bound each grep the way fff intends (and opencode does): return
    // PARTIAL results at a time budget instead of grinding to the outer
    // hard-kill with nothing. Kept a hair under the tool timeout so fff
    // self-bounds first; also hand fff the cancel flag so it stops mid-
    // search on abort rather than only when the outer select! fires.
    let grep_budget_ms = timeout_ms.saturating_sub(2_000).max(500);
    let abort = context.cancel.clone();
    let (
        mut items,
        files_with_matches,
        total_files_searched,
        next_file_offset,
        used_mode,
    ) = with_picker(&root, |picker| {
        let parser = QueryParser::<AiGrepConfig>::new(AiGrepConfig);
        let options = |grep_mode: GrepMode| GrepSearchOptions {
            page_limit: limit,
            mode: grep_mode,
            smart_case: !case_sensitive,
            before_context: context_lines,
            after_context: context_lines,
            classify_definitions: true,
            trim_whitespace: false,
            time_budget_ms: grep_budget_ms,
            abort_signal: abort.clone(),
            ..Default::default()
        };
        if let Some(terms) = &alternation {
            let constraints = parser.parse(&constraint_text);
            let refs: Vec<&str> = terms.iter().map(String::as_str).collect();
            let results = picker.multi_grep(
                &refs,
                &constraints.constraints,
                &options(GrepMode::PlainText),
            );
            return (
                grep_items(picker, &results),
                results.files_with_matches,
                results.total_files_searched,
                results.next_file_offset,
                "multi",
            );
        }
        let query = parser.parse(&query_text);
        let mut results = picker.grep(&query, &options(mode));
        let used_mode =
            if results.matches.is_empty() && mode != GrepMode::Fuzzy && fuzzy_safe {
                results = picker.grep(&query, &options(GrepMode::Fuzzy));
                if results.matches.is_empty() {
                    mode_label(mode)
                } else {
                    "fuzzy"
                }
            } else {
                mode_label(mode)
            };
        (
            grep_items(picker, &results),
            results.files_with_matches,
            results.total_files_searched,
            results.next_file_offset,
            used_mode,
        )
    })?;
    let overflowed_limit = items.len() > limit;
    items.truncate(limit);
    let output = render_grep_output("Grep", &items, files_with_matches, limit);

    Ok(ToolExecutionResult {
        title: format!("Grep {pattern}"),
        output,
        metadata: Some(json!({
            "pattern": pattern,
            "query": query_text,
            "include": include,
            "exclude": exclude,
            "mode": used_mode,
            "engine": "fff",
            "matches": items.len(),
            "filesWithMatches": files_with_matches,
            "totalFilesSearched": total_files_searched,
            "nextFileOffset": next_file_offset,
            "truncated": next_file_offset != 0 || overflowed_limit || items.len() >= limit,
            "timeout": timeout_ms,
            "items": items,
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
    let root = grep_root(&path);
    let constraint_query = grep_query_text(
        &context.cwd,
        &path,
        &root,
        include.as_deref(),
        Some(exclude.as_str()),
        "",
    );
    let parser = QueryParser::<AiGrepConfig>::new(AiGrepConfig);
    let query = parser.parse(&constraint_query);
    let refs = patterns.iter().map(String::as_str).collect::<Vec<_>>();
    // Use the same partial-result time budget and cancellation propagation as
    // the single-pattern grep path.
    let grep_budget_ms = timeout_ms.saturating_sub(2_000).max(500);
    let abort = context.cancel.clone();
    let (mut items, files_with_matches, total_files_searched, next_file_offset) =
        with_picker(&root, |picker| {
            let results = picker.multi_grep(
                &refs,
                &query.constraints,
                &GrepSearchOptions {
                    page_limit: limit,
                    before_context: context_lines,
                    after_context: context_lines,
                    classify_definitions: true,
                    trim_whitespace: false,
                    time_budget_ms: grep_budget_ms,
                    abort_signal: abort.clone(),
                    ..Default::default()
                },
            );
            (
                grep_items(picker, &results),
                results.files_with_matches,
                results.total_files_searched,
                results.next_file_offset,
            )
        })?;
    let overflowed_limit = items.len() > limit;
    items.truncate(limit);
    let output = render_grep_output("Grep", &items, files_with_matches, limit);

    Ok(ToolExecutionResult {
        title: format!("Grep {}", patterns.join(", ")),
        output,
        metadata: Some(json!({
            "patterns": patterns,
            "include": include,
            "exclude": exclude,
            "engine": "fff",
            "matches": items.len(),
            "filesWithMatches": files_with_matches,
            "totalFilesSearched": total_files_searched,
            "nextFileOffset": next_file_offset,
            "truncated": next_file_offset != 0 || overflowed_limit || items.len() >= limit,
            "timeout": timeout_ms,
            "items": items,
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

fn with_picker<T>(
    root: &Path,
    operation: impl FnOnce(&FilePicker) -> T,
) -> anyhow::Result<T> {
    crate::picker_registry::with_picker(root, operation)
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

fn grep_root(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
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
            .to_string();
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

fn grep_items(
    picker: &FilePicker,
    results: &fff_search::GrepResult<'_>,
) -> Vec<FffGrepItem> {
    results
        .matches
        .iter()
        .filter_map(|item| {
            let file = results.files.get(item.file_index)?;
            Some(FffGrepItem {
                path: file.relative_path(picker),
                line: item.line_number,
                text: truncate_line(&item.line_content),
                definition: item.is_definition,
                fuzzy_score: item.fuzzy_score,
            })
        })
        .collect()
}

fn render_grep_output(
    label: &str,
    items: &[FffGrepItem],
    files_with_matches: usize,
    limit: usize,
) -> String {
    if items.is_empty() {
        return "No files found".to_string();
    }
    let mut output = vec![format!(
        "{label}: Found {} matches in {files_with_matches} files",
        items.len()
    )];
    let mut current = "";
    for item in items {
        if current != item.path {
            if current != "" {
                output.push(String::new());
            }
            current = &item.path;
            output.push(format!("{}:", item.path));
        }
        let marker = if item.definition { " [def]" } else { "" };
        output.push(format!("  Line {}{marker}: {}", item.line, item.text));
    }
    if items.len() >= limit {
        output.push(String::new());
        output.push(format!(
            "(Results may be truncated: showing first {limit} matches. Narrow the query or use nextFileOffset metadata.)"
        ));
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
