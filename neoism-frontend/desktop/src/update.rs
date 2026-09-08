use neoism_backend::event::{EventProxy, RioEvent, RioEventType, WindowId};
use std::cmp::Ordering;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

const DEFAULT_REPO: &str = "parkers0405/neoism";
const PROGRESS_PREFIX: &str = "NEOISM_UPDATE\t";

pub(crate) fn spawn_check(proxy: EventProxy, window_id: WindowId) {
    let check_releases = std::env::var_os("NEOISM_DISABLE_UPDATE_CHECK").is_none()
        && (!cfg!(debug_assertions) || std::env::var_os("NEOISM_UPDATE_CHECK").is_some());
    if !check_releases && !cfg!(any(windows, target_os = "macos")) {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("neoism-update-check".to_string())
        .spawn(move || {
            if let Some(message) = previous_update_notice() {
                send_progress(&proxy, window_id, None, message, false, true);
            }
            if !check_releases {
                return;
            }
            if let Some(version) = available_version() {
                proxy.send_event(
                    RioEventType::Rio(RioEvent::UpdateAvailable { version }),
                    window_id,
                );
            }
        });
}

fn previous_update_notice() -> Option<String> {
    #[cfg(windows)]
    let root = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("Neoism/updates");
    #[cfg(target_os = "macos")]
    let root = dirs::home_dir()?.join("Library/Application Support/Neoism/updates");
    #[cfg(any(windows, target_os = "macos"))]
    return read_update_notice(
        &root,
        &std::env::current_exe().ok()?,
        env!("CARGO_PKG_VERSION"),
    );
    #[cfg(not(any(windows, target_os = "macos")))]
    None
}

/// Inspect only the newest receipt for THIS installation. Receipts from another
/// app/portable copy must neither report success nor suppress its update error.
#[cfg(any(windows, target_os = "macos", test))]
fn read_update_notice(
    root: &std::path::Path,
    exe: &std::path::Path,
    current: &str,
) -> Option<String> {
    use sha2::{Digest, Sha256};
    let exe = std::fs::canonicalize(exe).ok()?;
    let mut latest = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let path = entry.path().join("result.json");
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if metadata.len() > 256 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let target = if let Some(executable) = value["target_executable"].as_str() {
            std::path::PathBuf::from(executable)
        } else if let Some(directory) = value["target"]["directory"].as_str() {
            std::path::PathBuf::from(directory).join("neoism.exe")
        } else if let Some(bundle) = value["target"].as_str() {
            std::path::PathBuf::from(bundle).join("Contents/MacOS/neoism")
        } else {
            continue;
        };
        if std::fs::canonicalize(target).ok().as_ref() != Some(&exe) {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(time, _, _, _)| modified > *time)
        {
            latest = Some((modified, path, bytes, value));
        }
    }
    let (_, path, bytes, value) = latest?;
    let state = value["state"]
        .as_str()
        .or_else(|| value["status"].as_str())?;
    let expected = value["expected_version"].as_str().unwrap_or_default();
    if valid_release_tag(expected) && is_newer(current, expected) {
        return None;
    }
    let message = match state {
        "failed" => format!("The previous update failed: {}", value["message"].as_str()
            .or_else(|| value["error"].as_str()).unwrap_or("the installer did not finish")),
        "reboot_required" => "The previous Windows installer requested a restart; replacement was not verified. Review its result before retrying.".into(),
        "succeeded" | "installed" | "open_accepted" if is_newer(expected, current) =>
            format!("The installer reported {expected}, but this process is still running {current}. Close old windows and launch the updated installation."),
        _ => return None,
    };
    let fingerprint = format!("{:x}", Sha256::digest(&bytes));
    let acknowledged = path.with_extension("seen");
    if std::fs::read_to_string(&acknowledged).ok().as_deref()
        == Some(fingerprint.as_str())
    {
        return None;
    }
    // Keep the original receipt and logs intact; acknowledge only this revision.
    let _ = std::fs::write(acknowledged, fingerprint);
    Some(format!("{message}\nDetails: {}", path.display()))
}

type ProgressRecord = (Option<u8>, String, bool, bool);

#[derive(Default)]
struct InstallProgress {
    terminal: Option<ProgressRecord>,
    failure_reported: bool,
    restart_sent: bool,
}

