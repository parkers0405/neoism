//! Bounded, non-indexing search for roots that FFF deliberately cannot index.
//!
//! FFF is the fast path for project-sized roots. Its home/filesystem-root
//! guard is intentional: indexing and watching either root is expensive and
//! error-prone. These routines use ripgrep's `ignore`/`globset` traversal
//! stack directly, stop as soon as the requested page is full, and carry the
//! same timeout/cancellation contract as the indexed tools.

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use fff_search::GrepMode;
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::json;

use super::paths::truncate_line;
use super::{ToolContext, ToolExecutionResult};

const MAX_SEARCH_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamingFindItem {
    path: String,
    score: i32,
    git_status: Option<String>,
    size: u64,
    modified: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamingGrepItem {
    path: String,
    line: u64,
    text: String,
    definition: bool,
    fuzzy_score: Option<u16>,
}

pub(super) fn root_requires_fallback(root: &Path) -> bool {
    root_requires_fallback_for_home(root, dirs::home_dir().as_deref())
}

fn root_requires_fallback_for_home(root: &Path, home: Option<&Path>) -> bool {
    let root = crate::windows_process::canonicalize_path_lossy(root);
    if crate::windows_process::is_filesystem_root(&root) {
        return true;
    }
    home.map(crate::windows_process::canonicalize_path_lossy)
        .is_some_and(|home| home == root)
}

pub(super) struct GlobRequest<'a> {
    pub(super) context: &'a ToolContext,
    pub(super) path: &'a Path,
    pub(super) query: &'a str,
    pub(super) limit: usize,
    pub(super) offset: usize,
    pub(super) timeout_ms: u64,
    pub(super) fallback_reason: String,
}

pub(super) fn glob(request: GlobRequest<'_>) -> anyhow::Result<ToolExecutionResult> {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(request.timeout_ms);
    let matcher = PathMatcher::query(request.query)?;
    let excluded = PathMatcher::patterns(super::fff::DEFAULT_EXCLUDES.iter().copied())?;
    let root = request.path.to_path_buf();
    let root_for_filter = root.clone();
    let excluded_for_filter = excluded.clone();
    let mut builder = WalkBuilder::new(&root);
    builder
        // Matches `rg --files`: hidden paths are omitted unless callers use
        // grep, which intentionally mirrors `rg --hidden` below.
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .filter_entry(move |entry| {
            relative_search_path(&root_for_filter, entry.path())
                .is_none_or(|relative| !excluded_for_filter.matches(&relative))
        });

    let wanted = request
        .offset
        .saturating_add(request.limit)
        .saturating_add(1);
    let mut matched = Vec::new();
    let mut timed_out = false;
    let mut cancelled = false;
    for entry in builder.build() {
        if is_cancelled(request.context) {
            cancelled = true;
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(relative) = relative_search_path(&root, entry.path()) else {
            continue;
        };
        if !matcher.matches(&relative) {
            continue;
        }
        let metadata = entry.metadata().ok();
        matched.push(StreamingFindItem {
            path: relative,
            score: 0,
            git_status: None,
            size: metadata.as_ref().map_or(0, std::fs::Metadata::len),
            modified: metadata
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |modified| modified.as_secs()),
        });
        if matched.len() >= wanted {
            break;
        }
    }
    if cancelled {
        anyhow::bail!("glob aborted");
    }

    let discovered = matched.len();
    let mut items = matched
        .into_iter()
        .skip(request.offset)
        .take(request.limit)
        .collect::<Vec<_>>();
    let truncated = timed_out || discovered > request.offset.saturating_add(items.len());
    let mut output = items
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    if output.is_empty() {
        output.push(if timed_out {
            "No files found before the search deadline".to_string()
        } else {
            "No files found".to_string()
        });
    } else if timed_out {
        output.push(String::new());
        output.push("(Search stopped at the deadline; narrow the path or pattern for complete results.)".to_string());
    }
    // Avoid retaining excess allocation from a broad walk in tool metadata.
    items.shrink_to_fit();

    Ok(ToolExecutionResult {
        title: format!("Glob {}", request.query),
        output: output.join("\n"),
        metadata: Some(json!({
            "query": request.query,
            "offset": request.offset,
            "limit": request.limit,
            "totalAtLeast": discovered,
            "count": items.len(),
            "truncated": truncated,
            "timedOut": timed_out,
            "timeout": request.timeout_ms,
            "engine": "ripgrep",
            "fallbackReason": request.fallback_reason,
            "items": items,
        })),
    })
}

