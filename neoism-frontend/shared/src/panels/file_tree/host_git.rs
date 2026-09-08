//! Host Git decoration projection. No FilesService, GitService or guest path
//! discovery is involved; directory identities originate in host listings.
use super::{FileTree, GitStatus};
use neoism_protocol::{
    git::{GitChangeStatus, GitRepoStatus},
    host_path::HostPath,
};
use std::{collections::HashMap, path::PathBuf};

impl FileTree {
    /// Apply a snapshot only to its declared workspace. Repository-relative
    /// paths are anchored at the HOST repo root (not the workspace subdir).
    /// Returns whether decorations changed; callers can asynchronously re-list
    /// open directories to insert/remove deleted ghost rows using this map.
    pub fn apply_host_git_snapshot(&mut self, snapshot: &GitRepoStatus) -> bool {
        if self.root.as_ref().map(|p| p.as_os_str())
            != Some(std::ffi::OsStr::new(&snapshot.workspace_root))
        {
            return false;
        }
        let root = HostPath::new(&snapshot.workspace_root);
        let mut statuses: HashMap<std::ffi::OsString, GitStatus> = HashMap::new();
        if let Some(repo) = snapshot.repo_root.as_ref() {
            let repo = HostPath::new(repo);
            for file in &snapshot.files {
                let absolute = repo.join(&file.path);
                let Some(relative) = root.relative(absolute.as_str()) else {
                    continue;
                };
                let status = match file.status {
                    GitChangeStatus::Modified => GitStatus::Modified,
                    GitChangeStatus::Staged => GitStatus::StagedModified,
                    GitChangeStatus::Mixed => GitStatus::Mixed,
                    GitChangeStatus::Added => GitStatus::Added,
                    GitChangeStatus::Deleted => GitStatus::Deleted,
                    GitChangeStatus::Renamed => GitStatus::Renamed,
                    GitChangeStatus::Untracked => GitStatus::Untracked,
                    GitChangeStatus::Conflict => GitStatus::Conflict,
                };
                let mut prefix = relative.as_str();
                loop {
                    let path = std::ffi::OsString::from(root.join(prefix).as_str());
                    statuses
                        .entry(path)
                        .and_modify(|old| *old = old.merge(status))
                        .or_insert(status);
                    let Some((parent, _)) = prefix.rsplit_once('/') else {
                        break;
                    };
                    prefix = parent;
                }
            }
        }
        let changed = self.host_git_statuses != statuses;
        self.host_git_root = Some(std::ffi::OsString::from(&snapshot.workspace_root));
        // Local/host-joined scan compatibility; async host listings exclusively
        // use the raw-key map below and never consult guest PathBuf equality.
        self.git_statuses = statuses
            .iter()
            .map(|(path, status)| (PathBuf::from(path), *status))
            .collect();
        self.host_git_statuses = statuses;
        let mut changed = changed;
        for entry in &mut self.entries {
            let next = entry
                .path
                .as_ref()
                .and_then(|path| self.host_git_statuses.get(path.as_os_str()))
                .copied()
                .unwrap_or_default();
            changed |= entry.git_status != next;
            entry.git_status = next;
        }
        changed
    }