impl InstallProgress {
    fn observe(&mut self, record: ProgressRecord) -> Option<ProgressRecord> {
        if self.failure_reported {
            return None;
        }
        if record.3 {
            self.failure_reported = true;
            self.terminal = None;
            return Some((record.0, record.1, false, true));
        }
        if record.2 && record.0 == Some(100) {
            // The direct Unix replacement has finished and now waits for the
            // GUI to exit. Deferring this signal until child exit would deadlock.
            self.restart_sent = true;
            return Some(record);
        }
        if record.2 || record.0 == Some(100) {
            // A detached installer handoff is not an installed-success event.
            // First require the launching updater to exit successfully.
            let progress = (
                record.0.map(|value| value.min(95)),
                record.1.clone(),
                false,
                false,
            );
            self.terminal = Some(record);
            return Some(progress);
        }
        Some(record)
    }

    fn finish(self, result: Result<(), String>) -> Option<ProgressRecord> {
        if self.failure_reported {
            return None;
        }
        match result {
            Err(error) => Some((None, error, false, true)),
            Ok(()) if self.restart_sent => None,
            Ok(()) => self.terminal.or_else(|| {
                Some((
                    None,
                    "Updater exited without confirming installation or installer handoff"
                        .into(),
                    false,
                    true,
                ))
            }),
        }
    }
}

pub(crate) fn spawn_install(
    proxy: EventProxy,
    window_id: WindowId,
    version: String,
) -> std::io::Result<()> {
    if !valid_release_tag(&version) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid update release version",
        ));
    }
    let exe = std::env::current_exe()?;
    std::thread::Builder::new()
        .name("neoism-update-install".to_string())
        .spawn(move || {
            let child = crate::background_process::command(exe)
                .args([
                    "update",
                    "--gui",
                    "--relaunch",
                    "--parent-pid",
                    &std::process::id().to_string(),
                ])
                .arg("--target-version")
                .arg(&version)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(error) => {
                    send_progress(
                        &proxy,
                        window_id,
                        None,
                        error.to_string(),
                        false,
                        true,
                    );
                    return;
                }
            };
            let mut progress = InstallProgress::default();
            if let Some(stdout) = child.stdout.take() {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Some(record) = parse_progress_line(&line) else {
                        continue;
                    };
                    if let Some((percent, message, ready, failed)) =
                        progress.observe(record)
                    {
                        send_progress(&proxy, window_id, percent, message, ready, failed);
                    }
                }
            }
            let result =
                child
                    .wait()
                    .map_err(|error| error.to_string())
                    .and_then(|status| {
                        if status.success() {
                            Ok(())
                        } else {
                            Err(format!("Updater exited with {status}"))
                        }
                    });
            if let Some((percent, message, ready, failed)) = progress.finish(result) {
                send_progress(&proxy, window_id, percent, message, ready, failed);
            }
        })?;
    Ok(())
}

#[cfg(debug_assertions)]
pub(crate) fn spawn_install_preview(proxy: EventProxy, window_id: WindowId) {
    let _ = std::thread::Builder::new()
        .name("neoism-update-preview".to_string())
        .spawn(move || {
            for (percent, message) in [
                (5, "Checking the latest Neoism release"),
                (18, "Connecting to the release server"),
                (42, "Downloading Neoism"),
                (70, "Download complete"),
                (78, "Verifying the release checksum"),
                (88, "Extracting the Neoism release"),
                (96, "Staging the update"),
                (100, "Preview complete - release builds restart here"),
            ] {
                std::thread::sleep(std::time::Duration::from_millis(300));
                send_progress(
                    &proxy,
                    window_id,
                    Some(percent),
                    message.to_string(),
                    false,
                    false,
                );
            }
        });
}

fn parse_progress_line(line: &str) -> Option<(Option<u8>, String, bool, bool)> {
    let payload = line.strip_prefix(PROGRESS_PREFIX)?;
    let mut fields = payload.splitn(4, '\t');
    let percent = match fields.next()? {
        "-" => None,
        value => Some(value.parse::<u8>().ok()?.min(100)),
    };
    let ready = fields.next()? == "1";
    let failed = fields.next()? == "1";
    let message = fields.next()?.to_string();
    Some((percent, message, ready, failed))
}

fn send_progress(
    proxy: &EventProxy,
    window_id: WindowId,
    percent: Option<u8>,
    message: String,
    ready_to_restart: bool,
    failed: bool,
) {
    proxy.send_event(
        RioEventType::Rio(RioEvent::SelfUpdateProgress {
            percent,
            message,
            ready_to_restart,
            failed,
        }),
        window_id,
    );
}

