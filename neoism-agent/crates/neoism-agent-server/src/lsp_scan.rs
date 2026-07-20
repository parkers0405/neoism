use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use serde_json::Value;

use super::lsp_adapters::{
    adapters_for_root, AdapterOrigin, LanguageAdapter, ResolvedLspTransport,
    WorkspaceRootStrategy,
};
use super::lsp_languages::{LspOperation, WorkspaceScan};
use super::lsp_uri::path_to_file_uri;
use super::{
    LspCapabilities, LspCommandSource, LspDetection, LspServerState, LspStatus,
    LspWorkspace, IGNORED_DIRS, MAX_EVIDENCE, MAX_LSP_SERVERS_PER_QUERY, MAX_SCAN_FILES,
};
use crate::managed_lsp_path::managed_lsp_path_entries;

const MAX_CARGO_ROOT_CACHE_ENTRIES: usize = 64;
const CARGO_ROOT_NEGATIVE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CargoRootCacheKey {
    manifest: PathBuf,
    modified: SystemTime,
}

#[derive(Default)]
struct CargoRootCache {
    entries: HashMap<CargoRootCacheKey, Arc<OnceLock<CargoRootResolution>>>,
    insertion_order: VecDeque<CargoRootCacheKey>,
}

#[derive(Clone, Debug)]
struct CargoRootResolution {
    root: Option<PathBuf>,
    resolved_at: Instant,
}

impl CargoRootCache {
    fn cell_for(&mut self, key: CargoRootCacheKey) -> Arc<OnceLock<CargoRootResolution>> {
        if let Some(cached) = self.entries.get(&key) {
            let negative_expired = cached.get().is_some_and(|resolution| {
                resolution.root.is_none()
                    && resolution.resolved_at.elapsed() >= CARGO_ROOT_NEGATIVE_TTL
            });
            if !negative_expired {
                return Arc::clone(cached);
            }
        }

        // A changed mtime supersedes the old result immediately. This keeps a
        // frequently edited manifest from consuming the whole bounded cache.
        self.entries
            .retain(|cached, _| cached.manifest != key.manifest);
        self.insertion_order
            .retain(|cached| cached.manifest != key.manifest);
        while self.entries.len() >= MAX_CARGO_ROOT_CACHE_ENTRIES {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }

        let cell = Arc::new(OnceLock::new());
        self.entries.insert(key.clone(), Arc::clone(&cell));
        self.insertion_order.push_back(key);
        cell
    }
}

static CARGO_ROOT_CACHE: OnceLock<Mutex<CargoRootCache>> = OnceLock::new();

pub(super) fn detected_servers(
    root: &Path,
    scan: &WorkspaceScan,
    adapters: &[LanguageAdapter],
) -> Vec<LspStatus> {
    adapters
        .iter()
        .filter(|adapter| {
            language_detected(adapter, scan)
                || adapter.origin == AdapterOrigin::Configured
        })
        .map(|adapter| server_status(root, scan, adapter))
        .collect()
}

pub(super) fn normalized_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| {
        if root.is_absolute() {
            root.to_path_buf()
        } else {
            env::current_dir()
                .map(|directory| directory.join(root))
                .unwrap_or_else(|_| root.to_path_buf())
        }
    })
}

pub(super) fn normalized_file(root: &Path, file: &Path) -> PathBuf {
    let candidate = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    fs::canonicalize(&candidate).unwrap_or(candidate)
}

/// Resolve the workspace folder used to launch a file-scoped language
/// server. A Neoism workspace may contain nested projects (for example a
/// standalone Cargo crate under `fixtures/`). Starting the server only at the
/// top-level workspace leaves those files outside its project graph: syntax
/// hover can still work while semantic/compiler diagnostics stay empty.
///
/// Root selection is adapter metadata. Most adapters select their nearest
/// marker. Cargo-backed adapters first ask Cargo for the workspace containing
/// the nearest manifest, so sibling members share one rust-analyzer process.
pub(super) fn server_root_for_file(
    workspace_root: &Path,
    file: &Path,
    adapter: &LanguageAdapter,
) -> PathBuf {
    let nearest = nearest_marker_root(workspace_root, file, &adapter.markers);
    match &adapter.root_strategy {
        WorkspaceRootStrategy::NearestMarker => nearest,
        WorkspaceRootStrategy::CargoMetadata { manifest } => {
            nearest_manifest(workspace_root, file, manifest)
                .and_then(|manifest| cached_cargo_workspace_root(&manifest))
                .unwrap_or(nearest)
        }
    }
}

