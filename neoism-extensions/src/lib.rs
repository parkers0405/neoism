pub mod agent_config;
pub mod install_runner;
pub mod installed;
pub mod managed_bin;
pub mod manifest;
pub mod mason;
pub mod paths;

pub use agent_config::{
    agent_config_path, disable_builtin_mcp_entry, install_mcp_entry, uninstall_mcp_entry,
};
pub use install_runner::{
    install, reconcile_managed_installs, record_installed, supported_on_current_host,
    InstallError, InstallHandle, ProgressEvent, ReconcileReport,
};
pub use installed::{InstalledEntry, InstalledIndex};
pub use managed_bin::{
    managed_bin_map, managed_bin_map_from, managed_bin_map_json,
    managed_bin_map_json_from,
};
pub use manifest::*;
pub use mason::{
    ensure_cached_mason_registry, load_mason_registry, package_to_manifest,
    translate_registry, MasonPackage, MasonRegistry,
};

/// JSON text of the build-time generated MCP registry. The contents are
/// produced by `build.rs` from `data/mcp_curated.json` and are committed
/// to git so consumers do not need network at compile time.
pub fn bundled_mcp_registry() -> &'static str {
    include_str!("../data/mcp_registry.json")
}

/// Parse the bundled MCP registry into a `Vec<ExtensionManifest>`.
pub fn parse_bundled_mcp_registry() -> Result<Vec<ExtensionManifest>, InstallError> {
    serde_json::from_str(bundled_mcp_registry())
        .map_err(|e| InstallError::ParseManifest(e.to_string()))
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn bundled_registry_parses_and_has_entries() {
        let entries = parse_bundled_mcp_registry().expect("parse bundled registry");
        assert!(
            entries.len() >= 20,
            "expected >= 20 bundled MCP entries, got {}",
            entries.len()
        );
        for e in &entries {
            assert!(!e.id.is_empty(), "entry has empty id");
            assert!(!e.name.is_empty(), "entry `{}` has empty name", e.id);
            assert!(!e.version.is_empty(), "entry `{}` has empty version", e.id);
            let run = e
                .run
                .as_ref()
                .unwrap_or_else(|| panic!("entry `{}` missing run spec", e.id));
            assert!(
                !run.command.is_empty(),
                "entry `{}` has empty run.command",
                e.id
            );
            match &e.install {
                InstallKind::Npm {
                    package, version, ..
                }
                | InstallKind::Pip { package, version } => {
                    assert!(
                        !package.is_empty(),
                        "entry `{}` empty install package",
                        e.id
                    );
                    assert!(
                        !version.is_empty(),
                        "entry `{}` empty install version",
                        e.id
                    );
                }
                InstallKind::GithubRelease { .. }
                | InstallKind::Cargo { .. }
                | InstallKind::Go { .. }
                | InstallKind::Gem { .. } => {
                    // Other kinds aren't used by the bundled list today.
                }
            }
        }
    }
}
