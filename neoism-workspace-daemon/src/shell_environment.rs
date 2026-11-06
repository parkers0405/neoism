use std::path::Path;

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