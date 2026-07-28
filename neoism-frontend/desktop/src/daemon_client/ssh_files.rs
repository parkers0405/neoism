//! Shelled-out `ssh` [`FilesService`] for FOLLOW-THE-TERMINAL browsing.
//!
//! When the user types `ssh [user@]host` in a terminal pane the file
//! tree flips onto the remote host's disk — with zero setup on the
//! remote. Every listing shells out to the system `ssh` (so it honours
//! the user's `~/.ssh/config`, keys, and agent) over a per-session
//! ControlMaster connection, and parses `ls` output. It answers the
//! same async contract as the daemon files plane: `list_dir` returns
//! `IoError::Pending(request_id)` immediately and the real reply lands
//! later through `Screen::apply_daemon_files_message`.
//!
//! Delivery can't ride the `RioEvent` enum as a typed payload
//! (`neoism-backend` doesn't depend on `neoism-protocol`), so replies
//! travel on an `mpsc` channel and a unit [`RioEvent::SshFilesReady`]
//! nudges the UI thread to drain it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use neoism_protocol::files::{DirEntry, FilesServerMessage};

use crate::editor::file_tree::state::RemoteFileSource;
use crate::event::{EventProxy, RioEvent, RioEventType, WindowId};

/// Reply carried from a listing thread back to the UI drain: the
/// correlation id the tree's pending map keyed on, plus the built
/// files message.
pub type SshFilesReply = (u64, FilesServerMessage);

pub struct SshFiles {
    /// The `[user@]host` token pulled off the `ssh` command line.
    target: String,
    /// Extra flags parsed off the command (`-p 2222`, `-i key`, ...).
    /// May be empty; passed through verbatim to every `ssh` invocation
    /// so the browsing connection matches the interactive one.
    ssh_opts: Vec<String>,
    /// Per-session ControlMaster socket. Named from the target + the
    /// caller-supplied id (no rand/clock in this backend) and torn down
    /// in `Drop`.
    control_path: PathBuf,
    /// Remote directory the tree roots at. `"."` means "the ssh
    /// command's default cwd", i.e. the remote login home.
    root: PathBuf,
    /// UI-thread delivery channel for finished listings / file reads.
    reply_tx: Sender<SshFilesReply>,
    /// Request-id source. Starts at 1; each `list_dir` / read allocates
    /// the next value and hands it to the tree's pending map.
    handle_alloc: Arc<AtomicU64>,
    /// Wakes the window once a reply is queued so the drain runs.
    event_proxy: EventProxy,
    window_id: WindowId,
}

impl SshFiles {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target: String,
        ssh_opts: Vec<String>,
        root: PathBuf,
        id: u64,
        reply_tx: Sender<SshFilesReply>,
        event_proxy: EventProxy,
        window_id: WindowId,
    ) -> Self {
        let control_path = control_socket_path(&target, id);
        let this = Self {
            target,
            ssh_opts,
            control_path,
            root,
            reply_tx,
            handle_alloc: Arc::new(AtomicU64::new(1)),
            event_proxy,
            window_id,
        };
        this.spawn_control_master();
        this
    }

    /// Bring up the multiplexing master in the background. Detached in
    /// its own thread — `-f` makes `ssh` fork the master off and the
    /// spawned process exits fast, so `.status()` returns quickly and
    /// reaps it (no zombie). `BatchMode=yes` is the load-bearing flag:
    /// it fails fast instead of hanging on an interactive password
    /// prompt, so a keys/agent dev box connects silently and anything
    /// needing a password degrades to "tree shows empty" rather than a
    /// frozen UI. A failed master is non-fatal: later `ls` calls just
    /// fail and the tree stays empty.
    fn spawn_control_master(&self) {
        let mut command = Command::new("ssh");
        command
            .arg("-f")
            .arg("-N")
            .arg("-M")
            .arg("-S")
            .arg(&self.control_path)
            .arg("-o")
            .arg("ControlPersist=180")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=8")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .args(&self.ssh_opts)
            .arg(&self.target);
        let target = self.target.clone();
        std::thread::spawn(move || match command.status() {
            Ok(status) if status.success() => {
                tracing::info!(
                    target: "neoism::ssh_files",
                    %target,
                    "ssh control master established"
                );
            }
            Ok(status) => {
                tracing::warn!(
                    target: "neoism::ssh_files",
                    %target,
                    ?status,
                    "ssh control master exited non-zero (tree will be empty)"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::ssh_files",
                    %target,
                    %error,
                    "ssh control master failed to spawn"
                );
            }
        });
    }

    /// A `Command` primed with the shared multiplexing flags, ready for
    /// a `--`-separated remote command to be appended by the caller.
    fn ssh_base(&self) -> Command {
        let mut command = Command::new("ssh");
        command
            .arg("-S")
            .arg(&self.control_path)
            .arg("-o")
            .arg("BatchMode=yes")
            .args(&self.ssh_opts)
            .arg(&self.target)
            .arg("--");
        command
    }

    /// Absolute remote dir a tree path resolves to. Tree paths are
    /// `root.join(name)` so they already sit under `root`; anything
    /// that drifts out falls back to the root rather than listing a
    /// stray absolute path.
    fn resolve_dir<'a>(&'a self, path: &'a Path) -> &'a Path {
        if path.starts_with(&self.root) {
            path
        } else {
            &self.root
        }
    }

    fn next_request_id(&self) -> u64 {
        self.handle_alloc.fetch_add(1, Ordering::Relaxed)
    }
}

