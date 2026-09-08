//! macOS bundle updates. Selection and filesystem transactions are testable on
//! Linux; LaunchServices, codesign and process inspection stay in `native`.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub type Error = Box<dyn std::error::Error>;
const BUNDLE_ID: &str = "dev.neoism.Neoism";
const BINS: [&str; 3] = ["neoism", "neoism-workspace-daemon", "neoism-agent"];

#[derive(Debug, PartialEq, Eq)]
pub enum Installation {
    Bundle(PathBuf),
    Loose(PathBuf),
}

/// Only inspect local ancestors of the canonical invoking executable. Never
/// discover another installation in /Applications or the user's Applications.
pub fn resolve_installation(
    exe: &Path,
    mut bundle_id: impl FnMut(&Path) -> Result<String, Error>,
) -> Result<Installation, Error> {
    let exe = fs::canonicalize(exe)?;
    reject_ephemeral_target(&exe)?;
    for ancestor in exe.ancestors().skip(1) {
        let app_shaped = ancestor.extension().is_some_and(|ext| ext == "app")
            || ancestor.join("Contents/Info.plist").is_file();
        if !app_shaped {
            continue;
        }
        let macos = ancestor.join("Contents/MacOS");
        if exe.parent() != Some(macos.as_path())
            || bundle_id(ancestor)? != BUNDLE_ID
            || !BINS
                .iter()
                .any(|bin| exe.file_name().is_some_and(|name| name == *bin))
        {
            return Err(format!("{} is inside an unsupported app bundle; refusing a partial signed-bundle update", exe.display()).into());
        }
        return Ok(Installation::Bundle(ancestor.to_path_buf()));
    }
    Ok(Installation::Loose(exe))
}

fn reject_ephemeral_target(path: &Path) -> Result<(), Error> {
    if path.starts_with("/Volumes")
        || path
            .components()
            .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Err(format!("cannot self-update {} from a mounted volume or AppTranslocation; copy this installation to a writable local location and launch that copy first", path.display()).into());
    }
    Ok(())
}

fn check_version(actual: &str, expected: &str) -> Result<(), Error> {
    if actual.trim() != expected.strip_prefix('v').unwrap_or(expected) {
        return Err(format!(
            "bundle version mismatch: expected {expected}, got {}",
            actual.trim()
        )
        .into());
    }
    Ok(())
}

type Manifest = BTreeMap<PathBuf, String>;

fn check_compiled_version(
    output: &str,
    binary: &str,
    expected: &str,
) -> Result<(), Error> {
    let fields = output.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 2
        || !fields[0].eq_ignore_ascii_case(binary)
        || fields[1].strip_prefix('v').unwrap_or(fields[1])
            != expected.strip_prefix('v').unwrap_or(expected)
    {
        return Err(format!(
            "compiled version mismatch for {binary}: expected {expected}, got {:?}",
            output.trim()
        )
        .into());
    }
    Ok(())
}

/// Capture through files, not pipes: inherited writers cannot keep the helper
/// blocked. Kill the command's process group on timeout; cap diagnostic reads.
#[cfg(unix)]
fn bounded_output(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Result<std::process::Output, Error> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            stderr.seek(SeekFrom::Start(0))?;
            let mut diagnostic = String::new();
            stderr.take(64 * 1024).read_to_string(&mut diagnostic)?;
            return Err(
                format!("{command:?} timed out after {timeout:?}: {diagnostic}").into(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    fn read(file: &mut fs::File) -> std::io::Result<Vec<u8>> {
        file.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        file.take(64 * 1024).read_to_end(&mut data)?;
        Ok(data)
    }
    Ok(std::process::Output {
        status,
        stdout: read(&mut stdout)?,
        stderr: read(&mut stderr)?,
    })
}

fn hash_path(path: &Path, key: &Path, result: &mut Manifest) -> Result<(), Error> {
    let kind = fs::symlink_metadata(path)?.file_type();
    let value = if kind.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            hash_path(&entry.path(), &key.join(entry.file_name()), result)?;
        }
        "directory".to_owned()
    } else if kind.is_file() {
        let mut hash = Sha256::new();
        std::io::copy(&mut fs::File::open(path)?, &mut hash)?;
        format!("{:x}", hash.finalize())
    } else {
        return Err(format!(
            "unsupported link or special file in update: {}",
            path.display()
        )
        .into());
    };
    result.insert(key.to_path_buf(), value);
    Ok(())
}

/// Only the four owned loose components; never hash/copy the installation's
/// unrelated files or the tarball's separate Neoism.app.
fn loose_manifest(root: &Path, gui_name: &std::ffi::OsStr) -> Result<Manifest, Error> {
    let mut hashes = Manifest::new();
    for bin in BINS {
        let path = root.join(if bin == "neoism" {
            gui_name
        } else {
            std::ffi::OsStr::new(bin)
        });
        if !fs::symlink_metadata(&path)?.file_type().is_file() {
            return Err(format!(
                "update binary is not a regular file: {}",
                path.display()
            )
            .into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&path)?.permissions().mode() & 0o111 == 0 {
                return Err(format!("update binary is not executable: {bin}").into());
            }
        }
        hash_path(&path, Path::new(bin), &mut hashes)?;
    }
    if !root.join("web/index.html").is_file() {
        return Err("update payload is missing web/index.html".into());
    }
    hash_path(&root.join("web"), Path::new("web"), &mut hashes)?;
    Ok(hashes)
}

fn loose_components(target: &Path) -> Result<Vec<(&'static str, PathBuf)>, Error> {
    let parent = target.parent().ok_or("loose executable has no parent")?;
    if ["neoism-workspace-daemon", "neoism-agent", "web"]
        .iter()
        .any(|name| target.file_name().is_some_and(|file| file == *name))
    {
        return Err("loose GUI executable name collides with an update companion".into());
    }
    Ok(vec![
        ("neoism", target.to_owned()),
        (
            "neoism-workspace-daemon",
            parent.join("neoism-workspace-daemon"),
        ),
        ("neoism-agent", parent.join("neoism-agent")),
        ("web", parent.join("web")),
    ])
}

