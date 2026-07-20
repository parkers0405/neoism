//! Bridge from `InstalledIndex` to Neoism's managed tool resolution.
//!
//! Language-server and formatter adapters begin with a portable command name
//! (for example `rust-analyzer` or `black`). When a user installs a tool
//! through Extensions, Neoism drops a launcher under `paths::bin_dir()` and
//! records it in `installed.json`. This module resolves those records before
//! falling back to `$PATH`.
//!
//! Keying by both package id and executable filename lets a runtime adapter
//! resolve packages whose registry id differs from its command. Entries
//! without a `bin_path` (server-side packages, scripts
//! installed without a managed binary) are skipped — they have nothing
//! to offer the resolver.
//!
//! Cross-name fallback matters for packages such as `json-lsp`, whose runtime
//! command is `vscode-json-language-server`; the filename key makes that
//! relationship data-driven instead of an adapter-specific alias.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::install_runner::InstallError;
use crate::installed::InstalledIndex;
use crate::manifest::ExtensionManifest;
use crate::paths;

static STARTUP_MANIFESTS: OnceLock<Vec<ExtensionManifest>> = OnceLock::new();

/// Build a `{ extension_id → absolute_bin_path }` map from the live
/// `installed.json` index. Entries without a `bin_path` are dropped
/// — they cannot satisfy a `cmd[1]` lookup. Empty result is valid
/// (fresh install, nothing managed yet).
pub fn managed_bin_map() -> Result<BTreeMap<String, String>, InstallError> {
    // Command resolution runs during LSP startup, before the Extensions page
    // necessarily opens. Reconcile old, fully-installed Mason packages here
    // so an orphaned managed binary becomes immediately usable. This never
    // consults PATH; the reconciler only accepts binaries rooted in Neoism's
    // own managed store.
    let manifests = STARTUP_MANIFESTS.get_or_init(|| {
        let Ok(registry) =
            crate::mason::load_mason_registry(&crate::mason::mason_cache_path())
        else {
            return Vec::new();
        };
        let manifests = crate::mason::translate_registry(&registry);
        if let Err(error) = crate::install_runner::reconcile_managed_installs(&manifests)
        {
            eprintln!("neoism: managed extension reconciliation failed: {error}");
        }
        manifests
    });
    managed_bin_map_from_manifests(&paths::installed_record_path(), manifests)
}

/// Path-explicit variant for tests. Keeps the production path
/// untouched while letting tests point at a temp `installed.json`.
pub fn managed_bin_map_from(
    path: &Path,
) -> Result<BTreeMap<String, String>, InstallError> {
    managed_bin_map_from_manifests(path, &[])
}

/// Build the managed command map with catalog metadata. The manifest's
/// executable list resolves every package-provided command and also repairs
/// legacy records whose primary path pointed at a sibling CLI rather than the
/// language-server executable. No package id is special-cased.
pub fn managed_bin_map_from_manifests(
    path: &Path,
    manifests: &[ExtensionManifest],
) -> Result<BTreeMap<String, String>, InstallError> {
    let index = InstalledIndex::load_from(path)?;
    let manifest_by_id: BTreeMap<&str, &ExtensionManifest> = manifests
        .iter()
        .map(|manifest| (manifest.id.as_str(), manifest))
        .collect();
    let mut resolved = Vec::new();
    for (id, entry) in &index.entries {
        let Some(recorded_bin) = entry.bin_path.as_ref() else {
            continue;
        };
        let manifest = manifest_by_id.get(id.as_str()).copied();
        let primary = manifest
            .and_then(|manifest| manifest.run.as_ref())
            .and_then(|run| run.command.first())
            .and_then(|command| sibling_bin(recorded_bin, command))
            .unwrap_or_else(|| recorded_bin.clone());
        resolved.push((id.as_str(), primary, manifest));
    }

    // Canonical package ids always win over aliases, independent of BTreeMap
    // ordering or a package whose executable happens to equal another id.
    let mut out = BTreeMap::new();
    for (id, primary, _) in &resolved {
        out.insert((*id).to_string(), primary.display().to_string());
    }
    for (_, primary, manifest) in resolved {
        let primary_string = primary.display().to_string();
        if let Some(file_name) = primary.file_name().and_then(|name| name.to_str()) {
            out.entry(file_name.to_string())
                .or_insert_with(|| primary_string.clone());
        }
        if let Some(manifest) = manifest {
            let mut names = manifest.executables.clone();
            if let Some(command) =
                manifest.run.as_ref().and_then(|run| run.command.first())
            {
                if !names.contains(command) {
                    names.push(command.clone());
                }
            }
            for name in names {
                if let Some(bin) = sibling_bin(&primary, &name) {
                    out.entry(name).or_insert_with(|| bin.display().to_string());
                }
            }
        }
    }
    Ok(out)
}

