//! Source contracts supplement target compilation: std::process::Command has no
//! creation-flags getter, and Linux/Wine cannot prove native window visibility.

const SERVICE_HELPER: &str = include_str!("../src/background_process.rs");
const WORKSPACE: &str = include_str!("../src/workspace_management.rs");
const CREDENTIALS: &str = include_str!("../src/mcp_credentials.rs");
const CONFIG: &str = include_str!("../../neoism-agent-neoism-adapter/src/config.rs");
const DESKTOP_HELPER: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../neoism-frontend/desktop/src/background_process.rs"
));
const UPDATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../neoism-frontend/desktop/src/update.rs"
));
const ACP: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../neoism-frontend/desktop/src/neoism/acp/terminal.rs"
));
const DAEMON_HIDE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../neoism-workspace-daemon/src/windows_process.rs"
));

#[test]
fn service_helper_matches_gui_protocol_and_complete_hide_mask() {
    let service = SERVICE_HELPER.split("#[cfg(test)]").next().unwrap();
    for source in [service, DESKTOP_HELPER] {
        assert!(source.contains("CREATE_NO_WINDOW"));
        assert!(source.contains("CREATE_NEW_PROCESS_GROUP"));
        assert!(source.contains("--neoism-internal-background-command"));
        assert!(source.contains("eq_ignore_ascii_case(\"neoism.exe\")"));
        assert!(source.contains("#[cfg(not(windows))]"));
        assert!(!source.contains("DETACHED_PROCESS"));
    }
    // One mask after both host branches; don't accidentally hide only the GUI
    // wrapper while leaving standalone service commands visible.
    assert!(service.contains("command.creation_flags(HIDDEN_CONSOLE);"));
    assert_eq!(service.matches(".creation_flags(").count(), 1);
}

#[test]
fn scoped_git_acl_and_updater_launches_use_background_helpers() {
    assert!(!CONFIG.contains("Command::new(\"git\")"));
    assert!(CONFIG.contains("background_process::command(\"git\")"));
    assert!(!WORKSPACE.contains("Command::new(\"git\")"));
    assert!(
        WORKSPACE
            .matches("background_process::command(\"git\")")
            .count()
            >= 3
    );
    assert!(CREDENTIALS.contains("background_process::command(\"icacls.exe\")"));
    assert!(!CREDENTIALS.contains("Command::new(\"icacls.exe\")"));
    assert!(UPDATE.contains("background_process::command(\"curl\")"));
    assert!(UPDATE.contains("background_process::command(exe)"));
    assert!(!UPDATE.contains("Command::new("));
    // Keep curl's existing startup latency bounds on every platform.
    assert!(UPDATE.contains("\"--connect-timeout\""));
    assert!(UPDATE.contains("\"--max-time\""));
}

#[test]
fn acp_hides_the_direct_pipe_child_without_breaking_kill_ownership() {
    let creation = ACP
        .split("let mut child_cmd = Command::new(&command);")
        .nth(1)
        .unwrap();
    let before_spawn = creation.split("child_cmd.spawn()").next().unwrap();
    assert!(before_spawn.contains("#[cfg(windows)]"));
    assert!(before_spawn
        .contains("neoism_workspace_daemon::hide_std_command(&mut child_cmd)"));
    assert!(before_spawn.contains(".stdout(Stdio::piped())"));
    assert!(before_spawn.contains(".stderr(Stdio::piped())"));
    assert!(!ACP.contains("background_process::command("));
    assert!(ACP.contains("child.kill()"));
    assert!(DAEMON_HIDE
        .contains("HIDDEN_CONSOLE: u32 = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP"));
    assert!(DAEMON_HIDE.contains("command.creation_flags(HIDDEN_CONSOLE)"));
}
