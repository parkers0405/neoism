use super::*;
use crate::workspace as neo_workspace;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    /// Vault directory containing `path` (when it lives under the global
    /// vaults root) — the anchor for note + page-link suggestions.
    pub(crate) fn vault_dir_for_note_path(path: &Path) -> Option<PathBuf> {
        let vaults = neo_workspace::notes_vaults_dir();
        let rel = path.strip_prefix(&vaults).ok()?;
        let vault = rel.components().next()?;
        Some(vaults.join(vault.as_os_str()))
    }

    /// Page-link (`[[@`) suggestions across the given project roots.
    /// Returns (display label, inserted target): the label is the path
    /// relative to its project root — readable — while the target keeps
    /// the doc-relative form the link resolver needs.
    pub(crate) fn markdown_page_link_suggestions(
        roots: &[PathBuf],
        base_dir: &Path,
        current_doc: &Path,
        query: &str,
    ) -> Vec<(String, String)> {
        const LIMIT: usize = 12;
        const SCAN_LIMIT: usize = 3000;

        let query = Self::markdown_link_match_query(query);
        let query_lower = query.to_ascii_lowercase();
        let mut scored = Vec::new();
        for root in roots {
            let mut files = Vec::new();
            Self::collect_markdown_link_files(root, &mut files, SCAN_LIMIT);
            for path in files {
                if path == current_doc {
                    continue;
                }
                let label = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if label.is_empty() {
                    continue;
                }
                let label_lower = label.to_ascii_lowercase();
                let file_lower = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(label.as_str())
                    .to_ascii_lowercase();
                let score = if query_lower.is_empty() {
                    20
                } else if file_lower.starts_with(&query_lower) {
                    0
                } else if label_lower.starts_with(&query_lower) {
                    2
                } else if file_lower.contains(&query_lower) {
                    4
                } else if label_lower.contains(&query_lower) {
                    6
                } else {
                    continue;
                };
                let target = Self::relative_markdown_link_target(base_dir, &path);
                if target.is_empty() {
                    continue;
                }
                scored.push((score, label.len(), label, target));
            }
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        scored
            .into_iter()
            .take(LIMIT)
            .map(|(_, _, label, target)| (label, target))
            .collect()
    }

    pub(crate) fn markdown_link_suggestions(
        root: &Path,
        base_dir: &Path,
        current_doc: &Path,
        query: &str,
    ) -> Vec<String> {
        const LIMIT: usize = 12;
        const SCAN_LIMIT: usize = 3000;

        let mut files = Vec::new();
        Self::collect_markdown_link_files(root, &mut files, SCAN_LIMIT);
        let query = Self::markdown_link_match_query(query);
        let query_lower = query.to_ascii_lowercase();
        let mut scored = Vec::new();
        for path in files {
            if path == current_doc {
                continue;
            }
            let target = Self::relative_markdown_link_target(base_dir, &path);
            if target.is_empty() {
                continue;
            }
            let target_lower = target.to_ascii_lowercase();
            let file_lower = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(target.as_str())
                .to_ascii_lowercase();
            let score = if query_lower.is_empty() {
                20
            } else if file_lower.starts_with(&query_lower) {
                0
            } else if target_lower.starts_with(&query_lower) {
                2
            } else if file_lower.contains(&query_lower) {
                4
            } else if target_lower.contains(&query_lower) {
                6
            } else {
                continue;
            };
            scored.push((score, target.len(), target));
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        scored
            .into_iter()
            .take(LIMIT)
            .map(|(_, _, target)| target)
            .collect()
    }

    pub(crate) fn collect_markdown_link_files(
        root: &Path,
        out: &mut Vec<PathBuf>,
        limit: usize,
    ) {
        if out.len() >= limit || Self::skip_markdown_link_scan_dir(root) {
            return;
        }
        let Ok(read_dir) = fs::read_dir(root) else {
            return;
        };
        let mut entries = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            if out.len() >= limit {
                return;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if !Self::skip_markdown_link_scan_dir(&path) {
                    out.push(path.clone());
                }
                Self::collect_markdown_link_files(&path, out, limit);
            } else if file_type.is_file() {
                out.push(path);
            }
        }
    }

    pub(crate) fn skip_markdown_link_scan_dir(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name,
                    ".git"
                        | ".hg"
                        | ".svn"
                        | ".direnv"
                        | ".next"
                        | "node_modules"
                        | "target"
                        | "dist"
                        | "build"
                )
            })
    }

    pub(crate) fn markdown_link_match_query(query: &str) -> &str {
        let query = query.trim();
        if let Some((target, line)) = query.rsplit_once('-') {
            if !target.trim().is_empty() && line.chars().all(|ch| ch.is_ascii_digit()) {
                return target.trim();
            }
        }
        query
    }

    pub(crate) fn markdown_link_line_suffix_mode(query: &str) -> bool {
        let query = query.trim();
        query.rsplit_once('-').is_some_and(|(target, line)| {
            !target.trim().is_empty() && line.chars().all(|ch| ch.is_ascii_digit())
        })
    }

    pub(crate) fn markdown_create_note_target(query: &str) -> Option<String> {
        let query = query.trim().trim_matches('/');
        if query.is_empty()
            || query.starts_with('@')
            || query.starts_with('#')
            || query.contains("..")
        {
            return None;
        }
        let mut parts = Vec::new();
        for part in query.replace('\\', "/").split('/') {
            let sanitized = Self::sanitize_markdown_note_segment(part);
            if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
                return None;
            }
            parts.push(sanitized);
        }
        let last = parts.last_mut()?;
        let last_path = Path::new(last);
        if last_path.extension().is_none() {
            last.push_str(".md");
        }
        Some(parts.join("/"))
    }

    fn sanitize_markdown_note_segment(segment: &str) -> String {
        let mut out = String::new();
        let mut last_dash = false;
        for ch in segment.trim().chars() {
            if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.') {
                out.push(ch);
                last_dash = false;
            } else if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
        out.trim_matches(|ch| ch == '-' || ch == ' ').to_string()
    }

    pub(crate) fn relative_markdown_link_target(base_dir: &Path, path: &Path) -> String {
        let from = Self::path_components_for_relative(base_dir);
        let to = Self::path_components_for_relative(path);
        let mut common = 0usize;
        while common < from.len() && common < to.len() && from[common] == to[common] {
            common += 1;
        }

        let mut parts = Vec::new();
        for _ in common..from.len() {
            parts.push("..".to_string());
        }
        parts.extend(to.into_iter().skip(common));
        if parts.is_empty() {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string()
        } else {
            parts.join("/")
        }
    }

    pub(crate) fn path_components_for_relative(path: &Path) -> Vec<String> {
        path.components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                Component::ParentDir => Some("..".to_string()),
                _ => None,
            })
            .collect()
    }
}
