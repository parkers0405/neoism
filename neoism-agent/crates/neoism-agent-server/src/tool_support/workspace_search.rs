use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use neoism_agent_service_api::{
    FindFilesRequest, GrepWorkspaceRequest, WorkspaceFileMatch, WorkspaceGrepMatch,
    WorkspaceSearchMode,
};
use serde_json::{json, Value};

use super::args::{optional_string, required_string, usize_arg};
use super::paths::{display_path, existing_project_path};
use super::{process, ToolContext, ToolExecutionResult};

const DEFAULT_SEARCH_TIMEOUT_MS: u64 = 45_000;
const MAX_SEARCH_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    ".claude/worktrees",
    ".codex",
    ".neoism/cache",
    "target",
    "node_modules",
    "dist",
    ".tmp",
];

pub(super) async fn glob_tool(
    mut context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let timeout_ms = search_timeout_ms(&arguments);
    let cancel = context.cancel.clone();
    let worker_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    context.cancel = Some(worker_cancel.clone());
    run_search_blocking("glob", timeout_ms, cancel, worker_cancel, move || {
        glob_tool_sync(context, arguments, timeout_ms)
    })
    .await
}

fn glob_tool_sync(
    context: ToolContext,
    arguments: Value,
    timeout_ms: u64,
) -> anyhow::Result<ToolExecutionResult> {
    let query = required_string(&arguments, "pattern")?.trim().to_string();
    let raw_path = optional_string(&arguments, "path").unwrap_or_else(|| ".".into());
    let path = existing_project_path(&context, &raw_path)?;
    context.ensure_allowed("glob", &display_path(&context.cwd, &path))?;
    if !path.is_dir() {
        anyhow::bail!("glob path must be a directory: {}", path.display());
    }
    let limit = usize_arg(&arguments, "limit").unwrap_or(50).max(1);
    let offset = usize_arg(&arguments, "offset").unwrap_or(0);
    let include_hidden = arguments
        .get("includeHidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let result = context
        .services()
        .workspace_search
        .find_files(&FindFilesRequest {
            root: path.clone(),
            query: query.clone(),
            include_hidden,
            offset,
            limit,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl {
                timeout_ms,
                cancel: context.cancel.clone(),
            },
        })
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut output = result
        .items
        .iter()
        .map(|item| {
            let status = item
                .git_status
                .as_ref()
                .map(|status| format!(" [{status}]"))
                .unwrap_or_default();
            format!("{}{status}", item.path)
        })
        .collect::<Vec<_>>();
    if output.is_empty() {
        output.push("No files found".into());
    }
    Ok(ToolExecutionResult {
        title: if query.is_empty() {
            "Glob directory".into()
        } else {
            format!("Glob {query}")
        },
        output: output.join("\n"),
        metadata: Some(json!({
            "query": query, "includeHidden": include_hidden, "offset": offset, "limit": limit,
            "total": result.bounds.total, "totalAtLeast": result.bounds.total_at_least,
            "count": result.items.len(), "truncated": result.bounds.truncated,
            "timedOut": result.bounds.timed_out, "timeout": timeout_ms,
            "engine": result.engine, "fallbackReason": result.fallback_reason,
            "path": display_path(&context.cwd, &path), "items": file_items_json(&result.items),
        })),
    })
}

pub(super) async fn grep_tool(
    mut context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let timeout_ms = search_timeout_ms(&arguments);
    let cancel = context.cancel.clone();
    let worker_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    context.cancel = Some(worker_cancel.clone());
    run_search_blocking("grep", timeout_ms, cancel, worker_cancel, move || {
        grep_tool_sync(context, arguments, timeout_ms)
    })
    .await
}

/// Resolve the public tri-state option before crossing the boolean service API.
/// In particular, explicit false must not be overridden by uppercase patterns.
fn resolved_case_sensitive(arguments: &Value, patterns: &[String]) -> bool {
    arguments
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            patterns
                .iter()
                .flat_map(|p| p.chars())
                .any(char::is_uppercase)
        })
}

