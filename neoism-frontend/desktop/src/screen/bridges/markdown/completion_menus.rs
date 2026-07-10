use super::*;
use crate::workspace as neo_workspace;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn open_markdown_block_menu(&mut self, cursor_rect: Option<[f32; 4]>) {
        use crate::editor::markdown::state::MarkdownBlockTemplate;
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};

        // Notion-style previews: icons/short labels, never raw markdown
        // syntax like `[[@]]` or `- [ ]`.
        let items = vec![
            ContextMenuItem::new(
                "Task",
                "task",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::TaskList),
            )
            .with_preview("☐"),
            ContextMenuItem::new(
                "Text",
                "text",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Paragraph),
            )
            .with_preview("Aa"),
            ContextMenuItem::new(
                "Link Note",
                "link note",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::WikiLink),
            )
            .with_preview("\u{f15c}"),
            ContextMenuItem::new(
                "Page Link",
                "page link",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::CodeLink),
            )
            .with_preview("\u{f0c1}"),
            ContextMenuItem::new(
                "Heading 1",
                "h1",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Heading1),
            )
            .with_preview("H1"),
            ContextMenuItem::new(
                "Heading 2",
                "h2",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Heading2),
            )
            .with_preview("H2"),
            ContextMenuItem::new(
                "Heading 3",
                "h3",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Heading3),
            )
            .with_preview("H3"),
            ContextMenuItem::new(
                "Bullet List",
                "bullet",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::BulletList),
            )
            .with_preview("\u{f0ca}"),
            ContextMenuItem::new(
                "Quote",
                "quote",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Quote),
            )
            .with_preview("\u{f10d}"),
            ContextMenuItem::new(
                "Code Block",
                "code",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::CodeBlock),
            )
            .with_preview("\u{f121}"),
            ContextMenuItem::new(
                "Table",
                "table",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Table),
            )
            .with_preview("\u{f0ce}"),
            ContextMenuItem::new(
                "Divider",
                "divider",
                ContextMenuAction::MarkdownBlock(MarkdownBlockTemplate::Divider),
            )
            .with_preview("—"),
        ];
        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let [fallback_x, fallback_y] = self.markdown_mouse_logical();
        let query = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|markdown| markdown.slash_block_query_before_cursor())
            .unwrap_or_default();
        let (x, y) = cursor_rect
            .map(|[x, y, _w, h]| (x, y + h + 6.0))
            .unwrap_or((fallback_x, fallback_y));
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open_markdown_block(
            "Add block",
            items,
            query,
            x,
            y,
            size.width as f32 / scale_factor,
            menu_height,
        );
        if let Some([_, row_y, _, row_h]) = cursor_rect {
            // Never let the window-bottom clamp shove the menu onto the
            // line being typed — flip it above the row instead.
            self.renderer.context_menu.avoid_row(row_y, row_y + row_h);
        }
        self.mark_dirty();
    }

    pub(crate) fn refresh_markdown_block_menu(&mut self) -> bool {
        if !self.renderer.context_menu.is_markdown_block_completion() {
            return false;
        }
        let Some(query) =
            self.context_manager
                .current()
                .markdown
                .as_ref()
                .and_then(|markdown| {
                    if !matches!(
                        markdown.mode,
                        crate::editor::markdown::state::MarkdownMode::Insert
                    ) {
                        return None;
                    }
                    markdown.slash_block_query_before_cursor()
                })
        else {
            self.renderer.context_menu.close();
            self.mark_dirty();
            return true;
        };

        if self.renderer.context_menu.set_markdown_block_query(query) {
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn apply_markdown_block_template(
        &mut self,
        template: crate::editor::markdown::state::MarkdownBlockTemplate,
    ) -> bool {
        let Some(path) =
            self.context_manager
                .current_mut()
                .markdown
                .as_mut()
                .map(|markdown| {
                    markdown.apply_block_template(template);
                    markdown.path.clone()
                })
        else {
            return false;
        };
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.trail_cursor.reset();
        if neoism_ui::editor::markdown::bridge_policy::apply_markdown_block_template_refreshes_link_completion(
            template,
        ) {
            self.refresh_markdown_link_completion_menu();
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn apply_markdown_link_completion(&mut self, target: &str) -> bool {
        let Some(path) = self
            .context_manager
            .current_mut()
            .markdown
            .as_mut()
            .and_then(|markdown| {
                markdown
                    .apply_wiki_link_completion(target)
                    .then(|| markdown.path.clone())
            })
        else {
            return false;
        };
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn apply_markdown_spelling_replacement(
        &mut self,
        line: usize,
        start: usize,
        end: usize,
        replacement: &str,
    ) -> bool {
        let Some(path) = self
            .context_manager
            .current_mut()
            .markdown
            .as_mut()
            .and_then(|markdown| {
                markdown
                    .replace_spelling_word(line, start, end, replacement)
                    .then(|| markdown.path.clone())
            })
        else {
            return false;
        };
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn open_markdown_spelling_menu(&mut self) -> bool {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};

        let [x, y] = self.markdown_mouse_logical();
        let Some(target) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|markdown| markdown.spelling_word_at(x, y))
        else {
            return false;
        };
        let suggestions =
            crate::editor::markdown::render::spelling_suggestions(&target.word);
        if suggestions.is_empty() {
            return false;
        }
        let items = suggestions
            .into_iter()
            .map(|replacement| {
                ContextMenuItem::new(
                    replacement.clone(),
                    "fix",
                    ContextMenuAction::MarkdownSpellingReplace {
                        line: target.line,
                        start: target.start,
                        end: target.end,
                        replacement,
                    },
                )
            })
            .collect::<Vec<_>>();
        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let menu_height = self.context_menu_logical_height();
        self.renderer.context_menu.open(
            format!("Spelling: {}", target.word),
            items,
            x,
            y + 8.0,
            size.width as f32 / scale_factor,
            menu_height,
        );
        self.mark_dirty();
        true
    }

    pub(crate) fn refresh_markdown_link_completion_menu(&mut self) -> bool {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};

        let Some((query, cursor_rect, doc_path)) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|markdown| {
                if !matches!(
                    markdown.mode,
                    crate::editor::markdown::state::MarkdownMode::Insert
                ) {
                    return None;
                }
                markdown
                    .wiki_link_query_before_cursor()
                    .map(|query| (query, markdown.cursor_rect, markdown.path.clone()))
            })
        else {
            if self.renderer.context_menu.is_markdown_link_completion() {
                self.renderer.context_menu.close();
                self.mark_dirty();
                return true;
            }
            return false;
        };

        if matches!(
            query.kind,
            crate::editor::markdown::state::MarkdownWikiLinkKind::CodeRef
        ) && Self::markdown_link_line_suffix_mode(&query.query)
        {
            if self.renderer.context_menu.is_markdown_link_completion() {
                self.renderer.context_menu.close();
                self.mark_dirty();
                return true;
            }
            return false;
        }

        // The doc's VAULT (when it lives in one) anchors everything: note
        // suggestions scan the vault the doc belongs to — not whichever
        // workspace happens to be active — and page links search the
        // projects linked to THAT vault.
        let doc_vault = Self::vault_dir_for_note_path(&doc_path);
        let root = doc_vault
            .clone()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| doc_path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let base_dir = doc_path.parent().unwrap_or(root.as_path());
        // (display label, inserted target) — page links show project-relative
        // paths instead of a wall of ../../.
        let mut suggestions: Vec<(String, String)> = match query.kind {
            crate::editor::markdown::state::MarkdownWikiLinkKind::Heading => self
                .indexed_markdown_heading_suggestions(
                    &root,
                    base_dir,
                    &doc_path,
                    query.target.as_deref(),
                    &query.query,
                    12,
                )
                .unwrap_or_default()
                .into_iter()
                .map(|target| (target.clone(), target))
                .collect(),
            crate::editor::markdown::state::MarkdownWikiLinkKind::Note => self
                .indexed_markdown_link_suggestions(
                    &root,
                    base_dir,
                    &doc_path,
                    &query.query,
                    12,
                )
                .unwrap_or_else(|| {
                    Self::markdown_link_suggestions(
                        &root,
                        base_dir,
                        &doc_path,
                        &query.query,
                    )
                })
                .into_iter()
                .map(|target| (target.clone(), target))
                .collect(),
            crate::editor::markdown::state::MarkdownWikiLinkKind::CodeRef => {
                let page_sources: Vec<PathBuf> = doc_vault
                    .as_ref()
                    .map(|vault| {
                        neo_workspace::vault_project_links(vault)
                            .into_iter()
                            .map(|link| link.path)
                            .collect::<Vec<_>>()
                    })
                    .filter(|links: &Vec<PathBuf>| !links.is_empty())
                    .unwrap_or_else(|| vec![root.clone()]);
                Self::markdown_page_link_suggestions(
                    &page_sources,
                    base_dir,
                    &doc_path,
                    &query.query,
                )
            }
        };
        let create_target = if matches!(
            query.kind,
            crate::editor::markdown::state::MarkdownWikiLinkKind::Note
        ) {
            Self::markdown_create_note_target(&query.query).filter(|target| {
                !suggestions
                    .iter()
                    .any(|(_, item)| item.eq_ignore_ascii_case(target))
            })
        } else {
            None
        };
        if let Some(target) = create_target.clone() {
            suggestions.push((target.clone(), target));
        }
        if suggestions.is_empty() {
            if self.renderer.context_menu.is_markdown_link_completion() {
                self.renderer.context_menu.close();
                self.mark_dirty();
                return true;
            }
            return false;
        }

        let title = match query.kind {
            crate::editor::markdown::state::MarkdownWikiLinkKind::CodeRef => "Link page",
            crate::editor::markdown::state::MarkdownWikiLinkKind::Heading => {
                "Link heading"
            }
            crate::editor::markdown::state::MarkdownWikiLinkKind::Note => "Link note",
        };
        let items = suggestions
            .into_iter()
            .map(|(label, target)| {
                let creating = create_target
                    .as_ref()
                    .is_some_and(|create| create.eq_ignore_ascii_case(&target));
                let (hint, preview) = if creating {
                    ("Create", "+")
                } else {
                    match query.kind {
                        crate::editor::markdown::state::MarkdownWikiLinkKind::CodeRef => {
                            ("Enter", "\u{f0c1}")
                        }
                        crate::editor::markdown::state::MarkdownWikiLinkKind::Heading => {
                            ("Enter", "#")
                        }
                        crate::editor::markdown::state::MarkdownWikiLinkKind::Note => {
                            ("Enter", "\u{f15c}")
                        }
                    }
                };
                ContextMenuItem::new(
                    label,
                    hint,
                    ContextMenuAction::MarkdownLinkCompletion(target),
                )
                .with_preview(preview)
            })
            .collect::<Vec<_>>();
        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let [fallback_x, fallback_y] = self.markdown_mouse_logical();
        let menu_height = self.context_menu_logical_height();
        let window_width = size.width as f32 / scale_factor;
        // Anchor below the caret's row, flipping above it when the window
        // bottom would otherwise shove the menu over the text being typed.
        match cursor_rect {
            Some([x, y, _w, h]) => self.renderer.context_menu.open_avoiding_row(
                title,
                items,
                x,
                y,
                y + h,
                window_width,
                menu_height,
            ),
            None => self.renderer.context_menu.open(
                title,
                items,
                fallback_x,
                fallback_y,
                window_width,
                menu_height,
            ),
        }
        self.mark_dirty();
        true
    }
}