fn nearest_marker_root(
    workspace_root: &Path,
    file: &Path,
    markers: &[String],
) -> PathBuf {
    if markers.is_empty() {
        return workspace_root.to_path_buf();
    }
    for directory in bounded_ancestors(workspace_root, file) {
        if markers
            .iter()
            .any(|marker| directory_has_marker(directory, marker))
        {
            return fs::canonicalize(directory)
                .unwrap_or_else(|_| directory.to_path_buf());
        }
    }
    workspace_root.to_path_buf()
}

fn nearest_manifest(
    workspace_root: &Path,
    file: &Path,
    manifest: &str,
) -> Option<PathBuf> {
    bounded_ancestors(workspace_root, file)
        .map(|directory| directory.join(manifest))
        .find(|manifest| manifest.is_file())
}

fn bounded_ancestors<'a>(
    workspace_root: &'a Path,
    file: &'a Path,
) -> impl Iterator<Item = &'a Path> {
    let start = if file.is_dir() {
        file
    } else {
        file.parent().unwrap_or(workspace_root)
    };
    let bounded = file.starts_with(workspace_root);
    start
        .ancestors()
        .take_while(move |directory| !bounded || directory.starts_with(workspace_root))
}

fn cached_cargo_workspace_root(manifest: &Path) -> Option<PathBuf> {
    let cache = CARGO_ROOT_CACHE.get_or_init(|| Mutex::new(CargoRootCache::default()));
    cached_cargo_workspace_root_with(cache, manifest, cargo_workspace_root_uncached)
}

fn cached_cargo_workspace_root_with<F>(
    cache: &Mutex<CargoRootCache>,
    manifest: &Path,
    resolve: F,
) -> Option<PathBuf>
where
    F: FnOnce(&Path) -> Option<PathBuf>,
{
    let manifest = fs::canonicalize(manifest).ok()?;
    let modified = fs::metadata(&manifest).ok()?.modified().ok()?;
    let key = CargoRootCacheKey { manifest, modified };
    let cell = {
        let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
        cache.cell_for(key.clone())
    };
    cell.get_or_init(|| CargoRootResolution {
        root: resolve(&key.manifest),
        resolved_at: Instant::now(),
    })
    .root
    .clone()
}

fn cargo_workspace_root_uncached(manifest: &Path) -> Option<PathBuf> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).ok()?;
    let workspace_root = metadata.get("workspace_root")?.as_str()?;
    fs::canonicalize(workspace_root).ok()
}

fn directory_has_marker(directory: &Path, marker: &str) -> bool {
    if directory.join(marker).exists() {
        return true;
    }
    // Registry entries such as `.sln` and `.csproj` intentionally mean a
    // filename suffix, while an exact hidden marker (for example `.git`) was
    // already handled above.
    marker.starts_with('.')
        && fs::read_dir(directory).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(marker))
            })
        })
}

pub(super) fn file_query_specs(
    root: &Path,
    file: &Path,
    operation: LspOperation,
) -> Vec<LanguageAdapter> {
    let adapters = adapters_for_root(root);
    let strongest_match = adapters
        .iter()
        .filter(|adapter| operation_supported(adapter, operation))
        .filter_map(|adapter| adapter.match_priority(file))
        .max();
    adapters
        .into_iter()
        .filter(|adapter| operation_supported(adapter, operation))
        // The route is selected before endpoint availability: an invalid exact
        // Compose route must not silently fall through to generic YAML.
        .filter(|adapter| {
            strongest_match
                .is_some_and(|score| adapter.match_priority(file) == Some(score))
        })
        .filter(|adapter| adapter.is_valid() && adapter_endpoint_available(adapter))
        .take(MAX_LSP_SERVERS_PER_QUERY)
        .collect()
}