fn grep_tool_sync(
    context: ToolContext,
    arguments: Value,
    timeout_ms: u64,
) -> anyhow::Result<ToolExecutionResult> {
    let patterns = patterns_arg(&arguments)?;
    let original_pattern = patterns.join(", ");
    let raw_path = optional_string(&arguments, "path").unwrap_or_else(|| ".".into());
    let path = existing_project_path(&context, &raw_path)?;
    context.ensure_allowed("grep", &display_path(&context.cwd, &path))?;
    let limit = usize_arg(&arguments, "limit").unwrap_or(100).max(1);
    let context_lines = usize_arg(&arguments, "context").unwrap_or(0);
    let include = optional_string(&arguments, "include");
    let include_hidden = arguments
        .get("includeHidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let excludes = merged_excludes(optional_string(&arguments, "exclude").as_deref());
    let case_sensitive = resolved_case_sensitive(&arguments, &patterns);
    let mode = if arguments["pattern"].is_array() || patterns.len() > 1 {
        // Arrays are literal alternatives, including regex metacharacters.
        WorkspaceSearchMode::Plain
    } else {
        requested_mode(&arguments, &patterns[0])
    };
    // Reject unsupported fuzzy sizes rather than silently changing semantics.
    anyhow::ensure!(
        mode != WorkspaceSearchMode::Fuzzy || patterns[0].len() <= 1024,
        "fuzzy grep pattern must be at most 1024 bytes; use plain or regex for longer patterns"
    );
    let cursor = optional_string(&arguments, "cursor");
    let result = context
        .services()
        .workspace_search
        .grep(&GrepWorkspaceRequest {
            root: context.cwd.clone(),
            path,
            patterns: patterns.clone(),
            include: include.clone(),
            include_hidden,
            excludes: excludes.clone(),
            context_lines,
            case_sensitive,
            mode,
            cursor: cursor.clone(),
            limit,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl {
                timeout_ms,
                cancel: context.cancel.clone(),
            },
        })
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(ToolExecutionResult {
        title: format!("Grep {original_pattern}"),
        output: render_grep(&result),
        metadata: Some(json!({
            "patterns": patterns, "include": include, "includeHidden": include_hidden, "exclude": excludes.join(" "),
            "mode": result.mode, "engine": result.engine, "matches": result.items.len(),
            "filesWithMatches": result.files_with_matches,
            "totalFilesSearched": result.total_files_searched,
            "cursor": cursor, "nextCursor": result.next_cursor, "limit": limit,
            "pagination": "match-cursor", "total": result.bounds.total,
            "totalAtLeast": result.bounds.total_at_least,
            "contextLines": result.items.iter().map(|m| m.context_before.len() + m.context_after.len()).sum::<usize>(),
            "truncated": result.bounds.truncated, "timedOut": result.bounds.timed_out,
            "timeout": timeout_ms, "fallbackReason": result.fallback_reason,
            "items": grep_items_json(&result.items),
        })),
    })
}

async fn run_search_blocking<F>(
    tool: &'static str,
    timeout_ms: u64,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    worker_cancel: Arc<std::sync::atomic::AtomicBool>,
    operation: F,
) -> anyhow::Result<ToolExecutionResult>
where
    F: FnOnce() -> anyhow::Result<ToolExecutionResult> + Send + 'static,
{
    if cancel
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::SeqCst))
    {
        anyhow::bail!("{tool} aborted before start");
    }
    struct StopWorker(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for StopWorker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let _stop_worker = StopWorker(worker_cancel.clone());
    let started = Instant::now();
    let join = tokio::task::spawn_blocking(operation);
    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let result = tokio::select! {
        result = join => result.with_context(|| format!("{tool} worker panicked"))?,
        _ = &mut timeout => {
            worker_cancel.store(true, Ordering::SeqCst);
            anyhow::bail!("{tool} timed out after {timeout_ms}ms; narrow the path/exclude pattern, lower the limit, or retry with a higher timeout")
        },
        _ = process::wait_for_cancel(cancel) => {
            worker_cancel.store(true, Ordering::SeqCst);
            anyhow::bail!("{tool} aborted")
        },
    };
    if perf_logging_enabled() {
        match &result {
            Ok(output) => tracing::info!(
                tool,
                elapsed_ms = started.elapsed().as_millis() as u64,
                output_bytes = output.output.len(),
                "workspace search finish"
            ),
            Err(error) => {
                tracing::warn!(tool, elapsed_ms = started.elapsed().as_millis() as u64, error = %error, "workspace search failed")
            }
        }
    }
    result
}