pub(crate) fn latest_release(repo: &str) -> Result<String, String> {
    // The redirect endpoint avoids the unauthenticated API's shared-IP quota.
    let latest_url = format!("https://github.com/{repo}/releases/latest");
    #[cfg(windows)]
    let null_device = "NUL";
    #[cfg(not(windows))]
    let null_device = "/dev/null";

    let output = crate::background_process::command("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "3",
            "--max-time",
            "8",
            "-o",
            null_device,
            "-w",
            "%{url_effective}",
            "-A",
            "neoism-update-check",
            &latest_url,
        ])
        .output()
        .map_err(|error| format!("Cannot check the Neoism release: {error}"))?;
    if !output.status.success() {
        return Err(format!("GitHub release check failed ({})", output.status));
    }
    let effective_url = String::from_utf8(output.stdout)
        .map_err(|_| "GitHub returned an invalid release URL".to_string())?;
    release_tag_from_url(&effective_url)
        .filter(|tag| valid_release_tag(tag))
        .ok_or_else(|| "No valid published Neoism release found".to_string())
}

fn available_version() -> Option<String> {
    let repo = std::env::var("NEOISM_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let latest = latest_release(&repo).ok()?;
    if is_newer(&latest, env!("CARGO_PKG_VERSION")) {
        return Some(latest);
    }
    #[cfg(debug_assertions)]
    if std::env::var_os("NEOISM_UPDATE_CHECK").is_some() {
        return Some(format!("{latest} (preview)"));
    }
    None
}

fn release_tag_from_url(url: &str) -> Option<String> {
    url.trim()
        .rsplit_once("/releases/tag/")
        .map(|(_, tag)| tag.trim_end_matches('/'))
        .filter(|tag| {
            !tag.is_empty() && !tag.chars().any(|ch| matches!(ch, '/' | '?' | '#'))
        })
        .map(str::to_string)
}

pub(crate) fn valid_release_tag(tag: &str) -> bool {
    parse_version(tag).is_some()
        && tag.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+')
        })
}

pub(crate) fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate.cmp(&current) == Ordering::Greater,
        _ => false,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Version {
    core: [u64; 3],
    prerelease: Option<Vec<PrereleaseId>>,
}

#[derive(Debug, Eq, PartialEq)]
enum PrereleaseId {
    Numeric(u64),
    Text(String),
}

