use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{io, AcquisitionError, AcquisitionStage};

pub const LOCKFILE_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LuaPluginLockEntry {
    pub plugin_id: String,
    pub repository_url: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub plugin_version: String,
    pub manifest_checksum: String,
    pub tree_checksum: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub installed_revision_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LuaPluginLock {
    pub version: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, LuaPluginLockEntry>,
}

impl Default for LuaPluginLock {
    fn default() -> Self { Self { version: LOCKFILE_VERSION, plugins: BTreeMap::new() } }
}

impl LuaPluginLock {
    pub fn load(path: &Path) -> Result<Self, AcquisitionError> {
        match fs::read(path) {
            Ok(bytes) if bytes.is_empty() => Ok(Self::default()),
            Ok(bytes) => {
                let lock: Self = serde_json::from_slice(&bytes).map_err(|e| AcquisitionError::LockfileParse { path: path.to_path_buf(), message: e.to_string() })?;
                if lock.version != LOCKFILE_VERSION {
                    return Err(AcquisitionError::UnsupportedLockfileVersion { found: lock.version, supported: LOCKFILE_VERSION });
                }
                Ok(lock)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(io(AcquisitionStage::Lock, path, e)),
        }
    }

    pub(crate) fn save_atomic(&self, path: &Path) -> Result<(), AcquisitionError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| AcquisitionError::LockfileSerialize(e.to_string()))?;
        let parent = path.parent().ok_or_else(|| AcquisitionError::UnsafePath(path.to_path_buf()))?;
        fs::create_dir_all(parent).map_err(|e| io(AcquisitionStage::PublishLockfile, parent, e))?;
        let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("plugins.lock.json");
        let temporary = parent.join(format!(".{name}.tmp.{}.{}", std::process::id(), unique()));
        let result = (|| {
            let mut file = OpenOptions::new().create_new(true).write(true).open(&temporary)
                .map_err(|e| io(AcquisitionStage::PublishLockfile, &temporary, e))?;
            file.write_all(&bytes).map_err(|e| io(AcquisitionStage::PublishLockfile, &temporary, e))?;
            file.write_all(b"\n").map_err(|e| io(AcquisitionStage::PublishLockfile, &temporary, e))?;
            file.sync_all().map_err(|e| io(AcquisitionStage::PublishLockfile, &temporary, e))?;
            replace_atomic(&temporary, path)?;
            sync_parent(parent)?;
            Ok(())
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result
    }
}

#[cfg(not(windows))]
fn replace_atomic(from: &Path, to: &Path) -> Result<(), AcquisitionError> {
    fs::rename(from, to).map_err(|e| io(AcquisitionStage::PublishLockfile, to, e))
}

#[cfg(windows)]
fn replace_atomic(from: &Path, to: &Path) -> Result<(), AcquisitionError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH, REPLACEFILE_WRITE_THROUGH};
    let wide = |path: &Path| path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let from_w = wide(from);
    let to_w = wide(to);
    let ok = unsafe {
        if to.exists() {
            ReplaceFileW(to_w.as_ptr(), from_w.as_ptr(), std::ptr::null(), REPLACEFILE_WRITE_THROUGH, std::ptr::null_mut(), std::ptr::null_mut())
        } else {
            MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), MOVEFILE_WRITE_THROUGH)
        }
    };
    if ok == 0 { Err(io(AcquisitionStage::PublishLockfile, to, std::io::Error::last_os_error())) } else { Ok(()) }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<(), AcquisitionError> {
    fs::File::open(parent).and_then(|file| file.sync_all()).map_err(|e| io(AcquisitionStage::PublishLockfile, parent, e))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<(), AcquisitionError> { Ok(()) }

fn unique() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
}