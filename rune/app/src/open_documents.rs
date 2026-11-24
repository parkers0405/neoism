//! Finder opens the first workspace at launch, or a separate window if Neoism is running.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::App;

struct Request {
    directory: PathBuf,
    files: Vec<PathBuf>,
}

impl Request {
    fn into_state(self) -> App {
        let mut app = App::new_at(self.directory.to_string_lossy().into_owned());
        app.launch_documents = Some(self.files);
        app
    }
}

thread_local! {
    // AppKit delivers the cold-launch event before creating the first window. Once that
    // window initializes (`None`), subsequent requests must not change its workspace or tabs.
    static LAUNCH: RefCell<Option<Vec<Request>>> = const { RefCell::new(Some(Vec::new())) };
}

fn requests(paths: Vec<PathBuf>) -> Vec<Request> {
    let mut result: Vec<Request> = Vec::new();
    for path in paths.into_iter().filter(|p| p.is_absolute()) {
        let (directory, file) = if path.is_dir() {
            (path.as_path(), None)
        } else if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
            (parent, Some(name))
        } else { continue };
        let directory = directory.canonicalize().unwrap_or_else(|_| directory.to_path_buf());
        let i = result.iter().position(|request| request.directory == directory).unwrap_or_else(|| {
            result.push(Request { directory, files: Vec::new() });
            result.len() - 1
        });
        if let Some(file) = file {
            let request = &mut result[i];
            request.files.push(request.directory.join(file));
        }
    }
    result
}

pub(crate) fn receive(paths: Vec<PathBuf>) -> bool {
    let mut requests = requests(paths);
    if requests.is_empty() { return false; }
    LAUNCH.with_borrow_mut(|launch| {
        if let Some(pending) = launch { pending.append(&mut requests); }
    });
    open_windows(requests);
    true
}

pub(crate) fn initial_state() -> App {
    let mut requests = LAUNCH.with_borrow_mut(Option::take).unwrap_or_default().into_iter();
    let first = requests.next();
    open_windows(requests);
    first.map_or_else(App::new, Request::into_state)
}

fn open_windows(requests: impl IntoIterator<Item = Request>) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for request in requests {
        let key = format!("finder-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        rune::open_window(rune::WindowSpec::from_app(key, crate::window_app(|| request.into_state())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_files_open_in_their_parent_and_folders_open_as_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let project = root.join("My Project");
        let other = root.join("Other Project");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(&other).unwrap();
        let markdown = project.join("Read me #1.md");
        let source = project.join("main.rs");
        let requests = requests(vec![markdown.clone(), other.clone(), source.clone()]);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].directory, project);
        assert_eq!(requests[0].files, [markdown, source]);
        assert_eq!(requests[1].directory, other);
        assert!(requests[1].files.is_empty());
    }
}