/// Adapters that own a document lifecycle, independent of any individual
/// feature flag. didOpen/didChange/didSave are shared prerequisites for hover,
/// completion, rename, diagnostics, and every other textDocument feature; a
/// completion-only server must not receive stale disk text merely because it
/// does not advertise diagnostics.
pub(super) fn file_lifecycle_specs(root: &Path, file: &Path) -> Vec<LanguageAdapter> {
    let adapters = adapters_for_root(root);
    let strongest_match = adapters
        .iter()
        .filter_map(|adapter| adapter.match_priority(file))
        .max();
    adapters
        .into_iter()
        // Select the strongest route before endpoint availability so an exact
        // unavailable route never leaks a document into a generic fallback.
        .filter(|adapter| {
            strongest_match
                .is_some_and(|score| adapter.match_priority(file) == Some(score))
        })
        .filter(|adapter| adapter.is_valid() && adapter_endpoint_available(adapter))
        .take(MAX_LSP_SERVERS_PER_QUERY)
        .collect()
}

pub(super) fn operation_supported(
    adapter: &LanguageAdapter,
    operation: LspOperation,
) -> bool {
    match operation {
        LspOperation::WorkspaceSymbols => adapter.workspace_symbols,
        LspOperation::Completion => adapter.completion,
        LspOperation::Hover => adapter.hover,
        LspOperation::Definition => adapter.definition,
        LspOperation::References => adapter.references,
        LspOperation::Implementation => adapter.implementation,
        LspOperation::CallHierarchy => adapter.call_hierarchy,
        LspOperation::Diagnostics => adapter.diagnostics,
        LspOperation::DocumentSymbols => adapter.document_symbols,
        LspOperation::Formatting => adapter.formatting,
        LspOperation::CodeActions => adapter.code_actions,
        LspOperation::Rename => adapter.rename,
    }
}

pub(super) fn adapter_endpoint_available(adapter: &LanguageAdapter) -> bool {
    match &adapter.transport {
        ResolvedLspTransport::Stdio { command, .. } => {
            crate::lsp::resolve_lsp_command(&adapter.id, command.clone()).1
                != LspCommandSource::Missing
        }
        ResolvedLspTransport::Tcp { .. } => true,
        ResolvedLspTransport::Invalid => false,
    }
}

pub(super) fn server_status(
    root: &Path,
    scan: &WorkspaceScan,
    adapter: &LanguageAdapter,
) -> LspStatus {
    server_status_at(root, root, None, scan, adapter)
}

pub(super) fn server_status_for_file(
    workspace_root: &Path,
    file: &Path,
    scan: &WorkspaceScan,
    adapter: &LanguageAdapter,
) -> LspStatus {
    let project_root = server_root_for_file(workspace_root, file, adapter);
    server_status_at(workspace_root, &project_root, Some(file), scan, adapter)
}

