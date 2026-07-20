use super::*;

pub(super) fn emit(tx: &UnboundedSender<ProgressEvent>, ev: ProgressEvent) {
    let _ = tx.send(ev);
}

pub(super) fn host_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.extend([
            home.join(".local").join("bin"),
            home.join(".cargo").join("bin"),
            home.join(".local").join("share").join("mise").join("shims"),
            home.join(".asdf").join("shims"),
        ]);
    }
    #[cfg(unix)]
    paths.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    if let Ok(path) = std::env::join_paths(unique) {
        command.env("PATH", path);
    }
    command
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
