//! Shared confirmation contract for native and web review headers.
use std::path::PathBuf;
use crate::widgets::modal::{ModalAction, ModalButton, ModalSpec};

pub fn restore_confirmation(root: PathBuf, path: String) -> ModalSpec {
    ModalSpec {
        title: "Restore unstaged changes?".into(),
        body: format!("Discard unstaged changes to `{path}`? The working file will be restored from the index."),
        meta: "Staged changes are preserved. Discarded edits cannot be undone.".into(),
        input: None,
        buttons: vec![
            ModalButton::new("Restore", "Enter", ModalAction::GitRestore { root, path }),
            ModalButton::new("Cancel", "Esc", ModalAction::Close),
        ],
        busy: false,
        blocking: true,
    }
}

pub fn restore_block_confirmation(
    root: PathBuf,
    path: String,
    block: neoism_protocol::git::GitChangeBlock,
) -> ModalSpec {
    ModalSpec {
        title: "Restore this edit?".into(),
        body: format!("Discard only this edit in `{path}`? Other working changes and staged changes are preserved."),
        meta: "Only the selected change block is restored. Discarded edits cannot be undone.".into(),
        input: None,
        buttons: vec![
            ModalButton::new("Restore edit", "Enter", ModalAction::GitRestoreBlock { root, path, block }),
            ModalButton::new("Cancel", "Esc", ModalAction::Close),
        ],
        busy: false,
        blocking: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_confirmation_keeps_exact_typed_target_and_payload() {
        let block = neoism_protocol::git::GitChangeBlock {
            old_start: 4, new_start: 7,
            old_lines: vec!["before".into()], new_lines: vec!["after".into()],
        };
        let spec = restore_block_confirmation("/repo".into(), "a.rs".into(), block.clone());
        assert!(spec.blocking);
        assert!(spec.body.contains("only this edit"));
        assert!(spec.body.contains("Other working changes and staged changes are preserved"));
        assert!(matches!(&spec.buttons[0].action, ModalAction::GitRestoreBlock { root, path, block: captured }
            if root == &PathBuf::from("/repo") && path == "a.rs" && captured == &block));
        assert!(matches!(&spec.buttons[1].action, ModalAction::Close));
    }

    #[test]
    fn restore_requires_explicit_root_scoped_confirmation() {
        let spec = restore_confirmation("/repo".into(), "src/a.rs".into());
        assert!(spec.blocking);
        assert!(spec.body.contains("index"));
        assert!(spec.meta.contains("preserved"));
        assert!(matches!(&spec.buttons[0].action, ModalAction::GitRestore { root, path } if root == &PathBuf::from("/repo") && path == "src/a.rs"));
        assert!(matches!(&spec.buttons[1].action, ModalAction::Close));
    }
}