pub(super) struct GrepRequest<'a> {
    pub(super) context: &'a ToolContext,
    pub(super) path: &'a Path,
    pub(super) patterns: &'a [String],
    pub(super) include: Option<&'a str>,
    pub(super) exclude: &'a str,
    pub(super) context_lines: usize,
    pub(super) case_sensitive: bool,
    pub(super) mode: GrepMode,
    pub(super) limit: usize,
    pub(super) timeout_ms: u64,
    pub(super) fallback_reason: String,
}

pub(super) fn grep(request: GrepRequest<'_>) -> anyhow::Result<ToolExecutionResult> {
    let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
    let matcher =
        LineMatcher::new(request.patterns, request.mode, request.case_sensitive)?;
    let include = request.include.map(PathMatcher::pattern).transpose()?;
    let excluded = PathMatcher::patterns(
        request
            .exclude
            .split([',', ' '])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| item.trim_start_matches('!')),
    )?;
    let root = if request.path.is_dir() {
        request.path.to_path_buf()
    } else {
        request.path.parent().unwrap_or(request.path).to_path_buf()
    };

    let mut items = Vec::new();
    let mut files_with_matches = HashSet::new();
    let mut total_files_searched = 0usize;
    let mut timed_out = false;
    let mut cancelled = false;
    let collect_limit = request.limit.saturating_add(1);

    if request.path.is_file() {
        search_one_file(
            request.path,
            &root,
            &matcher,
            include.as_ref(),
            &excluded,
            request.context_lines,
            collect_limit,
            deadline,
            request.context,
            &mut items,
            &mut files_with_matches,
            &mut total_files_searched,
            &mut timed_out,
            &mut cancelled,
        )?;
    } else {
        let root_for_filter = root.clone();
        let excluded_for_filter = excluded.clone();
        let mut builder = WalkBuilder::new(&root);
        builder
            // Matches OpenCode/ripgrep's content search: include hidden files,
            // but never descend into ignored/default-excluded trees.
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .ignore(true)
            .follow_links(false)
            .max_filesize(Some(MAX_SEARCH_FILE_BYTES))
            .filter_entry(move |entry| {
                relative_search_path(&root_for_filter, entry.path())
                    .is_none_or(|relative| !excluded_for_filter.matches(&relative))
            });
        for entry in builder.build() {
            if items.len() >= collect_limit {
                break;
            }
            if is_cancelled(request.context) {
                cancelled = true;
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            search_one_file(
                entry.path(),
                &root,
                &matcher,
                include.as_ref(),
                &excluded,
                request.context_lines,
                collect_limit,
                deadline,
                request.context,
                &mut items,
                &mut files_with_matches,
                &mut total_files_searched,
                &mut timed_out,
                &mut cancelled,
            )?;
            if timed_out || cancelled {
                break;
            }
        }
    }
    if cancelled {
        anyhow::bail!("grep aborted");
    }

    let overflowed_limit = items.len() > request.limit;
    items.truncate(request.limit);
    let mut output = render_grep_output(&items, files_with_matches.len(), request.limit);
    if timed_out {
        if items.is_empty() {
            output = "No files found before the search deadline".to_string();
        } else {
            output.push_str(
                "\n\n(Search stopped at the deadline; narrow the path or pattern for complete results.)",
            );
        }
    }
    Ok(ToolExecutionResult {
        title: format!("Grep {}", request.patterns.join(", ")),
        output,
        metadata: Some(json!({
            "patterns": request.patterns,
            "include": request.include,
            "exclude": request.exclude,
            "mode": matcher.label(),
            "engine": "ripgrep",
            "matches": items.len(),
            "filesWithMatches": files_with_matches.len(),
            "totalFilesSearched": total_files_searched,
            "nextFileOffset": 0,
            "truncated": timed_out || overflowed_limit,
            "timedOut": timed_out,
            "timeout": request.timeout_ms,
            "fallbackReason": request.fallback_reason,
            "items": items,
        })),
    })
}

