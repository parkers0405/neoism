//! Windows updater bootstrap. Installation happens only after this process exits.
//! The returned JSON result is a durable handoff receipt, NOT installed success.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn stage(
    msi: &Path,
    temp_dir: &Path,
    gui_pid: Option<u32>,
    relaunch: bool,
    invoking_exe: &Path,
    expected_version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    let receipt_dir = PathBuf::from(local)
        .join("Neoism")
        .join("updates")
        .join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
    std::fs::create_dir_all(&receipt_dir)?;
    let result = receipt_dir.join("result.json");
    let helper = receipt_dir.join("finish-update.ps1");
    std::fs::write(&helper, include_str!("windows_update.ps1"))?;
    // A trusted system PowerShell, not an arbitrary PATH-shadowed executable.
    let powershell =
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut child = std::process::Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&helper)
        .arg("-UpdaterPid")
        .arg(std::process::id().to_string())
        .arg("-GuiPid")
        .arg(gui_pid.unwrap_or_default().to_string())
        .arg("-MsiPath")
        .arg(msi)
        .arg("-TempDir")
        .arg(temp_dir)
        .arg("-InvokingExe")
        .arg(invoking_exe)
        .arg("-ExpectedVersion")
        .arg(expected_version.trim_start_matches('v'))
        .arg("-ResultPath")
        .arg(&result)
        .arg("-Relaunch")
        .arg(if relaunch { "1" } else { "0" })
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // Preflight (payload extraction, identity, target selection, update lock) runs
    // while the current GUI remains usable. Do not close it merely on spawn().
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    loop {
        if let Ok(bytes) = std::fs::read(&result) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                match value["state"].as_str() {
                    Some("handed_off") => return Ok(result),
                    Some("failed" | "reboot_required") => {
                        return Err(format!(
                            "{} (details: {})",
                            value["message"].as_str().unwrap_or("Windows update failed"),
                            result.display()
                        )
                        .into());
                    }
                    _ => {}
                }
            }
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "Windows update helper exited ({status}) before handoff; details: {}",
                result.display()
            )
            .into());
        }
        if Instant::now() >= deadline {
            // Explicit cancellation is checked before handoff and again before
            // replacement. A slow helper must not install after a reported error.
            std::fs::write(receipt_dir.join("cancel"), b"preflight timed out")?;
            return Err(format!(
                "Windows update preflight timed out; cancelled (details: {})",
                result.display()
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
