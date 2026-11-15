use std::path::Path;

/// Called only by the standalone process runner, before any worker threads exist.
#[cfg(target_os = "macos")]
pub(crate) fn initialize_tool_path() {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let home = dirs::home_dir();
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    let captured = capture_shell_path(Path::new(&shell), home.as_deref(), std::time::Duration::from_secs(3));
    if captured.is_none() {
        eprintln!("neoism: login-shell PATH unavailable; using inherited PATH and tool-directory fallbacks");
    }
    if let Ok(path) = tool_path(&inherited, captured.as_deref(), home.as_deref()) {
        std::env::set_var("PATH", path);
    }
}

#[cfg(target_os = "macos")]
fn tool_path(
    inherited: &std::ffi::OsStr,
    shell: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
) -> Result<std::ffi::OsString, std::env::JoinPathsError> {
    let mut paths = Vec::new();
    // Finder's system-only PATH must not outrank shell-selected tools. Preserve
    // custom launch-time precedence for CLI users (including activated envs).
    let system_only = std::env::split_paths(inherited).all(|path| {
        matches!(path.to_str(), Some("/usr/bin" | "/bin" | "/usr/sbin" | "/sbin" | ""))
    });
    let sources = if system_only { [shell, Some(inherited)] } else { [Some(inherited), shell] };
    for source in sources.into_iter().flatten() {
        for path in std::env::split_paths(source) {
            if !path.as_os_str().is_empty() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    let mut fallback = Vec::new();
    if let Some(home) = home {
        for suffix in [".cargo/bin", ".local/bin", ".nix-profile/bin", ".local/state/nix/profile/bin"] {
            fallback.push(home.join(suffix));
        }
    }
    for path in ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/opt/homebrew/opt/rustup/bin", "/usr/local/bin", "/usr/local/sbin", "/usr/local/opt/rustup/bin", "/nix/var/nix/profiles/default/bin"] {
        fallback.push(path.into());
    }
    for path in fallback {
        if path.is_dir() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    std::env::join_paths(paths)
}

#[cfg(target_os = "macos")]
fn capture_shell_path(shell: &Path, home: Option<&Path>, timeout: std::time::Duration) -> Option<std::ffi::OsString> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::{ffi::OsStringExt, process::CommandExt};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut command = Command::new(shell);
    // Read the exported value rather than expanding $PATH: fish represents it
    // as a list internally. NUL records also preserve spaces and newlines.
    command.args(["-lic", "/usr/bin/printf '\\0NEOISM_ENV\\0'; /usr/bin/env -0"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .process_group(0);
    // Do not source shell configuration from an untrusted workspace directory.
    command.current_dir(home.unwrap_or(Path::new("/")));
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Nonblocking reads bound both noisy profiles and descendants retaining stdout.
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    let nonblocking = flags >= 0 && unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0;
    let started = Instant::now();
    let mut bytes = Vec::new();
    let mut result = None;
    while nonblocking && started.elapsed() < timeout && bytes.len() < 64 * 1024 {
        let mut buffer = [0; 4096];
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                bytes.extend_from_slice(&buffer[..count]);
                let marker = b"\0NEOISM_ENV\0";
                if let Some(start) = bytes.windows(marker.len()).position(|part| part == marker) {
                    let records = &bytes[start + marker.len()..];
                    for record in records.split_inclusive(|byte| *byte == 0) {
                        if let Some(value) = record.strip_suffix(&[0]).and_then(|record| record.strip_prefix(b"PATH=")) {
                            if !value.is_empty() {
                                result = Some(std::ffi::OsString::from_vec(value.to_vec()));
                            }
                            break;
                        }
                    }
                    if result.is_some() {
                        break;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    // The probe owns a separate process group, including profile-spawned helpers.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
    let _ = child.wait();
    result
}

#[cfg(all(test, target_os = "macos"))]
mod path_tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn preserves_explicit_precedence_and_adds_shell_and_profile_paths() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".cargo/bin")).unwrap();
        let path = tool_path(OsStr::new("/custom/bin:/usr/bin"), Some(OsStr::new("/brew/bin:/usr/bin:/with space/bin")), Some(home.path())).unwrap();
        let paths: Vec<_> = std::env::split_paths(&path).collect();
        assert_eq!(&paths[..4], &[Path::new("/custom/bin"), Path::new("/usr/bin"), Path::new("/brew/bin"), Path::new("/with space/bin")]);
        assert!(paths.contains(&home.path().join(".cargo/bin")));
        assert_eq!(paths.iter().filter(|p| *p == Path::new("/usr/bin")).count(), 1);
        let gui_path = tool_path(OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin"), Some(OsStr::new("/brew/bin:/usr/bin:/bin")), Some(home.path())).unwrap();
        assert_eq!(std::env::split_paths(&gui_path).next().unwrap(), Path::new("/brew/bin"));
    }

    #[test]
    fn recovered_path_resolves_server_and_its_child_runtime() {
        use neoism_agent_service_api::{ExecutablePurpose, ExecutableRequest, ExecutableService, StandardExecutableService};
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("custom tools/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, script) in [("test-language-server", "#!/bin/sh\nexec test-runtime\n"), ("test-runtime", "#!/bin/sh\nprintf runtime-found\n")] {
            let executable = bin.join(name);
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = tool_path(OsStr::new("/usr/bin:/bin"), Some(bin.as_os_str()), Some(home.path())).unwrap();
        let mut request = ExecutableRequest::new("test-language-server", ExecutablePurpose::LanguageServer);
        request.search_paths = std::env::split_paths(&path).collect();
        let server = StandardExecutableService.resolve(&request).unwrap();
        let output = std::process::Command::new(server.path).env("PATH", path).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"runtime-found");
    }

    #[test]
    fn captures_login_profile_despite_noise_and_bounds_hung_profiles() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join(".bash_profile");
        std::fs::write(&profile, "printf 'profile noise\\n'; export PATH='/custom tools/bin:/usr/bin:/bin'\n").unwrap();
        // Bash uses HOME for its profile, so use a wrapper to isolate the test
        // from the real user's dotfiles without mutating the test process env.
        let wrapper = home.path().join("shell");
        std::fs::write(&wrapper, format!("#!/bin/sh\nexport HOME='{}'\nexec /bin/bash \"$@\"\n", home.path().display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let timeout = std::time::Duration::from_secs(2);
        assert_eq!(capture_shell_path(&wrapper, Some(home.path()), timeout).unwrap(), OsStr::new("/custom tools/bin:/usr/bin:/bin"));
        std::fs::write(&profile, "/bin/sleep 30\n").unwrap();
        let started = std::time::Instant::now();
        assert!(capture_shell_path(&wrapper, Some(home.path()), std::time::Duration::from_millis(100)).is_none());
        assert!(started.elapsed() < timeout);
    }
}

/// A host-linked frontend/daemon may need a loader override (for example
/// LD_LIBRARY_PATH=/usr/lib for the native display). Nix executables have their
/// own loader/RPATH; inheriting that override mixes two glibcs and can kill the
/// shell before its first prompt. Change only the child's environment overlay, not the
/// daemon's, and leave ordinary system/custom shells' overrides alone.
pub(crate) fn prepare_shell_environment(env: &mut Vec<(String, String)>, executable: &Path) {
    #[cfg(target_os = "linux")]
    if executable.starts_with("/nix/store") {
        env.retain(|(key, _)| key != "LD_LIBRARY_PATH");
        // PtySessionConfig.env is an overlay on Command's inherited environment,
        // not a replacement. An absent key would still inherit the daemon's
        // value. Empty disables the override without introducing a new env API.
        env.push(("LD_LIBRARY_PATH".into(), String::new()));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (env, executable);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn nix_shell_does_not_inherit_host_loader_override() {
        let mut env = vec![("LD_LIBRARY_PATH".into(), "/usr/lib".into()), ("TERM".into(), "xterm-256color".into())];
        prepare_shell_environment(&mut env, Path::new("/nix/store/example-zsh/bin/zsh"));
        assert_eq!(env, vec![("TERM".into(), "xterm-256color".into()), ("LD_LIBRARY_PATH".into(), String::new())]);
    }

    #[test]
    fn system_and_custom_shell_environments_are_preserved() {
        for shell in ["/usr/bin/zsh", "/opt/shell/bin/bash", "/nix/store-like/bin/zsh"] {
            let mut env = vec![("LD_LIBRARY_PATH".into(), "/custom/lib".into())];
            let original = env.clone();
            prepare_shell_environment(&mut env, Path::new(shell));
            assert_eq!(env, original);
        }
    }

    #[test]
    fn configured_nix_zsh_stays_interactive_with_debug_host_environment() {
        use std::io::Write;
        use std::process::Stdio;

        let Some(shell) = std::env::var_os("SHELL") else { return };
        let Ok(executable) = std::fs::canonicalize(shell) else { return };
        if !executable.starts_with("/nix/store") || executable.file_name().and_then(|s| s.to_str()) != Some("zsh") { return }
        let mut env = vec![("LD_LIBRARY_PATH".into(), "/usr/lib".into())];
        prepare_shell_environment(&mut env, &executable);
        // teletypewriter's Command inherits the daemon environment and then
        // overlays PtySessionConfig.env. Omitting a key does NOT unset it.
        let mut child = std::process::Command::new(executable)
            .env("LD_LIBRARY_PATH", "/usr/lib").envs(env).arg("-i")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(11));
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            panic!("shell exited before input: {}", String::from_utf8_lossy(&output.stderr));
        }
        child.stdin.as_mut().unwrap().write_all(b"printf 'NEOISM_SHELL_STABLE\\n'\nexit 0\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("NEOISM_SHELL_STABLE"));
    }
}