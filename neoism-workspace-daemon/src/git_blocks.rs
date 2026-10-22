//! Exact partial edits. No client patch, fuzzy application, or whole-file fallback.
use git2::{DiffOptions, Patch, Repository};
use neoism_protocol::git::GitChangeBlock;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const LIMIT: usize = 2 * 1024 * 1024;
type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct Lock(PathBuf, bool);
impl Drop for Lock {
    fn drop(&mut self) {
        if self.1 {
            let _ = fs::remove_file(&self.0);
        }
    }
}

fn literal(root: &Path, path: &str) -> Result<PathBuf> {
    if path.is_empty()
        || path.contains(['\0', '\\'])
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err("expected a literal repository-relative file path".into());
    }
    let mut full = root.to_path_buf();
    for part in Path::new(path).components() {
        if part
            .as_os_str()
            .to_string_lossy()
            .trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(".git")
        {
            return Err("Git metadata is not a file selection".into());
        }
        full.push(part);
        match fs::symlink_metadata(&full) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("symlink paths are unsupported".into())
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(error(e)),
        }
    }
    Ok(full)
}
fn read_work(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    let meta = file.metadata().map_err(error)?;
    if !meta.is_file() || meta.len() > LIMIT as u64 {
        return Err("unsupported file type or file exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    text(&bytes)?;
    Ok(Some(bytes))
}
fn text(bytes: &[u8]) -> Result<()> {
    if bytes.len() > LIMIT || bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return Err("partial edits require UTF-8 text <=2 MiB".into());
    }
    Ok(())
}
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|b| *b == b'\n').collect()
}
fn display(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    String::from_utf8_lossy(bytes.strip_suffix(b"\r").unwrap_or(bytes)).into_owned()
}

/// Compute maximal Add/Remove runs with the same three-context presentation as
/// the cards. EOF markers never terminate or contribute to a run.
fn blocks(old: &[u8], new: &[u8]) -> Result<Vec<GitChangeBlock>> {
    let mut opts = DiffOptions::new();
    opts.context_lines(3).interhunk_lines(0);
    let patch = Patch::from_buffers(
        old,
        Some(Path::new("file")),
        new,
        Some(Path::new("file")),
        Some(&mut opts),
    )
    .map_err(error)?;
    let mut result = Vec::new();
    for h in 0..patch.num_hunks() {
        let (hunk, count) = patch.hunk(h).map_err(error)?;
        let mut a = if hunk.old_lines() == 0 {
            hunk.old_start()
        } else {
            hunk.old_start().saturating_sub(1)
        } as usize;
        let mut b = if hunk.new_lines() == 0 {
            hunk.new_start()
        } else {
            hunk.new_start().saturating_sub(1)
        } as usize;
        let mut run: Option<GitChangeBlock> = None;
        for l in 0..count {
            let line = patch.line_in_hunk(h, l).map_err(error)?;
            match line.origin() {
                '-' | '+' => {
                    let run = run.get_or_insert_with(|| GitChangeBlock {
                        old_start: a,
                        new_start: b,
                        old_lines: vec![],
                        new_lines: vec![],
                    });
                    if line.origin() == '-' {
                        run.old_lines.push(display(line.content()));
                        a += 1;
                    } else {
                        run.new_lines.push(display(line.content()));
                        b += 1;
                    }
                }
                ' ' => {
                    if let Some(run) = run.take() {
                        result.push(run);
                    }
                    a += 1;
                    b += 1;
                }
                '=' | '<' | '>' => (),
                _ => return Err("unsupported diff metadata".into()),
            }
        }
        if let Some(run) = run {
            result.push(run);
        }
    }
    Ok(result)
}
fn splice(
    source: &[u8],
    start: usize,
    count: usize,
    replacement: &[&[u8]],
) -> Result<Vec<u8>> {
    let source = lines(source);
    let end = start.checked_add(count).ok_or("invalid range")?;
    if end > source.len() {
        return Err("invalid range".into());
    }
    let mut out = Vec::new();
    for line in source[..start]
        .iter()
        .chain(replacement.iter())
        .chain(source[end..].iter())
    {
        out.extend_from_slice(line);
    }
    text(&out)?;
    Ok(out)
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = crate::process::background_command("git")
        .arg("-C")
        .arg(root)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args(args)
        .output()
        .map_err(error)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(out.stdout)
}
fn conversions(root: &Path, path: &str) -> Result<()> {
    let attrs = git(
        root,
        &[
            "check-attr",
            "-z",
            "filter",
            "text",
            "eol",
            "working-tree-encoding",
            "ident",
            "diff",
            "--",
            path,
        ],
    )?;
    for fields in attrs.split(|b| *b == 0).collect::<Vec<_>>().chunks(3) {
        if fields.len() == 3 && fields[2] != b"unspecified" {
            return Err("partial edits refuse attributed files (filters/newline conversion/diff drivers)".into());
        }
    }
    for key in ["core.autocrlf", "core.safecrlf"] {
        if let Ok(value) = git(root, &["config", "--get", key]) {
            if value != b"false\n" {
                return Err("partial edits refuse automatic newline conversion".into());
            }
        }
    }
    Ok(())
}
fn blob(repo: &Repository, id: git2::Oid) -> Result<Vec<u8>> {
    let blob = repo.find_blob(id).map_err(error)?;
    text(blob.content())?;
    Ok(blob.content().to_vec())
}
fn head(repo: &Repository) -> Result<Option<git2::Oid>> {
    match repo.head() {
        Ok(h) => Ok(Some(h.peel_to_commit().map_err(error)?.id())),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(error(e)),
    }
}