fn server_status_at(
    workspace_root: &Path,
    project_root: &Path,
    file: Option<&Path>,
    scan: &WorkspaceScan,
    adapter: &LanguageAdapter,
) -> LspStatus {
    let (command, command_source, endpoint_available) = match &adapter.transport {
        ResolvedLspTransport::Stdio { command, .. } => {
            let (command, source) =
                crate::lsp::resolve_lsp_command(&adapter.id, command.clone());
            let available = source != LspCommandSource::Missing;
            (command, source, available)
        }
        ResolvedLspTransport::Tcp {
            host,
            port,
            built_in,
        } => (
            vec![format!("tcp://{host}:{port}")],
            if *built_in {
                LspCommandSource::BuiltIn
            } else {
                LspCommandSource::Config
            },
            true,
        ),
        ResolvedLspTransport::Invalid => (Vec::new(), LspCommandSource::Missing, false),
    };
    let usable = adapter.is_valid() && endpoint_available;
    let broken_reason = usable
        .then(|| match file {
            Some(file) => super::lsp_service::service().broken_reason_for_file(
                workspace_root,
                file,
                adapter,
            ),
            None => super::lsp_service::service().broken_reason(workspace_root, adapter),
        })
        .flatten();
    let message = adapter.configuration_error.clone().or(broken_reason).or_else(|| {
        (!endpoint_available).then(|| {
            let endpoint = command
                .first()
                .cloned()
                .unwrap_or_else(|| "missing endpoint".to_string());
            format!(
                "{} was detected, but `{endpoint}` was not found in Neoism extensions or PATH",
                adapter.name
            )
        })
    });
    let mut detected_extensions = matching_extensions(adapter, scan);
    if adapter.origin == AdapterOrigin::Configured {
        for extension in adapter.extensions() {
            detected_extensions
                .entry(extension.to_string())
                .or_insert_with(|| scan.extensions.get(extension).copied().unwrap_or(0));
        }
    }
    let routed_languages = adapter
        .routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<BTreeSet<_>>();
    let language = if routed_languages.len() == 1 {
        routed_languages
            .iter()
            .next()
            .copied()
            .unwrap_or(adapter.id.as_str())
    } else {
        adapter.id.as_str()
    };
    LspStatus {
        id: adapter.id.clone(),
        name: adapter.name.clone(),
        status: if usable && message.is_none() {
            if super::lsp_service::service().client_connected_at(
                workspace_root,
                project_root,
                adapter,
            ) {
                LspServerState::Connected
            } else {
                LspServerState::Available
            }
        } else {
            LspServerState::Error
        },
        language: language.to_string(),
        command,
        command_source,
        workspace: LspWorkspace {
            root: project_root.display().to_string(),
            root_uri: path_to_file_uri(project_root),
        },
        capabilities: LspCapabilities {
            workspace_symbols: adapter.workspace_symbols && usable,
            completion: adapter.completion && usable,
            hover: adapter.hover && usable,
            definition: adapter.definition && usable,
            references: adapter.references && usable,
            implementation: adapter.implementation && usable,
            call_hierarchy: adapter.call_hierarchy && usable,
            diagnostics: adapter.diagnostics && usable,
            document_symbols: adapter.document_symbols && usable,
            formatting: adapter.formatting && usable,
            code_actions: adapter.code_actions && usable,
            rename: adapter.rename && usable,
        },
        detected: LspDetection {
            files: scan.files,
            markers: matching_markers(adapter, scan),
            extensions: detected_extensions,
            command_available: usable,
            message,
        },
    }
}

pub(super) fn language_detected(adapter: &LanguageAdapter, scan: &WorkspaceScan) -> bool {
    adapter.markers.iter().any(|marker| {
        scan.markers.contains(marker) || extension_marker_matches(marker, &scan.markers)
    }) || scan
        .markers
        .iter()
        .any(|file_name| adapter.filename_matches(file_name))
        || adapter
            .extensions()
            .any(|extension| scan.extensions.contains_key(extension))
}

fn matching_markers(adapter: &LanguageAdapter, scan: &WorkspaceScan) -> Vec<String> {
    let mut matches = adapter
        .markers
        .iter()
        .filter(|marker| {
            scan.markers.contains(*marker)
                || extension_marker_matches(marker, &scan.markers)
        })
        .take(MAX_EVIDENCE)
        .cloned()
        .collect::<Vec<_>>();
    for file_name in scan
        .markers
        .iter()
        .filter(|file_name| adapter.filename_matches(file_name))
    {
        if matches.len() >= MAX_EVIDENCE {
            break;
        }
        if !matches.contains(file_name) {
            matches.push(file_name.clone());
        }
    }
    matches
}

fn matching_extensions(
    adapter: &LanguageAdapter,
    scan: &WorkspaceScan,
) -> BTreeMap<String, usize> {
    adapter
        .extensions()
        .filter_map(|extension| {
            scan.extensions
                .get(extension)
                .map(|count| (extension.to_string(), *count))
        })
        .take(MAX_EVIDENCE)
        .collect()
}

fn extension_marker_matches(marker: &str, markers: &BTreeSet<String>) -> bool {
    marker.starts_with('.') && markers.iter().any(|found| found.ends_with(marker))
}

pub(super) fn scan_workspace(root: &Path, adapters: &[LanguageAdapter]) -> WorkspaceScan {
    let mut scan = WorkspaceScan::default();
    scan_dir(root, root, adapters, &mut scan);
    scan
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    adapters: &[LanguageAdapter],
    scan: &mut WorkspaceScan,
) {
    if scan.files >= MAX_SCAN_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if scan.files >= MAX_SCAN_FILES {
            break;
        }
        let path = entry.path();
        let file_name = entry.file_name();
        if is_ignored_dir_name(&file_name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            scan_dir(root, &path, adapters, scan);
        } else if file_type.is_file() {
            scan.files += 1;
            observe_file(root, &path, adapters, scan);
        }
    }
}