#[allow(clippy::too_many_arguments)]
fn search_one_file(
    file_path: &Path,
    root: &Path,
    matcher: &LineMatcher,
    include: Option<&PathMatcher>,
    excluded: &PathMatcher,
    context_lines: usize,
    collect_limit: usize,
    deadline: Instant,
    context: &ToolContext,
    items: &mut Vec<StreamingGrepItem>,
    files_with_matches: &mut HashSet<String>,
    total_files_searched: &mut usize,
    timed_out: &mut bool,
    cancelled: &mut bool,
) -> anyhow::Result<()> {
    let Some(relative) = relative_search_path(root, file_path) else {
        return Ok(());
    };
    if excluded.matches(&relative)
        || include.is_some_and(|include| !include.matches(&relative))
    {
        return Ok(());
    }
    let Ok(metadata) = file_path.metadata() else {
        return Ok(());
    };
    if metadata.len() > MAX_SEARCH_FILE_BYTES {
        return Ok(());
    }
    let Ok(file) = File::open(file_path) else {
        return Ok(());
    };
    *total_files_searched += 1;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    let mut line_number = 0u64;
    let mut before = VecDeque::<(u64, String)>::with_capacity(context_lines);
    let mut after_remaining = 0usize;
    let mut last_emitted = 0u64;

    loop {
        if items.len() >= collect_limit {
            break;
        }
        if is_cancelled(context) {
            *cancelled = true;
            break;
        }
        if Instant::now() >= deadline {
            *timed_out = true;
            break;
        }
        bytes.clear();
        let read = match reader.read_until(b'\n', &mut bytes) {
            Ok(read) => read,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        if bytes.contains(&0) {
            // Same binary-file guard as ripgrep's default search behavior.
            break;
        }
        line_number += 1;
        while bytes
            .last()
            .is_some_and(|byte| matches!(*byte, b'\n' | b'\r'))
        {
            bytes.pop();
        }
        let line = String::from_utf8_lossy(&bytes).into_owned();
        let matched = matcher.matches(&line);
        if matched {
            files_with_matches.insert(relative.clone());
            for (number, text) in before.iter() {
                if *number > last_emitted && items.len() < collect_limit {
                    items.push(grep_item(&relative, *number, text, None));
                    last_emitted = *number;
                }
            }
            if line_number > last_emitted && items.len() < collect_limit {
                items.push(grep_item(
                    &relative,
                    line_number,
                    &line,
                    matcher.fuzzy_score(&line),
                ));
                last_emitted = line_number;
            }
            after_remaining = context_lines;
        } else if after_remaining > 0 {
            if line_number > last_emitted && items.len() < collect_limit {
                items.push(grep_item(&relative, line_number, &line, None));
                last_emitted = line_number;
            }
            after_remaining -= 1;
        }
        if context_lines > 0 {
            before.push_back((line_number, line));
            while before.len() > context_lines {
                before.pop_front();
            }
        }
    }
    Ok(())
}

fn grep_item(
    path: &str,
    line: u64,
    text: &str,
    fuzzy_score: Option<u16>,
) -> StreamingGrepItem {
    StreamingGrepItem {
        path: path.to_string(),
        line,
        text: truncate_line(text),
        definition: false,
        fuzzy_score,
    }
}

fn render_grep_output(
    items: &[StreamingGrepItem],
    files_with_matches: usize,
    limit: usize,
) -> String {
    if items.is_empty() {
        return "No files found".to_string();
    }
    let mut output = vec![format!(
        "Grep: Found {} matches in {files_with_matches} files",
        items.len()
    )];
    let mut current = "";
    for item in items {
        if current != item.path {
            if !current.is_empty() {
                output.push(String::new());
            }
            current = &item.path;
            output.push(format!("{}:", item.path));
        }
        output.push(format!("  Line {}: {}", item.line, item.text));
    }
    if items.len() >= limit {
        output.push(String::new());
        output.push(format!(
            "(Results may be truncated: showing first {limit} matches. Narrow the query.)"
        ));
    }
    output.join("\n")
}

#[derive(Clone)]
struct PathMatcher {
    patterns: Vec<(GlobMatcher, bool)>,
}

impl PathMatcher {
    fn query(pattern: &str) -> anyhow::Result<Self> {
        let pattern = pattern.trim();
        let pattern = if has_glob_meta(pattern) {
            pattern.to_string()
        } else {
            format!("*{}*", globset::escape(pattern))
        };
        Self::pattern(&pattern)
    }

    fn pattern(pattern: &str) -> anyhow::Result<Self> {
        Self::patterns(std::iter::once(pattern))
    }

    fn patterns<'a>(patterns: impl IntoIterator<Item = &'a str>) -> anyhow::Result<Self> {
        let mut compiled = Vec::new();
        for pattern in patterns {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            let pattern = pattern.trim_start_matches('!');
            let match_basename = !pattern.contains(['/', '\\']);
            let matcher = GlobBuilder::new(pattern)
                .case_insensitive(true)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map_err(|error| {
                    anyhow::anyhow!("invalid search glob {pattern:?}: {error}")
                })?
                .compile_matcher();
            compiled.push((matcher, match_basename));
        }
        Ok(Self { patterns: compiled })
    }

    fn matches(&self, relative: &str) -> bool {
        if relative.is_empty() {
            return false;
        }
        let basename = relative.rsplit('/').next().unwrap_or(relative);
        self.patterns.iter().any(|(matcher, match_basename)| {
            matcher.is_match(relative) || (*match_basename && matcher.is_match(basename))
        })
    }
}

