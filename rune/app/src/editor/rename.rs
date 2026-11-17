//! A successful host rename moves the live buffer, never reopens its disk contents.
use crate::{app::App, tabs::TabKind};

fn host(root: &str, path: &str) -> String {
    if crate::startup::absolute(path) { path.replace('\\', "/") }
    else { format!("{}/{}", root.trim_end_matches('/'), path) }
}

impl App {
    pub(crate) fn editor_rename_paths(&self, root: &str, from: &str, to: &str) -> Vec<(String, String)> {
        let from = host(root, from);
        let to = host(root, to);
        self.buffers.iter().filter_map(|(key, buffer)| {
            let path = buffer.host_path();
            let suffix = if path == from { "" } else { path.strip_prefix(&from)?.strip_prefix('/')? };
            let moved = if suffix.is_empty() { to.clone() } else { format!("{to}/{suffix}") };
            let moved = if crate::startup::absolute(key) { moved } else {
                moved.strip_prefix(&format!("{}/", buffer.scope.trim_end_matches('/'))).unwrap_or(&moved).to_owned()
            };
            Some((key.clone(), moved))
        }).collect()
    }

    pub(crate) fn prepare_editor_rename(&mut self, root: &str, from: &str, to: &str) -> bool {
        let paths = self.editor_rename_paths(root, from, to);
        if paths.iter().any(|(old, new)| self.buffers[old].saving.is_some() || self.buffers[old].renaming
            || (old != new && self.buffers.contains_key(new) && !paths.iter().any(|(key, _)| key == new))) {
            self.modal = Some(crate::ui::modal::Modal::notice("Cannot rename yet", "Wait for any save or rename to finish, and close an already-open destination file."));
            return false;
        }
        for (old, _) in paths { self.buffers.get_mut(&old).unwrap().renaming = true; }
        true
    }

    pub(crate) fn cancel_editor_rename(&mut self, root: &str, from: &str) {
        for (old, _) in self.editor_rename_paths(root, from, from) {
            self.buffers.get_mut(&old).unwrap().renaming = false;
        }
    }