/// All files are staged/verified before entering. Roll back every completed
/// move on any later failure, including installed-version and relaunch checks.
/// A crash leaves the lock, backup and movement journal for manual recovery.
fn replace_loose_with(
    target: &Path,
    staged: &Path,
    backup: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    verify_and_launch: impl FnOnce() -> Result<(), Error>,
) -> Result<(), Error> {
    let components = loose_components(target)?;
    // A missing late component must never result in an early GUI replacement.
    loose_manifest(staged, std::ffi::OsStr::new("neoism"))?;
    fs::create_dir(backup)?;
    let mut moved = Vec::new();
    let result = (|| -> Result<(), Error> {
        for (name, destination) in &components {
            let old = backup.join(name);
            // Journal intent before each pair; existence of backup/staged files
            // disambiguates a crash between the two renames.
            write_json(
                &backup.join("transaction.json"),
                &serde_json::json!({
                    "target_executable": target, "component": name, "phase": "replacing",
                }),
            )?;
            let existed = match fs::symlink_metadata(destination) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if existed {
                rename(destination, &old)?;
            }
            moved.push((*name, destination.clone(), existed, false));
            rename(&staged.join(name), destination)?;
            moved.last_mut().unwrap().3 = true;
        }
        verify_and_launch()?;
        write_json(
            &backup.join("transaction.json"),
            &serde_json::json!({
                "target_executable": target, "phase": "installed", "health_verified": false,
            }),
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (name, destination, existed, installed) in moved.into_iter().rev() {
            if installed {
                if let Err(rollback) = rename(&destination, &staged.join(name)) {
                    rollback_errors.push(format!("move rejected {name}: {rollback}"));
                    continue;
                }
            }
            if existed {
                if let Err(rollback) = rename(&backup.join(name), &destination) {
                    rollback_errors.push(format!("restore {name}: {rollback}"));
                }
            }
        }
        return if rollback_errors.is_empty() {
            Err(format!(
                "loose replacement failed; original components restored: {error}"
            )
            .into())
        } else {
            Err(format!("loose replacement failed: {error}; rollback failed: {}; retained backup: {}", rollback_errors.join("; "), backup.display()).into())
        };
    }
    Ok(())
}

/// Hash the entire payload, not only the GUI. Reject links/special files instead
/// of following a payload link out of the authenticated extraction directory.
fn manifest(root: &Path) -> Result<Manifest, Error> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err("update bundle root must be a real directory, not a link".into());
    }
    let mut result = Manifest::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        hash_path(&entry.path(), Path::new(&entry.file_name()), &mut result)?;
    }
    for required in BINS
        .iter()
        .map(|bin| format!("Contents/MacOS/{bin}"))
        .chain([
            "Contents/Info.plist".to_owned(),
            "Contents/Resources/web/index.html".to_owned(),
            "Contents/Resources/neoism.icns".to_owned(),
        ])
    {
        if !root.join(&required).is_file() {
            return Err(format!("update payload is missing {required}").into());
        }
    }
    #[cfg(unix)]
    for bin in BINS {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(root.join("Contents/MacOS").join(bin))?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(format!("update binary is not executable: {bin}").into());
        }
    }
    Ok(result)
}

fn verify_manifest(root: &Path, expected: &Manifest) -> Result<(), Error> {
    if &manifest(root)? != expected {
        return Err(
            format!("bundle contents/hash mismatch at {}", root.display()).into(),
        );
    }
    Ok(())
}

