//! Persistent, byte-preserving local ignore policy for the one reserved container.
use super::*;
use std::io::{Read, Write};
const CONTAINER: &str = ".neoism-pr-worktrees";
const MARKER: &str = ".neoism-managed-v1";
const MARKER_BYTES: &[u8] = b"Neoism managed PR worktrees v1\n";
const RULE: &[u8] = b"/.neoism-pr-worktrees/";

fn collision(repo: &Repository, target: Option<&str>) -> Result<()> {
    let index = repo.index().map_err(|_| {
        local_error("Cannot inspect the index for a managed-directory collision")
    })?;
    if index.iter().any(|e| {
        e.path == CONTAINER.as_bytes() || e.path.starts_with(b".neoism-pr-worktrees/")
    }) {
        return Err(local_error(
            "The reserved managed-worktree name is tracked; no ignore rule was added",
        ));
    }
    let mut trees = Vec::new();
    if let Ok(tree) = repo.head().and_then(|r| r.peel_to_tree()) {
        trees.push(tree);
    }
    if let Some(target) = target {
        trees.push(
            repo.find_commit(
                git2::Oid::from_str(target)
                    .map_err(|_| invalid("Invalid worktree head"))?,
            )
            .and_then(|c| c.tree())
            .map_err(|_| {
                local_error("Cannot inspect PR tree for a reserved-directory collision")
            })?,
        );
    }
    if trees
        .iter()
        .any(|tree| tree.get_path(Path::new(CONTAINER)).is_ok())
    {
        return Err(local_error("A checked-out or target tree tracks the reserved managed-worktree name; checkout refused"));
    }
    Ok(())
}
fn validate_container(path: &Path, common: &Path, allow_legacy: bool) -> Result<()> {
    plain_managed_directory(path)?;
    let marker = path.join(MARKER);
    let marked = match fs::symlink_metadata(&marker) {
        Ok(m)
            if m.is_file()
                && !m.file_type().is_symlink()
                && m.len() == MARKER_BYTES.len() as u64 =>
        {
            fs::read(&marker).ok().as_deref() == Some(MARKER_BYTES)
        }
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(local_error(
                "Cannot inspect managed-directory ownership marker",
            ))
        }
    };
    let mut children = 0;
    for entry in fs::read_dir(path)
        .map_err(|_| local_error("Cannot inspect reserved managed directory"))?
    {
        let entry = entry
            .map_err(|_| local_error("Cannot inspect reserved managed directory"))?;
        if entry.file_name() == MARKER && marked {
            continue;
        }
        children += 1;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            local_error("Reserved directory collides with unrelated content")
        })?;
        if !name.starts_with("neoism-pr-") {
            return Err(local_error("Reserved directory contains unrelated content; it will not be silently ignored"));
        }
        plain_managed_directory(&entry.path())?;
        let child = Repository::open(entry.path()).map_err(|_|local_error("Reserved directory contains an unregistered or incomplete worktree; inspect it before retrying"))?;
        if !child.is_worktree()
            || child
                .workdir()
                .and_then(|p| crate::path::canonicalize(p).ok())
                != crate::path::canonicalize(&entry.path()).ok()
            || (!marked
                && child
                    .head()
                    .ok()
                    .and_then(|r| r.shorthand().map(str::to_owned))
                    .is_none_or(|branch| {
                        !branch.starts_with("neoism/pr-")
                            || branch.replace('/', "-") != name
                    }))
            || crate::path::canonicalize(child.commondir()).ok().as_deref()
                != Some(common)
        {
            return Err(local_error("Reserved directory contains a different repository; it will not be ignored"));
        }
    }
    if !marked && (!allow_legacy || children == 0) {
        return Err(local_error("Reserved managed-worktree directory already exists without a valid ownership marker"));
    }
    if !marked {
        // Migrate ONLY old Neoism-shaped, registered worktrees of this repository.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
            .map_err(|_| local_error("Cannot establish managed-directory ownership"))?;
        file.write_all(MARKER_BYTES)
            .and_then(|_| file.sync_all())
            .map_err(|_| local_error("Cannot persist managed-directory ownership"))?;
    }
    Ok(())
}
fn exclude_bytes(path: &Path) -> Result<(Vec<u8>, Option<fs::Permissions>)> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => {
            let mut bytes = Vec::new();
            fs::File::open(path)
                .and_then(|f| f.take(1024 * 1024 + 1).read_to_end(&mut bytes))
                .map_err(|_| local_error("Cannot read existing Git exclude rules"))?;
            if bytes.len() > 1024 * 1024 {
                return Err(local_error("Git exclude file exceeds the supported size"));
            }
            Ok((bytes, Some(m.permissions())))
        }
        Ok(_) => Err(local_error(
            "Git info/exclude must be a regular file, not a symlink",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), None)),
        Err(_) => Err(local_error("Cannot inspect Git exclude rules")),
    }
}
fn replace(lock: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(lock, target)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = lock.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                0x1 | 0x8,
            )
        } == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