    pub(crate) fn apply_editor_renamed(&mut self, root: &str, from: &str, to: &str) {
        let paths = self.editor_rename_paths(root, from, to);
        for (old, new) in paths {
            if old == new { self.buffers.get_mut(&old).unwrap().renaming = false; continue; }
            let Some(mut buffer) = self.buffers.remove(&old) else { continue };
            buffer.path = new.clone();
            buffer.renaming = false;
            buffer.kept_scroll = buffer.scroll.get().0;
            buffer.shown = false;
            buffer.code_cache = Default::default();
            buffer.markdown_cache = Default::default();
            buffer.git = Default::default();
            self.buffers.insert(new.clone(), buffer);
            for tab in &mut self.tabs {
                if matches!(&tab.kind, TabKind::Editor { path } if path == &old) { tab.kind = TabKind::Editor { path: new.clone() }; }
            }
            self.lsp_retarget(&old, &new);
            self.collab_retarget(&new);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Focus, crdt::CrdtTextBuffer};
    use neoism_protocol::crdt::CrdtServerMessage as In;

    #[test]
    fn rename_preserves_dirty_replica_undo_caret_and_tab_identity() {
        let mut app = App::new();
        let old = "/vault/Agenda.md";
        let new = "/vault/Planning.md";
        app.open_file(old, None);
        app.apply_opened_file(old, b"saved\nsecond\n");
        let id = App::buffer_id(old);
        let hub = CrdtTextBuffer::with_text(9_000_000_000, "saved\nsecond\n");
        app.apply_crdt(0, In::Snapshot { buffer_id: id, update_v1: hub.encode_full_update_v1(), state_vector_v1: hub.state_vector_v1() });
        let b = app.buffers.get_mut(old).unwrap();
        b.doc.save_undo();
        b.doc.lines[0] = "unsaved".into();
        b.doc.mark_edited();
        app.collab_flush();
        assert!(app.buffers[old].shared.as_ref().unwrap().in_flight());
        let owner = app.buffers[old].shared.as_ref().unwrap().owner_id.clone();
        let tab = app.active_tab().id;
        app.focus = Focus::Sidebar;
        assert!(app.prepare_editor_rename("/vault", "Agenda.md", "Planning.md"));
        let b = app.buffers.get_mut(old).unwrap();
        b.doc.break_undo_group();
        b.doc.lines[1] = "typed while renaming".into();
        b.doc.mark_edited();
        b.doc.cursor_line = 1;
        b.doc.cursor_col = 2;
        b.doc.follow_cursor = false;
        b.scroll.set((180.0, 400.0, 100.0));
        app.collab_flush();
        app.apply_editor_renamed("/vault", "Agenda.md", "Planning.md");
        assert!(!app.buffers.contains_key(old));
        assert_eq!(app.active_tab().id, tab);
        assert_eq!(app.active_tab().kind, TabKind::Editor { path: new.into() });
        assert_eq!(app.focus, Focus::Sidebar);
        let b = app.active_buffer_mut().unwrap();
        assert_eq!(b.doc.lines, ["unsaved", "typed while renaming"]);
        assert!(b.dirty());
        assert!(!b.renaming);
        assert_eq!(b.kept_scroll, 180.0);
        assert_eq!((b.doc.cursor_line, b.doc.cursor_col), (1, 2));
        assert!(!b.doc.follow_cursor);
        let binding = b.shared.as_ref().unwrap();
        assert_eq!(binding.buffer_id, App::buffer_id(new));
        assert_eq!(binding.owner_id, owner);
        assert!(binding.is_seeded());
        let text = b.doc.text_for_disk();
        assert!(b.doc.undo());
        app.collab_flush();
        assert_ne!(app.active_buffer().unwrap().doc.text_for_disk(), text);
        app.active_buffer_mut().unwrap().doc.redo();
        app.collab_flush();
        assert_eq!(app.active_buffer().unwrap().doc.text_for_disk(), text);
    }

    #[test]
    fn folder_rename_updates_only_descendants_and_conflicts_leave_buffers_untouched() {
        let mut app = App::new();
        app.cwd = "/project".into();
        for path in ["Ideas/a.md", "Ideas/deep/b.md", "Ideas-other/c.md", "/other/Ideas/a.md"] {
            app.open_file(path, None);
            app.apply_opened_file(path, b"saved\n");
        }
        assert!(app.prepare_editor_rename("/project", "Ideas", "Archive"));
        app.cancel_editor_rename("/project", "Ideas");
        assert!(!app.buffers["Ideas/a.md"].renaming);
        assert_eq!(app.editor_rename_paths("/project", "Ideas", "Archive").len(), 2);
        app.apply_editor_renamed("/project", "Ideas", "Archive");
        assert!(app.buffers.contains_key("Archive/a.md"));
        assert!(app.buffers.contains_key("Archive/deep/b.md"));
        assert!(app.buffers.contains_key("Ideas-other/c.md"));
        assert!(app.buffers.contains_key("/other/Ideas/a.md"));
        app.open_file("Existing.md", None);
        app.apply_opened_file("Existing.md", b"keep this\n");
        assert!(!app.prepare_editor_rename("/project", "Archive/a.md", "Existing.md"));
        assert_eq!(app.buffers["Existing.md"].doc.lines, ["keep this"]);
        assert!(!app.buffers["Archive/a.md"].renaming);
    }

    #[test]
    fn rejected_stale_save_keeps_dirty_text_and_does_not_fall_back_to_the_old_filename() {
        let mut app = App::new();
        let path = "/vault/Old.md";
        app.open_file(path, None);
        app.apply_opened_file(path, b"saved\n");
        let tab = app.active_tab().id;
        let b = app.active_buffer_mut().unwrap();
        b.doc.lines[0] = "unsaved".into();
        b.doc.mark_edited();
        b.saving = Some(b.doc.lines.clone());
        b.saving_request = Some(13);
        b.close_after_save = Some(tab);
        assert!(app.apply_crdt(13, In::Error { buffer_id: Some(App::buffer_id(path)), message: "buffer was renamed; reopen its new path".into() }));
        let b = app.active_buffer().unwrap();
        assert_eq!(b.doc.lines, ["unsaved"]);
        assert!(b.dirty());
        assert!(b.saving.is_none());
        assert!(b.saving_request.is_none());
        assert!(b.close_after_save.is_none());
        assert!(b.message.as_ref().unwrap().contains("renamed"));
        assert_eq!(app.active_tab().id, tab);
    }
}