/// A conservative per-target lock. An interrupted/crashed updater leaves this
/// directory behind, so a second updater fails closed rather than stealing it.
/// owner.json points to the durable operation result for manual recovery.
struct UpdateLock {
    path: PathBuf,
    release_on_drop: bool,
}
impl UpdateLock {
    fn acquire(target: &Path) -> Result<Self, Error> {
        let parent = target.parent().ok_or("bundle has no parent")?;
        let name = target.file_name().ok_or("bundle has no name")?;
        let mut hash = Sha256::new();
        hash.update(name.as_encoded_bytes());
        let path = parent.join(format!(".neoism-update-{:x}.lock", hash.finalize()));
        fs::create_dir(&path).map_err(|error| format!(
            "cannot lock update target {}: {error}. Target must be writable and no other update may be running. If an earlier helper crashed, inspect {}/owner.json before manually recovering its lock",
            target.display(), path.display()
        ))?;
        Ok(Self {
            path,
            release_on_drop: true,
        })
    }
}
impl Drop for UpdateLock {
    fn drop(&mut self) {
        if self.release_on_drop {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Roll back BOTH a failed second rename and failed post-install verification /
/// relaunch request. Keep the previous bundle even after success: open accepting
/// a request is not a health check. No fixed .new/.old paths are ever removed.
fn replace_bundle(
    target: &Path,
    staged: &Path,
    backup: &Path,
    verify_and_launch: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    replace_bundle_with(
        target,
        staged,
        backup,
        |from, to| fs::rename(from, to),
        verify_and_launch,
    )
}

fn replace_bundle_with(
    target: &Path,
    staged: &Path,
    backup: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    verify_and_launch: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    if backup.exists() {
        return Err("backup already exists; refusing to overwrite it".into());
    }
    rename(target, backup)?;
    if let Err(error) = rename(staged, target) {
        return match rename(backup, target) {
            Ok(()) => Err(format!("replacement failed; original restored: {error}").into()),
            Err(rollback) => Err(format!("replacement failed: {error}; rollback failed: {rollback}; original retained at {}", backup.display()).into()),
        };
    }
    if let Err(error) = verify_and_launch(target) {
        // Move the rejected payload back out before restoring the original.
        let rollback = rename(target, staged).and_then(|()| rename(backup, target));
        return match rollback {
            Ok(()) => Err(format!("replacement verification/relaunch failed; original restored: {error}").into()),
            Err(rollback) => Err(format!("replacement failed: {error}; rollback failed: {rollback}; original retained at {}", backup.display()).into()),
        };
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum HelperArgs {
    Plan(PathBuf),
    Legacy {
        target: PathBuf,
        source: PathBuf,
        gui_pid: Option<u32>,
        relaunch: bool,
        expected_version: String,
    },
}

fn helper_args(
    args: &[std::ffi::OsString],
    compiled_version: &str,
) -> Result<Option<HelperArgs>, Error> {
    if args.first().and_then(|arg| arg.to_str()) != Some("--macos-update-helper") {
        return Ok(None);
    }
    match args.len() {
        2 => Ok(Some(HelperArgs::Plan(PathBuf::from(&args[1])))),
        // This ABI is deployed: old updaters execute the NEW payload helper.
        // Never remove it. An optional sixth argument is an expected version.
        5 | 6 => {
            let pid = args[3]
                .to_str()
                .ok_or("invalid macOS GUI pid")?
                .parse::<u32>()?;
            let relaunch = match args[4].to_str() {
                Some("0") => false,
                Some("1") => true,
                _ => return Err("invalid macOS relaunch flag".into()),
            };
            let expected_version = if args.len() == 6 {
                args[5].to_str().ok_or("invalid expected version")?
            } else {
                compiled_version
            };
            Ok(Some(HelperArgs::Legacy {
                target: PathBuf::from(&args[1]),
                source: PathBuf::from(&args[2]),
                gui_pid: (pid != 0).then_some(pid),
                relaunch,
                expected_version: expected_version.to_owned(),
            }))
        }
        _ => Err("invalid macOS update helper arguments".into()),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Plan {
    schema_version: u32,
    #[serde(default)]
    loose: bool,
    #[serde(default)]
    relaunch_directory: Option<PathBuf>,
    expected_version: String,
    target: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    lock: PathBuf,
    operation: PathBuf,
    updater_pid: u32,
    gui_pid: Option<u32>,
    relaunch: bool,
    signed: bool,
    manifest: Manifest,
}

impl Plan {
    fn target_executable(&self) -> PathBuf {
        if self.loose {
            self.target.clone()
        } else {
            self.target.join("Contents/MacOS/neoism")
        }
    }
}

/// Stable JSON contract for the GUI/CLI: only installed/open_accepted indicate
/// verified disk replacement, never application health. Failed includes errors
/// and the backup location, including when rollback itself failed.
#[derive(serde::Serialize)]
struct HelperResult<'a> {
    schema_version: u32,
    status: &'a str,
    expected_version: &'a str,
    target: &'a Path,
    target_executable: PathBuf,
    backup_path: &'a Path,
    log_path: PathBuf,
    health_verified: bool,
    observed_gui_pid: Option<u32>,
    error: Option<String>,
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), Error> {
    let parent = path.parent().ok_or("result has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn record(plan: &Plan, status: &str, error: Option<String>) -> Result<(), Error> {
    record_with_pid(plan, status, error, None)
}

fn record_with_pid(
    plan: &Plan,
    status: &str,
    error: Option<String>,
    observed_gui_pid: Option<u32>,
) -> Result<(), Error> {
    let log_path = plan.operation.join("helper.log");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    writeln!(
        log,
        "{:?} {status}: {}",
        std::time::SystemTime::now(),
        error.as_deref().unwrap_or("")
    )?;
    log.sync_all()?;
    write_json(
        &plan.operation.join("result.json"),
        &HelperResult {
            schema_version: 1,
            status,
            expected_version: &plan.expected_version,
            target: &plan.target,
            target_executable: plan.target_executable(),
            backup_path: &plan.backup,
            log_path,
            health_verified: false,
            observed_gui_pid,
            error,
        },
    )
}

#[cfg(target_os = "macos")]
pub use native::{installation, run_helper, stage_loose_update, stage_update};

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn checked(command: &mut Command) -> Result<String, Error> {
        let output = bounded_output(command, Duration::from_secs(30))?;
        if !output.status.success() {
            return Err(format!(
                "{command:?} exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    fn plist(app: &Path, key: &str) -> Result<String, Error> {
        Ok(checked(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", &format!("Print :{key}")])
                .arg(app.join("Contents/Info.plist")),
        )?
        .trim()
        .to_owned())
    }

    pub fn bundle_version(app: &Path) -> Result<String, Error> {
        plist(app, "CFBundleShortVersionString")
    }

    pub fn installation(exe: &Path) -> Result<Installation, Error> {
        resolve_installation(exe, |app| plist(app, "CFBundleIdentifier"))
    }

    fn signed(app: &Path) -> Result<bool, Error> {
        let output = bounded_output(
            Command::new("/usr/bin/codesign")
                .args(["-d", "--verbose=2"])
                .arg(app),
            Duration::from_secs(30),
        )?;
        if output.status.success() {
            return Ok(true);
        }
        // Do not treat a corrupt signature or tool failure as an unsigned app.
        if !app.join("Contents/_CodeSignature").exists()
            && String::from_utf8_lossy(&output.stderr)
                .contains("code object is not signed at all")
        {
            return Ok(false);
        }
        Err(format!(
            "cannot inspect bundle signature: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }

    fn verify(
        app: &Path,
        version: &str,
        hashes: &Manifest,
        require_signature: bool,
    ) -> Result<(), Error> {
        if plist(app, "CFBundleIdentifier")? != BUNDLE_ID
            || plist(app, "CFBundleExecutable")? != "neoism"
        {
            return Err("release bundle identity/executable mismatch".into());
        }
        check_version(&bundle_version(app)?, version)?;
        verify_manifest(app, hashes)?;
        if require_signature || signed(app)? {
            // Preserve ad-hoc/Developer-ID policy; do not require notarization
            // for the deliberately ad-hoc release channel.
            checked(
                Command::new("/usr/bin/codesign")
                    .args(["--verify", "--deep", "--strict", "--verbose=2"])
                    .arg(app),
            )?;
        }
        verify_binary_versions(
            &app.join("Contents/MacOS"),
            std::ffi::OsStr::new("neoism"),
            version,
        )?;
        Ok(())
    }

    fn verify_binary_versions(
        root: &Path,
        gui_name: &std::ffi::OsStr,
        version: &str,
    ) -> Result<(), Error> {
        for binary in BINS {
            let path = root.join(if binary == "neoism" {
                gui_name
            } else {
                std::ffi::OsStr::new(binary)
            });
            let output = checked(Command::new(path).arg("--version"))?;
            check_compiled_version(&output, binary, version)?;
        }
        Ok(())
    }

    fn verify_loose(
        root: &Path,
        gui_name: &std::ffi::OsStr,
        version: &str,
        hashes: &Manifest,
        require_signature: bool,
    ) -> Result<(), Error> {
        if &loose_manifest(root, gui_name)? != hashes {
            return Err("loose installation contents/hash mismatch".into());
        }
        for binary in BINS {
            let path = root.join(if binary == "neoism" {
                gui_name
            } else {
                std::ffi::OsStr::new(binary)
            });
            if require_signature || signed(&path)? {
                checked(
                    Command::new("/usr/bin/codesign")
                        .args(["--verify", "--strict", "--verbose=2"])
                        .arg(path),
                )?;
            }
        }
        verify_binary_versions(root, gui_name, version)
    }

    pub struct Handoff {
        pub result_path: PathBuf,
        pub log_path: PathBuf,
        pub target_executable: PathBuf,
    }

    /// Called only after the parent's archive checksum verification. The helper
    /// is a private copy of the invoking executable (not unvalidated release
    /// code), so the plan ABI is controlled by this updater version.
    pub fn stage_update(
        target: &Path,
        source: &Path,
        expected_version: &str,
        gui_pid: Option<u32>,
        relaunch: bool,
    ) -> Result<Handoff, Error> {
        stage_installation(target, source, expected_version, gui_pid, relaunch, false)
    }

    pub fn stage_loose_update(
        target_executable: &Path,
        source: &Path,
        expected_version: &str,
        gui_pid: Option<u32>,
        relaunch: bool,
    ) -> Result<Handoff, Error> {
        stage_installation(
            target_executable,
            source,
            expected_version,
            gui_pid,
            relaunch,
            true,
        )
    }

    fn stage_installation(
        target: &Path,
        source: &Path,
        expected_version: &str,
        gui_pid: Option<u32>,
        relaunch: bool,
        loose: bool,
    ) -> Result<Handoff, Error> {
        let target = fs::canonicalize(target)?;
        reject_ephemeral_target(&target)?;
        let parent = target.parent().ok_or("update target has no parent")?;
        // All renamed CLI copies in one directory share daemon/agent/web files,
        // so serialize on that component set rather than only the GUI filename.
        let lock_target = if loose {
            parent.join(".neoism-loose-components")
        } else {
            target.clone()
        };
        let mut lock = UpdateLock::acquire(&lock_target)?;
        let gui_name = std::ffi::OsStr::new("neoism");
        let (hashes, require_signature) = if loose {
            if installation(&target)? != Installation::Loose(target.clone()) {
                return Err("loose update target resolves inside a bundle".into());
            }
            loose_components(&target)?;
            let hashes = loose_manifest(source, gui_name)?;
            let signature = signed(&target)?;
            verify_loose(source, gui_name, expected_version, &hashes, signature)?;
            (hashes, signature)
        } else {
            if plist(&target, "CFBundleIdentifier")? != BUNDLE_ID {
                return Err("update target is not a Neoism bundle".into());
            }
            let hashes = manifest(source)?;
            let signature = signed(&target)? || signed(source)?;
            verify(source, expected_version, &hashes, signature)?;
            (hashes, signature)
        };
        // Same-filesystem staging also fails early on a read-only installation.
        let staging = tempfile::Builder::new()
            .prefix(".neoism-update-")
            .tempdir_in(parent)?;
        let staged = staging.path().join(if loose {
            "replacement"
        } else {
            "replacement.app"
        });
        if loose {
            fs::create_dir(&staged)?;
            for component in BINS.into_iter().chain(["web"]) {
                checked(
                    Command::new("/usr/bin/ditto")
                        .arg(source.join(component))
                        .arg(staged.join(component)),
                )?;
            }
            verify_loose(
                &staged,
                gui_name,
                expected_version,
                &hashes,
                require_signature,
            )?;
        } else {
            checked(Command::new("/usr/bin/ditto").arg(source).arg(&staged))?;
            verify(&staged, expected_version, &hashes, require_signature)?;
        }
        let updates = dirs::home_dir()
            .ok_or("cannot resolve update log directory")?
            .join("Library/Application Support/Neoism/updates");
        fs::create_dir_all(&updates)?;
        let operation = tempfile::Builder::new()
            .prefix("macos-")
            .tempdir_in(updates)?
            .keep();
        let plan = Plan {
            schema_version: 1,
            loose,
            relaunch_directory: std::env::current_dir().ok(),
            expected_version: expected_version.to_owned(),
            target,
            staged,
            backup: staging
                .path()
                .join(if loose { "previous" } else { "previous.app" }),
            lock: lock.path.clone(),
            operation: operation.clone(),
            updater_pid: std::process::id(),
            gui_pid,
            relaunch,
            signed: require_signature,
            manifest: hashes,
        };
        let plan_path = operation.join("plan.json");
        write_json(&plan_path, &plan)?;
        write_json(&lock.path.join("owner.json"), &plan_path)?;
        record(&plan, "staged", None)?;
        let helper = operation.join("neoism-update-helper");
        fs::copy(std::env::current_exe()?, &helper)?;
        let log = fs::OpenOptions::new()
            .append(true)
            .open(operation.join("helper.log"))?;
        let mut command = Command::new(helper);
        command
            .arg("--macos-update-helper")
            .arg(&plan_path)
            .current_dir(&operation)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        // setsid detaches from the controlling terminal/session before the old
        // GUI can close the terminal that launched a CLI update.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                Ok(())
            });
        }
        if let Err(error) = command.spawn() {
            record(&plan, "failed", Some(error.to_string()))?;
            return Err(error.into());
        }
        // The helper owns cleanup now. Keep both payload and lock across exit.
        let _ = staging.keep();
        lock.release_on_drop = false;
        Ok(Handoff {
            result_path: operation.join("result.json"),
            log_path: operation.join("helper.log"),
            target_executable: plan.target_executable(),
        })
    }

    fn process_path(pid: u32) -> Result<Option<PathBuf>, Error> {
        // proc_pidpath identifies the mapped executable, not argv or a truncated
        // process name. This never signals unrelated Neoism installations.
        #[link(name = "proc")]
        unsafe extern "C" {
            fn proc_pidpath(
                pid: libc::c_int,
                buffer: *mut libc::c_void,
                buffersize: u32,
            ) -> libc::c_int;
        }
        let mut bytes = vec![0u8; 4096];
        let len = unsafe {
            proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32)
        };
        if len <= 0 {
            if !crate::process_is_alive(pid) {
                return Ok(None);
            }
            return Err(
                format!("cannot identify running update companion pid {pid}").into(),
            );
        }
        use std::os::unix::ffi::OsStringExt;
        bytes.truncate(
            bytes
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(len as usize),
        );
        Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(bytes))))
    }

    fn companions(plan: &Plan) -> Result<Vec<u32>, Error> {
        let binaries = if plan.loose {
            loose_components(&plan.target)?
                .into_iter()
                .take(3)
                .map(|(_, path)| path)
                .collect::<Vec<_>>()
        } else {
            BINS.iter()
                .map(|bin| plan.target.join("Contents/MacOS").join(bin))
                .collect()
        };
        let mut names = BINS.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        if let Some(name) = plan.target_executable().file_name() {
            names.push(name.to_string_lossy().into_owned());
        }
        names.sort();
        names.dedup();
        let mut pids = Vec::new();
        for name in names {
            // Full command matching avoids truncated daemon names. Escape the
            // potentially renamed GUI filename; proc_pidpath is the authority.
            let pattern = name.chars().fold(String::new(), |mut text, ch| {
                if "\\[]().*+?{}^$|".contains(ch) {
                    text.push('\\');
                }
                text.push(ch);
                text
            });
            let output = bounded_output(
                Command::new("/usr/bin/pgrep").args(["-f", &pattern]),
                Duration::from_secs(5),
            )?;
            if !output.status.success() && output.status.code() != Some(1) {
                return Err(format!(
                    "cannot enumerate {name}: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
            for pid in String::from_utf8(output.stdout)?
                .lines()
                .filter_map(|line| line.parse::<u32>().ok())
            {
                if pid != std::process::id()
                    && process_path(pid)?.is_some_and(|path| binaries.contains(&path))
                {
                    pids.push(pid);
                }
            }
        }
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }

    fn finish(plan: &Plan) -> Result<(), Error> {
        if plan.loose {
            verify_loose(
                &plan.staged,
                std::ffi::OsStr::new("neoism"),
                &plan.expected_version,
                &plan.manifest,
                plan.signed,
            )?;
        } else {
            verify(
                &plan.staged,
                &plan.expected_version,
                &plan.manifest,
                plan.signed,
            )?;
        }
        record(plan, "waiting", None)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        crate::wait_for_process_exit(
            plan.updater_pid,
            deadline.saturating_duration_since(Instant::now()),
        )?;
        if let Some(pid) = plan.gui_pid.filter(|pid| *pid != 0) {
            crate::wait_for_process_exit(
                pid,
                deadline.saturating_duration_since(Instant::now()),
            )?;
        }
        let mut signalled = std::collections::HashSet::new();
        loop {
            let pids = companions(plan)?;
            if pids.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for bundle GUI/daemon/agent to exit; original left intact".into());
            }
            for pid in pids {
                if signalled.insert(pid)
                    && unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0
                    && crate::process_is_alive(pid)
                {
                    return Err(format!(
                        "cannot stop update companion {pid}: {}",
                        std::io::Error::last_os_error()
                    )
                    .into());
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        record(plan, "replacing", None)?;
        if plan.loose {
            replace_loose_with(
                &plan.target,
                &plan.staged,
                &plan.backup,
                |from, to| fs::rename(from, to),
                || {
                    verify_loose(
                        plan.target
                            .parent()
                            .ok_or("loose executable has no parent")?,
                        plan.target
                            .file_name()
                            .ok_or("loose executable has no name")?,
                        &plan.expected_version,
                        &plan.manifest,
                        plan.signed,
                    )?;
                    if plan.relaunch {
                        let log = fs::OpenOptions::new()
                            .append(true)
                            .open(plan.operation.join("helper.log"))?;
                        Command::new(plan.target_executable())
                            .current_dir(
                                plan.relaunch_directory
                                    .clone()
                                    .or_else(dirs::home_dir)
                                    .ok_or("cannot resolve relaunch directory")?,
                            )
                            .stdin(Stdio::null())
                            .stdout(log.try_clone()?)
                            .stderr(log)
                            .spawn()?;
                    }
                    Ok(())
                },
            )?;
        } else {
            replace_bundle(&plan.target, &plan.staged, &plan.backup, |target| {
                verify(target, &plan.expected_version, &plan.manifest, plan.signed)?;
                if plan.relaunch {
                    checked(Command::new("/usr/bin/open").arg("-n").arg(target))?;
                }
                Ok(())
            })?;
        }
        // Best-effort observation of the exact new executable, not a process
        // name or another installed copy. Absence is not hidden as health.
        let mut observed_gui_pid = None;
        if plan.relaunch {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(pids) = companions(plan) {
                    observed_gui_pid = pids.into_iter().find(|pid| {
                        process_path(*pid)
                            .ok()
                            .flatten()
                            .is_some_and(|path| path == plan.target_executable())
                    });
                    if observed_gui_pid.is_some() {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        // Retain backup: LaunchServices acceptance/process presence is not health.
        record_with_pid(
            plan,
            if plan.relaunch && !plan.loose {
                "open_accepted"
            } else {
                "installed"
            },
            None,
            observed_gui_pid,
        )
    }

    /// Runs before the normal logger. The caller routes process stdout/stderr to
    /// helper.log, and every operational failure is persisted in result.json.
    pub fn run_helper() -> Result<bool, Error> {
        let result = run_helper_inner();
        if let Err(error) = &result {
            // Includes invalid arguments/plans and logging failures, before the
            // normal app logger exists. Legacy callers discarded both streams.
            let diagnostic = (|| -> Result<(), Error> {
                let root = dirs::home_dir()
                    .ok_or("cannot resolve update log directory")?
                    .join("Library/Application Support/Neoism/updates");
                fs::create_dir_all(&root)?;
                let dir = tempfile::Builder::new()
                    .prefix("helper-error-")
                    .tempdir_in(root)?
                    .keep();
                let log_path = dir.join("helper.log");
                let mut log = fs::File::create(&log_path)?;
                writeln!(log, "macOS update helper failed: {error}")?;
                log.sync_all()?;
                write_json(
                    &dir.join("result.json"),
                    &serde_json::json!({
                        "schema_version": 1, "status": "failed", "health_verified": false,
                        "error": error.to_string(), "log_path": log_path,
                    }),
                )?;
                eprintln!(
                    "macOS update helper failed: {error}; diagnostic: {}",
                    dir.display()
                );
                Ok(())
            })();
            if let Err(log_error) = diagnostic {
                eprintln!("macOS update helper failed: {error}; cannot persist diagnostic: {log_error}");
            }
        }
        result
    }

    fn run_helper_inner() -> Result<bool, Error> {
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        let Some(args) = helper_args(&args, env!("CARGO_PKG_VERSION"))? else {
            return Ok(false);
        };
        let plan_path = match args {
            HelperArgs::Plan(path) => path,
            HelperArgs::Legacy {
                target,
                source,
                gui_pid,
                relaunch,
                expected_version,
            } => {
                // Older callers did not detach us or keep stderr. Detach as
                // early as possible, and persist even preflight failures.
                unsafe {
                    libc::signal(libc::SIGHUP, libc::SIG_IGN);
                    if libc::setsid() == -1 && libc::getsid(0) != libc::getpid() {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                let updates = dirs::home_dir()
                    .ok_or("cannot resolve update log directory")?
                    .join("Library/Application Support/Neoism/updates");
                fs::create_dir_all(&updates)?;
                let diagnostic = tempfile::Builder::new()
                    .prefix("legacy-")
                    .tempdir_in(updates)?
                    .keep();
                let result =
                    stage_update(&target, &source, &expected_version, gui_pid, relaunch);
                let (status, error, result_path, log_path) = match &result {
                    Ok(handoff) => (
                        "handed_off",
                        None,
                        Some(&handoff.result_path),
                        Some(&handoff.log_path),
                    ),
                    Err(error) => ("failed", Some(error.to_string()), None, None),
                };
                write_json(
                    &diagnostic.join("result.json"),
                    &serde_json::json!({
                        "schema_version": 1, "status": status, "expected_version": expected_version,
                        "target": target, "error": error, "health_verified": false,
                        "result_path": result_path, "log_path": log_path,
                    }),
                )?;
                let mut log = fs::File::create(diagnostic.join("helper.log"))?;
                writeln!(
                    log,
                    "{status}: {}",
                    error
                        .as_deref()
                        .unwrap_or("see result_path for the detached helper outcome")
                )?;
                log.sync_all()?;
                result?;
                return Ok(true);
            }
        };
        let plan: Plan = serde_json::from_slice(&fs::read(&plan_path)?)?;
        let result = (|| {
            if plan.schema_version != 1 {
                return Err("unsupported macOS update plan schema".into());
            }
            let owner: PathBuf =
                serde_json::from_slice(&fs::read(plan.lock.join("owner.json"))?)?;
            if owner != plan_path {
                return Err("update lock owner mismatch".into());
            }
            finish(&plan)
        })();
        if let Err(error) = &result {
            record(&plan, "failed", Some(error.to_string()))?;
        }
        // Only remove a lock owned by this plan; failures before ownership was
        // established must not unlock another updater.
        let rollback_incomplete = result
            .as_ref()
            .err()
            .is_some_and(|error| error.to_string().contains("rollback failed"));
        if !rollback_incomplete
            && fs::read(plan.lock.join("owner.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<PathBuf>(&bytes).ok())
                .is_some_and(|owner| owner == plan_path)
        {
            fs::remove_dir_all(&plan.lock)?;
        }
        result?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(root: &Path, name: &str) -> PathBuf {
        let app = root.join(name);
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(app.join("Contents/Resources/web")).unwrap();
        for bin in BINS {
            let path = app.join("Contents/MacOS").join(bin);
            fs::write(&path, bin).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        fs::write(app.join("Contents/Info.plist"), BUNDLE_ID).unwrap();
        fs::write(app.join("Contents/Resources/web/index.html"), "web").unwrap();
        fs::write(app.join("Contents/Resources/neoism.icns"), "icon").unwrap();
        app
    }
    fn resolve(exe: &Path) -> Result<Installation, Error> {
        resolve_installation(exe, |app| {
            Ok(fs::read_to_string(app.join("Contents/Info.plist"))?)
        })
    }

    fn loose_fixture(root: &Path, version: &str) -> PathBuf {
        fs::create_dir_all(root.join("web")).unwrap();
        for binary in BINS {
            let path = root.join(binary);
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s\\n' '{binary} {version}'\n"),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        fs::write(root.join("web/index.html"), version).unwrap();
        root.join("neoism")
    }

    #[test]
    fn compiled_versions_reject_new_plist_with_old_binary_and_wrong_component() {
        for binary in BINS {
            assert!(check_compiled_version(
                &format!("{binary} 0.7.8\n"),
                binary,
                "v0.7.8"
            )
            .is_ok());
            assert!(check_compiled_version(
                &format!("{binary} 0.7.7\n"),
                binary,
                "v0.7.8"
            )
            .is_err());
            assert!(check_compiled_version(
                &format!("{binary} 0.7.8-beta.1\n"),
                binary,
                "v0.7.8"
            )
            .is_err());
        }
        for bad in [
            "0.7.8",
            "neoism-agent 0.7.8",
            "neoism 0.7.8 trailing",
            "neoism 0.7.80",
            "",
        ] {
            assert!(check_compiled_version(bad, "neoism", "v0.7.8").is_err());
        }
    }

    #[test]
    fn loose_missing_component_never_replaces_the_gui() {
        for missing in ["neoism-agent", "web/index.html"] {
            let root = tempfile::tempdir().unwrap();
            let target = loose_fixture(&root.path().join("installed"), "0.7.7");
            let staged = root.path().join("staged");
            loose_fixture(&staged, "0.7.8");
            let original =
                loose_manifest(target.parent().unwrap(), std::ffi::OsStr::new("neoism"))
                    .unwrap();
            fs::remove_file(staged.join(missing)).unwrap();
            assert!(replace_loose_with(
                &target,
                &staged,
                &root.path().join("backup"),
                |_, _| panic!("nothing may move before every component is staged"),
                || Ok(())
            )
            .is_err());
            assert_eq!(
                loose_manifest(target.parent().unwrap(), std::ffi::OsStr::new("neoism"))
                    .unwrap(),
                original
            );
        }
    }

    #[test]
    fn loose_late_rename_failure_rolls_back_all_three_binaries_and_web() {
        let root = tempfile::tempdir().unwrap();
        let target = loose_fixture(&root.path().join("installed"), "0.7.7");
        let staged = root.path().join("staged");
        loose_fixture(&staged, "0.7.8");
        let original =
            loose_manifest(target.parent().unwrap(), std::ffi::OsStr::new("neoism"))
                .unwrap();
        let error = replace_loose_with(
            &target,
            &staged,
            &root.path().join("backup"),
            |from, to| {
                if from == staged.join("web") {
                    return Err(std::io::Error::other(
                        "injected late web rename failure",
                    ));
                }
                fs::rename(from, to)
            },
            || panic!("late replacement failure must not verify or launch"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("original components restored"));
        assert_eq!(
            loose_manifest(target.parent().unwrap(), std::ffi::OsStr::new("neoism"))
                .unwrap(),
            original
        );
    }

    #[test]
    fn loose_installed_version_mismatch_rolls_back_every_component() {
        let root = tempfile::tempdir().unwrap();
        let target = loose_fixture(&root.path().join("installed"), "0.7.7");
        let staged = root.path().join("staged");
        loose_fixture(&staged, "0.7.8");
        let original =
            loose_manifest(target.parent().unwrap(), std::ffi::OsStr::new("neoism"))
                .unwrap();
        let error = replace_loose_with(
            &target,
            &staged,
            &root.path().join("backup"),
            |from, to| fs::rename(from, to),
            || check_compiled_version("neoism-agent 0.7.7", "neoism-agent", "v0.7.8"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("compiled version mismatch"));
        assert_eq!(
            loose_manifest(target.parent().unwrap(), std::ffi::OsStr::new("neoism"))
                .unwrap(),
            original
        );
    }

    #[test]
    fn loose_success_preserves_unrelated_files_other_app_and_actual_gui_name() {
        let root = tempfile::tempdir().unwrap();
        let original_target = loose_fixture(&root.path().join("installed"), "0.7.7");
        let target = original_target.with_file_name("my-renamed-neoism");
        fs::rename(original_target, &target).unwrap();
        fs::write(target.with_file_name("other-cli"), "unrelated CLI").unwrap();
        let other_app = bundle(root.path(), "Applications/Neoism.app");
        let app_hashes = manifest(&other_app).unwrap();
        let staged = root.path().join("staged");
        loose_fixture(&staged, "0.7.8");
        let expected = loose_manifest(&staged, std::ffi::OsStr::new("neoism")).unwrap();
        let backup = root.path().join("backup");
        replace_loose_with(
            &target,
            &staged,
            &backup,
            |from, to| fs::rename(from, to),
            || {
                assert_eq!(
                    loose_manifest(
                        target.parent().unwrap(),
                        target.file_name().unwrap()
                    )?,
                    expected
                );
                Ok(())
            },
        )
        .unwrap();
        assert!(backup.join("neoism").is_file());
        assert!(backup.join("web/index.html").is_file());
        assert!(!target.with_file_name("neoism").exists());
        assert_eq!(
            fs::read_to_string(target.with_file_name("other-cli")).unwrap(),
            "unrelated CLI"
        );
        assert_eq!(manifest(&other_app).unwrap(), app_hashes);
    }

    #[test]
    fn loose_rollback_failure_retains_original_component_backup() {
        let root = tempfile::tempdir().unwrap();
        let target = loose_fixture(&root.path().join("installed"), "0.7.7");
        let staged = root.path().join("staged");
        loose_fixture(&staged, "0.7.8");
        let backup = root.path().join("backup");
        let error = replace_loose_with(
            &target,
            &staged,
            &backup,
            |from, to| {
                if from == staged.join("web") || from == backup.join("neoism-agent") {
                    return Err(std::io::Error::other(
                        "injected replacement/rollback failure",
                    ));
                }
                fs::rename(from, to)
            },
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("rollback failed"));
        assert!(backup.join("neoism-agent").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn version_probes_are_bounded_and_capture_failure_stderr() {
        use std::process::Command;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        loose_fixture(root.path(), "0.7.8");
        for binary in BINS {
            let out = bounded_output(
                Command::new(root.path().join(binary)).arg("--version"),
                Duration::from_secs(2),
            )
            .unwrap();
            assert!(out.status.success());
            check_compiled_version(
                std::str::from_utf8(&out.stdout).unwrap(),
                binary,
                "v0.7.8",
            )
            .unwrap();
        }
        let failed = bounded_output(
            Command::new("/bin/sh").args(["-c", "echo 'probe failed' >&2; exit 3"]),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(failed.status.code(), Some(3));
        assert!(String::from_utf8(failed.stderr)
            .unwrap()
            .contains("probe failed"));
        let start = Instant::now();
        let error = bounded_output(
            Command::new("/bin/sh").args(["-c", "echo 'hanging probe' >&2; sleep 30"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(error.to_string().contains("timed out"));
        assert!(error.to_string().contains("hanging probe"));
    }

    #[test]
    fn deployed_helper_abi_uses_new_payload_compiled_version() {
        let args = [
            "--macos-update-helper",
            "/Applications/Neoism.app",
            "/tmp/release/Neoism.app",
            "123",
            "1",
        ]
        .map(std::ffi::OsString::from);
        assert_eq!(
            helper_args(&args, "0.7.8").unwrap(),
            Some(HelperArgs::Legacy {
                target: PathBuf::from("/Applications/Neoism.app"),
                source: PathBuf::from("/tmp/release/Neoism.app"),
                gui_pid: Some(123),
                relaunch: true,
                expected_version: "0.7.8".into(),
            })
        );
        let mut explicit = args.to_vec();
        explicit.push("v0.8.0".into());
        assert!(
            matches!(helper_args(&explicit, "0.7.8").unwrap(), Some(HelperArgs::Legacy { expected_version, .. }) if expected_version == "v0.8.0")
        );
        let plan =
            ["--macos-update-helper", "/tmp/plan.json"].map(std::ffi::OsString::from);
        assert_eq!(
            helper_args(&plan, "0.7.8").unwrap(),
            Some(HelperArgs::Plan(PathBuf::from("/tmp/plan.json")))
        );
        assert!(helper_args(&["update".into()], "0.7.8").unwrap().is_none());
        assert!(helper_args(&["--macos-update-helper".into()], "0.7.8").is_err());
    }

    #[test]
    fn loose_cli_never_selects_either_app_copy() {
        let root = tempfile::tempdir().unwrap();
        bundle(root.path(), "Applications/Neoism.app");
        bundle(root.path(), "home/Applications/Neoism.app");
        let cli = root.path().join("bin/neoism");
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        fs::write(&cli, "cli").unwrap();
        assert_eq!(
            resolve(&cli).unwrap(),
            Installation::Loose(fs::canonicalize(cli).unwrap())
        );
    }

    #[test]
    fn selects_actual_renamed_bundle_not_other_copy() {
        let root = tempfile::tempdir().unwrap();
        bundle(root.path(), "Applications/Neoism.app");
        let renamed = bundle(root.path(), "home/Applications/Neoism Preview.app");
        assert_eq!(
            resolve(&renamed.join("Contents/MacOS/neoism")).unwrap(),
            Installation::Bundle(renamed)
        );
        let no_extension = bundle(root.path(), "Neoism renamed without extension");
        assert_eq!(
            resolve(&no_extension.join("Contents/MacOS/neoism")).unwrap(),
            Installation::Bundle(no_extension)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_launchers_resolve_to_actual_bundle_or_loose_executable() {
        let root = tempfile::tempdir().unwrap();
        let app = bundle(root.path(), "Renamed.app");
        let launcher = root.path().join("neoism");
        std::os::unix::fs::symlink(app.join("Contents/MacOS/neoism"), &launcher).unwrap();
        assert_eq!(resolve(&launcher).unwrap(), Installation::Bundle(app));
        fs::remove_file(&launcher).unwrap();
        let cli = root.path().join("loose");
        fs::write(&cli, "cli").unwrap();
        std::os::unix::fs::symlink(&cli, &launcher).unwrap();
        assert_eq!(resolve(&launcher).unwrap(), Installation::Loose(cli));
    }

    #[test]
    fn foreign_or_malformed_bundle_is_not_a_loose_install() {
        let root = tempfile::tempdir().unwrap();
        let app = bundle(root.path(), "Other.app");
        fs::write(app.join("Contents/Info.plist"), "org.other.app").unwrap();
        assert!(resolve(&app.join("Contents/MacOS/neoism")).is_err());
        let nested = app.join("Contents/Resources/neoism");
        fs::write(&nested, "neoism").unwrap();
        assert!(resolve(&nested).is_err());
    }

    #[test]
    fn mounted_and_translocated_targets_fail_explicitly() {
        assert!(reject_ephemeral_target(Path::new(
            "/Volumes/Neoism/Neoism.app/Contents/MacOS/neoism"
        ))
        .is_err());
        assert!(reject_ephemeral_target(Path::new(
            "/private/var/folders/x/AppTranslocation/id/d/Neoism.app"
        ))
        .is_err());
    }

    #[test]
    fn concurrent_updates_cannot_acquire_the_same_target_lock() {
        let root = tempfile::tempdir().unwrap();
        let app = bundle(root.path(), "Neoism.app");
        let first = UpdateLock::acquire(&app).unwrap();
        assert!(UpdateLock::acquire(&app).is_err());
        let other = bundle(root.path(), "Other.app");
        assert!(UpdateLock::acquire(&other).is_ok());
        drop(first);
        assert!(UpdateLock::acquire(&app).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_target_fails_before_replacement() {
        use std::os::unix::fs::PermissionsExt;
        // Root deliberately bypasses Unix mode bits; this is not a mount test.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let app = bundle(root.path(), "Neoism.app");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let failed = UpdateLock::acquire(&app).is_err();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(failed);
        assert!(app.join("Contents/MacOS/neoism").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn payload_links_and_non_executable_companions_fail_verification() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let app = bundle(root.path(), "Neoism.app");
        let agent = app.join("Contents/MacOS/neoism-agent");
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(manifest(&app).is_err());
        fs::set_permissions(agent, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(
            "/etc/passwd",
            app.join("Contents/Resources/external"),
        )
        .unwrap();
        assert!(manifest(&app).is_err());
    }

    #[test]
    fn staged_rename_failure_restores_original() {
        let root = tempfile::tempdir().unwrap();
        let target = bundle(root.path(), "Neoism.app");
        let staged = bundle(root.path(), "staged.app");
        let backup = root.path().join("backup.app");
        let error = replace_bundle_with(
            &target,
            &staged,
            &backup,
            |from, to| {
                if from == staged {
                    return Err(std::io::Error::other("injected second rename failure"));
                }
                fs::rename(from, to)
            },
            |_| panic!("must not verify a failed replacement"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("original restored"));
        assert!(target.is_dir());
        assert!(!backup.exists());
    }

    #[test]
    fn expected_version_mismatch_rolls_back_without_false_completion() {
        let root = tempfile::tempdir().unwrap();
        let target = bundle(root.path(), "Neoism.app");
        let staged = bundle(root.path(), "staged.app");
        fs::write(target.join("old-marker"), "old").unwrap();
        let error =
            replace_bundle(&target, &staged, &root.path().join("backup.app"), |_| {
                check_version("0.7.6", "v0.7.7")
            })
            .unwrap_err();
        assert!(error.to_string().contains("version mismatch"));
        assert!(target.join("old-marker").exists());
        assert!(check_version("0.7.7\n", "v0.7.7").is_ok());
    }

    #[test]
    fn successful_replacement_retains_backup_and_checks_every_binary_and_resource() {
        let root = tempfile::tempdir().unwrap();
        let target = bundle(root.path(), "Neoism.app");
        let staged = bundle(root.path(), "staged.app");
        let expected = manifest(&staged).unwrap();
        for path in [
            "Contents/MacOS/neoism",
            "Contents/MacOS/neoism-workspace-daemon",
            "Contents/MacOS/neoism-agent",
            "Contents/Resources/web/index.html",
        ] {
            let original = fs::read(staged.join(path)).unwrap();
            fs::write(staged.join(path), "tampered").unwrap();
            assert!(verify_manifest(&staged, &expected).is_err());
            fs::write(staged.join(path), original).unwrap();
        }
        let backup = root.path().join("backup.app");
        replace_bundle(&target, &staged, &backup, |app| {
            verify_manifest(app, &expected)
        })
        .unwrap();
        assert!(backup.is_dir());
    }

    #[test]
    fn open_failure_restores_original_and_rollback_failure_preserves_backup() {
        let root = tempfile::tempdir().unwrap();
        let target = bundle(root.path(), "Neoism.app");
        let staged = bundle(root.path(), "staged.app");
        let backup = root.path().join("backup.app");
        assert!(replace_bundle(&target, &staged, &backup, |_| Err(
            "open rejected".into()
        ))
        .is_err());
        assert!(target.is_dir());
        let error = replace_bundle_with(
            &target,
            &staged,
            &backup,
            |from, to| {
                if from == staged || from == backup {
                    return Err(std::io::Error::other("injected rename failure"));
                }
                fs::rename(from, to)
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("rollback failed"));
        assert!(backup.is_dir());
    }

    #[test]
    fn durable_failed_result_does_not_claim_health_or_completion() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = Plan {
            schema_version: 1,
            loose: false,
            relaunch_directory: None,
            expected_version: "v0.7.7".into(),
            target: root.path().join("Neoism.app"),
            staged: root.path().join("staged.app"),
            backup: root.path().join("backup.app"),
            lock: root.path().join("lock"),
            operation: root.path().to_owned(),
            updater_pid: 0,
            gui_pid: None,
            relaunch: true,
            signed: false,
            manifest: Manifest::new(),
        };
        record(&plan, "failed", Some("bundle version mismatch".into())).unwrap();
        let result: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(result["status"], "failed");
        assert_eq!(result["health_verified"], false);
        assert_eq!(result["expected_version"], "v0.7.7");
        assert_eq!(
            result["target_executable"],
            plan.target.join("Contents/MacOS/neoism").to_str().unwrap()
        );
        plan.loose = true;
        plan.target = root.path().join("bin/custom-neoism");
        record(&plan, "installed", None).unwrap();
        let result: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(result["target_executable"], plan.target.to_str().unwrap());
        assert_eq!(result["status"], "installed");
        assert_eq!(result["health_verified"], false);
        assert!(root.path().join("helper.log").is_file());
    }
}
