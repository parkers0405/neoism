use super::*;
use std::path::PathBuf;

impl Screen<'_> {
    pub(crate) fn open_markdown_file_link_prompt(&mut self) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };
        let Some(pane) = self.context_manager.current().markdown.as_ref() else {
            return;
        };
        if pane.remote_source {
            self.file_tree_notify(
                "Use a host-relative Markdown link in a remote document",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        self.renderer.modal.open(ModalSpec {
            title: "Link to Markdown File".into(),
            body: "Enter an existing Markdown file path. Files outside the vault are supported. Relative paths start at this document's folder.".into(),
            meta: "Inserts a standard Markdown link at the cursor; does not copy or move the file.".into(),
            input: Some(ModalInputSpec { value: String::new(), placeholder: "../project/docs/overview.md".into() }),
            buttons: vec![
                ModalButton::new("Insert Link", "Enter", ModalAction::MarkdownFileLink { document: pane.path.clone(), value: String::new() }),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            blocking: true,
            busy: false,
        });
        self.mark_dirty();
    }

    pub(crate) fn insert_markdown_file_link(&mut self, document: PathBuf, input: &str) {
        let result = (|| -> Result<(), String> {
            let input = input.trim();
            if input.is_empty() {
                return Err("Enter a Markdown file path".into());
            }
            let target = if let Some(suffix) = input.strip_prefix("~/") {
                dirs::home_dir()
                    .ok_or("Home directory is unavailable")?
                    .join(suffix)
            } else if input.starts_with("file://") {
                url::Url::parse(input)
                    .map_err(|error| error.to_string())?
                    .to_file_path()
                    .map_err(|_| "Invalid file URL")?
            } else {
                document
                    .parent()
                    .ok_or("Document has no parent folder")?
                    .join(input)
            };
            let target = std::fs::canonicalize(target)
                .map_err(|error| format!("Cannot link file: {error}"))?;
            if !target.is_file()
                || !neoism_ui::editor::markdown::is_markdown_path(&target)
            {
                return Err("Choose an existing Markdown file".into());
            }
            let link = neoism_ui::editor::markdown::links::markdown_file_link(
                &document, &target,
            )
            .ok_or("Cannot represent this file path as a link")?;
            let pane = self
                .context_manager
                .markdown_pane_mut_by_path(&document)
                .ok_or("The source document is no longer open")?;
            if pane.remote_source {
                return Err("Local paths cannot be inserted into a host-owned document by this picker".into());
            }
            pane.insert_text(&link);
            self.sync_markdown_tab_modified(&document, true);
            Ok(())
        })();
        if let Err(error) = result {
            self.file_tree_notify(
                error,
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
        self.mark_dirty();
    }
}