fn sibling_bin(bin: &Path, name: &str) -> Option<PathBuf> {
    if executable_name_matches(bin, name) && bin.exists() {
        return Some(bin.to_path_buf());
    }
    if let Some(candidate) = sibling_from_parent(bin, name) {
        return Some(candidate);
    }
    if let Ok(target) = std::fs::read_link(bin) {
        let resolved_target = if target.is_absolute() {
            target
        } else {
            bin.parent()?.join(target)
        };
        if let Some(candidate) = sibling_from_parent(&resolved_target, name) {
            return Some(candidate);
        }
    }
    let canonical = std::fs::canonicalize(bin).ok()?;
    sibling_from_parent(&canonical, name)
}

fn executable_name_matches(path: &Path, name: &str) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    file_name == name
        || file_name.strip_suffix(".exe") == Some(name)
        || file_name.strip_suffix(".cmd") == Some(name)
        || file_name.strip_suffix(".bat") == Some(name)
}

fn sibling_from_parent(bin: &Path, name: &str) -> Option<PathBuf> {
    let parent = bin.parent()?;
    std::iter::once(name.to_string())
        .chain(["exe", "cmd", "bat"].map(|suffix| format!("{name}.{suffix}")))
        .map(|name| parent.join(name))
        .find(|candidate| candidate.is_file())
}

/// JSON-encode `managed_bin_map()` for compatibility consumers. On any error
/// (missing index, bad JSON, IO), return `"{}"`; command resolution can still
/// fall back to `$PATH`.
pub fn managed_bin_map_json() -> String {
    managed_bin_map()
        .ok()
        .and_then(|m| serde_json::to_string(&m).ok())
        .unwrap_or_else(|| "{}".to_string())
}