impl RemoteFileSource for SshFiles {
    fn root(&self) -> &Path {
        &self.root
    }

    fn request_read_file(&self, path: &Path) -> u64 {
        let request_id = self.next_request_id();
        let reply_path = path.to_string_lossy().into_owned();
        let mut command = self.ssh_base();
        command.arg("cat").arg("--").arg(shell_quote(&reply_path));
        let reply_tx = self.reply_tx.clone();
        let event_proxy = self.event_proxy.clone();
        let window_id = self.window_id;
        // Detached thread: this backend has no tokio runtime handle, and
        // the `ssh` call blocks. Running it on a std thread that owns the
        // reply channel is the whole async story here.
        std::thread::spawn(move || {
            let message = match command.output() {
                Ok(output) => FilesServerMessage::FileContent {
                    path: reply_path,
                    bytes: output.stdout,
                },
                Err(error) => FilesServerMessage::Error {
                    message: format!("ssh cat failed: {error}"),
                },
            };
            reply_and_wake(&reply_tx, &event_proxy, window_id, request_id, message);
        });
        request_id
    }

    fn as_files_service(&self) -> &dyn neoism_ui::services::FilesService {
        self
    }
}

impl neoism_ui::services::FilesService for SshFiles {
    fn list_dir(
        &self,
        path: &Path,
    ) -> Result<Vec<neoism_ui::services::DirEntry>, neoism_ui::services::IoError> {
        let request_id = self.next_request_id();
        let dir = self.resolve_dir(path);
        // `-p` appends a trailing `/` to directories (and only dirs), so
        // the child kind is a one-char test on each line. `-A` includes
        // dotfiles (the tree filters them itself). `-1` forces one entry
        // per line. Reply `path` echoes what the tree asked for — the
        // tree keys the splice on `request_id`, not this string.
        let reply_path = path.to_string_lossy().into_owned();
        let dir_arg = shell_quote(&dir.to_string_lossy());
        let mut command = self.ssh_base();
        command
            .arg("ls")
            .arg("-Ap")
            .arg("-1")
            .arg("--color=never")
            .arg("--")
            .arg(dir_arg);
        let reply_tx = self.reply_tx.clone();
        let event_proxy = self.event_proxy.clone();
        let window_id = self.window_id;
        std::thread::spawn(move || {
            let message = match command.output() {
                Ok(output) => FilesServerMessage::DirListing {
                    path: reply_path,
                    entries: parse_ls_output(&output.stdout),
                },
                Err(error) => FilesServerMessage::Error {
                    message: format!("ssh ls failed: {error}"),
                },
            };
            reply_and_wake(&reply_tx, &event_proxy, window_id, request_id, message);
        });
        Err(neoism_ui::services::IoError::Pending(request_id))
    }

