//! Read-only staging intent snapshots for hosts with canonical document owners.
//! Existing panel actions remain available to hosts that do not need a save barrier.
#[cfg(test)]
mod tests;
use super::types::{FileStatus, GitAction, VisualRowKind};
use super::GitDiffPanel;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitStagePlan {
    pub root: PathBuf,
    /// Exact repository-relative paths selected when the action was invoked.
    pub paths: Vec<String>,
    pub unstage: bool,
}

impl GitDiffPanel {
    pub fn file_stage_plan(&self, index: usize) -> Option<GitStagePlan> {
        let data = self.data.lock().ok()?;
        let file = data.files.get(index)?;
        Some(GitStagePlan {
            root: data.repo_root.clone()?,
            paths: vec![file.path.clone()],
            unstage: file.staged,
        })
    }

    pub fn selected_stage_plan(&self) -> Option<GitStagePlan> {
        self.file_stage_plan(self.selected)
    }

    pub fn folder_stage_plan(&self, visual_index: usize) -> Option<GitStagePlan> {
        let VisualRowKind::Group { file_indices, .. } =
            &self.visual_rows.get(visual_index)?.kind
        else {
            return None;
        };
        let data = self.data.lock().ok()?;
        let children: Vec<_> = file_indices
            .iter()
            .filter_map(|index| data.files.get(*index))
            .collect();
        if children.is_empty() {
            return None;
        }
        let unstage = children
            .iter()
            .all(|file| file.staged && file.status != FileStatus::Mixed);
        let paths = children
            .into_iter()
            .filter(|file| unstage || !file.staged || file.status == FileStatus::Mixed)
            .map(|file| file.path.clone())
            .collect();
        Some(GitStagePlan {
            root: data.repo_root.clone()?,
            paths,
            unstage,
        })
    }

    pub fn stage_all_plan(&self) -> Option<GitStagePlan> {
        let data = self.data.lock().ok()?;
        Some(GitStagePlan {
            root: data.repo_root.clone()?,
            paths: data
                .files
                .iter()
                .filter(|file| !file.staged || file.status == FileStatus::Mixed)
                .map(|file| file.path.clone())
                .collect(),
            unstage: false,
        })
    }

    /// Peek at a menu action without selecting it, closing the menu, or doing IO.
    /// `None` slot means the currently keyboard-selected action.
    pub fn action_stage_plan(&self, slot: Option<usize>) -> Option<GitStagePlan> {
        let menu = self.actions_menu.content();
        let item = menu
            .items()
            .get(slot.unwrap_or_else(|| menu.selected_index()))?;
        if !item.enabled || !self.git_action_enabled(item.action) {
            return None;
        }
        match item.action {
            GitAction::StageAll => self.stage_all_plan(),
            GitAction::ToggleStageSelected => self.selected_stage_plan(),
            _ => None,
        }
    }

    /// Submit a previously captured exact target list as ONE mutation. In
    /// particular, a folder must not fan out into independently busy-gated jobs.
    /// The native host calls this only after its canonical save barrier clears.
    pub fn apply_stage_plan(&mut self, plan: GitStagePlan) -> bool {
        if self.io.is_none()
            || self.loading()
            || self.repo_root().as_ref() != Some(&plan.root)
            || plan.paths.is_empty()
        {
            return false;
        }
        self.run_mutation(move |io, root| {
            for path in plan.paths {
                if plan.unstage {
                    io.unstage(root, &path)?;
                } else {
                    io.stage(root, &path)?;
                }
            }
            Ok(())
        });
        true
    }
}
