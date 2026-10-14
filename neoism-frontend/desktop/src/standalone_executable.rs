// Standalone process locator shared by persistent and temporary hosts.

/// Keep the running inode executable across Linux rebuilds/updater unlink.
/// Do not canonicalize procfs: its resolved pathname may already be deleted.
pub(crate) fn standalone_executable() -> std::io::Result<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    { Ok(std::path::PathBuf::from("/proc/self/exe")) }
    #[cfg(not(target_os = "linux"))]
    { std::env::current_exe() }
}