    fn read_file(&self, _path: &Path) -> Result<Vec<u8>, neoism_ui::services::IoError> {
        // Pane opens fetch bytes through `request_read_file`'s async
        // reply; a synchronous read has nothing to hand back.
        Err(neoism_ui::services::IoError::Other(
            "remote file reads go through the async ssh fetch".into(),
        ))
    }

    fn write_file(
        &self,
        _path: &Path,
        _bytes: &[u8],
    ) -> Result<(), neoism_ui::services::IoError> {
        Err(neoism_ui::services::IoError::Other(
            "editing over ssh isn't supported yet".into(),
        ))
    }

    fn stat(
        &self,
        _path: &Path,
    ) -> Result<neoism_ui::services::DirEntry, neoism_ui::services::IoError> {
        Err(neoism_ui::services::IoError::Other(
            "remote stat over ssh isn't supported".into(),
        ))
    }
}

impl Drop for SshFiles {
    fn drop(&mut self) {
        // Best-effort teardown of the multiplexing master. `ControlPersist`
        // would reap it eventually anyway; this just returns the socket
        // promptly when the tree leaves remote mode.
        let _ = Command::new("ssh")
            .arg("-S")
            .arg(&self.control_path)
            .arg("-O")
            .arg("exit")
            .args(&self.ssh_opts)
            .arg(&self.target)
            .output();
    }
}

/// Send a reply onto the UI channel and post the drain nudge.
fn reply_and_wake(
    reply_tx: &Sender<SshFilesReply>,
    event_proxy: &EventProxy,
    window_id: WindowId,
    request_id: u64,
    message: FilesServerMessage,
) {
    if reply_tx.send((request_id, message)).is_ok() {
        event_proxy.send_event(RioEventType::Rio(RioEvent::SshFilesReady), window_id);
    }
}

/// Per-session ControlMaster socket path under the system temp dir.
/// The target is sanitized to a filesystem-safe token and bounded so
/// the socket path stays well under the OS `sun_path` limit.
fn control_socket_path(target: &str, id: u64) -> PathBuf {
    let mut token: String = target
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    token.truncate(48);
    std::env::temp_dir().join(format!("neoism-ssh-{token}-{id}.sock"))
}

/// Single-quote a string for a POSIX shell, escaping embedded single
/// quotes as `'\''`. The remote runs the `ls`/`cat` args through its
/// login shell, so paths with spaces or quotes must arrive intact.
fn shell_quote(raw: &str) -> String {
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('\'');
    for ch in raw.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// Turn `ls -Ap -1` stdout into wire `DirEntry`s. A trailing `/` (from
/// `-p`) marks a directory; strip it for the name. Empty lines skipped.
fn parse_ls_output(stdout: &[u8]) -> Vec<DirEntry> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            if let Some(name) = line.strip_suffix('/') {
                DirEntry {
                    name: name.to_string(),
                    is_dir: true,
                    size: None,
                }
            } else {
                DirEntry {
                    name: line.to_string(),
                    is_dir: false,
                    size: None,
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("src"), "'src'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn parse_ls_output_marks_dirs_by_trailing_slash() {
        let entries = parse_ls_output(b"src/\nmain.rs\n.hidden\ndocs/\n");
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].name, "src");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "main.rs");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[2].name, ".hidden");
        assert!(!entries[2].is_dir);
        assert_eq!(entries[3].name, "docs");
        assert!(entries[3].is_dir);
    }

    #[test]
    fn parse_ls_output_skips_blank_lines() {
        let entries = parse_ls_output(b"\n\nonly.txt\n\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "only.txt");
    }

    #[test]
    fn control_socket_path_is_sanitized() {
        let path = control_socket_path("user@dev.box:2222", 7);
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("neoism-ssh-user_dev_box_2222-7"));
        assert!(name.ends_with(".sock"));
    }
}
