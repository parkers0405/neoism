use neoism_backend::event::{EventProxy, RioEvent, RioEventType, WindowId};
use std::cmp::Ordering;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

const DEFAULT_REPO: &str = "parkers0405/neoism";
const PROGRESS_PREFIX: &str = "NEOISM_UPDATE\t";

pub(crate) fn spawn_check(proxy: EventProxy, window_id: WindowId) {
    if std::env::var_os("NEOISM_DISABLE_UPDATE_CHECK").is_some()
        || (cfg!(debug_assertions) && std::env::var_os("NEOISM_UPDATE_CHECK").is_none())
    {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("neoism-update-check".to_string())
        .spawn(move || {
            if let Some(version) = available_version() {
                proxy.send_event(
                    RioEventType::Rio(RioEvent::UpdateAvailable { version }),
                    window_id,
                );
            }
        });
}

pub(crate) fn spawn_install(
    proxy: EventProxy,
    window_id: WindowId,
) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::thread::Builder::new()
        .name("neoism-update-install".to_string())
        .spawn(move || {
            let child = Command::new(exe)
                .args([
                    "update",
                    "--gui",
                    "--relaunch",
                    "--parent-pid",
                    &std::process::id().to_string(),
                ])
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
            let mut terminal_status = false;
            if let Some(stdout) = child.stdout.take() {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Some((percent, message, ready, failed)) =
                        parse_progress_line(&line)
                    else {
                        continue;
                    };
                    terminal_status |= ready || failed;
                    send_progress(&proxy, window_id, percent, message, ready, failed);
                }
            }
            match child.wait() {
                Ok(status) if status.success() || terminal_status => {}
                Ok(status) => send_progress(
                    &proxy,
                    window_id,
                    None,
                    format!("Updater exited with {status}"),
                    false,
                    true,
                ),
                Err(error) => {
                    send_progress(&proxy, window_id, None, error.to_string(), false, true)
                }
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

fn available_version() -> Option<String> {
    let repo = std::env::var("NEOISM_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let latest_url = format!("https://github.com/{repo}/releases/latest");
    #[cfg(windows)]
    let null_device = "NUL";
    #[cfg(not(windows))]
    let null_device = "/dev/null";

    let output = Command::new("curl")
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
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let effective_url = String::from_utf8(output.stdout).ok()?;
    let latest = release_tag_from_url(&effective_url)?;
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

fn is_newer(candidate: &str, current: &str) -> bool {
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
