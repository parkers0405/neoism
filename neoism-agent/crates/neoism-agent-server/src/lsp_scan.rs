use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use super::lsp_languages::{LanguageSpec, LspOperation, WorkspaceScan, LANGUAGE_SPECS};
use super::lsp_uri::path_to_file_uri;
use super::{
    LspCapabilities, LspCommandSource, LspDetection, LspServerState, LspStatus,
    LspWorkspace, IGNORED_DIRS, MAX_EVIDENCE, MAX_LSP_SERVERS_PER_QUERY, MAX_SCAN_FILES,
};
use crate::managed_lsp_path::managed_lsp_path_entries;

pub(super) fn detected_servers(root: &Path, scan: &WorkspaceScan) -> Vec<LspStatus> {
    LANGUAGE_SPECS
        .iter()
        .filter(|spec| language_detected(spec, scan))
        .map(|spec| server_status(root, scan, spec))
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

pub(super) fn file_query_specs<'a>(
    scan: &'a WorkspaceScan,
    file: &Path,
    operation: LspOperation,
) -> impl Iterator<Item = &'a LanguageSpec> + 'a {
    let extension = file
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    let has_extension_match = extension.as_deref().is_some_and(|extension| {
        LANGUAGE_SPECS
            .iter()
            .any(|spec| spec.extensions.contains(&extension))
    });
    LANGUAGE_SPECS
        .iter()
        .filter(move |spec| operation_supported(spec, operation))
        .filter(move |spec| {
            if let Some(extension) = extension.as_deref() {
                if has_extension_match {
                    return spec.extensions.contains(&extension);
                }
            }
            language_detected(spec, scan)
        })
        .filter(|spec| crate::lsp::lsp_command_available(spec.id, spec.command))
        .take(MAX_LSP_SERVERS_PER_QUERY)
}

fn operation_supported(spec: &LanguageSpec, operation: LspOperation) -> bool {
    match operation {
        LspOperation::WorkspaceSymbols => spec.workspace_symbols,
        LspOperation::Hover => spec.hover,
        LspOperation::Definition => spec.definition,
        LspOperation::References => spec.references,
        LspOperation::Implementation => spec.implementation,
        LspOperation::CallHierarchy => spec.call_hierarchy,
        LspOperation::Diagnostics => spec.diagnostics,
        LspOperation::DocumentSymbols => spec.document_symbols,
        LspOperation::Rename => spec.rename,
        LspOperation::Completion
        | LspOperation::Formatting
        | LspOperation::CodeActions => true,
    }
}

pub(super) fn server_status(
    root: &Path,
    scan: &WorkspaceScan,
    spec: &LanguageSpec,
) -> LspStatus {
    let raw_command: Vec<String> = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect();
    let (command, command_source) = crate::lsp::resolve_lsp_command(spec.id, raw_command);
    let command_available = command_source != LspCommandSource::Missing;
    LspStatus {
        id: spec.id.to_string(),
        name: spec.name.to_string(),
        status: if command_available {
            LspServerState::Available
        } else {
            LspServerState::Error
        },
        language: spec.id.to_string(),
        command,
        command_source,
        workspace: LspWorkspace {
            root: root.display().to_string(),
            root_uri: path_to_file_uri(root),
        },
        capabilities: LspCapabilities {
            workspace_symbols: spec.workspace_symbols && command_available,
            hover: spec.hover && command_available,
            definition: spec.definition && command_available,
            references: spec.references && command_available,
            implementation: spec.implementation && command_available,
            call_hierarchy: spec.call_hierarchy && command_available,
            diagnostics: spec.diagnostics && command_available,
            document_symbols: spec.document_symbols && command_available,
            formatting: command_available,
            code_actions: command_available,
        },
        detected: LspDetection {
            files: scan.files,
            markers: matching_markers(spec, scan),
            extensions: matching_extensions(spec, scan),
            command_available,
            message: (!command_available).then(|| {
                format!(
                    "{} was detected, but `{}` was not found in Neoism extensions or PATH",
                    spec.name, spec.command[0]
                )
            }),
        },
    }
}

pub(super) fn language_detected(spec: &LanguageSpec, scan: &WorkspaceScan) -> bool {
    spec.markers.iter().any(|marker| {
        scan.markers.contains(*marker) || extension_marker_matches(marker, &scan.markers)
    }) || spec
        .extensions
        .iter()
        .any(|extension| scan.extensions.contains_key(*extension))
}

fn matching_markers(spec: &LanguageSpec, scan: &WorkspaceScan) -> Vec<String> {
    spec.markers
        .iter()
        .filter(|marker| {
            scan.markers.contains(**marker)
                || extension_marker_matches(marker, &scan.markers)
        })
        .take(MAX_EVIDENCE)
        .map(|marker| (*marker).to_string())
        .collect()
}

fn matching_extensions(
    spec: &LanguageSpec,
    scan: &WorkspaceScan,
) -> BTreeMap<String, usize> {
    spec.extensions
        .iter()
        .filter_map(|extension| {
            scan.extensions
                .get(*extension)
                .map(|count| ((*extension).to_string(), *count))
        })
        .take(MAX_EVIDENCE)
        .collect()
}

fn extension_marker_matches(marker: &str, markers: &BTreeSet<String>) -> bool {
    marker.starts_with('.') && markers.iter().any(|found| found.ends_with(marker))
}

pub(super) fn scan_workspace(root: &Path) -> WorkspaceScan {
    let mut scan = WorkspaceScan::default();
    scan_dir(root, root, &mut scan);
    scan
}

fn scan_dir(root: &Path, dir: &Path, scan: &mut WorkspaceScan) {
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
            scan_dir(root, &path, scan);
        } else if file_type.is_file() {
            scan.files += 1;
            observe_file(root, &path, scan);
        }
    }
}

fn observe_file(root: &Path, path: &Path, scan: &mut WorkspaceScan) {
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

    for spec in LANGUAGE_SPECS {
        for marker in spec.markers {
            if marker == &file_name || path_matches_marker(root, path, marker) {
                scan.markers.insert(marker_for_path(root, path, marker));
            }
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
    if path.is_file() {
        return true;
    }

    #[cfg(windows)]
    {
        for extension in ["exe", "cmd", "bat"] {
            if path.with_extension(extension).is_file() {
                return true;
            }
        }
    }

    false
}
