use std::path::{Path, PathBuf};

use super::helpers::*;
use super::types::*;

pub fn markdown_link_open_action(
    target: &MarkdownLinkTarget,
    path_is_dir: bool,
    path_is_markdown: bool,
    path_exists: bool,
) -> MarkdownLinkOpenAction {
    if path_is_dir {
        return MarkdownLinkOpenAction::OpenDirectory;
    }

    if path_is_markdown {
        return MarkdownLinkOpenAction::OpenMarkdown {
            create_missing_note: !target.code_ref && !path_exists,
        };
    }

    MarkdownLinkOpenAction::OpenEditor
}

impl MarkdownPane {
    pub fn wiki_link_query_before_cursor(&self) -> Option<MarkdownWikiLinkQuery> {
        let bounds = self.wiki_link_bounds_before_cursor()?;
        let line = self.lines.get(self.cursor_line)?;
        let inner = line.get(bounds.inner_start..self.cursor_col)?;
        if inner.contains('|') {
            return None;
        }
        let inner = inner.trim_start();
        if let Some(query) = inner.strip_prefix('@') {
            return Some(MarkdownWikiLinkQuery {
                query: query.to_string(),
                target: None,
                kind: MarkdownWikiLinkKind::CodeRef,
            });
        }
        if let Some((target, query)) = inner.split_once('#') {
            return Some(MarkdownWikiLinkQuery {
                query: query.to_string(),
                target: Some(target.trim().to_string()),
                kind: MarkdownWikiLinkKind::Heading,
            });
        }
        Some(MarkdownWikiLinkQuery {
            query: inner.to_string(),
            target: None,
            kind: MarkdownWikiLinkKind::Note,
        })
    }

    pub fn apply_wiki_link_completion(&mut self, target: &str) -> bool {
        let Some(bounds) = self.wiki_link_bounds_before_cursor() else {
            return false;
        };
        let Some(query) = self.wiki_link_query_before_cursor() else {
            return false;
        };
        let target = target.trim();
        if target.is_empty() {
            return false;
        }

        self.save_undo();
        let replacement = match query.kind {
            MarkdownWikiLinkKind::CodeRef => {
                format!("@{}", target.trim_start_matches('@').trim())
            }
            MarkdownWikiLinkKind::Note | MarkdownWikiLinkKind::Heading => {
                target.to_string()
            }
        };
        if let Some(close_start) = bounds.close_start {
            let replaced_len = close_start.saturating_sub(bounds.inner_start);
            self.lines[self.cursor_line]
                .replace_range(bounds.inner_start..close_start, &replacement);
            self.adjust_source_len(replacement.len() as isize - replaced_len as isize);
            self.cursor_col = bounds.inner_start + replacement.len();
        } else {
            let link = format!("[[{replacement}]]");
            let replaced_len = self.cursor_col.saturating_sub(bounds.open_start);
            self.lines[self.cursor_line]
                .replace_range(bounds.open_start..self.cursor_col, &link);
            self.adjust_source_len(link.len() as isize - replaced_len as isize);
            self.cursor_col = bounds.open_start + 2 + replacement.len();
        }
        self.mode = MarkdownMode::Insert;
        self.visual_anchor = None;
        self.mouse_select_anchor = None;
        self.vim.clear_pending();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_undo();
        true
    }

    pub fn spelling_word_at(&self, x: f32, y: f32) -> Option<MarkdownMisspelling> {
        let block = self
            .block_rects
            .iter()
            .find(|block| point_in_rect(x, y, block.rect))
            .copied()?;
        if self.is_inside_code_block(block.line) {
            return None;
        }
        let line = self.lines.get(block.line)?;
        let col = self.cursor_col_from_point(block, x, y);
        let (start, end) = word_bounds_at(line, col)?;
        let word = line.get(start..end)?.to_string();
        super::spellcheck::is_misspelled_word(&word).then_some(MarkdownMisspelling {
            line: block.line,
            start,
            end,
            word,
        })
    }