fn index_snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            fs::read(path).map(Some).map_err(error)
        }
        Ok(_) => Err("unsupported index file type".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(error(e)),
    }
}

// Generate a valid empty index without releasing the real index.lock or touching
// the real index. The UUID path must not exist when read-tree opens it.
fn empty_index(root: &Path, git_dir: &Path) -> Result<Vec<u8>> {
    let alt = Lock(
        git_dir.join(format!("neoism-block-index-{}", uuid::Uuid::new_v4())),
        true,
    );
    let out = crate::process::background_command("git")
        .arg("-C")
        .arg(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_INDEX_FILE", &alt.0)
        .args(["read-tree", "--empty"])
        .output()
        .map_err(error)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    fs::read(&alt.0).map_err(error)
}

fn new_entry(path: &str, full: &Path) -> Result<git2::IndexEntry> {
    let meta = fs::symlink_metadata(full).map_err(error)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("untracked selection is not a regular file".into());
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        }
    };
    #[cfg(not(unix))]
    let mode = 0o100644;
    Ok(git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: git2::Oid::zero(),
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    })
}

/// Stage or restore a single exact HEAD→worktree selection. The same selection
/// must independently be a complete index→worktree run (only old_start may
/// shift). This rejects already-staged and overlapping mixed changes.
/// Raw blob slices, never normalized request text, supply replacement bytes.
pub fn apply_change_block(
    root: &Path,
    path: &str,
    block: &GitChangeBlock,
    restore: bool,
) -> Result<()> {
    let scope = root.canonicalize().map_err(error)?;
    let repo = Repository::discover(&scope).map_err(error)?;
    let repo_root = repo
        .workdir()
        .ok_or("bare repository")?
        .canonicalize()
        .map_err(error)?;
    let root = repo_root.as_path();
    let full = literal(root, path)?;
    if !full.starts_with(&scope) {
        return Err("change block path is outside the declared workspace".into());
    }
    conversions(root, path)?;
    let index_path = repo.path().join("index");
    let lock_path = repo.path().join("index.lock");
    // Git's exclusive index lock serializes both operations with index writers.
    let mut lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| format!("cannot lock index: {e}"))?;
    let mut lock = Lock(lock_path, true);
    let index_bytes = index_snapshot(&index_path)?;
    let initial_bytes = match &index_bytes {
        Some(bytes) => {
            lock_file
                .set_permissions(fs::metadata(&index_path).map_err(error)?.permissions())
                .map_err(error)?;
            bytes.clone()
        }
        None => empty_index(root, repo.path())?,
    };
    lock_file.write_all(&initial_bytes).map_err(error)?;
    drop(lock_file);
    let mut index = git2::Index::open(&lock.0).map_err(error)?;
    if index.has_conflicts() {
        return Err("resolve index conflicts first".into());
    }
    let indexed = index.get_path(Path::new(path), 0);
    let untracked = indexed.is_none();
    if untracked && restore {
        return Err("restore refuses untracked files".into());
    }
    let mut entry = match indexed {
        Some(entry) => entry,
        None => new_entry(path, &full)?,
    };
    if !matches!(entry.mode, 0o100644 | 0o100755) {
        return Err("unsupported index mode".into());
    }
    if entry.flags & 0x8000 != 0 || entry.flags_extended != 0 {
        return Err(
            "assume-unchanged, skip-worktree, and intent-to-add entries are unsupported"
                .into(),
        );
    }
    let base = if untracked {
        vec![]
    } else {
        blob(&repo, entry.id)?
    };
    text(&base)?;
    let head_id = head(&repo)?;
    let old = if let Some(oid) = head_id {
        let tree = repo
            .find_commit(oid)
            .map_err(error)?
            .tree()
            .map_err(error)?;
        match tree.get_path(Path::new(path)) {
            Ok(e) if matches!(e.filemode(), 0o100644 | 0o100755) => {
                if untracked {
                    return Err("selection overlaps a staged deletion".into());
                }
                if e.filemode() as u32 != entry.mode {
                    return Err("selection has an unsupported staged mode change".into());
                }
                blob(&repo, e.id())?
            }
            Ok(_) => return Err("unsupported HEAD mode".into()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => vec![],
            Err(e) => return Err(error(e)),
        }
    } else {
        vec![]
    };
    text(&old)?;
    let work = read_work(&full)?;
    let working = work.as_deref().unwrap_or_default();
    if untracked
        && (work.is_none()
            || block.old_start != 0
            || block.new_start != 0
            || !block.old_lines.is_empty()
            || block.new_lines.len() != lines(working).len())
    {
        return Err("untracked staging requires the entire exact addition block".into());
    }
    if !blocks(&old, working)?.iter().any(|b| b == block) {
        return Err("stale selection: not an exact HEAD-to-worktree change block".into());
    }
    let candidates: Vec<_> = blocks(&base, working)?
        .into_iter()
        .filter(|b| {
            b.new_start == block.new_start
                && b.old_lines == block.old_lines
                && b.new_lines == block.new_lines
        })
        .collect();
    if candidates.len() != 1 {
        return Err("selection is staged-only or overlaps staged changes".into());
    }
    let selected = &candidates[0];
    // Normalization hides CRLF/EOF differences; a staged byte-level change in
    // the selected old range is still overlap, not permission to overwrite it.
    let head_lines = lines(&old);
    let index_lines = lines(&base);
    if head_lines[block.old_start..block.old_start + block.old_lines.len()]
        != index_lines[selected.old_start..selected.old_start + selected.old_lines.len()]
    {
        return Err("selection overlaps staged newline/content changes".into());
    }
    let target = if restore {
        let raw = lines(&base);
        splice(
            working,
            selected.new_start,
            selected.new_lines.len(),
            &raw[selected.old_start..selected.old_start + selected.old_lines.len()],
        )?
    } else {
        let raw = lines(working);
        splice(
            &base,
            selected.old_start,
            selected.old_lines.len(),
            &raw[selected.new_start..selected.new_start + selected.new_lines.len()],
        )?
    };
    // Prepare before revalidation; never mutate using request-provided bytes.
    let target_oid = if restore {
        None
    } else {
        Some(repo.blob(&target).map_err(error)?)
    };
    if literal(root, path)? != full
        || read_work(&full)? != work
        || index_snapshot(&index_path)? != index_bytes
        || head(&repo)? != head_id
    {
        return Err(
            "repository changed during partial operation; refresh and retry".into(),
        );
    }
    conversions(root, path)?;
    if restore {
        // Exclusive same-directory temporary + atomic replacement keeps readers
        // from observing partially written text. Keep original executable bits.
        let temp = Lock(
            full.with_file_name(format!(".neoism-block-{}", uuid::Uuid::new_v4())),
            true,
        );
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp.0)
            .map_err(error)?;
        if let Ok(meta) = fs::metadata(&full) {
            f.set_permissions(meta.permissions()).map_err(error)?;
        }
        #[cfg(unix)]
        if work.is_none() {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(entry.mode & 0o777))
                .map_err(error)?;
        }
        f.write_all(&target).map_err(error)?;
        f.sync_all().map_err(error)?;
        drop(f);
        if read_work(&full)? != work
            || index_snapshot(&index_path)? != index_bytes
            || head(&repo)? != head_id
        {
            return Err("repository changed before restore".into());
        }
        literal(root, path)?;
        fs::rename(&temp.0, &full).map_err(error)?;
    } else {
        entry.id = target_oid.unwrap();
        entry.file_size = target.len() as u32;
        // Zero stat cache: never incorrectly report a partially staged file clean.
        entry.mtime = git2::IndexTime::new(0, 0);
        entry.ctime = git2::IndexTime::new(0, 0);
        if work.is_none() && target.is_empty() {
            index.remove_path(Path::new(path)).map_err(error)?;
        } else {
            index.add(&entry).map_err(error)?;
        }
        index.write().map_err(error)?;
        if read_work(&full)? != work
            || index_snapshot(&index_path)? != index_bytes
            || head(&repo)? != head_id
        {
            return Err("repository changed before stage".into());
        }
        if untracked && new_entry(path, &literal(root, path)?)?.mode != entry.mode {
            return Err("file mode changed before stage".into());
        }
        fs::rename(&lock.0, &index_path).map_err(error)?;
        lock.1 = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(old: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]).unwrap();
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
        )
        .unwrap();
        git(dir.path(), &["config", "user.name", "Test"]).unwrap();
        git(dir.path(), &["config", "core.autocrlf", "false"]).unwrap();
        fs::write(dir.path().join("file"), old).unwrap();
        git(dir.path(), &["add", "--", "file"]).unwrap();
        git(dir.path(), &["commit", "-qm", "base"]).unwrap();
        dir
    }
    fn index(root: &Path) -> Vec<u8> {
        git(root, &["show", ":file"]).unwrap()
    }
    #[test]
    fn two_blocks_in_one_hunk_and_staged_other_block() {
        let old = b"a\nb\nc\nd\ne\nf\n";
        let new = b"A\nb\nc\nD\ne\nf\n";
        let dir = fixture(old);
        fs::write(dir.path().join("file"), new).unwrap();
        let selected = blocks(old, new).unwrap();
        assert_eq!(selected.len(), 2);
        let whole_hunk = GitChangeBlock {
            old_start: 0,
            new_start: 0,
            old_lines: lines(old).iter().map(|l| display(l)).collect(),
            new_lines: lines(new).iter().map(|l| display(l)).collect(),
        };
        assert!(apply_change_block(dir.path(), "file", &whole_hunk, false).is_err());
        apply_change_block(dir.path(), "file", &selected[0], false).unwrap();
        assert_eq!(index(dir.path()), b"A\nb\nc\nd\ne\nf\n");
        let index_file = fs::read(dir.path().join(".git/index")).unwrap();
        apply_change_block(dir.path(), "file", &selected[1], true).unwrap();
        assert_eq!(
            fs::read(dir.path().join("file")).unwrap(),
            b"A\nb\nc\nd\ne\nf\n"
        );
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index_file);
        assert!(apply_change_block(dir.path(), "file", &selected[0], false).is_err());
    }
    #[test]
    fn shifted_index_anchor_preserves_staged_insert() {
        let old = b"a\nb\nc\nd\ne\nf\n";
        let staged = b"prefix\na\nb\nc\nd\ne\nf\n";
        let new = b"prefix\na\nb\nc\nd\nE\nf\n";
        let dir = fixture(old);
        fs::write(dir.path().join("file"), staged).unwrap();
        git(dir.path(), &["add", "file"]).unwrap();
        fs::write(dir.path().join("file"), new).unwrap();
        let selection = blocks(old, new).unwrap().pop().unwrap();
        apply_change_block(dir.path(), "file", &selection, false).unwrap();
        assert_eq!(index(dir.path()), new);
    }
    #[test]
    fn mixed_and_stale_refuse_without_mutation() {
        let dir = fixture(b"a\nb\nc\n");
        fs::write(dir.path().join("file"), b"staged\nb\nc\n").unwrap();
        git(dir.path(), &["add", "file"]).unwrap();
        fs::write(dir.path().join("file"), b"working\nb\nc\n").unwrap();
        let selection = blocks(b"a\nb\nc\n", b"working\nb\nc\n").unwrap().remove(0);
        let before = fs::read(dir.path().join(".git/index")).unwrap();
        for restore in [false, true] {
            assert!(apply_change_block(dir.path(), "file", &selection, restore).is_err());
        }
        assert_eq!(before, fs::read(dir.path().join(".git/index")).unwrap());
        let mut stale = selection;
        stale.new_start += 1;
        assert!(apply_change_block(dir.path(), "file", &stale, false).is_err());
    }
    #[test]
    fn newline_crlf_insert_delete_roundtrip() {
        for (old, new) in [
            (&b"a\r\nb\r\nc\r\n"[..], &b"a\r\nB\r\nc\r\n"[..]),
            (&b"a\nb"[..], &b"a\nB"[..]),
            (&b"a\nb"[..], &b"a\nb\n"[..]),
            (&b"a\nb\n"[..], &b"insert\na\nb\n"[..]),
            (&b"a\nb\n"[..], &b"b\n"[..]),
            (&b""[..], &b"first"[..]),
            (&b"last"[..], &b""[..]),
        ] {
            for restore in [false, true] {
                let dir = fixture(old);
                fs::write(dir.path().join("file"), new).unwrap();
                let selections = blocks(old, new).unwrap();
                assert_eq!(selections.len(), 1);
                apply_change_block(dir.path(), "file", &selections[0], restore).unwrap();
                assert_eq!(index(dir.path()), if restore { old } else { new });
                assert_eq!(
                    fs::read(dir.path().join("file")).unwrap(),
                    if restore { old } else { new }
                );
            }
        }
    }
    #[test]
    fn locks_filters_binary_paths_and_permissions() {
        let dir = fixture(b"old\n");
        fs::write(dir.path().join("file"), b"new\n").unwrap();
        let selection = blocks(b"old\n", b"new\n").unwrap().remove(0);
        let message = neoism_protocol::git::GitClientMessage::ApplyBlock {
            path: "file".into(),
            block: selection.clone(),
            restore: false,
        };
        assert!(matches!(
            super::super::required_permission(&message),
            neoism_protocol::pairing::Permission::WriteFiles
        ));
        let lock = dir.path().join(".git/index.lock");
        fs::write(&lock, b"other writer").unwrap();
        assert!(apply_change_block(dir.path(), "file", &selection, false).is_err());
        assert_eq!(fs::read(&lock).unwrap(), b"other writer");
        fs::remove_file(lock).unwrap();
        for path in ["../file", "/file", ".git/index", ":(glob)*"] {
            assert!(apply_change_block(dir.path(), path, &selection, false).is_err());
        }
        fs::write(dir.path().join(".gitattributes"), b"file filter=anything\n").unwrap();
        assert!(apply_change_block(dir.path(), "file", &selection, false).is_err());
        fs::remove_file(dir.path().join(".gitattributes")).unwrap();
        fs::write(dir.path().join("file"), b"binary\0\n").unwrap();
        assert!(apply_change_block(dir.path(), "file", &selection, false).is_err());
    }
    #[test]
    fn restore_leaves_other_working_blocks_and_staged_binary_entry() {
        let old = b"a\nb\nc\nd\ne\nf\ng\n";
        let new = b"A\nb\nc\nD\ne\nf\nG\n";
        let dir = fixture(old);
        fs::write(dir.path().join("other"), b"staged\0binary\r\n").unwrap();
        git(dir.path(), &["add", "other"]).unwrap();
        let other_id = git(dir.path(), &["rev-parse", ":other"]).unwrap();
        fs::write(dir.path().join("file"), new).unwrap();
        let selected = blocks(old, new).unwrap();
        assert_eq!(selected.len(), 3);
        let before = fs::read(dir.path().join(".git/index")).unwrap();
        apply_change_block(dir.path(), "file", &selected[1], true).unwrap();
        assert_eq!(
            fs::read(dir.path().join("file")).unwrap(),
            b"A\nb\nc\nd\ne\nf\nG\n"
        );
        assert_eq!(before, fs::read(dir.path().join(".git/index")).unwrap());
        apply_change_block(dir.path(), "file", &selected[0], false).unwrap();
        assert_eq!(index(dir.path()), b"A\nb\nc\nd\ne\nf\ng\n");
        assert_eq!(git(dir.path(), &["rev-parse", ":other"]).unwrap(), other_id);
        assert_eq!(
            git(dir.path(), &["show", ":other"]).unwrap(),
            b"staged\0binary\r\n"
        );
    }
    #[test]
    fn actual_deleted_file_stage_and_restore() {
        for restore in [false, true] {
            let dir = fixture(b"deleted\n");
            fs::remove_file(dir.path().join("file")).unwrap();
            let selected = blocks(b"deleted\n", b"").unwrap().remove(0);
            apply_change_block(dir.path(), "file", &selected, restore).unwrap();
            if restore {
                assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"deleted\n");
            } else {
                assert!(!dir.path().join("file").exists());
                assert!(git(dir.path(), &["show", ":file"]).is_err());
            }
        }
    }
    // These literal pathspec characters cannot be Windows file names.
    #[cfg(not(windows))]
    #[test]
    fn literal_metacharacter_path() {
        let dir = fixture(b"old\n");
        git(dir.path(), &["mv", "file", ":(glob)*[?]"]).unwrap();
        git(dir.path(), &["commit", "-qm", "rename"]).unwrap();
        fs::write(dir.path().join(":(glob)*[?]"), b"new\n").unwrap();
        let selected = blocks(b"old\n", b"new\n").unwrap().remove(0);
        apply_change_block(dir.path(), ":(glob)*[?]", &selected, false).unwrap();
        assert_eq!(
            git(dir.path(), &["show", ":0::(glob)*[?]"]).unwrap(),
            b"new\n"
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_parent_symlink_refuse() {
        use std::os::unix::fs::symlink;
        let dir = fixture(b"old\n");
        let selected = blocks(b"old\n", b"new\n").unwrap().remove(0);
        fs::rename(dir.path().join("file"), dir.path().join("target")).unwrap();
        symlink("target", dir.path().join("file")).unwrap();
        assert!(apply_change_block(dir.path(), "file", &selected, true).is_err());
        symlink(".", dir.path().join("parent")).unwrap();
        assert!(
            apply_change_block(dir.path(), "parent/target", &selected, true).is_err()
        );
    }
    #[test]
    fn nested_workspace_scope_rejects_siblings_but_accepts_repo_relative_child() {
        let dir = fixture(b"old\n");
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/file"), b"old\n").unwrap();
        git(dir.path(), &["add", "sub/file"]).unwrap();
        git(dir.path(), &["commit", "-qm", "nested"]).unwrap();
        fs::write(dir.path().join("file"), b"new\n").unwrap();
        fs::write(dir.path().join("sub/file"), b"new\n").unwrap();
        let scope = dir.path().join("sub");
        let selected = blocks(b"old\n", b"new\n").unwrap().remove(0);
        let before = fs::read(dir.path().join(".git/index")).unwrap();
        for restore in [false, true] {
            assert!(apply_change_block(&scope, "file", &selected, restore)
                .unwrap_err()
                .contains("outside the declared workspace"));
        }
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), before);
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"new\n");
        apply_change_block(&scope, "sub/file", &selected, false).unwrap();
        assert_eq!(git(dir.path(), &["show", ":sub/file"]).unwrap(), b"new\n");
        assert_eq!(index(dir.path()), b"old\n");
    }
    #[test]
    fn untracked_stage_preserves_staged_entries_and_refuses_restore_or_partial_selection()
    {
        let dir = fixture(b"base\n");
        fs::write(dir.path().join("file"), b"staged\r\n").unwrap();
        git(dir.path(), &["add", "file"]).unwrap();
        let staged_id = git(dir.path(), &["ls-files", "--stage", "--", "file"]).unwrap();
        let bytes = b"new\r\nfile-without-final-newline";
        fs::write(dir.path().join("new"), bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                dir.path().join("new"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        fs::write(dir.path().join("unrelated"), b"do not stage\n").unwrap();
        let selection = blocks(b"", bytes).unwrap().remove(0);
        let before = fs::read(dir.path().join(".git/index")).unwrap();
        assert!(apply_change_block(dir.path(), "new", &selection, true)
            .unwrap_err()
            .contains("untracked"));
        let mut partial = selection.clone();
        partial.new_lines.pop();
        assert!(apply_change_block(dir.path(), "new", &partial, false).is_err());
        assert_eq!(before, fs::read(dir.path().join(".git/index")).unwrap());
        apply_change_block(dir.path(), "new", &selection, false).unwrap();
        assert_eq!(git(dir.path(), &["show", ":new"]).unwrap(), bytes);
        #[cfg(unix)]
        assert_eq!(
            Repository::open(dir.path())
                .unwrap()
                .index()
                .unwrap()
                .get_path(Path::new("new"), 0)
                .unwrap()
                .mode,
            0o100755
        );
        assert_eq!(
            git(dir.path(), &["ls-files", "--stage", "--", "file"]).unwrap(),
            staged_id
        );
        assert_eq!(index(dir.path()), b"staged\r\n");
        assert!(git(dir.path(), &["show", ":unrelated"]).is_err());
        assert_eq!(fs::read(dir.path().join("new")).unwrap(), bytes);
    }
    #[test]
    fn fresh_unborn_without_index_stages_exact_addition_and_restore_leaves_index_absent()
    {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]).unwrap();
        git(dir.path(), &["config", "core.autocrlf", "false"]).unwrap();
        let bytes = b"first\r\nlast";
        fs::write(dir.path().join("new"), bytes).unwrap();
        let selection = blocks(b"", bytes).unwrap().remove(0);
        let index_path = dir.path().join(".git/index");
        assert!(!index_path.exists());
        assert!(apply_change_block(dir.path(), "new", &selection, true)
            .unwrap_err()
            .contains("untracked"));
        assert!(!index_path.exists());
        assert!(!dir.path().join(".git/index.lock").exists());
        apply_change_block(dir.path(), "new", &selection, false).unwrap();
        assert_eq!(git(dir.path(), &["show", ":new"]).unwrap(), bytes);
        assert_eq!(git(dir.path(), &["ls-files"]).unwrap(), b"new\n");
        assert!(!dir.path().join(".git/index.lock").exists());
        assert_eq!(head(&Repository::open(dir.path()).unwrap()).unwrap(), None);
    }
    #[test]
    fn missing_index_entry_with_head_path_refuses_staged_deletion() {
        let dir = fixture(b"old\n");
        git(dir.path(), &["rm", "--cached", "file"]).unwrap();
        fs::write(dir.path().join("file"), b"new\n").unwrap();
        let selected = blocks(b"old\n", b"new\n").unwrap().remove(0);
        assert!(apply_change_block(dir.path(), "file", &selected, false)
            .unwrap_err()
            .contains("staged deletion"));
    }
    #[test]
    fn unborn_head_safely_refuses_staged_overlap() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]).unwrap();
        fs::write(dir.path().join("file"), b"staged\n").unwrap();
        git(dir.path(), &["add", "file"]).unwrap();
        fs::write(dir.path().join("file"), b"working\n").unwrap();
        let selection = blocks(b"", b"working\n").unwrap().remove(0);
        assert!(apply_change_block(dir.path(), "file", &selection, false).is_err());
    }
}
