//! Project the shared daemon watch onto workspace-relative rows, including closed folders.
use super::{Entry, Tree};
use neoism_protocol::{git::{GitChangeStatus, GitRepoStatus}, host_path::HostPath};
use std::collections::HashMap;

impl Tree {
    pub(crate) fn apply_git(&mut self, snapshot: &GitRepoStatus) {
        let root = HostPath::new(&snapshot.workspace_root);
        let mut projected = HashMap::<String, (GitChangeStatus, bool)>::new();
        if let Some(repo) = &snapshot.repo_root {
            let repo = HostPath::new(repo);
            for file in &snapshot.files {
                let Some(relative) = root.relative(repo.join(&file.path).as_str()).filter(|p| !p.is_empty()) else { continue };
                let mut path = relative.as_str();
                let mut directory = false;
                loop {
                    projected.entry(path.to_owned()).and_modify(|(status, is_dir)| {
                        if priority(file.status) > priority(*status) { *status = file.status; }
                        *is_dir |= directory;
                    }).or_insert((file.status, directory));
                    let Some((parent, _)) = path.rsplit_once('/') else { break };
                    path = parent;
                    directory = true;
                }
            }
        }
        let statuses = projected.iter().map(|(path, (status, _))| (path.clone(), *status)).collect();
        if self.git_statuses == statuses { return; }
        let selected = self.rows().get(self.selected).map(|row| row.path.clone());
        self.git_statuses = statuses;
        self.git_deleted.clear();
        for (path, (status, is_dir)) in projected {
            if status != GitChangeStatus::Deleted { continue; }
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", &path));
            self.git_deleted.entry(dir.to_owned()).or_default().push(Entry { name: name.to_owned(), is_dir });
        }
        let rows = self.rows();
        self.selected = selected.and_then(|path| rows.iter().position(|row| row.path == path))
            .unwrap_or_else(|| self.selected.min(rows.len().saturating_sub(1)));
    }
}

fn priority(status: GitChangeStatus) -> u8 {
    use GitChangeStatus::*;
    match status { Untracked => 1, Modified => 2, Staged | Renamed => 3, Mixed => 4, Added => 5, Deleted => 6, Conflict => 7 }
}

pub(crate) fn decoration(status: GitChangeStatus, p: &crate::ui::theme::Palette) -> (&'static str, u32) {
    use GitChangeStatus::*;
    match status {
        Modified => ("M", p.yellow), Staged => ("S", p.green), Mixed => ("M*", p.magenta),
        Added => ("A", p.green), Deleted => ("D", p.red), Renamed => ("R", p.blue),
        Untracked => ("?", p.cyan), Conflict => ("!", p.red),
    }
}

