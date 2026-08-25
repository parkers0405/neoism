//! Migration ratchets for the Agent plugin boundary.
//!
//! The legacy server still contains architecture debt, so known files have
//! explicit ceilings. New files and new architecture directories get no
//! allowance. When a migration removes debt, lower the matching ceiling in the
//! same change; never raise one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const SERVER: &str = "neoism-agent/crates/neoism-agent-server/src/";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("plugin-api crate must remain below the workspace root")
        .to_path_buf()
}

fn walk(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| {
            panic!("failed to inventory {}: {error}", directory.display())
        })
        .map(|entry| entry.expect("failed to read inventory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) != Some("target") {
                walk(&path, files);
            }
        } else {
            files.push(path);
        }
    }
}

fn relative(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .expect("inventoried path must be in the workspace")
        .to_string_lossy()
        .replace('\\', "/")
}

fn production_rust_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    walk(&workspace_root().join("neoism-agent/crates"), &mut files);
    files
        .into_iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("rs"))
        .filter(|path| path.components().any(|part| part.as_os_str() == "src"))
        .map(|path| {
            let name = relative(&path);
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {name}: {error}"));
            (name, source)
        })
        .collect()
}

fn occurrences(source: &str, needle: &str) -> usize {
    source.match_indices(needle).count()
}

fn assert_ratchet(
    debt: &str,
    actual: &BTreeMap<String, usize>,
    ceilings: &[(&str, usize)],
) {
    let ceilings = ceilings.iter().copied().collect::<BTreeMap<_, _>>();
    let mut failures = Vec::new();
    for (path, count) in actual {
        let ceiling = ceilings.get(path.as_str()).copied().unwrap_or(0);
        if *count > ceiling {
            failures.push(format!("{path}: {count} (ceiling {ceiling})"));
        }
    }
    assert!(
        failures.is_empty(),
        "{debt} architecture debt increased:\n{}\n\
         Move the dependency behind plugin-api capabilities, or lower (never raise) the explicit migration ceiling.",
        failures.join("\n")
    );
}

#[test]
fn production_source_inventory_is_live_and_unique() {
    let sources = production_rust_sources();
    assert!(
        sources.len() > 50,
        "Agent production Rust inventory unexpectedly found only {} files",
        sources.len()
    );
    let unique = sources
        .iter()
        .map(|(path, _)| path)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        sources.len(),
        "production source inventory contains duplicates"
    );
}