    /// Decorate an authoritative host directory listing and synthesize deleted
    /// children without asking the guest filesystem or normalizing host paths.
    pub(super) fn apply_host_git_children(
        &self,
        dir: &std::path::Path,
        depth: u8,
        children: &mut Vec<super::TreeEntry>,
    ) {
        use super::{NodeKind, TreeEntry};
        if self.root.as_ref().map(|p| p.as_os_str()) != self.host_git_root.as_deref() {
            return;
        }
        for entry in children.iter_mut() {
            entry.git_status = entry
                .path
                .as_ref()
                .and_then(|path| self.host_git_statuses.get(path.as_os_str()))
                .copied()
                .unwrap_or_default();
        }
        let dir = HostPath::new(dir.to_string_lossy());
        // Linear projection, including mass deletes: never scan every status
        // for every deleted leaf or every existing row on a Mac guest.
        let directories: std::collections::HashSet<String> = self
            .host_git_statuses
            .keys()
            .filter_map(|path| dir.relative(&path.to_string_lossy()))
            .filter_map(|relative| {
                relative.split_once('/').map(|(name, _)| name.to_owned())
            })
            .collect();
        let mut seen: std::collections::HashSet<std::ffi::OsString> = children
            .iter()
            .filter_map(|entry| entry.path.as_ref().map(|p| p.as_os_str().to_owned()))
            .collect();
        for (path, status) in &self.host_git_statuses {
            if *status != GitStatus::Deleted {
                continue;
            }
            let Some(relative) = dir
                .relative(&path.to_string_lossy())
                .filter(|r| !r.is_empty())
            else {
                continue;
            };
            let (label, descendant) = relative
                .split_once('/')
                .map(|(a, _)| (a, true))
                .unwrap_or((&relative, false));
            if !super::scan::entry_is_visible(
                std::path::Path::new(dir.as_str()),
                label,
                self.show_hidden,
            ) {
                continue;
            }
            let child_path = dir.join(label);
            if !seen.insert(std::ffi::OsString::from(child_path.as_str())) {
                continue;
            }
            // Parent rollups are also Deleted; directory kind comes from raw
            // descendants, not guest PathBuf::starts_with.
            let has_children = descendant || directories.contains(label);
            children.push(TreeEntry {
                label: label.into(),
                depth,
                kind: if has_children {
                    NodeKind::Dir { open: false }
                } else {
                    NodeKind::File
                },
                path: Some(child_path.as_str().into()),
                git_status: *status,
                virtual_kind: None,
            });
        }
        children.sort_by(|a, b| {
            let a_file = matches!(a.kind, NodeKind::File);
            let b_file = matches!(b.kind, NodeKind::File);
            a_file
                .cmp(&b_file)
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NodeKind, TreeEntry};
    use super::*;
    use neoism_protocol::git::GitFileChange;
    fn file(path: &str, status: GitChangeStatus) -> GitFileChange {
        GitFileChange {
            path: path.into(),
            status,
            additions: 1,
            deletions: 0,
            staged: false,
        }
    }
    fn entry(path: &str, kind: NodeKind) -> TreeEntry {
        TreeEntry {
            label: path.into(),
            path: Some(path.into()),
            kind,
            depth: 0,
            git_status: GitStatus::None,
            virtual_kind: None,
        }
    }
    #[test]
    fn host_decorations_are_lexical_for_windows_and_unix_hosts() {
        for repo in [r"C:\host\repo", r"\\server\share\repo", "/home/linux/repo"] {
            let repo = HostPath::new(repo);
            let root = repo.join("sub");
            let mut tree = FileTree::empty();
            tree.root = Some(root.as_str().into());
            tree.entries = vec![
                entry(root.join("tracked.rs").as_str(), NodeKind::File),
                entry(root.join("dir").as_str(), NodeKind::Dir { open: false }),
            ];
            let mut snapshot = GitRepoStatus {
                workspace_root: root.as_str().into(),
                repo_root: Some(repo.as_str().into()),
                branch: Some("main".into()),
                files: vec![
                    file("sub/tracked.rs", GitChangeStatus::Staged),
                    file("sub/dir/new.rs", GitChangeStatus::Untracked),
                    file("sub/old.rs", GitChangeStatus::Deleted),
                    file("sub/renamed.rs", GitChangeStatus::Renamed),
                    file("outside.rs", GitChangeStatus::Modified),
                ],
                error: None,
            };
            assert!(tree.apply_host_git_snapshot(&snapshot));
            assert_eq!(tree.entries[0].git_status, GitStatus::StagedModified);
            assert_eq!(tree.entries[1].git_status, GitStatus::Untracked);
            assert!(!tree
                .git_statuses
                .contains_key(&PathBuf::from(repo.join("outside.rs").as_str())));
            let mut ghosts = vec![];
            tree.apply_host_git_children(&PathBuf::from(root.as_str()), 0, &mut ghosts);
            assert!(ghosts.iter().any(|e| e.path.as_ref().unwrap().as_os_str()
                == std::ffi::OsStr::new(root.join("old.rs").as_str())
                && e.git_status == GitStatus::Deleted));
            let saved = tree.git_statuses.clone();
            snapshot.workspace_root = repo.join("another-workspace").as_str().into();
            assert!(!tree.apply_host_git_snapshot(&snapshot));
            assert_eq!(tree.git_statuses, saved);
            snapshot.workspace_root = root.as_str().into();
            snapshot.files.clear();
            assert!(tree.apply_host_git_snapshot(&snapshot));
            assert!(tree.entries.iter().all(|e| e.git_status == GitStatus::None));
            assert!(super::super::scan::entries_from_dir_listing(
                &PathBuf::from(root.as_str()),
                0,
                &tree.git_statuses,
                vec![],
                true
            )
            .is_empty());
        }
    }
    #[test]
    fn unix_backslash_names_never_alias_directories_on_windows_guests() {
        let mut tree = FileTree::empty();
        tree.root = Some("/host/repo".into());
        let snapshot = GitRepoStatus {
            workspace_root: "/host/repo".into(),
            repo_root: Some("/host/repo".into()),
            branch: None,
            error: None,
            files: vec![
                file(r"src\literal", GitChangeStatus::Untracked),
                file("src/literal", GitChangeStatus::Deleted),
            ],
        };
        tree.entries = vec![
            entry(r"/host/repo/src\literal", NodeKind::File),
            entry("/host/repo/src/literal", NodeKind::File),
        ];
        assert!(tree.apply_host_git_snapshot(&snapshot));
        assert_eq!(tree.entries[0].git_status, GitStatus::Untracked);
        assert_eq!(tree.entries[1].git_status, GitStatus::Deleted);
        let mut children = vec![entry(r"/host/repo/src\literal", NodeKind::File)];
        tree.apply_host_git_children(
            std::path::Path::new("/host/repo"),
            0,
            &mut children,
        );
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|e| e.path.as_ref().unwrap().as_os_str()
            == std::ffi::OsStr::new("/host/repo/src")
            && e.kind == (NodeKind::Dir { open: false })));
        assert!(children.iter().any(|e| e.path.as_ref().unwrap().as_os_str()
            == std::ffi::OsStr::new(r"/host/repo/src\literal")
            && e.git_status == GitStatus::Untracked));
    }
}