pub(crate) fn fixture(app: &mut crate::app::App) {
    use GitChangeStatus::*;
    let files = [("src/modified.rs", Modified), ("src/staged.rs", Staged), ("src/mixed.rs", Mixed),
        ("src/added.rs", Added), ("src/deleted.rs", Deleted), ("src/renamed.rs", Renamed),
        ("src/untracked.rs", Untracked), ("src/conflict.rs", Conflict), ("old/nested/gone.rs", Deleted),
        ("a-long-modified-filename-that-must-leave-room-for-its-marker.rs", Modified)];
    app.apply_dir_listing("", &serde_json::json!([
        {"name": "src", "is_dir": true}, {"name": "README.md", "is_dir": false},
        {"name": files.last().unwrap().0, "is_dir": false},
    ]));
    let entries: Vec<_> = files.iter().filter_map(|(path, status)| path.strip_prefix("src/")
        .filter(|_| *status != Deleted).map(|name| serde_json::json!({"name": name, "is_dir": false}))).collect();
    app.apply_dir_listing("src", &serde_json::json!(entries));
    app.tree.open.extend(["src".into(), "old".into(), "old/nested".into()]);
    let snapshot = GitRepoStatus {
        workspace_root: app.cwd.clone(), repo_root: Some(app.cwd.clone()), branch: Some("main".into()), upstream: None,
        files: files.into_iter().map(|(path, status)| neoism_protocol::git::GitFileChange {
            path: path.into(), status, additions: 0, deletions: 0, staged: status == Staged,
        }).collect(), error: None,
    };
    app.apply(crate::daemon::Inbound::Reply { family: "Git".into(), request_id: 0,
        message: serde_json::json!({"RepoStatus": {"token": "neoism-rune-git-status", "snapshot": snapshot}}) });
    app.sidebar = Some(crate::app::Sidebar::Files);
    app.focus = crate::app::Focus::Sidebar;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::App, daemon::Inbound};
    use neoism_protocol::git::{GitChangeStatus::*, GitFileChange};

    fn snapshot(root: &str, repo: &str, files: &[(&str, GitChangeStatus)]) -> GitRepoStatus {
        GitRepoStatus { workspace_root: root.into(), repo_root: Some(repo.into()), branch: Some("main".into()), upstream: None,
            files: files.iter().map(|(path, status)| GitFileChange { path: (*path).into(), status: *status, additions: 0, deletions: 0, staged: false }).collect(), error: None }
    }

    fn deliver(app: &mut App, token: &str, snapshot: &GitRepoStatus) {
        app.apply(Inbound::Reply { family: "Git".into(), request_id: 0,
            message: serde_json::json!({"RepoStatus": {"token": token, "snapshot": snapshot}}) });
    }

    #[test]
    fn git_paths_are_host_relative_and_roll_up_closed_folders() {
        for (root, repo) in [("/repo/app", "/repo"), (r"C:\repo\app", r"C:\repo"), (r"\\host\share\repo\app", r"\\host\share\repo")] {
            let mut tree = Tree::default();
            tree.apply_git(&snapshot(root, repo, &[("app/src/a.rs", Modified), ("app/src/b.rs", Staged),
                ("app/src/c.rs", Mixed), ("app/src/d.rs", Added), ("app/src/e.rs", Deleted),
                ("app/src/f.rs", Conflict), ("application/wrong.rs", Conflict), ("outside.rs", Modified)]));
            assert_eq!(tree.git_statuses["src"], Conflict);
            assert_eq!(tree.git_statuses["src/a.rs"], Modified);
            assert_eq!(tree.git_statuses.len(), 7, "outside-workspace files must not leak in");
            tree.apply_git(&snapshot(root, repo, &[("app/src/a.rs", Modified), ("app/src/b.rs", Staged)]));
            assert_eq!(tree.git_statuses["src"], Staged, "rollups use the shipping priority");
        }
    }

    #[test]
    fn shared_watch_updates_and_clears_without_losing_tree_state() {
        let mut app = App::new();
        app.cwd = "/repo".into();
        app.tree.set_listing("".into(), vec![Entry { name: "src".into(), is_dir: true }]);
        app.tree.set_listing("src".into(), vec![Entry { name: "main.rs".into(), is_dir: false }]);
        app.tree.open.insert("src".into());
        app.tree.selected = 1;
        let mut status = snapshot("/repo", "/repo", &[("src/main.rs", Mixed)]);
        deliver(&mut app, "neoism-rune-git-status", &status);
        assert_eq!(app.tree.git_statuses["src/main.rs"], Mixed);
        assert!(app.notes.git_statuses.is_empty());
        status.files[0].status = Conflict;
        deliver(&mut app, "another-watch", &status);
        status.workspace_root = "/another-workspace".into();
        deliver(&mut app, "neoism-rune-git-status", &status);
        status.workspace_root = "/repo".into();
        status.error = Some("temporary Git error".into());
        deliver(&mut app, "neoism-rune-git-status", &status);
        assert_eq!(app.tree.git_statuses["src/main.rs"], Mixed, "invalid/error snapshots retain the last good decorations");
        status.error = None;
        status.files.clear();
        deliver(&mut app, "neoism-rune-git-status", &status);
        assert!(app.tree.git_statuses.is_empty());
        assert!(app.tree.open.contains("src"));
        assert_eq!(app.tree.rows()[app.tree.selected].path, "src/main.rs");
    }

    #[test]
    fn deleted_rows_merge_with_disk_listings_and_disappear_when_clean() {
        let mut tree = Tree::default();
        tree.set_listing("".into(), vec![Entry { name: "src".into(), is_dir: true }, Entry { name: "z.rs".into(), is_dir: false }]);
        tree.set_listing("src".into(), vec![Entry { name: "still.rs".into(), is_dir: false }]);
        tree.open.extend(["old".into(), "old/nested".into(), "src".into()]);
        tree.selected = 2;
        let mut status = snapshot("/repo", "/repo", &[("old/nested/gone.rs", Deleted), ("src/gone.rs", Deleted), ("src/still.rs", Deleted)]);
        tree.apply_git(&status);
        let rows = tree.rows();
        assert_eq!(rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["old", "old/nested", "old/nested/gone.rs", "src", "src/gone.rs", "src/still.rs", "z.rs"]);
        assert!(rows[0].entry.is_dir && rows[1].entry.is_dir && !rows[2].entry.is_dir);
        assert_eq!(rows[tree.selected].path, "z.rs", "new ghost rows must not move the selection to another file");
        status.repo_root = None;
        status.files.clear();
        tree.apply_git(&status);
        assert_eq!(tree.rows().iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["src", "src/still.rs", "z.rs"]);
        assert_eq!(tree.rows()[tree.selected].path, "z.rs");
    }
}