enum LineMatcher {
    Regex(Regex),
    Literal {
        needles: Vec<String>,
        case_sensitive: bool,
    },
    Fuzzy {
        needle: String,
        case_sensitive: bool,
    },
}

impl LineMatcher {
    fn new(
        patterns: &[String],
        mode: GrepMode,
        force_case: bool,
    ) -> anyhow::Result<Self> {
        let case_sensitive = force_case
            || patterns
                .iter()
                .flat_map(|pattern| pattern.chars())
                .any(char::is_uppercase);
        match mode {
            GrepMode::Regex => {
                let expression = if patterns.len() == 1 {
                    patterns[0].clone()
                } else {
                    patterns
                        .iter()
                        .map(|pattern| format!("(?:{})", regex::escape(pattern)))
                        .collect::<Vec<_>>()
                        .join("|")
                };
                Ok(Self::Regex(
                    RegexBuilder::new(&expression)
                        .case_insensitive(!case_sensitive)
                        .build()
                        .map_err(|error| {
                            anyhow::anyhow!("invalid grep regex: {error}")
                        })?,
                ))
            }
            GrepMode::Fuzzy => Ok(Self::Fuzzy {
                needle: patterns.join(" "),
                case_sensitive,
            }),
            GrepMode::PlainText => Ok(Self::Literal {
                needles: patterns.to_vec(),
                case_sensitive,
            }),
        }
    }