fn requested_mode(arguments: &Value, pattern: &str) -> WorkspaceSearchMode {
    match optional_string(arguments, "mode")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "regex" => WorkspaceSearchMode::Regex,
        "fuzzy" => WorkspaceSearchMode::Fuzzy,
        "plain" | "literal" | "text" => WorkspaceSearchMode::Plain,
        _ if has_regex_metacharacters(pattern) => WorkspaceSearchMode::Regex,
        _ => WorkspaceSearchMode::Plain,
    }
}

fn has_regex_metacharacters(pattern: &str) -> bool {
    let mut escaped = false;
    for ch in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if matches!(
            ch,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
        ) {
            return true;
        }
    }
    false
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
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    } else if let Some(value) = raw.as_str() {
        // Preserve a single regex or literal pattern; comma/newline lists are
        // accepted for compatibility with multi-pattern callers.
        if value.contains('\n') {
            value
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        } else {
            vec![value.to_string()]
        }
    } else {
        Vec::new()
    };
    if patterns.is_empty() {
        anyhow::bail!("tool argument pattern must contain at least one pattern");
    }
    Ok(patterns)
}

fn merged_excludes(extra: Option<&str>) -> Vec<String> {
    DEFAULT_EXCLUDES
        .iter()
        .copied()
        .chain(extra.into_iter().flat_map(|value| value.split([',', ' '])))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.trim_start_matches('!').to_string())
        .collect()
}

fn search_timeout_ms(arguments: &Value) -> u64 {
    usize_arg(arguments, "timeout")
        .map(|v| v as u64)
        .unwrap_or(DEFAULT_SEARCH_TIMEOUT_MS)
        .clamp(1_000, MAX_SEARCH_TIMEOUT_MS)
}

fn perf_logging_enabled() -> bool {
    std::env::var_os("NEOISM_AGENT_PERF_LOG")
        .as_deref()
        .is_some_and(|v| {
            matches!(
                v.to_string_lossy().as_ref(),
                "1" | "true" | "TRUE" | "yes" | "YES"
            )
        })
}

fn file_items_json(items: &[WorkspaceFileMatch]) -> Value {
    Value::Array(items.iter().map(|item| json!({
    "path": item.path, "score": item.score, "gitStatus": item.git_status, "size": item.size, "modified": item.modified,
})).collect())
}

fn grep_items_json(items: &[WorkspaceGrepMatch]) -> Value {
    Value::Array(items.iter().map(|item| json!({
    "path": item.path, "line": item.line, "text": item.text, "definition": item.definition, "fuzzyScore": item.fuzzy_score,
    "contextBefore": item.context_before, "contextAfter": item.context_after,
})).collect())
}

