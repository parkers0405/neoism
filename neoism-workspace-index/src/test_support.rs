//! Process-wide environment isolation shared by all notes tests.
use std::fs;
use std::path::{Path, PathBuf};

/// `NEOISM_NOTES_HOME` is process-global state and the vault registry
/// lives inside it, but `cargo test` runs tests on parallel threads of
/// one process. Any test that reads or mutates the notes home must hold
/// this lock for its full duration — otherwise another test's `set_var`
/// lands between this test's setup and its assertions, and registry
/// writes teleport into the other test's temp home.
static NOTES_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn lock_notes_home() -> std::sync::MutexGuard<'static, ()> {
    NOTES_HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A per-test notes home: takes the global lock, points
/// `NEOISM_NOTES_HOME` at a fresh temp dir, and restores the previous
/// value when dropped.
pub(crate) struct NotesHome {
    root: PathBuf,
    previous: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for NotesHome {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl AsRef<Path> for NotesHome {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

impl Drop for NotesHome {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var("NEOISM_NOTES_HOME", value),
                None => std::env::remove_var("NEOISM_NOTES_HOME"),
            }
        }
    }
}

pub(crate) fn temp_notes_home(name: &str) -> NotesHome {
    let lock = lock_notes_home();
    let root = std::env::temp_dir()
        .join(format!("neoism-notes-home-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let previous = std::env::var_os("NEOISM_NOTES_HOME");
    unsafe {
        std::env::set_var("NEOISM_NOTES_HOME", &root);
    }
    NotesHome {
        root,
        previous,
        _lock: lock,
    }
}