    pub fn replace_spelling_word(
        &mut self,
        line: usize,
        start: usize,
        end: usize,
        replacement: &str,
    ) -> bool {
        let Some(source) = self.lines.get(line) else {
            return false;
        };
        if start >= end
            || end > source.len()
            || !source.is_char_boundary(start)
            || !source.is_char_boundary(end)
        {
            return false;
        }
        self.save_undo();
        self.lines[line].replace_range(start..end, replacement);
        self.adjust_source_len(replacement.len() as isize - (end - start) as isize);
        self.cursor_line = line;
        self.cursor_col = start + replacement.len();
        self.mode = MarkdownMode::Insert;
        self.visual_anchor = None;
        self.mouse_select_anchor = None;
        self.vim.clear_pending();
        self.follow_cursor = true;
        self.rebuild_blocks();
        self.commit_undo();
        true
    }

    pub fn resolve_markdown_link(&self, inner: &str) -> Option<MarkdownLinkTarget> {
        if let Some(cached) = self.link_target_cache.borrow().get(inner).cloned() {
            return cached;
        }
        let parsed = parse_markdown_link_parts(inner)?;
        let path = if parsed.target.is_empty() {
            self.path.clone()
        } else {
            self.resolve_markdown_link_path(&parsed.target)
        };
        let line = parsed.line.or_else(|| {
            parsed
                .heading
                .as_deref()
                .and_then(|heading| self.resolve_markdown_heading_line(&path, heading))
        });
        let target = Some(MarkdownLinkTarget {
            path,
            line,
            code_ref: parsed.code_ref,
        });
        self.link_target_cache
            .borrow_mut()
            .insert(inner.to_string(), target.clone());
        target
    }

    pub(crate) fn resolve_markdown_link_path(&self, target: &str) -> PathBuf {
        // Host-owned document schemes (notebook actions, EPUB spine links)
        // and web URLs must survive resolution verbatim. Treating them as a
        // relative filesystem path destroys the scheme and prevents the host
        // from dispatching the click.
        if target.contains("://") {
            return PathBuf::from(target);
        }
        let target = PathBuf::from(target);
        let base = if target.is_absolute() {
            target
        } else {
            self.path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(target)
        };
        let mut candidates = vec![base.clone()];
        if base.extension().is_none() {
            for ext in ["md", "markdown", "mdx"] {
                let mut with_ext = base.clone();
                with_ext.set_extension(ext);
                candidates.push(with_ext);
            }
        }
        candidates
            .iter()
            .find(|candidate| candidate.exists())
            .cloned()
            .unwrap_or_else(|| {
                if base.extension().is_none() {
                    let mut with_ext = base;
                    with_ext.set_extension("md");
                    with_ext
                } else {
                    base
                }
            })
    }

    pub(crate) fn resolve_markdown_heading_line(
        &self,
        path: &Path,
        heading: &str,
    ) -> Option<usize> {
        let source = std::fs::read_to_string(path).ok()?;
        markdown_heading_line(&source, heading)
    }

    pub(super) fn wiki_link_bounds_before_cursor(
        &self,
    ) -> Option<MarkdownWikiLinkBounds> {
        let line = self.lines.get(self.cursor_line)?;
        let cursor = floor_char_boundary(line, self.cursor_col.min(line.len()));
        let prefix = line.get(..cursor)?;
        let open_start = prefix.rfind("[[")?;
        let inner_start = open_start + 2;
        let inner = line.get(inner_start..cursor)?;
        if inner.contains("]]") {
            return None;
        }
        let suffix = line.get(cursor..).unwrap_or_default();
        let next_open = suffix.find("[[");
        let close_start = suffix.find("]]").and_then(|close| {
            if next_open.is_some_and(|open| open < close) {
                None
            } else {
                Some(cursor + close)
            }
        });
        Some(MarkdownWikiLinkBounds {
            open_start,
            inner_start,
            close_start,
        })
    }
}
