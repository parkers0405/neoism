//! HEAD-to-live-buffer gutter marks, shared by Markdown and code tabs.
use std::{cell::RefCell, rc::Rc};
use crate::{app::App, daemon::Family};
use neoism_protocol::git::{GitClientMessage, GitServerMessage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mark {
    pub added: bool,
    pub modified: bool,
    pub deleted: bool,
}

#[derive(Default)]
pub struct State {
    pub base: Option<String>,
    pending: Option<u64>,
    refresh: bool,
    cache: RefCell<Option<(u64, Rc<Vec<Mark>>)>>,
}

impl State {
    pub fn marks(&self, doc: &super::Document) -> Rc<Vec<Mark>> {
        let mut cache = self.cache.borrow_mut();
        if let Some((_, marks)) = cache.as_ref().filter(|(rev, _)| *rev == doc.revision) {
            return marks.clone();
        }
        let marks = Rc::new(self.base.as_ref().map_or_else(Vec::new, |base| compute(base, &doc.text_for_disk())));
        *cache = Some((doc.revision, marks.clone()));
        marks
    }
}

fn compute(base: &str, current: &str) -> Vec<Mark> {
    let mut marks = vec![Mark::default(); current.split('\n').count().max(1)];
    let diff = similar::TextDiff::configure()
        .timeout(std::time::Duration::from_millis(10))
        .diff_lines(base, current);
    for op in diff.ops() {
        let range = op.new_range();
        match op.tag() {
            similar::DiffTag::Equal => {}
            similar::DiffTag::Insert | similar::DiffTag::Replace => {
                for mark in marks.iter_mut().take(range.end).skip(range.start) {
                    mark.added = op.tag() == similar::DiffTag::Insert;
                    mark.modified = op.tag() == similar::DiffTag::Replace;
                }
            }
            similar::DiffTag::Delete => {
                let anchor = range.start.saturating_sub(1).min(marks.len() - 1);
                marks[anchor].deleted = true;
            }
        }
    }
    marks
}

impl App {
    pub(crate) fn request_editor_git_base(&mut self, path: &str) {
        let Some(buffer) = self.buffers.get_mut(path) else { return };
        let Some(repo) = &self.git_panel.repo_root else { buffer.git = Default::default(); return };
        let root = neoism_protocol::host_path::HostPath::new(repo);
        let Some(relative) = root.relative(&buffer.host_path()).filter(|p| !p.is_empty()) else { return };
        if buffer.git.pending.is_some() { buffer.git.refresh = true; return; }
        let Some(daemon) = &self.daemon else { return };
        buffer.git.pending = Some(daemon.request(Family::Git,
            serde_json::to_value(GitClientMessage::ReadBase { path: relative }).unwrap(),
            Some(repo.clone())));
    }

    pub(crate) fn refresh_editor_git_bases(&mut self) {
        let paths: Vec<_> = self.buffers.keys().cloned().collect();
        for path in paths { self.request_editor_git_base(&path); }
    }

    pub(crate) fn apply_editor_git(&mut self, message: &GitServerMessage, request_id: u64) -> bool {
        let Some(path) = self.buffers.iter().find(|(_, b)| b.git.pending == Some(request_id)).map(|(p, _)| p.clone()) else { return false };
        let buffer = self.buffers.get_mut(&path).unwrap();
        buffer.git.pending = None;
        if let GitServerMessage::BaseFile { text, .. } = message {
            if buffer.git.base.as_ref() != Some(text) {
                buffer.git.base = Some(text.clone());
                *buffer.git.cache.borrow_mut() = None;
            }
        }
        if std::mem::take(&mut buffer.git.refresh) { self.request_editor_git_base(&path); }
        true
    }
}

pub(crate) fn fixture(app: &mut App, markdown: bool) {
    let (path, base, current) = if markdown {
        ("changes.md", "# Git changes\n\nOriginal paragraph.\n\nKeep this paragraph.\nRemoved paragraph.\n\nEnd.\n",
         "# Git changes\n\nUpdated paragraph.\n\nKeep this paragraph.\n\nEnd.\n\n- Added item\n- Another added item\n")
    } else {
        ("changes.rs", "fn main() {\n    let count = 1;\n    println!(\"hello\");\n    old_call();\n}\n",
         "fn main() {\n    let count = 2;\n    println!(\"hello\");\n}\n\nfn added() {\n    println!(\"new\");\n}\n")
    };
    app.open_file(path, None);
    app.apply_opened_file(path, current.as_bytes());
    app.buffers.get_mut(path).unwrap().git.base = Some(base.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additions_replacements_and_deletions_keep_live_line_positions() {
        let marks = compute("one\ntwo\nthree\nfour\nfive\n", "one\nchanged\nthree\nfive\nnew\n");
        assert!(marks[1].modified);
        assert!(marks[2].deleted);
        assert!(marks[4].added);
        assert_eq!(marks[0], Mark::default());
        assert!(compute("gone\n", "")[0].deleted);
        assert!(compute("", "new\n")[0].added);
        assert!(compute("same\n", "same\n").iter().all(|m| *m == Mark::default()));
    }

    #[test]
    fn both_file_types_update_after_unsaved_edit_and_baseline_reply() {
        for path in ["file.rs", "note.md"] {
            let mut app = App::new();
            app.open_file(path, None);
            app.apply_opened_file(path, b"one\ntwo\n");
            let b = app.buffers.get_mut(path).unwrap();
            b.git.pending = Some(42);
            assert!(app.apply_editor_git(&GitServerMessage::BaseFile { path: path.into(), text: "one\n".into() }, 42));
            let b = app.buffers.get_mut(path).unwrap();
            assert!(b.git.marks(&b.doc)[1].added);
            b.doc.replace_from_disk("one\n");
            assert!(b.git.marks(&b.doc).iter().all(|m| *m == Mark::default()));
            b.doc.insert_str_at_cursor("changed");
            b.doc.mark_edited();
            assert!(b.git.marks(&b.doc)[0].modified);
        }
    }
}