fn append_exclude(common: &Path) -> Result<()> {
    let info = common.join("info");
    match fs::create_dir(&info) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(local_error("Cannot establish Git info directory")),
    }
    plain_managed_directory(&info)?;
    let exclude = info.join("exclude");
    let lock = info.join("exclude.lock");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&lock).map_err(|_| {
        local_error(
            "Git exclude rules are locked or unavailable; no rules were overwritten",
        )
    })?;
    struct Lock(PathBuf, bool);
    impl Drop for Lock {
        fn drop(&mut self) {
            if self.1 {
                let _ = fs::remove_file(&self.0);
            }
        }
    }
    let mut guard = Lock(lock.clone(), true);
    let (before, permissions) = exclude_bytes(&exclude)?;
    // Last-line placement wins over older local negations; preserve every byte.
    if before
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .last()
        .is_some_and(|line| line.strip_suffix(b"\r").unwrap_or(line) == RULE)
    {
        return Ok(());
    }
    let mut after = before.clone();
    if !after.is_empty() && !after.ends_with(b"\n") {
        after.push(b'\n');
    }
    after.extend_from_slice(RULE);
    after.push(b'\n');
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)
            .map_err(|_| local_error("Cannot preserve exclude permissions"))?;
    }
    file.write_all(&after)
        .and_then(|_| file.sync_all())
        .map_err(|_| local_error("Cannot persist local Git exclude rule"))?;
    drop(file);
    if exclude_bytes(&exclude)?.0 != before {
        return Err(local_error(
            "Git exclude rules changed concurrently; retry without overwriting them",
        ));
    }
    replace(&lock, &exclude)
        .map_err(|_| local_error("Cannot atomically replace Git exclude rules"))?;
    guard.1 = false;
    #[cfg(unix)]
    {
        let _ = fs::File::open(&info).and_then(|f| f.sync_all());
    }
    Ok(())
}

pub(super) fn managed_container(prepared: &Prepared) -> Result<PathBuf> {
    let (repo, workdir) = repository(&prepared.root)?;
    if workdir != prepared.root {
        return Err(local_error(
            "Managed worktrees require an exact repository-root declaration",
        ));
    }
    collision(&repo, Some(&prepared.head))?;
    let common = crate::path::canonicalize(repo.commondir())
        .map_err(|_| local_error("Cannot establish common Git metadata directory"))?;
    plain_managed_directory(&common)?;
    // The persistent common exclude applies to ALL linked worktrees. Refuse
    // existing collisions there, rather than hide somebody else's directory.
    let mut roots = vec![prepared.root.clone()];
    if let Ok(main) = Repository::open(&common) {
        if let Some(path) = main.workdir() {
            roots.push(path.into());
        }
    }
    for name in repo
        .worktrees()
        .map_err(|_| local_error("Cannot enumerate linked worktree scope"))?
        .iter()
        .flatten()
    {
        let worktree = repo
            .find_worktree(name)
            .map_err(|_| local_error("Cannot inspect linked worktree scope"))?;
        roots.push(worktree.path().into());
    }
    roots.sort();
    roots.dedup();
    for root in roots {
        if !root.exists() {
            continue;
        }
        let sibling = Repository::open(&root)
            .map_err(|_| local_error("Cannot inspect linked worktree repository"))?;
        collision(&sibling, None)?;
        let container = root.join(CONTAINER);
        if fs::symlink_metadata(&container).is_ok() {
            validate_container(&container, &common, true)?;
        }
    }
    plain_managed_directory(&prepared.root)?;
    let container = prepared.root.join(CONTAINER);
    let created = match fs::create_dir(&container) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&container, fs::Permissions::from_mode(0o700))
                    .map_err(|_| local_error("Cannot secure managed directory"))?;
            }
            fs::write(container.join(MARKER), MARKER_BYTES)
                .map_err(|_| local_error("Cannot mark managed directory ownership"))?;
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => return Err(local_error("Cannot create managed directory")),
    };
    validate_container(&container, &common, false)?;
    let result=append_exclude(&common).and_then(|_| {
        let reopened=Repository::open(&prepared.root).map_err(|_|local_error("Cannot verify managed-directory exclusion"))?;
        if !reopened.status_should_ignore(Path::new(CONTAINER)).map_err(|_|local_error("Cannot verify managed-directory exclusion"))? { return Err(local_error("Repository ignore rules override the managed-directory exclude; resolve that conflict explicitly")); }
        Ok(())
    });
    if result.is_err() && created {
        let _ = fs::remove_file(container.join(MARKER));
        let _ = fs::remove_dir(&container);
    }
    result?;
    Ok(container)
}
