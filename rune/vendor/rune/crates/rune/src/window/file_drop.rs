//! A file drag from another app: what the host gathers between passes.

use std::time::Duration;

/// A file drag from another app over the window. winit reports one event per file; the host
/// gathers them and hands the runtime one drag and one drop (`Host::flush_files`).
#[derive(Default)]
pub(super) struct FileDrop {
    /// What winit reported since the last pass, in order.
    pub(super) steps: Vec<FileStep>,
    /// Samples the pointer while the drag is over the window, and where it last was.
    pub(super) pointer: Option<crate::input::drop_pointer::DropPointer>,
    pub(super) at: Option<(f32, f32)>,
}

pub(super) enum FileStep {
    Enter(Vec<std::path::PathBuf>),
    Drop(Vec<std::path::PathBuf>),
    Leave,
}

impl FileDrop {
    /// One file of a drag that entered (`drop` false) or dropped: the run of files it belongs
    /// to grows, or a new run starts.
    pub(super) fn file(&mut self, path: std::path::PathBuf, drop: bool) {
        match self.steps.last_mut() {
            Some(FileStep::Enter(files)) if !drop => files.push(path),
            Some(FileStep::Drop(files)) if drop => files.push(path),
            _ => self.steps.push(if drop { FileStep::Drop(vec![path]) } else { FileStep::Enter(vec![path]) }),
        }
    }
}

/// How often a file drag's position is sampled while it's over the window.
pub(super) const DROP_SAMPLE: Duration = Duration::from_millis(16);
