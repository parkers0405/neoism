//! Windows updater bootstrap. Installation happens only after this process exits.
//! Receipts prove handoff or verified completion; process exit alone is never success.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn helper_creation_flags() -> u32 {
    // CREATE_NO_WINDOW is ignored when combined with DETACHED_PROCESS (Win32
    // Process Creation Flags). A Windows child outlives its parent regardless;
    // avoid the detached/no-console combination for the PowerShell host.
    CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
}

fn spawn_helper(
    host: &Path,
    helper: &Path,
    result: &Path,
    msi: &Path,
    temp_dir: &Path,
    gui_pid: Option<u32>,
    relaunch: bool,
    invoking_exe: &Path,
    expected_version: &str,
) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;

    std::process::Command::new(host)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(helper)
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
        .arg(result)
        .arg("-Relaunch")
        .arg(if relaunch { "1" } else { "0" })
        .creation_flags(helper_creation_flags())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
}

fn verified_success(value: &serde_json::Value, expected_version: &str) -> bool {
    value["state"] == "succeeded"
        && value["installation_verified"] == true
        && value["expected_version"] == expected_version.trim_start_matches('v')
}

fn should_fallback(
    status: std::process::ExitStatus,
    receipt_exists: bool,
    is_primary: bool,
) -> bool {
    status.success() && !receipt_exists && is_primary
}

#[cfg(test)]
mod tests {
    use super::{helper_creation_flags, should_fallback, spawn_helper, verified_success};
    use std::os::windows::process::ExitStatusExt;

    #[test]
    fn helper_uses_hidden_console_and_a_separate_process_group_without_detaching() {
        assert_eq!(helper_creation_flags(), 0x0800_0200);
        assert_eq!(helper_creation_flags() & 0x0000_0008, 0);
    }

    #[test]
    fn production_spawn_runs_a_real_powershell_host_when_available() {
        let Some(host) = std::env::var_os("NEOISM_TEST_PWSH") else {
            return; // Set to the absolute path of powershell.exe or portable pwsh.exe.
        };
        if let Some(directory) = std::env::var_os("NEOISM_TEST_SPAWNER_DIR") {
            // This test process is the short-lived updater. Leave PowerShell
            // running when it exits; the outer test checks the durable receipt.
            let directory = std::path::PathBuf::from(directory);
            let script = directory.join("spawn-test.ps1");
            let result = directory.join("result.json");
            spawn_helper(
                std::path::Path::new(&host),
                &script,
                &result,
                &script,
                &directory,
                None,
                false,
                &script,
                "v0.7.110-nightly.20260921.3",
            )
            .unwrap();
            return;
        }
        let directory = std::env::temp_dir()
            .join(format!("neoism-worker-spawn-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("spawn-test.ps1");
        let result = directory.join("result.json");
        std::fs::write(&script, r#"param([int]$UpdaterPid, [int]$GuiPid, [string]$MsiPath, [string]$TempDir, [string]$InvokingExe, [string]$ExpectedVersion, [string]$ResultPath, [int]$Relaunch)
try {
    $owner = [Diagnostics.Process]::GetProcessById($UpdaterPid)
    try { $owner.WaitForExit() } finally { $owner.Dispose() }
} catch [ArgumentException] {
    # The spawner already exited before PowerShell opened its process handle.
}
[IO.File]::WriteAllText($ResultPath, '{"state":"handed_off","spawner_exited":true}')
"#).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "windows_update::tests::production_spawn_runs_a_real_powershell_host_when_available"])
            .env("NEOISM_TEST_SPAWNER_DIR", &directory)
            .status().unwrap();
        assert!(
            status.success(),
            "the short-lived updater did not spawn PowerShell"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !result.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&result).unwrap(),
            r#"{"state":"handed_off","spawner_exited":true}"#
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn only_verified_completion_can_replace_a_missed_handoff() {
        let expected = "v0.7.110-nightly.20260921.3";
        let success = serde_json::json!({"state":"succeeded", "installation_verified":true, "expected_version":"0.7.110-nightly.20260921.3"});
        assert!(verified_success(&success, expected));
        assert!(!verified_success(
            &serde_json::json!({"state":"handed_off", "installation_verified":false, "expected_version":"0.7.110-nightly.20260921.3"}),
            expected
        ));
        assert!(!verified_success(
            &serde_json::json!({"state":"succeeded", "installation_verified":false, "expected_version":"0.7.110-nightly.20260921.3"}),
            expected
        ));
        assert!(!verified_success(&success, "v0.7.111"));
    }

    #[test]
    fn retry_only_a_noop_primary_host_before_any_receipt() {
        let ok = std::process::ExitStatus::from_raw(0);
        let failed = std::process::ExitStatus::from_raw(1);
        assert!(should_fallback(ok, false, true));
        assert!(!should_fallback(ok, true, true));
        assert!(!should_fallback(ok, false, false));
        assert!(!should_fallback(failed, false, true));
    }
}

pub fn stage(
    msi: &Path,
    temp_dir: &Path,
    gui_pid: Option<u32>,
    relaunch: bool,
    invoking_exe: &Path,
    expected_version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
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
    let powershell =
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    // Windows PowerShell can be a no-op compatibility stub (notably under
    // Wine/Proton). Only retry with the trusted system-wide PowerShell 7 when
    // it exited successfully WITHOUT ever writing a receipt: a written
    // preflight failure must not be hidden, and a handoff must not be repeated.
    let fallback = std::env::var_os("ProgramFiles")
        .map(|root| PathBuf::from(root).join("PowerShell/7/pwsh.exe"))
        .filter(|path| path.is_file());
    let mut host = powershell.clone();
    let mut child = spawn_helper(
        &host,
        &helper,
        &result,
        msi,
        temp_dir,
        gui_pid,
        relaunch,
        invoking_exe,
        expected_version,
    )?;

    // Preflight (payload extraction, identity, target selection, update lock) runs
    // while the current GUI remains usable. Do not close it merely on spawn().
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    loop {
        if let Ok(bytes) = std::fs::read(&result) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                match value["state"].as_str() {
                    Some("handed_off") => return Ok(result),
                    Some("succeeded") if verified_success(&value, expected_version) => {
                        if child.try_wait()?.is_some_and(|status| status.success()) {
                            return Ok(result);
                        }
                    }
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
            // Recheck the atomic receipt after observing exit: the worker may
            // have completed a final write between the earlier read and wait.
            if let Ok(bytes) = std::fs::read(&result) {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if status.success() && verified_success(&value, expected_version) {
                        return Ok(result);
                    }
                    // A final handed_off receipt without a living helper is NOT
                    // success: no process remains to replace the installation.
                    if matches!(
                        value["state"].as_str(),
                        Some("failed" | "reboot_required")
                    ) {
                        return Err(format!(
                            "{} (details: {})",
                            value["message"].as_str().unwrap_or("Windows update failed"),
                            result.display()
                        )
                        .into());
                    }
                }
            }
            if should_fallback(status, result.exists(), host == powershell) {
                if let Some(next) = &fallback {
                    host = next.clone();
                    child = spawn_helper(
                        &host,
                        &helper,
                        &result,
                        msi,
                        temp_dir,
                        gui_pid,
                        relaunch,
                        invoking_exe,
                        expected_version,
                    )?;
                    continue;
                }
            }
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
