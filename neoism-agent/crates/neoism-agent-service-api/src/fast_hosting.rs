//! Owned temporary dedicated daemons. This is lifecycle isolation, NOT an
//! execution sandbox: invited collaborators' shells/agents run as the host user.
use std::{
    io,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

#[cfg(unix)]
mod process_owner;

pub struct FastDaemon {
    child: Child,
    _state: tempfile::TempDir,
    pub directory: PathBuf,
    pub address: SocketAddr,
    stopped: bool,
    #[cfg(unix)]
    processes: process_owner::ProcessOwner,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}
// The Windows job handle is exclusively owned and only closed during teardown.
#[cfg(windows)]
unsafe impl Send for FastDaemon {}
impl FastDaemon {
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        #[cfg(unix)]
        self.processes.refresh();
        self.child.try_wait()
    }
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        self.processes.kill();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for FastDaemon {
    fn drop(&mut self) {
        self.stop();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

pub fn validate_fast_directory(directory: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    let root = dunce::canonicalize(directory)?;
    #[cfg(not(windows))]
    let root = std::fs::canonicalize(directory)?;
    if !root.is_dir() || root.parent().is_none() {
        return Err(io::Error::other(
            "Choose a project directory, not a filesystem root, for Fast Host",
        ));
    }
    Ok(root)
}
fn original_config_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("NEOISM_CONFIG_HOME") {
        return Some(directory.into());
    }
    #[cfg(windows)]
    {
        return dirs::home_dir().map(|home| home.join("AppData/Local/neoism"));
    }
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|home| home.join(".config/neoism"));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
            .map(|dir| dir.join("neoism"))
    }
}

fn credential_source_paths(parent_directory: &Path) -> (PathBuf, PathBuf) {
    // Match LocalProviderCredentialStore/LocalMcpCredentialStore, BEFORE the
    // child changes cwd. In particular Windows/HOME-unset and relative explicit
    // paths must still refer to the host's real stores, not the chosen project.
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        });
    let provider = std::env::var_os("NEOISM_AGENT_AUTH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            state_home
                .clone()
                .unwrap_or_else(|| ".state".into())
                .join("neoism-agent/auth.json")
        });
    let mcp = std::env::var_os("NEOISM_AGENT_MCP_AUTH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("NEOISM_AGENT_STATE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    state_home
                        .map(|base| base.join("neoism"))
                        .unwrap_or_else(|| ".neoism/state".into())
                })
                .join("mcp-auth.json")
        });
    (
        resolve_before_chdir(provider, parent_directory),
        resolve_before_chdir(mcp, parent_directory),
    )
}
fn resolve_before_chdir(path: PathBuf, parent_directory: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        parent_directory.join(path)
    }
}

fn free_loopback() -> io::Result<SocketAddr> {
    TcpListener::bind("127.0.0.1:0")?.local_addr()
}

/// Fresh state, loopback-only bind, mandatory daemon auth, exact declared root.
/// Unix descendants use a process group and a daemon-side parent death watcher;
/// Windows descendants use a kill-on-close job. No detached saved-server path.
pub fn spawn_fast_daemon(
    binary: &Path,
    directory: &Path,
    token: &str,
) -> io::Result<FastDaemon> {
    let directory = validate_fast_directory(directory)?;
    let binary = std::fs::canonicalize(binary)?;
    let state = tempfile::Builder::new()
        .prefix("neoism-fast-daemon-")
        .tempdir()?;
    let address = free_loopback()?;
    let agent_address = free_loopback()?;
    // Snapshot only the product config, never the saved server/device registry
    // or daemon token. Preserve model/provider defaults without letting this
    // temporary daemon write the desktop's global config.
    let credentials = state.path().join("credentials");
    std::fs::create_dir(&credentials)?;
    let source_config = original_config_directory().map(|dir| dir.join("config.json"));
    if let Some(source) = source_config.filter(|path| path.is_file()) {
        std::fs::copy(source, credentials.join("config.json"))?;
    }
    let (provider_auth, mcp_auth) = credential_source_paths(&std::env::current_dir()?);
    let mut command = Command::new(binary);
    command
        .args([
            "--addr",
            &address.to_string(),
            "--no-unix-socket",
            "--ephemeral",
            "--temporary-parent-pid",
            &std::process::id().to_string(),
            "--state-dir",
        ])
        .arg(state.path().join("state"))
        .arg("--workspace")
        .arg(&directory)
        .current_dir(&directory)
        .env("NEOISM_AGENT_AUTH_PATH", provider_auth)
        .env("NEOISM_AGENT_MCP_AUTH_PATH", mcp_auth)
        .env("NEOISM_FAST_SERVER", "1")
        .env("NEOISM_REQUIRE_AUTH", "1")
        .env("NEOISM_DAEMON_TOKEN", token)
        .env("NEOISM_DAEMON_DATA_DIR", state.path().join("auth"))
        .env("NEOISM_CONFIG_DIR", state.path().join("config"))
        .env("NEOISM_CONFIG_HOME", state.path().join("credentials"))
        .env("NEOISM_AGENT_SERVER", format!("http://{agent_address}"))
        .env("NEOISM_SERVER", format!("http://{agent_address}"))
        .env_remove("NEOISM_PROVISION_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if job.is_null()
                || SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
            {
                let error = io::Error::last_os_error();
                let _ = child.kill();
                let _ = child.wait();
                if !job.is_null() {
                    CloseHandle(job);
                }
                return Err(error);
            }
            job
        }
    };
    #[cfg(not(windows))]
    let _ = &mut child;
    #[cfg(unix)]
    let processes = process_owner::ProcessOwner::new(child.id());
    Ok(FastDaemon {
        #[cfg(unix)]
        processes,
        child,
        _state: state,
        directory,
        address,
        stopped: false,
        #[cfg(windows)]
        job,
    })
}