fn observe_file(
    root: &Path,
    path: &Path,
    adapters: &[LanguageAdapter],
    scan: &mut WorkspaceScan,
) {
    if let Some(extension) = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
    {
        *scan.extensions.entry(extension).or_default() += 1;
    }
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return;
    };
    for adapter in adapters {
        for marker in &adapter.markers {
            if marker == file_name || path_matches_marker(root, path, marker) {
                scan.markers.insert(marker_for_path(root, path, marker));
            }
        }
        if adapter.filename_matches(file_name) {
            scan.markers.insert(file_name.to_string());
        }
    }
}

fn path_matches_marker(root: &Path, path: &Path, marker: &str) -> bool {
    marker.starts_with('.')
        && path
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .is_some_and(|relative| relative.ends_with(marker))
}

fn marker_for_path(root: &Path, path: &Path, marker: &str) -> String {
    if marker.starts_with('.') {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    } else {
        marker.to_string()
    }
}

fn is_ignored_dir_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| IGNORED_DIRS.iter().any(|ignored| ignored == &name))
}

pub(super) fn command_available(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return executable_exists(path.to_path_buf());
    }
    let managed = managed_lsp_path_entries();
    let env_paths = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    managed
        .into_iter()
        .chain(env_paths)
        .any(|directory| executable_exists(directory.join(command)))
}

