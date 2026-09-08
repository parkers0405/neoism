//! Native hosts must take the same queued-RPC path as web hosts. Returning
//! immediately here (on the caller thread) prevents the native status/diff
//! worker from ever seeing a foreign filesystem root.
use neoism_ui::panels::git_diff::{FileChange, FileStatus, GitDiffIo, GitDiffPanel};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    thread::ThreadId,
};

struct HostIo {
    thread: ThreadId,
    calls: Mutex<Vec<String>>,
}
impl HostIo {
    fn record(&self, name: &str) {
        assert_eq!(
            self.thread,
            std::thread::current().id(),
            "host IO entered native worker"
        );
        self.calls.lock().unwrap().push(name.into());
    }
}
impl GitDiffIo for HostIo {
    fn is_host_driven(&self) -> bool {
        true
    }
    fn request_diff(&self, _: &str) {
        self.record("diff");
    }
    fn collect_files(&self, _: &Path) -> Vec<FileChange> {
        self.record("files");
        vec![]
    }
    fn stage(&self, _: &Path, _: &str) -> Result<(), String> {
        self.record("stage");
        Ok(())
    }
    fn unstage(&self, _: &Path, _: &str) -> Result<(), String> {
        self.record("unstage");
        Ok(())
    }
    fn commit(&self, _: &Path, _: &str) -> Result<(), String> {
        self.record("commit");
        Ok(())
    }
    fn list_branches(&self, _: &Path) -> Vec<String> {
        self.record("branches");
        vec![]
    }
    fn checkout(&self, _: &Path, _: &str) -> Result<(), String> {
        self.record("checkout");
        Ok(())
    }
}

#[test]
fn host_status_diff_stage_checkout_only_enqueue_and_keep_host_path_identity() {
    for root in [r"C:\host\repo", "/home/linux/repo"] {
        let io = Arc::new(HostIo {
            thread: std::thread::current().id(),
            calls: Mutex::new(vec![]),
        });
        let mut panel = GitDiffPanel::new();
        panel.set_io_provider(io.clone());
        panel.open(Some(root.into()), Some("main".into()));
        assert_eq!(&*io.calls.lock().unwrap(), &["files"]);
        panel.host_set_files(vec![FileChange {
            path: "src/main.rs".into(),
            status: FileStatus::Modified,
            additions: 1,
            deletions: 1,
            staged: false,
        }]);
        let (path, _) = panel.selected_file_target().unwrap();
        assert_eq!(
            path.as_os_str(),
            std::ffi::OsStr::new(
                neoism_protocol::host_path::HostPath::new(root)
                    .join("src/main.rs")
                    .as_str()
            )
        );
        panel.select_file(0);
        panel.load_branches();
        panel.toggle_stage_selected();
        panel.switch_branch("next".into());
        assert_eq!(
            &*io.calls.lock().unwrap(),
            &["files", "diff", "branches", "stage", "checkout"]
        );
    }
}
