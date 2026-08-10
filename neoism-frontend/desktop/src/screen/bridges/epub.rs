use super::super::*;
use std::path::PathBuf;

fn append_epub_toc_buttons(
    entries: &[crate::editor::epub::EpubTocEntry],
    depth: usize,
    buttons: &mut Vec<neoism_ui::widgets::modal::ModalButton>,
) {
    use neoism_ui::widgets::modal::{ModalAction, ModalButton};

    for entry in entries {
        let label = if depth == 0 {
            entry.label.clone()
        } else {
            format!("{}{}", "  ".repeat(depth), entry.label)
        };
        buttons.push(ModalButton::new(
            label,
            if depth == 0 { "Chapter" } else { "Section" },
            ModalAction::EpubGoTo {
                href: entry.href.clone(),
            },
        ));
        append_epub_toc_buttons(&entry.children, depth + 1, buttons);
    }
}

impl Screen<'_> {
    pub(crate) fn activate_rich_document_path(&mut self, path: PathBuf) {
        if is_epub_path(&path) {
            self.activate_epub_path(path);
        } else if crate::editor::notebook::is_notebook_path(&path) {
            self.activate_notebook_path(path);
        } else {
            self.activate_markdown_path(path);
        }
    }

    pub(crate) fn open_epub_table_of_contents(&mut self) -> bool {
        let Some(result) = self
            .context_manager
            .current_mut()
            .epub
            .as_mut()
            .map(|epub| epub.open_contents_page())
        else {
            return false;
        };
        if let Err(error) = result {
            self.renderer.notifications.push(
                format!("Could not open book contents: {error}"),
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
        self.renderer.modal.close();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_table_of_contents_modal(&mut self) -> bool {
        use neoism_ui::widgets::modal::ModalSpec;

        let Some(epub) = self.context_manager.current().epub.as_ref() else {
            return false;
        };
        if epub.book.toc.is_empty() {
            self.renderer.notifications.push(
                "This book does not include a table of contents".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            return true;
        }

        let mut buttons = Vec::new();
        append_epub_toc_buttons(&epub.book.toc, 0, &mut buttons);
        let title = epub.book.metadata.title.clone();
        let authors = epub.book.metadata.creators.join(", ");
        self.renderer.modal.open(ModalSpec {
            title: "Table of Contents".to_string(),
            body: title,
            meta: if authors.trim().is_empty() {
                "Choose a chapter · ↑↓ navigate · Enter open · Esc close".to_string()
            } else {
                format!("{authors} · ↑↓ navigate · Enter open · Esc close")
            },
            input: None,
            buttons,
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_chapter_pages(&mut self, href: &str) -> bool {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let Some(index) = self
            .context_manager
            .current()
            .epub
            .as_ref()
            .and_then(|epub| epub.book.chapter_index_for_href(href))
        else {
            return false;
        };
        let (title, chapter_href) = {
            let epub = self.context_manager.current().epub.as_ref().unwrap();
            (
                epub.book.chapters[index].title.clone(),
                epub.book.chapters[index].href.clone(),
            )
        };
        let previews = match self
            .context_manager
            .current_mut()
            .epub
            .as_mut()
            .unwrap()
            .page_previews_for_chapter(index)
        {
            Ok(previews) => previews,
            Err(error) => {
                self.renderer.notifications.push(
                    format!("Could not paginate chapter: {error}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                return true;
            }
        };
        let page_count = previews.len().max(1);
        let mut buttons = previews
            .into_iter()
            .enumerate()
            .map(|(page, preview)| {
                ModalButton::new(
                    format!("Page {}  ·  {preview}", page + 1),
                    format!("{} of {page_count}", page + 1),
                    ModalAction::EpubGoToPage {
                        href: chapter_href.clone(),
                        page,
                    },
                )
            })
            .collect::<Vec<_>>();
        buttons.push(ModalButton::new(
            "Back to contents",
            "Esc",
            ModalAction::EpubOpenContents,
        ));
        self.renderer.modal.open(ModalSpec {
            title,
            body: format!("{page_count} reflowed reader pages"),
            meta: "Each row opens exactly one bounded page.".to_string(),
            input: None,
            buttons,
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_note_prompt(&mut self) -> bool {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let Some(selected_text) = self
            .context_manager
            .current()
            .epub
            .as_ref()
            .and_then(|epub| epub.markdown.visual_selection())
            .map(|(_, _, text)| text)
        else {
            return false;
        };
        let preview = selected_text
            .split_whitespace()
            .take(18)
            .collect::<Vec<_>>()
            .join(" ");
        self.renderer.modal.open(ModalSpec {
            title: "Note on highlighted text".to_string(),
            body: format!("“{preview}”"),
            meta: "Markdown note. Enter adds a line; Ctrl+Enter saves. Long lines wrap."
                .to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "Write a note…".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Save note",
                    "Enter",
                    ModalAction::EpubAddNote {
                        value: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_annotations(&mut self) -> bool {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let Some(epub) = self.context_manager.current().epub.as_ref() else {
            return false;
        };
        let title = epub.book.metadata.title.clone();
        let annotations = epub.state.annotations.clone();
        let mut buttons = annotations
            .iter()
            .rev()
            .map(|annotation| {
                let quote = compact_book_text(&annotation.selected_text, 8);
                let label = if annotation.note.trim().is_empty() {
                    format!("“{quote}”")
                } else {
                    format!("“{quote}” — {}", compact_book_text(&annotation.note, 6))
                };
                ModalButton::new(
                    label,
                    "View",
                    ModalAction::EpubOpenAnnotation {
                        id: annotation.id.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();
        buttons.push(ModalButton::new("Close", "Esc", ModalAction::Close));
        self.renderer.modal.open(ModalSpec {
            title: format!("{title} — Highlights & notes"),
            body: if annotations.is_empty() {
                "No highlights yet. Press v, select text, then H to highlight or N to attach a note."
                    .to_string()
            } else {
                format!(
                    "{} saved annotation{}",
                    annotations.len(),
                    if annotations.len() == 1 { "" } else { "s" }
                )
            },
            meta: "Newest first. Enter opens an annotation.".to_string(),
            input: None,
            buttons,
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_annotation_collections(
        &mut self,
        annotation_id: &str,
    ) -> bool {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let Some(epub) = self.context_manager.current().epub.as_ref() else {
            return false;
        };
        let mut buttons = epub
            .annotation_collections(annotation_id)
            .into_iter()
            .map(|(collection_id, name, selected)| {
                ModalButton::new(
                    format!("{} {name}", if selected { "●" } else { "○" }),
                    if selected { "included" } else { "add" },
                    ModalAction::EpubToggleAnnotationCollection {
                        annotation_id: annotation_id.to_string(),
                        collection_id,
                    },
                )
            })
            .collect::<Vec<_>>();
        buttons.push(ModalButton::new(
            "Create new collection…",
            "new .md file",
            ModalAction::EpubPromptNewAnnotationCollection {
                annotation_id: annotation_id.to_string(),
            },
        ));
        buttons.push(ModalButton::new("Done", "Esc", ModalAction::Close));
        self.renderer.modal.open(ModalSpec {
            title: "Highlight collections".to_string(),
            body: "A highlight can belong to multiple study collections. Each collection is a Markdown file beside this book's note.".to_string(),
            meta: "Toggle any collection or create a new one.".to_string(),
            buttons,
            input: None,
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_new_collection_prompt(
        &mut self,
        annotation_id: &str,
    ) -> bool {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };
        self.renderer.modal.open(ModalSpec {
            title: "New highlight collection".to_string(),
            body: "Name the topic or study this highlight belongs to.".to_string(),
            meta: "Creates a Markdown file beside the regular book note.".to_string(),
            buttons: vec![
                ModalButton::new(
                    "Create and add",
                    "Enter",
                    ModalAction::EpubCreateAnnotationCollection {
                        annotation_id: annotation_id.to_string(),
                        value: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "e.g. Distributed systems".to_string(),
            }),
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_annotation_detail(&mut self, id: &str) -> bool {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let Some(annotation) =
            self.context_manager
                .current()
                .epub
                .as_ref()
                .and_then(|epub| {
                    epub.state
                        .annotations
                        .iter()
                        .find(|annotation| annotation.id == id)
                        .cloned()
                })
        else {
            return false;
        };
        let note = annotation.note.trim();
        self.renderer.modal.open(ModalSpec {
            title: "Book annotation".to_string(),
            body: format!("“{}”", annotation.selected_text.trim()),
            meta: if note.is_empty() {
                "Highlight only — no note attached.".to_string()
            } else {
                format!("Note: {note}")
            },
            input: None,
            buttons: vec![
                ModalButton::new(
                    "Go to highlight",
                    "Enter",
                    ModalAction::EpubGoToAnnotation {
                        id: annotation.id.clone(),
                    },
                ),
                ModalButton::new(
                    "Edit note",
                    "e",
                    ModalAction::EpubEditAnnotation {
                        id: annotation.id.clone(),
                    },
                ),
                ModalButton::new(
                    "Save to collection…",
                    "c",
                    ModalAction::EpubOpenAnnotationCollections {
                        id: annotation.id.clone(),
                    },
                ),
                ModalButton::new(
                    "Delete",
                    "Permanent",
                    ModalAction::EpubDeleteAnnotation {
                        id: annotation.id.clone(),
                    },
                ),
                ModalButton::new(
                    "Back",
                    "Esc",
                    ModalAction::EpubOpenAnnotation { id: String::new() },
                ),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_annotation_edit_prompt(&mut self, id: &str) -> bool {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let Some(annotation) =
            self.context_manager
                .current()
                .epub
                .as_ref()
                .and_then(|epub| {
                    epub.state
                        .annotations
                        .iter()
                        .find(|annotation| annotation.id == id)
                        .cloned()
                })
        else {
            return false;
        };
        self.renderer.modal.open(ModalSpec {
            title: "Edit book note".to_string(),
            body: format!("“{}”", compact_book_text(&annotation.selected_text, 18)),
            meta: "The highlight stays in place.".to_string(),
            input: Some(ModalInputSpec {
                value: annotation.note,
                placeholder: "Write a note…".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Save note",
                    "Ctrl+Enter",
                    ModalAction::EpubUpdateAnnotation {
                        id: annotation.id,
                        value: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
        true
    }

    pub(crate) fn open_epub_annotation_menu_at(
        &mut self,
        x: f32,
        y: f32,
        existing_only: bool,
    ) -> bool {
        use neoism_ui::panels::context_menu::{
            ContextMenuAction, ContextMenuItem, ContextMenuSwatch, EpubContextAction,
        };

        let annotation = self
            .context_manager
            .current()
            .epub
            .as_ref()
            .and_then(|epub| {
                epub.markdown
                    .text_position_at_point(x, y)
                    .and_then(|position| {
                        epub.annotation_at_source_position(position.line, position.col)
                    })
            })
            .cloned();
        let has_selection = self
            .context_manager
            .current()
            .epub
            .as_ref()
            .and_then(|epub| epub.markdown.visual_selection())
            .is_some();
        if annotation.is_none() && (existing_only || !has_selection) {
            return false;
        }
        let annotation_id = annotation.as_ref().map(|value| value.id.clone());
        let mut items = ["yellow", "green", "blue", "pink", "purple"]
            .into_iter()
            .map(|color| {
                let selected = annotation
                    .as_ref()
                    .is_some_and(|value| normalize_context_color(&value.color) == color);
                ContextMenuItem::new(
                    format!("{} {}", if selected { "●" } else { "○" }, title_case(color)),
                    if selected { "current" } else { "highlight" },
                    ContextMenuAction::Epub(EpubContextAction::SetHighlightColor {
                        annotation_id: annotation_id.clone(),
                        color: color.to_string(),
                    }),
                )
                .with_swatch(match color {
                    "green" => ContextMenuSwatch::Green,
                    "blue" => ContextMenuSwatch::Blue,
                    "pink" => ContextMenuSwatch::Pink,
                    "purple" => ContextMenuSwatch::Purple,
                    _ => ContextMenuSwatch::Yellow,
                })
            })
            .collect::<Vec<_>>();
        items.push(ContextMenuItem::new(
            if annotation
                .as_ref()
                .is_some_and(|value| !value.note.trim().is_empty())
            {
                "Edit note"
            } else {
                "Add note…"
            },
            "N",
            ContextMenuAction::Epub(EpubContextAction::AddNote {
                annotation_id: annotation_id.clone(),
            }),
        ));
        if let Some(annotation) = annotation.as_ref() {
            items.push(ContextMenuItem::new(
                "Save to collection…",
                "C",
                ContextMenuAction::Epub(EpubContextAction::ManageCollections {
                    annotation_id: annotation.id.clone(),
                }),
            ));
            if !annotation.note.trim().is_empty() {
                items.push(ContextMenuItem::new(
                    "Open in book note",
                    "↗",
                    ContextMenuAction::Epub(EpubContextAction::OpenBookNote {
                        annotation_id: annotation.id.clone(),
                    }),
                ));
            }
        }
        items.push(ContextMenuItem::new(
            "Ask Neoism about this",
            "A",
            ContextMenuAction::Epub(EpubContextAction::AskNeoism {
                annotation_id: annotation_id.clone(),
            }),
        ));
        items.push(ContextMenuItem::new(
            "Copy quote",
            "Ctrl+C",
            ContextMenuAction::Epub(EpubContextAction::CopyQuote {
                annotation_id: annotation_id.clone(),
            }),
        ));
        if let Some(annotation) = annotation {
            items.push(ContextMenuItem::new(
                "Remove highlight",
                "Delete",
                ContextMenuAction::Epub(EpubContextAction::DeleteAnnotation {
                    annotation_id: annotation.id,
                }),
            ));
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        self.renderer.context_menu.open(
            "Highlight",
            items,
            x,
            y + 8.0,
            size.width as f32 / scale_factor,
            self.context_menu_logical_height(),
        );
        self.mark_dirty();
        true
    }

    pub(crate) fn execute_epub_context_action(
        &mut self,
        action: neoism_ui::panels::context_menu::EpubContextAction,
        clipboard: &mut neoism_backend::clipboard::Clipboard,
    ) {
        use neoism_backend::clipboard::ClipboardType;
        use neoism_ui::panels::context_menu::EpubContextAction;

        match action {
            EpubContextAction::SetHighlightColor {
                annotation_id,
                color,
            } => {
                let id = match annotation_id {
                    Some(id) => Some(id),
                    None => self
                        .context_manager
                        .current_mut()
                        .epub
                        .as_mut()
                        .and_then(|epub| {
                            epub.add_highlight_from_selection(String::new()).ok()
                        })
                        .flatten(),
                };
                let result = id.as_deref().and_then(|id| {
                    self.context_manager
                        .current_mut()
                        .epub
                        .as_mut()
                        .map(|epub| epub.set_annotation_color(id, &color))
                });
                if let Some(Err(error)) = result {
                    self.file_tree_notify(
                        format!("Could not update highlight: {error}"),
                        neoism_ui::panels::notifications::NotificationLevel::Error,
                    );
                }
            }
            EpubContextAction::AddNote { annotation_id } => {
                if let Some(id) = annotation_id {
                    self.open_epub_annotation_edit_prompt(&id);
                } else {
                    self.open_epub_note_prompt();
                }
            }
            EpubContextAction::OpenBookNote { annotation_id } => {
                self.open_epub_book_note(&annotation_id);
            }
            EpubContextAction::ManageCollections { annotation_id } => {
                self.open_epub_annotation_collections(&annotation_id);
            }
            EpubContextAction::CopyQuote { annotation_id } => {
                let text = annotation_id
                    .as_deref()
                    .and_then(|id| {
                        self.context_manager
                            .current()
                            .epub
                            .as_ref()
                            .and_then(|epub| {
                                epub.state
                                    .annotations
                                    .iter()
                                    .find(|annotation| annotation.id == id)
                            })
                            .map(|annotation| annotation.selected_text.clone())
                    })
                    .or_else(|| {
                        self.context_manager
                            .current()
                            .epub
                            .as_ref()
                            .and_then(|epub| epub.markdown.visual_selection())
                            .map(|(_, _, text)| text)
                    });
                if let Some(text) = text {
                    clipboard.set(ClipboardType::Clipboard, text);
                }
            }
            EpubContextAction::AskNeoism { annotation_id } => {
                let context = self
                    .context_manager
                    .current()
                    .epub
                    .as_ref()
                    .and_then(|epub| {
                        let annotation = annotation_id
                            .as_deref()
                            .and_then(|id| {
                                epub.state
                                    .annotations
                                    .iter()
                                    .find(|annotation| annotation.id == id)
                            });
                        let quote = annotation
                            .map(|annotation| annotation.selected_text.clone())
                            .or_else(|| {
                                epub.markdown
                                    .visual_selection()
                                    .map(|(_, _, text)| text)
                            })?;
                        let chapter = epub
                            .book
                            .chapters
                            .get(epub.chapter_index)
                            .map(|chapter| chapter.title.as_str())
                            .unwrap_or("Unknown chapter");
                        Some(format!(
                            "Help me think about this passage from “{}” by {} ({}):\n\n> {}\n\nExplain its significance and connections. Do not save anything to my notes unless I ask.",
                            epub.book.metadata.title,
                            if epub.book.metadata.creators.is_empty() {
                                "Unknown author".to_string()
                            } else {
                                epub.book.metadata.creators.join(", ")
                            },
                            chapter,
                            quote.trim().replace('\n', "\n> "),
                        ))
                    });
                if let Some(prompt) = context {
                    self.open_neoism_agent_tab();
                    if let Some(agent) =
                        self.context_manager.current_mut().neoism_agent.as_mut()
                    {
                        agent.insert_text(&prompt);
                    }
                }
            }
            EpubContextAction::DeleteAnnotation { annotation_id } => {
                if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                    let _ = epub.remove_annotation(&annotation_id);
                }
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn open_epub_book_note(&mut self, annotation_id: &str) -> bool {
        let path = self
            .context_manager
            .current_mut()
            .epub
            .as_mut()
            .and_then(|epub| epub.sync_annotation_to_book_note(annotation_id).ok());
        let Some(path) = path else {
            return false;
        };
        self.renderer.notes_sidebar.refresh_notes();
        self.open_path_in_markdown(path);
        let marker = format!("<!-- neoism-epub-annotation:start {annotation_id} -->");
        if let Some(markdown) = self.context_manager.current_mut().active_markdown_mut() {
            if let Some(line) =
                markdown.lines.iter().position(|line| line.trim() == marker)
            {
                markdown.reveal_source_line(line.saturating_add(1));
            }
        }
        self.mark_dirty();
        true
    }

    pub fn open_path_in_epub(&mut self, path: PathBuf) {
        let workspace_root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| path.parent().map(std::path::Path::to_path_buf));
        if let Some(root) = workspace_root {
            self.set_active_workspace_root(root, false);
        }
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.buffer_tabs.ensure_terminal_tab();
        // EPUBs use the rich-document tab target; activation dispatches by
        // extension so no generic tab type needs to know book internals.
        self.renderer.buffer_tabs.open_markdown(path.clone());
        self.renderer.file_tree.set_active_path(Some(path.clone()));
        if let Some(id) = self.current_workspace_id() {
            self.workspace_editor_active_paths.insert(id, path.clone());
        }
        self.activate_epub_path(path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_epub_path(&mut self, path: PathBuf) {
        if let Some((_route_id, node)) = self.context_manager.epub_node_by_path(&path) {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self
            .context_manager
            .add_stacked_epub(path, rich_text_id, &mut self.sugarloaf)
        {
            self.file_tree_notify(
                "Could not open EPUB reader",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }

    pub(crate) fn rebind_current_epub_path(
        &mut self,
        old: &std::path::Path,
        new: PathBuf,
    ) {
        let Some(epub) = self.context_manager.current_mut().epub.as_mut() else {
            return;
        };
        if epub.book.path != old {
            return;
        }
        epub.book.path = new.clone();
        epub.markdown.path = new.clone();
        epub.state.last_known_path = new.canonicalize().unwrap_or_else(|_| new.clone());
        let _ = epub.save_state();
        self.renderer.buffer_tabs.rename_path(old, new.clone());
        self.renderer.file_tree.set_active_path(Some(new.clone()));
        if let Some(id) = self.current_workspace_id() {
            self.workspace_editor_active_paths.insert(id, new);
        }
        self.mark_dirty();
    }

    pub(crate) fn epub_next_chapter(&mut self) -> bool {
        let result = self
            .context_manager
            .current_mut()
            .epub
            .as_mut()
            .map(|epub| epub.next_chapter());
        match result {
            Some(Ok(changed)) => {
                if changed {
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                }
                changed
            }
            Some(Err(error)) => {
                self.renderer.notifications.push(
                    format!("Could not open next chapter: {error}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            None => false,
        }
    }

    pub(crate) fn epub_previous_chapter(&mut self) -> bool {
        let result = self
            .context_manager
            .current_mut()
            .epub
            .as_mut()
            .map(|epub| epub.previous_chapter());
        match result {
            Some(Ok(changed)) => {
                if changed {
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                }
                changed
            }
            Some(Err(error)) => {
                self.renderer.notifications.push(
                    format!("Could not open previous chapter: {error}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            None => false,
        }
    }

    /// Turn one bounded reader page, rolling across spine chapters at either edge.
    pub(crate) fn turn_epub_page(&mut self, direction: i8) -> bool {
        let direction = direction.signum();
        if direction == 0 || self.context_manager.current().epub.is_none() {
            return false;
        }
        let result = self
            .context_manager
            .current_mut()
            .epub
            .as_mut()
            .map(|epub| {
                if direction > 0 {
                    epub.next_page()
                } else {
                    epub.previous_page()
                }
            });
        match result {
            Some(Ok(changed)) => {
                if changed {
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                }
                changed
            }
            Some(Err(error)) => {
                self.renderer.notifications.push(
                    format!("Could not turn book page: {error}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                self.mark_dirty();
                true
            }
            None => false,
        }
    }

    /// Narrow, unobtrusive page-turn hit zones at the book pane's edges.
    /// Links win before this check, and the center remains available for text
    /// selection, so book interaction does not feel like one giant button.
    pub(crate) fn epub_page_turn_direction_at(&self, x: f32, y: f32) -> Option<i8> {
        self.context_manager.current().epub.as_ref()?;
        let scale = self.sugarloaf.scale_factor();
        let grid = self.context_manager.current_grid();
        let item = grid.current_item()?;
        let margin = grid.get_scaled_margin();
        let left = (item.layout_rect[0] + margin.left) / scale;
        let top = (item.layout_rect[1] + margin.top) / scale;
        let width = item.layout_rect[2] / scale;
        let height = item.layout_rect[3] / scale;
        if x < left || x > left + width || y < top || y > top + height {
            return None;
        }
        let edge = (width * 0.1).clamp(44.0, 72.0);
        if x <= left + edge {
            Some(-1)
        } else if x >= left + width - edge {
            Some(1)
        } else {
            None
        }
    }

    pub(crate) fn epub_page_turn_hovered(&self) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        self.epub_page_turn_direction_at(x, y).is_some()
    }
}

fn compact_book_text(value: &str, max_words: usize) -> String {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.len() <= max_words {
        words.join(" ")
    } else {
        format!("{}…", words[..max_words].join(" "))
    }
}

fn normalize_context_color(color: &str) -> &str {
    match color {
        "green" | "blue" | "pink" | "purple" => color,
        _ => "yellow",
    }
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

pub fn is_epub_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
}