/// Path-explicit JSON variant for tests.
pub fn managed_bin_map_json_from(path: &Path) -> String {
    managed_bin_map_from(path)
        .ok()
        .and_then(|m| serde_json::to_string(&m).ok())
        .unwrap_or_else(|| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::{InstalledEntry, InstalledIndex};
    use crate::manifest::{InstallKind, RunSpec};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "neoism-extensions-managed-bin-{}-{}",
            std::process::id(),
            nanoid()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn nanoid() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{:x}", n)
    }

    fn entry(id: &str, bin: Option<&str>) -> InstalledEntry {
        InstalledEntry {
            id: id.to_string(),
            version: "1.0.0".to_string(),
            install_kind: "test".to_string(),
            bin_path: bin.map(PathBuf::from),
            installed_at: 0,
        }
    }

    fn npm_manifest(id: &str, primary: &str, executables: &[&str]) -> ExtensionManifest {
        ExtensionManifest {
            id: id.to_string(),
            name: id.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            author: String::new(),
            downloads: None,
            categories: vec!["LSP".to_string()],
            languages: Vec::new(),
            repository_url: None,
            homepage: None,
            executables: executables.iter().map(|name| (*name).to_string()).collect(),
            install: InstallKind::Npm {
                package: id.to_string(),
                version: "1.0.0".to_string(),
                extra_packages: Vec::new(),
            },
            run: Some(RunSpec {
                command: vec![primary.to_string()],
                env: BTreeMap::new(),
            }),
            env_keys: Vec::new(),
        }
    }

    #[test]
    fn empty_index_yields_empty_json_object() {
        let dir = tempdir();
        let path = dir.join("installed.json");
        // No file written — `load_from` returns default.
        let json = managed_bin_map_json_from(&path);
        assert_eq!(json, "{}");
    }

    #[test]
    fn map_includes_entries_with_bin_paths() {
        let dir = tempdir();
        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry(
            "rust-analyzer",
            Some("/opt/neoism/bin/rust-analyzer"),
        ));
        idx.install_record(entry("pyright", Some("/opt/neoism/bin/pyright-langserver")));
        idx.save_to(&path).unwrap();

        let map = managed_bin_map_from(&path).unwrap();
        // 2 id-keyed entries + 1 bin-filename alias for pyright's
        // `pyright-langserver` (rust-analyzer's bin filename equals its
        // id, so its alias is a no-op).
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get("rust-analyzer").map(String::as_str),
            Some("/opt/neoism/bin/rust-analyzer")
        );
        assert_eq!(
            map.get("pyright").map(String::as_str),
            Some("/opt/neoism/bin/pyright-langserver")
        );
        assert_eq!(
            map.get("pyright-langserver").map(String::as_str),
            Some("/opt/neoism/bin/pyright-langserver")
        );
    }

    #[test]
    fn entries_without_bin_path_are_skipped() {
        let dir = tempdir();
        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry("scripts-only", None));
        idx.install_record(entry("real", Some("/x/y/real")));
        idx.save_to(&path).unwrap();

        let map = managed_bin_map_from(&path).unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("real"));
        assert!(!map.contains_key("scripts-only"));
    }

    #[test]
    fn typescript_language_server_id_resolves_via_cmd1_fallback() {
        // lua's `ts_ls` server uses cmd[1] = "typescript-language-server",
        // which matches the Mason install id. The id-keyed lookup is the
        // hit path here; the bin-filename alias matches the same key, so
        // the map carries exactly one entry under that key.
        let dir = tempdir();
        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry(
            "typescript-language-server",
            Some("/opt/neoism/bin/typescript-language-server"),
        ));
        idx.save_to(&path).unwrap();

        let map = managed_bin_map_from(&path).unwrap();
        // Lua resolves `managed_bin[server.name="ts_ls"]` → miss, then
        // `managed_bin[server.cmd[1]="typescript-language-server"]` → hit.
        assert_eq!(
            map.get("typescript-language-server").map(String::as_str),
            Some("/opt/neoism/bin/typescript-language-server")
        );
    }

    #[test]
    fn json_lsp_id_resolves_via_bin_filename_alias() {
        // Mason install id = "json-lsp"; lua's `jsonls` server uses
        // cmd[1] = "vscode-json-language-server". Neither matches the id
        // directly, so the bin-filename alias is the only path that
        // resolves the managed binary.
        let dir = tempdir();
        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry(
            "json-lsp",
            Some("/opt/neoism/bin/vscode-json-language-server"),
        ));
        idx.save_to(&path).unwrap();

        let map = managed_bin_map_from(&path).unwrap();
        assert_eq!(
            map.get("json-lsp").map(String::as_str),
            Some("/opt/neoism/bin/vscode-json-language-server")
        );
        assert_eq!(
            map.get("vscode-json-language-server").map(String::as_str),
            Some("/opt/neoism/bin/vscode-json-language-server")
        );
    }

    #[test]
    fn catalog_primary_repairs_a_legacy_record_and_maps_every_executable() {
        let dir = tempdir();
        let npm_bin = dir.join("node_modules").join(".bin");
        std::fs::create_dir_all(&npm_bin).unwrap();
        let pyright = npm_bin.join("pyright");
        let pyright_langserver = npm_bin.join("pyright-langserver");
        std::fs::write(&pyright, b"#!/usr/bin/env node\n").unwrap();
        std::fs::write(&pyright_langserver, b"#!/usr/bin/env node\n").unwrap();

        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry("pyright", Some(pyright.to_str().unwrap())));
        idx.save_to(&path).unwrap();

        let manifest = npm_manifest(
            "pyright",
            "pyright-langserver",
            &["pyright", "pyright-langserver"],
        );
        let map = managed_bin_map_from_manifests(&path, &[manifest]).unwrap();
        let expected = pyright_langserver.display().to_string();
        assert_eq!(map.get("pyright"), Some(&expected));
        assert_eq!(map.get("pyright-langserver"), Some(&expected));
        assert_eq!(
            map.get("pyright").map(String::as_str),
            Some(expected.as_str()),
            "package id resolves the manifest-selected primary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn catalog_primary_is_found_next_to_a_legacy_symlink_target() {
        let dir = tempdir();
        let public_bin = dir.join("extensions").join("bin");
        let npm_bin = dir
            .join("extensions")
            .join("installed")
            .join("pyright")
            .join("node_modules")
            .join(".bin");
        std::fs::create_dir_all(&public_bin).unwrap();
        std::fs::create_dir_all(&npm_bin).unwrap();
        let pyright = npm_bin.join("pyright");
        let pyright_langserver = npm_bin.join("pyright-langserver");
        std::fs::write(&pyright, b"#!/usr/bin/env node\n").unwrap();
        std::fs::write(&pyright_langserver, b"#!/usr/bin/env node\n").unwrap();
        let wrapped_pyright = public_bin.join("pyright");
        std::os::unix::fs::symlink(&pyright, &wrapped_pyright).unwrap();

        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry("pyright", Some(wrapped_pyright.to_str().unwrap())));
        idx.save_to(&path).unwrap();

        let manifest = npm_manifest(
            "pyright",
            "pyright-langserver",
            &["pyright", "pyright-langserver"],
        );
        let map = managed_bin_map_from_manifests(&path, &[manifest]).unwrap();
        let expected = pyright_langserver.display().to_string();
        assert_eq!(map.get("pyright"), Some(&expected));
        assert_eq!(map.get("pyright-langserver"), Some(&expected));
    }

    #[test]
    fn bin_filename_alias_does_not_overwrite_existing_id_entry() {
        // Name collision: one entry's `id` equals another entry's bin
        // filename. The id-keyed entry must win — the alias is
        // best-effort and uses `or_insert`, so it never clobbers a real
        // id. The id key is also inserted last via `.insert()`, so
        // even if the alias landed first it will be overwritten by the
        // canonical id-keyed entry on the corresponding iteration.
        let dir = tempdir();
        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        // BTreeMap iter is alphabetic: `aaa-other` is visited before
        // `foo`, so the alias `foo` lands first; the canonical `foo`
        // id-keyed entry then overwrites it.
        idx.install_record(entry("aaa-other", Some("/alt/foo")));
        idx.install_record(entry("foo", Some("/canonical/foo")));
        idx.save_to(&path).unwrap();

        let map = managed_bin_map_from(&path).unwrap();
        assert_eq!(map.get("foo").map(String::as_str), Some("/canonical/foo"));
        assert_eq!(map.get("aaa-other").map(String::as_str), Some("/alt/foo"));
    }

    #[test]
    fn json_round_trips_into_a_map() {
        let dir = tempdir();
        let path = dir.join("installed.json");
        let mut idx = InstalledIndex::default();
        idx.install_record(entry("a", Some("/bin/a")));
        idx.install_record(entry("b", Some("/bin/b")));
        idx.save_to(&path).unwrap();

        let json = managed_bin_map_json_from(&path);
        let decoded: BTreeMap<String, String> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.get("a").map(String::as_str), Some("/bin/a"));
        assert_eq!(decoded.get("b").map(String::as_str), Some("/bin/b"));
    }
}
