//! Resolve an agent-pane link target (`file://...` or relative path)
//! to an on-disk absolute path. Native-only because it touches the
//! filesystem (`Path::exists`, the optional name search). Pure
//! function — the host injects a name-search callback so the policy
//! stays free of `Screen` borrowing concerns.

use std::fs;
use std::path::{Path, PathBuf};

use super::input_controller::decode_percent_path;

/// Directory names skipped during the name-search descent. Match the
/// desktop fork's prior behaviour: VCS metadata and common heavy
/// build/dependency trees that would dwarf the actual workspace.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules"];

/// Strip an optional `file://` prefix, decode URI path bytes, and trim
/// whitespace. Plain filesystem references remain byte-for-byte unchanged.
fn normalize_target(target: &str) -> String {
    let target = target.trim();
    target
        .strip_prefix("file://")
        .map(decode_percent_path)
        .unwrap_or_else(|| target.to_string())
}

/// Resolve `target` against `root`:
///
/// 1. If the path is absolute and exists, return it.
/// 2. Try `root.join(target)`; if that exists, return it.
/// 3. If `target` is a single component (no slashes), fall back to
///    `find_by_name(root, target, 4)` to search the tree.
/// 4. Otherwise return `None`.
pub fn resolve_link_path(
    target: &str,
    root: &Path,
    find_by_name: impl FnOnce(&Path, &str, usize) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let raw = normalize_target(target);
    let path = PathBuf::from(&raw);
    if path.is_absolute() {
        return path.exists().then_some(path);
    }
    let direct = root.join(&path);
    if direct.exists() {
        return Some(direct);
    }
    if path.components().count() == 1 {
        return find_by_name(root, &raw, 4);
    }
    None
}

/// Breadth-then-depth filesystem walk used as the fallback for
/// single-component link targets. Returns the first entry under
/// `root` whose file name equals `name`, descending up to
/// `max_depth` levels. Skips `.git`, `target`, and `node_modules`
/// to keep the walk bounded in workspace roots.
///
/// Pure but native (filesystem) — left in `neoism-ui` behind the
/// `cfg(not(target_arch = "wasm32"))` gate.
pub fn find_file_by_name(root: &Path, name: &str, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|file| file.to_str()) == Some(name) {
            return Some(path);
        }
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skip = path
            .file_name()
            .and_then(|file| file.to_str())
            .is_some_and(|file| SKIP_DIRS.contains(&file));
        if skip {
            continue;
        }
        if let Some(found) = find_file_by_name(&path, name, max_depth - 1) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_existing_path_returned() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let absolute = tmp.path().to_string_lossy().to_string();
        let resolved = resolve_link_path(&absolute, Path::new("/"), |_, _, _| None);
        assert_eq!(resolved.as_deref(), Some(tmp.path()));
    }

    #[test]
    fn file_prefix_stripped() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let url = format!("file://{}", tmp.path().display());
        let resolved = resolve_link_path(&url, Path::new("/"), |_, _, _| None);
        assert_eq!(resolved.as_deref(), Some(tmp.path()));
    }

    #[test]
    fn file_uri_percent_encoded_spaces_are_decoded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Patriot Report.md");
        std::fs::write(&path, "report").unwrap();
        let url = format!("file://{}", path.display()).replace(' ', "%20");
        let resolved = resolve_link_path(&url, Path::new("/"), |_, _, _| None);
        assert_eq!(resolved.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn falls_back_to_name_search_for_single_component() {
        let calls = std::cell::Cell::new(0usize);
        let resolved =
            resolve_link_path("name.md", Path::new("/root"), |root, name, depth| {
                calls.set(calls.get() + 1);
                assert_eq!(root, Path::new("/root"));
                assert_eq!(name, "name.md");
                assert_eq!(depth, 4);
                Some(PathBuf::from("/root/sub/name.md"))
            });
        assert_eq!(calls.get(), 1);
        assert_eq!(resolved, Some(PathBuf::from("/root/sub/name.md")));
    }

    #[test]
    fn does_not_search_for_multi_component_paths() {
        let resolved = resolve_link_path("a/b.md", Path::new("/root"), |_, _, _| {
            panic!("should not be called")
        });
        assert_eq!(resolved, None);
    }
}