fn executable_exists(path: PathBuf) -> bool {
    if is_executable_file(&path) {
        return true;
    }
    #[cfg(windows)]
    {
        for extension in ["exe", "cmd", "bat"] {
            if is_executable_file(&path.with_extension(extension)) {
                return true;
            }
        }
    }
    false
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return metadata.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempWorkspace {
        path: PathBuf,
    }

    impl TempWorkspace {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "neoism-lsp-root-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("create root fixture");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create fixture parent");
            }
            fs::write(&path, contents).expect("write fixture");
            path
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn rust_adapter() -> LanguageAdapter {
        let spec = crate::lsp::lsp_languages::LANGUAGE_SPECS
            .iter()
            .find(|spec| spec.id == "rust")
            .expect("built-in Rust adapter");
        LanguageAdapter::from_builtin(spec)
    }

    #[test]
    fn cargo_metadata_maps_sibling_members_to_the_outer_workspace() {
        let workspace = TempWorkspace::new("siblings");
        workspace.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"member-a\", \"member-b\"]\nresolver = \"2\"\n",
        );
        workspace.write(
            "member-a/Cargo.toml",
            "[package]\nname = \"member-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        let first = workspace.write("member-a/src/lib.rs", "pub fn first() {}\n");
        workspace.write(
            "member-b/Cargo.toml",
            "[package]\nname = \"member-b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        let second = workspace.write("member-b/src/lib.rs", "pub fn second() {}\n");
        let expected = fs::canonicalize(&workspace.path).expect("canonical root");
        let adapter = rust_adapter();

        assert_eq!(
            server_root_for_file(&workspace.path, &first, &adapter),
            expected
        );
        assert_eq!(
            server_root_for_file(&workspace.path, &second, &adapter),
            expected
        );
    }

    #[test]
    fn nested_cargo_workspace_manifest_resolves_to_itself() {
        let workspace = TempWorkspace::new("nested");
        workspace.write(
            "nested/Cargo.toml",
            "[package]\nname = \"nested-root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nresolver = \"2\"\n",
        );
        let file = workspace.write("nested/src/lib.rs", "pub fn nested() {}\n");
        let expected = fs::canonicalize(workspace.path.join("nested"))
            .expect("canonical nested root");

        assert_eq!(
            server_root_for_file(&workspace.path, &file, &rust_adapter()),
            expected
        );
    }

    #[test]
    fn cargo_metadata_failure_falls_back_to_the_nearest_marker() {
        let workspace = TempWorkspace::new("invalid");
        workspace.write("nested/Cargo.toml", "this is not valid TOML\n");
        let file = workspace.write("nested/src/lib.rs", "pub fn nested() {}\n");
        let expected = fs::canonicalize(workspace.path.join("nested"))
            .expect("canonical nested root");

        assert_eq!(
            server_root_for_file(&workspace.path, &file, &rust_adapter()),
            expected
        );
    }

    #[test]
    fn cargo_metadata_cache_reuses_successes_and_failures() {
        let workspace = TempWorkspace::new("cache");
        let manifest = workspace.write("Cargo.toml", "[workspace]\n");
        let expected = fs::canonicalize(&workspace.path).expect("canonical root");
        let success_cache = Mutex::new(CargoRootCache::default());
        let mut success_calls = 0;

        let first = cached_cargo_workspace_root_with(&success_cache, &manifest, |_| {
            success_calls += 1;
            Some(expected.clone())
        });
        let second = cached_cargo_workspace_root_with(&success_cache, &manifest, |_| {
            success_calls += 1;
            None
        });
        assert_eq!(first, Some(expected.clone()));
        assert_eq!(second, Some(expected));
        assert_eq!(success_calls, 1);

        let failure_cache = Mutex::new(CargoRootCache::default());
        let mut failure_calls = 0;
        assert_eq!(
            cached_cargo_workspace_root_with(&failure_cache, &manifest, |_| {
                failure_calls += 1;
                None
            }),
            None
        );
        assert_eq!(
            cached_cargo_workspace_root_with(&failure_cache, &manifest, |_| {
                failure_calls += 1;
                Some(workspace.path.clone())
            }),
            None
        );
        assert_eq!(failure_calls, 1);
    }

    #[test]
    fn cargo_metadata_cache_retries_an_expired_failure() {
        let workspace = TempWorkspace::new("expired-failure");
        let manifest = workspace.write("Cargo.toml", "[workspace]\n");
        let canonical_manifest = fs::canonicalize(&manifest).expect("canonical manifest");
        let modified = fs::metadata(&canonical_manifest)
            .expect("manifest metadata")
            .modified()
            .expect("manifest mtime");
        let key = CargoRootCacheKey {
            manifest: canonical_manifest,
            modified,
        };
        let cell = Arc::new(OnceLock::new());
        cell.set(CargoRootResolution {
            root: None,
            resolved_at: Instant::now()
                .checked_sub(CARGO_ROOT_NEGATIVE_TTL + Duration::from_millis(1))
                .expect("past instant"),
        })
        .expect("seed expired failure");
        let cache = Mutex::new(CargoRootCache {
            entries: HashMap::from([(key.clone(), cell)]),
            insertion_order: VecDeque::from([key]),
        });
        let expected = fs::canonicalize(&workspace.path).expect("canonical root");
        let mut calls = 0;

        assert_eq!(
            cached_cargo_workspace_root_with(&cache, &manifest, |_| {
                calls += 1;
                Some(expected.clone())
            }),
            Some(expected)
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn cargo_metadata_cache_is_bounded() {
        let mut cache = CargoRootCache::default();
        for index in 0..(MAX_CARGO_ROOT_CACHE_ENTRIES + 5) {
            cache.cell_for(CargoRootCacheKey {
                manifest: PathBuf::from(format!("/fixture/{index}/Cargo.toml")),
                modified: std::time::UNIX_EPOCH,
            });
        }
        assert_eq!(cache.entries.len(), MAX_CARGO_ROOT_CACHE_ENTRIES);
        assert_eq!(cache.insertion_order.len(), MAX_CARGO_ROOT_CACHE_ENTRIES);
    }

    #[cfg(unix)]
    #[test]
    fn command_availability_requires_an_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "neoism-lsp-command-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create command fixture");
        let command = dir.join("fixture-language-server");
        fs::write(&command, b"#!/bin/sh\nexit 0\n").expect("write command fixture");

        let mut permissions = fs::metadata(&command)
            .expect("command metadata")
            .permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&command, permissions).expect("make non-executable");
        assert!(!command_available(command.to_str().expect("utf-8 path")));

        let mut permissions = fs::metadata(&command)
            .expect("command metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).expect("make executable");
        assert!(command_available(command.to_str().expect("utf-8 path")));

        fs::remove_dir_all(dir).expect("remove command fixture");
    }
}