    fn matches(&self, line: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(line),
            Self::Literal {
                needles,
                case_sensitive,
            } => {
                if *case_sensitive {
                    needles.iter().any(|needle| line.contains(needle))
                } else {
                    let line = line.to_lowercase();
                    needles
                        .iter()
                        .any(|needle| line.contains(&needle.to_lowercase()))
                }
            }
            Self::Fuzzy {
                needle,
                case_sensitive,
            } => fuzzy_match(line, needle, *case_sensitive),
        }
    }

    fn fuzzy_score(&self, line: &str) -> Option<u16> {
        matches!(self, Self::Fuzzy { .. })
            .then(|| line.chars().count().min(u16::MAX as usize) as u16)
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Regex(_) => "regex",
            Self::Literal { .. } => "plain",
            Self::Fuzzy { .. } => "fuzzy",
        }
    }
}

fn fuzzy_match(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    let (haystack, needle) = if case_sensitive {
        (haystack.to_string(), needle.to_string())
    } else {
        (haystack.to_lowercase(), needle.to_lowercase())
    };
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|wanted| chars.by_ref().any(|item| item == wanted))
}

fn relative_search_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    (!relative.is_empty()).then_some(relative)
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern.contains(['*', '?', '[', '{'])
}

fn is_cancelled(context: &ToolContext) -> bool {
    context
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.load(Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn context(root: &Path) -> ToolContext {
        ToolContext::new(root)
            .with_permissions(BTreeMap::from([("*".to_string(), json!("allow"))]))
    }

    #[test]
    fn broad_roots_require_streaming_fallback() {
        assert!(root_requires_fallback_for_home(
            Path::new("/home/tester"),
            Some(Path::new("/home/tester"))
        ));
        assert!(root_requires_fallback_for_home(Path::new("/"), None));
        #[cfg(windows)]
        {
            assert!(root_requires_fallback_for_home(Path::new(r"C:\"), None));
            assert!(root_requires_fallback_for_home(Path::new(r"\\?\C:\"), None));
            assert!(!root_requires_fallback_for_home(
                Path::new(r"\\?\C:\Users\project"),
                Some(Path::new(r"\\?\C:\Users"))
            ));
        }
        assert!(!root_requires_fallback_for_home(
            Path::new("/home/tester/project"),
            Some(Path::new("/home/tester"))
        ));
    }

    #[test]
    fn streaming_search_is_bounded_and_skips_default_excludes() {
        let root = std::env::temp_dir().join(format!(
            "neoism-streaming-search-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".config/neoism")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "official endpoint\n").unwrap();
        std::fs::write(root.join("target/generated.rs"), "official endpoint\n").unwrap();
        std::fs::write(
            root.join(".config/neoism/mcp.json"),
            "hosted official endpoint\n",
        )
        .unwrap();
        let context = context(&root);

        let glob = glob(GlobRequest {
            context: &context,
            path: &root,
            query: "*.rs",
            limit: 10,
            offset: 0,
            timeout_ms: 5_000,
            fallback_reason: "test".to_string(),
        })
        .unwrap();
        assert!(glob.output.contains("src/lib.rs"));
        assert!(!glob.output.contains("target/generated.rs"));
        assert_eq!(glob.metadata.as_ref().unwrap()["engine"], "ripgrep");

        let grep = grep(GrepRequest {
            context: &context,
            path: &root,
            patterns: &["official endpoint".to_string()],
            include: None,
            exclude: "target node_modules dist",
            context_lines: 0,
            case_sensitive: false,
            mode: GrepMode::PlainText,
            limit: 10,
            timeout_ms: 5_000,
            fallback_reason: "test".to_string(),
        })
        .unwrap();
        assert!(grep.output.contains("src/lib.rs"));
        assert!(grep.output.contains(".config/neoism/mcp.json"));
        assert!(!grep.output.contains("target/generated.rs"));
        assert_eq!(grep.metadata.as_ref().unwrap()["engine"], "ripgrep");

        let _ = std::fs::remove_dir_all(root);
    }
}