fn render_grep(result: &neoism_agent_service_api::GrepWorkspaceResult) -> String {
    let items = &result.items;
    let mut output = vec![if items.is_empty() {
        if result.bounds.truncated {
            "Grep: No matches in this partial page".into()
        } else {
            "Grep: No matches found".into()
        }
    } else {
        format!(
            "Grep: Found {} matches in {} files (this page)",
            items.len(),
            result.files_with_matches
        )
    }];
    let mut current = "";
    for item in items {
        if current != item.path {
            if !current.is_empty() {
                output.push(String::new());
            }
            current = &item.path;
            output.push(format!("{}:", item.path));
        }
        for (index, text) in item.context_before.iter().enumerate() {
            output.push(format!(
                "  Context {}: {text}",
                item.line
                    .saturating_sub((item.context_before.len() - index) as u64)
            ));
        }
        let marker = if item.definition { " [def]" } else { "" };
        output.push(format!("  Line {}{marker}: {}", item.line, item.text));
        for (index, text) in item.context_after.iter().enumerate() {
            output.push(format!(
                "  Context {}: {text}",
                item.line + index as u64 + 1
            ));
        }
    }
    if let Some(cursor) = &result.next_cursor {
        output.push(format!("(More results: pass cursor={cursor} with the same query/path/options. Limit may change; pages respect match and output budgets.)"));
    } else if result.bounds.truncated {
        output.push("(Partial results; no safe continuation cursor. Retry from the input cursor with a higher timeout or narrower scope.)".into());
    }
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_case_option_explicit_booleans_override_smartcase() {
        for patterns in [
            vec!["WORKSPACE".into()],
            vec!["workspace".into()],
            vec!["workspace".into(), "OTHER".into()],
        ] {
            assert!(!resolved_case_sensitive(
                &json!({"caseSensitive": false}),
                &patterns
            ));
            assert!(resolved_case_sensitive(
                &json!({"caseSensitive": true}),
                &patterns
            ));
            assert_eq!(
                resolved_case_sensitive(&json!({}), &patterns),
                patterns
                    .iter()
                    .flat_map(|p| p.chars())
                    .any(char::is_uppercase),
            );
        }
    }

    #[test]
    fn grep_render_preserves_context_and_cursor_advice_below_central_output_caps() {
        let result = neoism_agent_service_api::GrepWorkspaceResult {
            next_cursor: Some("opaque-next-page".into()),
            items: (0..8)
                .map(|index| WorkspaceGrepMatch {
                    path: "file.txt".into(),
                    line: 101 + index,
                    text: "needle".into(),
                    definition: false,
                    fuzzy_score: None,
                    context_before: vec!["ctx".into(); 100],
                    context_after: vec!["ctx".into(); 100],
                })
                .collect(),
            files_with_matches: 1,
            total_files_searched: 1,
            bounds: neoism_agent_service_api::WorkspaceSearchBounds {
                truncated: true,
                ..Default::default()
            },
            mode: "plain".into(),
            engine: Some("fff".into()),
            fallback_reason: None,
        };
        let output = render_grep(&result);
        assert!(output.len() <= 51_200);
        assert!(output.lines().count() <= 2_000);
        assert!(output.contains("Found 8 matches"));
        assert_eq!(
            output
                .lines()
                .filter(|line| line.starts_with("  Context "))
                .count(),
            1_600
        );
        assert!(output.contains("pass cursor=opaque-next-page"));
    }

    #[tokio::test]
    async fn timeout_and_cancellation_stop_blocking_search_workers() {
        for timeout in [true, false] {
            let user_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (worker, finished) = (worker_cancel.clone(), stopped.clone());
            let user = user_cancel.clone();
            let trigger = tokio::spawn(async move {
                if !timeout {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    user.store(true, Ordering::SeqCst);
                }
            });
            let result = run_search_blocking(
                "grep",
                if timeout { 10 } else { 1_000 },
                Some(user_cancel.clone()),
                worker_cancel,
                move || {
                    while !worker.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    finished.store(true, Ordering::SeqCst);
                    anyhow::bail!("worker aborted")
                },
            )
            .await;
            trigger.await.unwrap();
            assert!(result.is_err());
            for _ in 0..100 {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            assert!(
                stopped.load(Ordering::SeqCst),
                "detached worker must observe stop signal"
            );
            if timeout {
                assert!(
                    !user_cancel.load(Ordering::SeqCst),
                    "worker timeout must not poison caller cancellation flag"
                );
            }
        }
    }
}