#[test]
fn plugin_api_stays_transport_storage_and_product_independent() {
    let manifest = fs::read_to_string(
        workspace_root().join("neoism-agent/crates/neoism-agent-plugin-api/Cargo.toml"),
    )
    .expect("read plugin-api manifest");
    let dependencies = manifest_dependencies(&manifest);
    let allowed = ["futures-core", "neoism-agent-core", "serde", "serde_json", "thiserror"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let forbidden = dependencies
        .difference(&allowed)
        .copied()
        .collect::<Vec<_>>();
    assert!(
        forbidden.is_empty(),
        "plugin-api acquired non-contract dependencies: {forbidden:?}; Axum, Turso, server adapters, and product crates belong outside the public API"
    );

    let forbidden_source_tokens = [
        "axum::",
        "turso::",
        "neoism_agent_server",
        "neoism_agent_neoism_adapter",
        "neoism_workspace_daemon",
    ];
    for (path, source) in production_rust_sources()
        .into_iter()
        .filter(|(path, _)| path.contains("neoism-agent-plugin-api/src/"))
    {
        for token in forbidden_source_tokens {
            assert!(!source.contains(token), "plugin-api source {path} depends on forbidden product/transport token `{token}`");
        }
    }
}

#[test]
fn builtin_crates_never_depend_on_the_server() {
    let mut files = Vec::new();
    walk(&workspace_root().join("neoism-agent"), &mut files);
    for path in files.into_iter().filter(|path| {
        path.file_name().and_then(|name| name.to_str()) == Some("Cargo.toml")
    }) {
        let relative = relative(&path);
        let manifest = fs::read_to_string(&path).expect("read Agent crate manifest");
        let package = package_name(&manifest).unwrap_or_default();
        let is_builtin = package.contains("builtin")
            || package.contains("internal-plugin")
            || (package.contains("-plugin-") && package != "neoism-agent-plugin-api")
            || relative.contains("/plugins/");
        if is_builtin {
            assert!(
                !manifest_dependencies(&manifest).contains("neoism-agent-server"),
                "built-in plugin crate `{package}` depends on neoism-agent-server ({relative}); inject plugin-api/service-api capabilities instead"
            );
        }
    }

    // Existing built-ins are embedded in the server. This broad `crate::`
    // ratchet makes their current server coupling shrink and gives every new
    // file in this directory a strict zero allowance.
    let actual = production_rust_sources()
        .into_iter()
        .filter(|(path, _)| path.starts_with(&format!("{SERVER}plugins/")))
        .map(|(path, source)| (path, occurrences(&source, "crate::")))
        .collect::<BTreeMap<_, _>>();
    assert_ratchet(
        "built-ins referencing server internals",
        &actual,
        &[
            (
                "neoism-agent/crates/neoism-agent-server/src/plugins/mod.rs",
                21,
            ),
            (
                "neoism-agent/crates/neoism-agent-server/src/plugins/subagents.rs",
                42,
            ),
        ],
    );
}

#[test]
fn kernel_and_router_modules_do_not_learn_concrete_plugin_ids() {
    let actual = production_rust_sources()
        .into_iter()
        .filter(|(path, _)| {
            path.contains("/kernel/")
                || path.contains("/router/")
                || path.ends_with("_router.rs")
                || path.ends_with("_routes.rs")
        })
        .map(|(path, source)| (path, occurrences(&source, "dev.neoism")))
        .collect::<BTreeMap<_, _>>();
    assert_ratchet(
        "concrete plugin IDs in kernel/router modules",
        &actual,
        &[
            (
                "neoism-agent/crates/neoism-agent-server/src/app_router.rs",
                51,
            ),
            (
                "neoism-agent/crates/neoism-agent-server/src/goal_routes.rs",
                1,
            ),
            (
                "neoism-agent/crates/neoism-agent-server/src/session_routes.rs",
                1,
            ),
            (
                "neoism-agent/crates/neoism-agent-server/src/v2_routes.rs",
                6,
            ),
        ],
    );
}

#[test]
fn internal_plugins_do_not_capture_more_app_state() {
    let actual = production_rust_sources()
        .into_iter()
        .filter(|(path, _)| {
            path.starts_with(&format!("{SERVER}plugins/"))
                || path.contains("/internal_plugins/")
                || path.contains("/builtins/")
        })
        .map(|(path, source)| (path, occurrences(&source, "AppState")))
        .collect::<BTreeMap<_, _>>();
    assert_ratchet(
        "AppState references captured by internal plugins",
        &actual,
        &[
            (
                "neoism-agent/crates/neoism-agent-server/src/plugins/mod.rs",
                4,
            ),
            (
                "neoism-agent/crates/neoism-agent-server/src/plugins/subagents.rs",
                12,
            ),
        ],
    );
}

#[test]
fn workspace_runtime_optional_lifecycle_is_a_decreasing_ratchet() {
    let actual = production_rust_sources()
        .into_iter()
        .filter(|(path, _)| path.contains("workspace_runtime"))
        .map(|(path, source)| (path, occurrences(&source, "OnceLock<")))
        .collect::<BTreeMap<_, _>>();
    assert_ratchet(
        "concrete optional lifecycle fields in WorkspaceRuntime",
        &actual,
        &[(
            "neoism-agent/crates/neoism-agent-server/src/workspace_runtime.rs",
            6,
        )],
    );
}

#[test]
fn user_visible_kernel_tools_are_forbidden() {
    let source =
        fs::read_to_string(workspace_root().join(format!("{SERVER}tool_registry.rs")))
            .expect("read tool registry");
    let ids = ids_after_marker(&source, "ToolOwner::Kernel, owner,");
    assert!(ids.is_empty(), "user-visible kernel tools are forbidden: {ids:?}");
}

#[test]
fn hardcoded_plugin_route_switches_are_allowlisted_and_decreasing() {
    let expected = BTreeSet::<&str>::new();
    let mut actual = BTreeMap::new();
    for (path, source) in production_rust_sources() {
        let switches = quoted_arguments(&source, "if route(\"");
        if !switches.is_empty() {
            let unknown = switches.difference(&expected).collect::<Vec<_>>();
            assert!(unknown.is_empty(), "{path} added hardcoded plugin route switches: {unknown:?}; routes must be plugin contributions");
            actual.insert(path, switches.len());
        }
    }
    assert_ratchet(
        "hardcoded plugin route switches",
        &actual,
        &[],
    );
}

fn package_name(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && line.starts_with("name") {
            return Some(line.split_once('=')?.1.trim().trim_matches('"'));
        }
    }
    None
}

fn manifest_dependencies(manifest: &str) -> BTreeSet<&str> {
    let mut dependencies = BTreeSet::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies =
                line.ends_with("dependencies]") && !line.contains("dev-dependencies");
            continue;
        }
        if in_dependencies {
            if let Some((name, _)) = line.split_once('=') {
                let name = name
                    .trim()
                    .trim_matches('"')
                    .strip_suffix(".workspace")
                    .unwrap_or_else(|| name.trim().trim_matches('"'));
                if !name.is_empty() && !name.starts_with('#') {
                    dependencies.insert(name);
                }
                if let Some(package) = inline_string_field(line, "package") {
                    dependencies.insert(package);
                }
            }
        }
    }
    dependencies
}

fn inline_string_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.match_indices(field).find_map(|(index, _)| {
        let value = line[index + field.len()..]
            .trim_start()
            .strip_prefix('=')?;
        value.split_once('"')?.1.split_once('"').map(|item| item.0)
    })
}

fn ids_after_marker<'a>(source: &'a str, marker: &str) -> BTreeSet<&'a str> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut ids = BTreeSet::new();
    for (index, _) in lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(marker))
    {
        if let Some(id) = lines[index + 1..].iter().take(3).find_map(|line| {
            line.trim()
                .strip_prefix('"')?
                .split_once('"')
                .map(|item| item.0)
        }) {
            ids.insert(id);
        }
    }
    ids
}

fn quoted_arguments<'a>(source: &'a str, marker: &str) -> BTreeSet<&'a str> {
    source
        .match_indices(marker)
        .filter_map(|(index, _)| {
            source[index + marker.len()..]
                .split_once('"')
                .map(|item| item.0)
        })
        .collect()
}