impl Ord for PrereleaseId {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(left), Self::Numeric(right)) => left.cmp(right),
            (Self::Numeric(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd for PrereleaseId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => left.cmp(right),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_version(value: &str) -> Option<Version> {
    let value = value.trim().strip_prefix('v').unwrap_or(value.trim());
    let value = value.split_once('+').map_or(value, |(version, _)| version);
    let (core, prerelease) = value
        .split_once('-')
        .map_or((value, None), |(core, pre)| (core, Some(pre)));
    let mut parts = core.split('.');
    let core = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    if parts.next().is_some() {
        return None;
    }
    let prerelease = match prerelease {
        Some(value) => Some(
            value
                .split('.')
                .map(|part| {
                    if part.is_empty() {
                        None
                    } else if part.bytes().all(|byte| byte.is_ascii_digit()) {
                        part.parse().ok().map(PrereleaseId::Numeric)
                    } else if part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    {
                        Some(PrereleaseId::Text(part.to_string()))
                    } else {
                        None
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        None => None,
    };
    Some(Version { core, prerelease })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_release_tag_from_redirect() {
        assert_eq!(
            release_tag_from_url(
                "https://github.com/parkers0405/neoism/releases/tag/v0.8.0"
            ),
            Some("v0.8.0".to_string())
        );
        assert_eq!(
            release_tag_from_url("https://example.com/releases/latest"),
            None
        );
    }

    #[test]
    fn compares_semantic_versions() {
        assert!(is_newer("v0.8.0", "0.7.66"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("v1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("v0.7.66", "0.7.66"));
        assert!(!is_newer("v0.7.65", "0.7.66"));
        assert!(!is_newer("v1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("not-a-version", "0.7.66"));
    }

    #[test]
    fn helper_failure_notice_is_scoped_and_acknowledged_per_result() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("portable");
        let other = temp.path().join("other");
        let receipts = temp.path().join("updates");
        for dir in [&install, &other, &receipts.join("job")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let exe = install.join("neoism.exe");
        std::fs::write(&exe, b"test executable identity").unwrap();
        std::fs::write(other.join("neoism.exe"), b"other installation").unwrap();
        let receipt = receipts.join("job/result.json");
        let record = |state: &str| serde_json::json!({"state":state,"expected_version":"1.1.0", "message":"files locked", "target":{"directory":install}});
        std::fs::write(&receipt, record("failed").to_string()).unwrap();
        assert!(
            read_update_notice(&receipts, &other.join("neoism.exe"), "1.0.0").is_none()
        );
        assert!(read_update_notice(&receipts, &exe, "1.0.0")
            .unwrap()
            .contains("files locked"));
        assert!(read_update_notice(&receipts, &exe, "1.0.0").is_none());
        std::fs::write(&receipt, record("reboot_required").to_string()).unwrap();
        assert!(read_update_notice(&receipts, &exe, "1.0.0")
            .unwrap()
            .contains("restart"));
        std::fs::write(&receipt, record("succeeded").to_string()).unwrap();
        assert!(read_update_notice(&receipts, &exe, "1.0.0")
            .unwrap()
            .contains("still running 1.0.0"));
        assert!(read_update_notice(&receipts, &exe, "1.1.0").is_none());
        assert!(
            receipt.exists(),
            "never erase the durable installation evidence"
        );
    }

    #[test]
    fn bundle_receipt_does_not_apply_to_another_cli_installation() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Renamed.app");
        let bin = app.join("Contents/MacOS");
        let receipts = temp.path().join("updates");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(receipts.join("job")).unwrap();
        let exe = bin.join("neoism");
        std::fs::write(&exe, b"app identity").unwrap();
        let cli = temp.path().join("neoism");
        std::fs::write(&cli, b"separate CLI").unwrap();
        std::fs::write(receipts.join("job/result.json"), serde_json::json!({
            "status":"failed", "expected_version":"v1.1.0", "target":app, "error":"bundle restored"
        }).to_string()).unwrap();
        assert!(read_update_notice(&receipts, &cli, "1.0.0").is_none());
        assert!(read_update_notice(&receipts, &exe, "1.0.0")
            .unwrap()
            .contains("bundle restored"));
        std::fs::write(receipts.join("job/result.json"), serde_json::json!({
            "status":"failed", "expected_version":"v1.1.0", "target_executable":cli, "error":"CLI restored"
        }).to_string()).unwrap();
        assert!(read_update_notice(&receipts, &exe, "1.0.0").is_none());
        assert!(read_update_notice(&receipts, &cli, "1.0.0")
            .unwrap()
            .contains("CLI restored"));
    }

    #[test]
    fn pins_only_valid_release_tags() {
        for version in ["v0.7.96", "0.8.0-rc.1", "v1.0.0+build.2"] {
            assert!(valid_release_tag(version));
        }
        for version in [
            "",
            "v0.7.96/other",
            "v0.7.96?token=x",
            "v1.0.0+../bad",
            "v1.0.0+space here",
        ] {
            assert!(!valid_release_tag(version));
        }
    }

    #[test]
    fn installer_handoff_waits_for_clean_updater_exit() {
        let mut progress = InstallProgress::default();
        let staged = progress
            .observe((Some(95), "Installer staged".into(), true, false))
            .unwrap();
        assert!(!staged.2, "do not close the GUI before the updater exits");
        let handoff = progress.finish(Ok(())).unwrap();
        assert_eq!(handoff.0, Some(95), "handoff is not installed success");
        assert!(handoff.2);
        assert!(!handoff.3);
    }

    #[test]
    fn failed_updater_exit_is_not_hidden_by_earlier_handoff() {
        let mut progress = InstallProgress::default();
        progress.observe((Some(95), "Installer staged".into(), true, false));
        let failed = progress
            .finish(Err("updater exited with code 1".into()))
            .unwrap();
        assert!(!failed.2);
        assert!(failed.3);
    }

    #[test]
    fn direct_verified_unix_restart_does_not_wait_on_its_own_parent() {
        let mut progress = InstallProgress::default();
        let installed = progress
            .observe((Some(100), "Installed; restarting".into(), true, false))
            .unwrap();
        assert!(
            installed.2,
            "direct replacement waits for GUI exit before proceeding"
        );
        assert!(progress.finish(Ok(())).is_none());
    }

    #[test]
    fn completion_without_restart_waits_for_exit_and_failure_wins() {
        let mut progress = InstallProgress::default();
        assert_eq!(
            progress
                .observe((Some(100), "Already current".into(), false, false))
                .unwrap()
                .0,
            Some(95)
        );
        assert_eq!(
            progress.finish(Ok(())).unwrap(),
            (Some(100), "Already current".into(), false, false)
        );
        let mut progress = InstallProgress::default();
        let failed = progress
            .observe((None, "replacement failed".into(), true, true))
            .unwrap();
        assert!(!failed.2);
        assert!(failed.3);
        assert!(progress.finish(Err("exit 1".into())).is_none());
        assert!(InstallProgress::default().finish(Ok(())).unwrap().3);
    }

    #[test]
    fn parses_gui_progress_records() {
        assert_eq!(
            parse_progress_line("NEOISM_UPDATE\t42\t0\t0\tDownloading release"),
            Some((Some(42), "Downloading release".to_string(), false, false))
        );
        assert_eq!(
            parse_progress_line("NEOISM_UPDATE\t-\t0\t1\tnetwork failed"),
            Some((None, "network failed".to_string(), false, true))
        );
    }
}