/// Best-effort teardown of owned Unix descendants, including PTYs which
/// create their own sessions. This is lifecycle cleanup, not containment of
/// deliberately double-forked/detached programs started by trusted guests.
#[cfg(unix)]
pub fn terminate_fast_process_tree(pid: u32) {
    process_owner::ProcessOwner::new(pid).kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_paths_are_resolved_before_project_chdir() {
        let parent = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_before_chdir(".state/auth.json".into(), parent.path()),
            parent.path().join(".state/auth.json")
        );
        let absolute = parent.path().join("auth.json");
        assert_eq!(
            resolve_before_chdir(absolute.clone(), parent.path()),
            absolute
        );
        let (provider, mcp) = credential_source_paths(parent.path());
        assert!(provider.is_absolute());
        assert!(mcp.is_absolute());
    }
    #[cfg(unix)]
    #[test]
    fn child_uses_host_credential_stores_and_private_product_config() {
        use std::{os::unix::fs::PermissionsExt, time::Duration};
        let project = tempfile::tempdir().unwrap();
        let binary = project.path().join("env-daemon");
        std::fs::write(&binary, "#!/bin/sh\nprintf '%s\\n' \"$NEOISM_AGENT_AUTH_PATH\" \"$NEOISM_AGENT_MCP_AUTH_PATH\" \"$NEOISM_CONFIG_HOME\" > stores\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let expected = credential_source_paths(&std::env::current_dir().unwrap());
        let mut daemon =
            spawn_fast_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while daemon.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = std::fs::read_to_string(project.path().join("stores")).unwrap();
        let paths: Vec<_> = text.lines().map(PathBuf::from).collect();
        assert_eq!(paths[0], expected.0);
        assert_eq!(paths[1], expected.1);
        assert!(paths[2].starts_with(daemon._state.path()));
    }

    #[test]
    fn root_and_missing_directory_are_rejected() {
        #[cfg(unix)]
        assert!(validate_fast_directory(Path::new("/")).is_err());
        assert!(
            validate_fast_directory(Path::new("/nonexistent-fast-project-1234")).is_err()
        );
        let project = tempfile::tempdir().unwrap();
        assert!(validate_fast_directory(project.path()).is_ok());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn stop_cleans_retained_pty_after_direct_daemon_crash() {
        use std::{
            os::unix::fs::PermissionsExt,
            time::{Duration, Instant},
        };
        let project = tempfile::tempdir().unwrap();
        let binary = project.path().join("pty-daemon");
        std::fs::write(&binary, "#!/bin/sh\n/usr/bin/setsid /bin/sleep 30 &\necho $! > pty.pid\n/bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let mut daemon =
            spawn_fast_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !project.path().join("pty.pid").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(daemon.try_wait().unwrap().is_none()); // live ownership poll
        let pty: u32 = std::fs::read_to_string(project.path().join("pty.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        daemon.child.kill().unwrap();
        daemon.child.wait().unwrap();
        assert!(daemon.try_wait().unwrap().is_some());
        daemon.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pty}/stat"));
            if stat.is_err()
                || stat
                    .unwrap()
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .split_whitespace()
                    .next()
                    == Some("Z")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "retained PTY survived daemon stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn owned_daemon_is_reaped_and_project_files_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("fake-daemon");
        std::fs::write(&binary, "#!/bin/sh\nwhile :; do /bin/sleep 1; done\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let data = directory.path().join("keep.txt");
        std::fs::write(&data, "kept").unwrap();
        let mut daemon =
            spawn_fast_daemon(&binary, directory.path(), "private-master").unwrap();
        let state = daemon._state.path().to_owned();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(daemon.try_wait().unwrap().is_none());
        drop(daemon);
        assert!(!state.exists());
        assert_eq!(std::fs::read_to_string(&data).unwrap(), "kept");
    }
}
