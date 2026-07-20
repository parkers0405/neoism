// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use crate::notebook_runtime::managed_python_kernel_env;
use crate::workspace::extensions::{ExtensionEntry, ExtensionStatus};
use neoism_extensions::{
    ExtensionManifest, InstallError, InstallHandle, InstalledEntry, InstalledIndex,
    ProgressEvent,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const NEOISM_NOTES_MCP_ID: &str = "neoism-notes";
const NEOISM_MEMORY_MCP_ID: &str = "neoism-memory";
const NEOISM_PYTHON_KERNEL_ID: &str = "neoism-python-kernel";
const EVCXR_JUPYTER_KERNEL_ID: &str = "evcxr-jupyter-kernel";
const KERNEL_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const KERNEL_VALIDATE_TIMEOUT: Duration = Duration::from_secs(30);

/// Dedicated tokio runtime for extension installs. The desktop UI
/// thread runs on winit's event loop, not tokio, so `tokio::spawn`
/// from here panics ("no reactor running"). Per-install threads can't
/// own the runtime either because `JoinHandle` outlives the function
/// that spawned it (we poll completion on the main thread, see
/// `pump_install_progress`). One process-wide multi-threaded runtime
/// resolves both concerns: spawn calls succeed, handles stay valid,
/// and the runtime lives until process exit.
static EXT_RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Flipped to `true` by the detached `kick_catalog_seed_if_needed` task
/// once the package-catalog snapshot has been fetched (or refreshed from
/// the 24h-stale cache). The per-frame `render_neoism_extensions_panels`
/// observes the flag, re-seeds the visible Extensions pane (so the
/// language-server rows' Install buttons resolve real install plans),
/// and clears it back to `false`. On cache hit the flag flips before
/// the pane even opens, so no re-seed happens; on cache miss the flag
/// flips a few seconds in and the pane updates in place.
static CATALOG_CACHE_FRESH: AtomicBool = AtomicBool::new(false);

fn ext_runtime_handle() -> tokio::runtime::Handle {
    EXT_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("neoism-ext")
                .enable_all()
                .build()
                .expect("build extensions tokio runtime")
        })
        .handle()
        .clone()
}

/// In-flight install jobs tracked by manifest id. Lives on the host
/// `Renderer` (see `host::mod.rs::Renderer::install_tracker`) so that
/// progress drains and final cleanup outlast any single frame.
#[derive(Default)]
pub(crate) struct InstallTracker {
    pub in_flight: BTreeMap<String, InstallJob>,
}

/// Where the install was triggered from. Drives completion UX: the
/// Extensions panel just flips a row and pushes a notification; the
/// missing-LSP modal also needs to close the busy modal and open a
/// success/failure modal.
#[derive(Clone, Debug)]
pub(crate) enum InstallSource {
    ExtensionsPanel,
    PythonKernelModal {
        retry_notebook_cell: Option<(PathBuf, usize)>,
    },
    /// Triggered by an "Install <server>" modal button. `server` is the
    /// Neoism LSP engine's adapter id (e.g. `rust`, `typescript`). The
    /// `display` string is the user-facing label. The engine re-resolves
    /// its command sources on every status read, so the freshly managed
    /// binary is picked up with no extra plumbing.
    MissingLspModal {
        server: String,
        display: String,
    },
}

/// One install: the tokio task driving the runner + the mpsc receiver
/// carrying its progress stream + the most recent event so we can paint
/// even when no new events landed this frame.
pub(crate) struct InstallJob {
    pub install_handle: InstallHandle,
    pub progress_rx: UnboundedReceiver<ProgressEvent>,
    /// `None` until the runner emits its first event; once set we keep
    /// the latest snapshot here so the pane row reflects current state
    /// across frames where no new events arrived.
    pub last_percent: Option<u8>,
    pub last_status: String,
    /// Whether the user clicked Uninstall (synthesised job — no install
    /// runner). We still keep it in the tracker so a single per-frame
    /// pump owns finalisation for both kinds of work.
    pub uninstall: bool,
    pub source: InstallSource,
}

/// Translate a bundled `ExtensionManifest` into the panel's `ExtensionEntry`.
/// Status is derived from whether the manifest's id appears in
/// `installed.json` — fresh users see everything as `NotInstalled`.
/// Language-server rows never come through here (they are built straight
/// from the engine's adapter registry), so `lsp_source` is always `None`.
fn extension_manifest_to_entry(
    manifest: ExtensionManifest,
    installed_version: Option<String>,
) -> ExtensionEntry {
    let status = match installed_version {
        Some(version) => ExtensionStatus::Installed { version },
        None => ExtensionStatus::NotInstalled,
    };
    ExtensionEntry {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        author: manifest.author,
        downloads: manifest.downloads,
        categories: manifest.categories,
        languages: manifest.languages,
        repository_url: manifest.repository_url,
        status,
        lsp_source: None,
    }
}

/// The package catalog resolves install plans (download URL, version,
/// layout), while Neoism's LSP engine registry determines whether an
/// installed server can attach. Both must agree before we offer a
/// one-click install.
fn extension_manifest_supported_by_host(manifest: &ExtensionManifest) -> bool {
    neoism_extensions::supported_on_current_host(manifest)
}

fn manifest_has_registered_lsp_adapter(manifest: &ExtensionManifest) -> bool {
    if !manifest_is_lsp(manifest) {
        return false;
    }
    manifest
        .run
        .as_ref()
        .and_then(|run| run.command.first())
        .is_some_and(|command| {
            neoism_agent_server::language_server::supports_language_server_package(
                &manifest.id,
                command,
            )
        })
}

/// Missing-LSP prompts are stronger than catalog rows: offering an automatic
/// install promises the result can attach, so both a host install strategy and
/// a registered runtime adapter are required.
fn manifest_is_auto_installable_lsp(manifest: &ExtensionManifest) -> bool {
    extension_manifest_supported_by_host(manifest)
        && manifest_has_registered_lsp_adapter(manifest)
}

fn manifest_is_lsp(manifest: &ExtensionManifest) -> bool {
    manifest.categories.iter().any(|category| {
        let category = category.to_ascii_lowercase();
        category.contains("lsp") || category.contains("language server")
    })
}

/// Extra tab memberships for a language-server card, curated by adapter
/// id. Nearly every server advertises the `formatting` and `diagnostics`
/// capability bits, so flag-driven membership made the Formatters and
/// Linters tabs near-copies of Language Servers. Tab placement is earned
/// by what a tool is FOR, not by what its capability bits say:
/// - format-first tools (taplo — the TOML formatter/toolkit whose LSP is
///   the delivery vehicle) also join the Formatters tab;
/// - lint-first tools (eslint, ruff, oxlint, …) also join the Linters
///   tab — none ship in the built-in registry today, but workspace-
///   configured adapters carrying these ids classify correctly;
/// - everything else stays on Language Servers only, with formatting
///   support rendered as a card badge (`Formatting`) instead of a tab
///   membership.
struct AdapterTabRoles {
    formatter: bool,
    linter: bool,
}

fn adapter_tab_roles(adapter_id: &str) -> AdapterTabRoles {
    match adapter_id {
        // taplo: format-first TOML toolkit.
        "toml" => AdapterTabRoles {
            formatter: true,
            linter: false,
        },
        // Lint-first ids, recognised for workspace-configured adapters.
        "eslint" | "ruff" | "oxlint" | "ts_standard" | "standardjs" => AdapterTabRoles {
            formatter: false,
            linter: true,
        },
        // biome is a formatter+linter combo.
        "biome" => AdapterTabRoles {
            formatter: true,
            linter: true,
        },
        _ => AdapterTabRoles {
            formatter: false,
            linter: false,
        },
    }
}

/// Human badge for where the engine resolves an adapter's command.
fn command_source_label(
    source: &neoism_agent_server::language_server::LspCommandSource,
) -> &'static str {
    use neoism_agent_server::language_server::LspCommandSource;
    match source {
        LspCommandSource::BuiltIn => "built-in/socket",
        LspCommandSource::Extension => "extension",
        LspCommandSource::Config => "config",
        LspCommandSource::Path => "path",
        LspCommandSource::Missing => "missing",
    }
}

/// Language-server rows for the Extensions page: exactly one card per
/// adapter in the Neoism LSP engine's runtime registry, so the page can
/// never drift from what the engine can really attach. State is real:
/// `connected` comes from the engine's live client map (a cheap lock
/// read — deliberately NOT `status()`, which walks the workspace), the
/// source badge from where the engine resolves the binary right now,
/// and the install button only appears when the engine's managed
/// download can actually supply the adapter's executable.
///
/// Formatters / Linters tab membership is curated per adapter id (see
/// [`adapter_tab_roles`]) rather than derived from the ubiquitous
/// capability flags — formatting and lint diagnostics flow through the
/// language servers (there is no separate formatter registry to install
/// from), so servers that merely *support* formatting carry a badge-only
/// `Formatting` category on their card instead of flooding those tabs.
fn language_server_entries(
    workspace_root: Option<&std::path::Path>,
    installed: &InstalledIndex,
) -> Vec<ExtensionEntry> {
    use neoism_agent_server::language_server::{
        LspAdapterOrigin, LspAdapterTransport, LspCommandSource,
    };

    let adapters = match workspace_root {
        Some(root) => {
            neoism_agent_server::language_server::language_server_adapters_for(root)
        }
        None => neoism_agent_server::language_server::language_server_adapters(),
    };
    let live = workspace_root
        .map(neoism_agent_server::language_server::live_languages)
        .unwrap_or_default();

    adapters
        .into_iter()
        .map(|adapter| {
            let connected = live.contains(&adapter.id)
                || adapter.routes.iter().any(|route| live.contains(&route.id));
            let routed_languages = adapter
                .routes
                .iter()
                .map(|route| route.document_language_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let roles = adapter_tab_roles(&adapter.id);
            let mut categories =
                vec!["Language Server".to_string(), "LSP".to_string()];
            if roles.formatter {
                categories.push("Formatter".to_string());
            } else if adapter.capabilities.formatting {
                // Badge only: "Formatting" renders as a card chip but
                // matches no tab (`matches_tab` looks for "formatter"),
                // so ubiquitous formatting support stays visible without
                // flooding the Formatters tab.
                categories.push("Formatting".to_string());
            }
            if roles.linter {
                categories.push("Linter".to_string());
            }

            match &adapter.transport {
                LspAdapterTransport::Tcp {
                    default_host,
                    default_port,
                    ..
                } => {
                    categories.push("Built-in".to_string());
                    ExtensionEntry {
                        id: format!("builtin-lsp-{}", adapter.id),
                        name: format!("{} Language Server", adapter.name),
                        // The adapter follows Neoism's own release and is not a
                        // separately-versioned package. Leaving this blank avoids
                        // the generic card renderer inventing a misleading
                        // `vbuilt-in`.
                        version: String::new(),
                        description: format!(
                            "Built-in connection adapter for {routed_languages}; the host application must be running and provide its language server at {default_host}:{default_port}."
                        ),
                        author: "Neoism".to_string(),
                        downloads: None,
                        categories,
                        languages: vec![adapter.name],
                        status: ExtensionStatus::BuiltIn,
                        repository_url: None,
                        lsp_source: Some(
                            if connected { "connected" } else { "built-in/socket" }
                                .to_string(),
                        ),
                    }
                }
                LspAdapterTransport::Stdio { command } => {
                    let executable = command.first().cloned().unwrap_or_default();
                    let source = neoism_agent_server::language_server::command_source(
                        &adapter.id,
                        command.clone(),
                    );
                    // Row id doubles as the install/uninstall key, so it must
                    // match the catalog package the engine's managed download
                    // would install (`installed.json` is keyed the same way).
                    let package_id = adapter
                        .catalog_packages
                        .first()
                        .map(|package| package.package_id.clone());
                    let id = package_id
                        .clone()
                        .unwrap_or_else(|| format!("lsp-{}", adapter.id));
                    let installed_version =
                        installed.get(&id).map(|entry| entry.version.clone());
                    let status = if adapter.configuration_error.is_some() {
                        ExtensionStatus::Unavailable
                    } else {
                        match source {
                            LspCommandSource::Extension => ExtensionStatus::Installed {
                                version: installed_version
                                    .clone()
                                    .unwrap_or_else(|| "managed".to_string()),
                            },
                            LspCommandSource::Path
                            | LspCommandSource::Config
                            | LspCommandSource::BuiltIn => ExtensionStatus::Detected,
                            LspCommandSource::Missing => {
                                if package_id.is_some() {
                                    ExtensionStatus::NotInstalled
                                } else {
                                    ExtensionStatus::Unavailable
                                }
                            }
                        }
                    };
                    let mut description = format!(
                        "Language server for {routed_languages}; the Neoism LSP engine runs `{executable}` over stdio."
                    );
                    if adapter.capabilities.formatting {
                        description.push_str(" Provides document formatting.");
                    }
                    if matches!(adapter.origin, LspAdapterOrigin::Configured) {
                        description
                            .push_str(" Defined by this workspace's configuration.");
                    }
                    if let Some(error) = &adapter.configuration_error {
                        description.push_str(&format!(" Configuration error: {error}"));
                    }
                    ExtensionEntry {
                        id,
                        name: format!("{} Language Server", adapter.name),
                        version: installed_version.unwrap_or_default(),
                        description,
                        author: "Neoism".to_string(),
                        downloads: None,
                        categories,
                        languages: vec![adapter.name],
                        status,
                        repository_url: None,
                        lsp_source: Some(
                            if connected {
                                "connected"
                            } else {
                                command_source_label(&source)
                            }
                            .to_string(),
                        ),
                    }
                }
                LspAdapterTransport::Invalid => ExtensionEntry {
                    id: format!("lsp-{}", adapter.id),
                    name: format!("{} Language Server", adapter.name),
                    version: String::new(),
                    description: adapter
                        .configuration_error
                        .clone()
                        .map(|error| format!("Adapter configuration error: {error}"))
                        .unwrap_or_else(|| {
                            format!(
                                "Adapter for {routed_languages} has an invalid transport configuration."
                            )
                        }),
                    author: "Neoism".to_string(),
                    downloads: None,
                    categories,
                    languages: vec![adapter.name],
                    status: ExtensionStatus::Unavailable,
                    repository_url: None,
                    lsp_source: Some("missing".to_string()),
                },
            }
        })
        .collect()
}

/// Cards for the tree-sitter grammars compiled into Neoism itself.
/// Nothing to download: the old per-parser installer died with the
/// embedded editor, and syntax highlighting now ships in the binary.
fn built_in_syntax_entries() -> Vec<ExtensionEntry> {
    neoism_ui::syntax::built_in_grammars()
        .iter()
        .map(|(grammar_id, language)| ExtensionEntry {
            id: format!("grammar-{grammar_id}"),
            name: format!("{language} Syntax"),
            // Grammars version with the Neoism release itself; a blank
            // version keeps the card from inventing a fake package version.
            version: String::new(),
            description: format!(
                "Tree-sitter grammar for {language}, compiled into Neoism. Powers editor highlighting with nothing to install."
            ),
            author: "Neoism".to_string(),
            downloads: None,
            categories: vec![
                "Syntax Parser".to_string(),
                "Tree-sitter".to_string(),
                "Built-in".to_string(),
            ],
            languages: vec![(*language).to_string()],
            status: ExtensionStatus::BuiltIn,
            repository_url: None,
            lsp_source: None,
        })
        .collect()
}

/// Resolve catalog install manifests for every engine adapter whose
/// executable the managed download can supply. These back the
/// language-server rows' Install/Uninstall actions; rows whose package
/// is absent from the (possibly not-yet-fetched) catalog cache simply
/// resolve nothing and the dispatcher reports it honestly.
fn catalog_manifests_for_engine_adapters() -> Vec<ExtensionManifest> {
    let catalog_path = neoism_extensions::mason::mason_cache_path();
    let Ok(registry) = neoism_extensions::load_mason_registry(&catalog_path) else {
        return Vec::new();
    };
    let mut manifests = Vec::new();
    for adapter in neoism_agent_server::language_server::language_server_adapters() {
        for package in &adapter.catalog_packages {
            let Some(pkg) = registry.iter().find(|p| p.name == package.package_id) else {
                continue;
            };
            let Ok(manifest) = neoism_extensions::package_to_manifest(pkg) else {
                continue;
            };
            if manifest_is_auto_installable_lsp(&manifest) {
                manifests.push(manifest);
            }
        }
    }
    manifests
}

/// Whether a manifest represents an MCP server (vs an LSP / theme /
/// etc.). MCP servers get `mcp.<id>` written into the agent config on
/// install; nothing else does.
fn is_mcp_entry(manifest: &ExtensionManifest) -> bool {
    manifest
        .categories
        .iter()
        .any(|c| c.to_lowercase().contains("mcp"))
}

fn neoism_notes_mcp_manifest() -> ExtensionManifest {
    use neoism_extensions::{InstallKind, RunSpec};
    use std::collections::BTreeMap;

    ExtensionManifest {
        id: NEOISM_NOTES_MCP_ID.to_string(),
        name: "Neoism Notes".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "Built-in MCP-style notes access for Neoism agents, scoped to linked project notes when available.".to_string(),
        author: "Neoism".to_string(),
        downloads: None,
        categories: vec!["MCP Server".to_string(), "Built-in".to_string(), "Notes".to_string()],
        languages: Vec::new(),
        repository_url: None,
        homepage: None,
        executables: vec!["neoism-agent".to_string()],
        install: InstallKind::Cargo {
            crate_name: "neoism-agent-server".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            features: Vec::new(),
        },
        run: Some(RunSpec {
            command: vec!["neoism-agent".to_string(), "mcp".to_string(), "notes".to_string()],
            env: BTreeMap::new(),
        }),
        env_keys: Vec::new(),
    }
}

fn neoism_memory_mcp_manifest() -> ExtensionManifest {
    use neoism_extensions::{InstallKind, RunSpec};
    use std::collections::BTreeMap;

    ExtensionManifest {
        id: NEOISM_MEMORY_MCP_ID.to_string(),
        name: "Neoism Memory".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "Built-in MCP-style persistent memory for Neoism agents. Stores Claude-style MEMORY.md indexes and topic files in Neoism Notes vaults.".to_string(),
        author: "Neoism".to_string(),
        downloads: None,
        categories: vec![
            "MCP Server".to_string(),
            "Built-in".to_string(),
            "Memory".to_string(),
            "Notes".to_string(),
        ],
        languages: Vec::new(),
        repository_url: None,
        homepage: None,
        executables: vec!["neoism-agent".to_string()],
        install: InstallKind::Cargo {
            crate_name: "neoism-agent-server".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            features: Vec::new(),
        },
        run: Some(RunSpec {
            command: vec!["neoism-agent".to_string(), "mcp".to_string(), "memory".to_string()],
            env: BTreeMap::new(),
        }),
        env_keys: Vec::new(),
    }
}

fn neoism_python_kernel_manifest() -> ExtensionManifest {
    use neoism_extensions::{InstallKind, RunSpec};

    ExtensionManifest {
        id: NEOISM_PYTHON_KERNEL_ID.to_string(),
        name: "Neoism Python Kernel".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "Managed Python ipykernel runtime for Neoism notebooks. Installs into Neoism app data and registers a python3 kernelspec.".to_string(),
        author: "Neoism".to_string(),
        downloads: None,
        categories: vec!["Kernel".to_string(), "Notebook".to_string(), "Python".to_string()],
        languages: vec!["Python".to_string()],
        repository_url: None,
        homepage: None,
        executables: Vec::new(),
        install: InstallKind::Cargo {
            crate_name: "neoism-managed-kernel".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            features: Vec::new(),
        },
        run: Some(RunSpec {
            command: vec!["python".to_string(), "-m".to_string(), "ipykernel_launcher".to_string()],
            env: BTreeMap::new(),
        }),
        env_keys: Vec::new(),
    }
}

fn evcxr_jupyter_kernel_manifest() -> ExtensionManifest {
    use neoism_extensions::{InstallKind, RunSpec};

    ExtensionManifest {
        id: EVCXR_JUPYTER_KERNEL_ID.to_string(),
        name: "Rust Jupyter Kernel".to_string(),
        version: "latest".to_string(),
        description: "Evcxr Jupyter kernel for Rust notebooks. Installs evcxr_jupyter with Cargo and registers the Rust kernelspec.".to_string(),
        author: "Evcxr".to_string(),
        downloads: None,
        categories: vec!["Kernel".to_string(), "Notebook".to_string(), "Rust".to_string()],
        languages: vec!["Rust".to_string()],
        repository_url: Some("https://github.com/evcxr/evcxr".to_string()),
        homepage: Some("https://github.com/evcxr/evcxr/tree/main/evcxr_jupyter".to_string()),
        executables: vec!["evcxr_jupyter".to_string()],
        install: InstallKind::Cargo {
            crate_name: "evcxr_jupyter".to_string(),
            version: "latest".to_string(),
            features: Vec::new(),
        },
        run: Some(RunSpec {
            command: vec![
                "evcxr_jupyter".to_string(),
                "--control_file".to_string(),
                "{connection_file}".to_string(),
            ],
            env: std::collections::BTreeMap::new(),
        }),
        env_keys: Vec::new(),
    }
}

fn is_builtin_extension_id(id: &str) -> bool {
    matches!(id, NEOISM_NOTES_MCP_ID | NEOISM_MEMORY_MCP_ID)
}

fn ensure_builtin_mcp_installed(manifests: &[ExtensionManifest]) -> InstalledIndex {
    let mut installed = InstalledIndex::load().unwrap_or_default();
    for manifest in manifests {
        if installed.is_builtin_disabled(&manifest.id) {
            continue;
        }
        if !installed.is_installed(&manifest.id) {
            installed.install_record(InstalledEntry {
                id: manifest.id.clone(),
                version: manifest.version.clone(),
                install_kind: "builtin".to_string(),
                bin_path: std::env::current_exe().ok(),
                installed_at: now_millis_i64(),
            });
        }
        let bin_path = installed
            .get(&manifest.id)
            .and_then(|entry| entry.bin_path.clone())
            .or_else(|| std::env::current_exe().ok());
        if let Some(bin_path) = bin_path.as_deref() {
            let _ = neoism_extensions::agent_config::install_mcp_entry(
                &manifest.id,
                manifest,
                bin_path,
            );
        }
    }
    if let Err(error) = installed.save() {
        tracing::warn!(
            target: "neoism::extensions",
            ?error,
            "failed to persist built-in MCP install records"
        );
    }
    installed
}

fn install_builtin_mcp_record(manifest: &ExtensionManifest) -> Result<(), InstallError> {
    let mut installed = InstalledIndex::load().unwrap_or_default();
    installed.enable_builtin(&manifest.id);
    installed.install_record(InstalledEntry {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        install_kind: "builtin".to_string(),
        bin_path: std::env::current_exe().ok(),
        installed_at: now_millis_i64(),
    });
    installed.save()?;
    let bin_path = installed
        .get(&manifest.id)
        .and_then(|entry| entry.bin_path.clone())
        .or_else(|| std::env::current_exe().ok());
    if let Some(bin_path) = bin_path.as_deref() {
        neoism_extensions::agent_config::install_mcp_entry(
            &manifest.id,
            manifest,
            bin_path,
        )?;
    }
    Ok(())
}

fn now_millis_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

mod dispatch;
mod page;
mod render_input;

/// Translate a winit `KeyEvent` to the platform-neutral `KeyDescriptor`
/// the shared panel API expects. We only carry the bits the panel
/// actually consumes (state + logical key + modifiers) — physical key
/// is opaque to the panel today.
fn winit_key_to_descriptor(
    key: &neoism_window::event::KeyEvent,
    mods: neoism_window::keyboard::ModifiersState,
) -> neoism_ui::event::KeyDescriptor {
    use neoism_ui::event::{
        KeyDescriptor, KeyState, LogicalKey as UiLogical, Modifiers as UiMods,
        NamedKey as UiNamed, PhysicalKey as UiPhysical,
    };
    use neoism_window::event::ElementState;
    use neoism_window::keyboard::{Key, NamedKey as WinitNamed};

    let state = match key.state {
        ElementState::Pressed => KeyState::Pressed,
        ElementState::Released => KeyState::Released,
    };

    let logical = match &key.logical_key {
        Key::Named(named) => {
            let mapped = match named {
                WinitNamed::Enter => Some(UiNamed::Enter),
                WinitNamed::Tab => Some(UiNamed::Tab),
                WinitNamed::Escape => Some(UiNamed::Escape),
                WinitNamed::Backspace => Some(UiNamed::Backspace),
                WinitNamed::ArrowUp => Some(UiNamed::ArrowUp),
                WinitNamed::ArrowDown => Some(UiNamed::ArrowDown),
                WinitNamed::ArrowLeft => Some(UiNamed::ArrowLeft),
                WinitNamed::ArrowRight => Some(UiNamed::ArrowRight),
                WinitNamed::Home => Some(UiNamed::Home),
                WinitNamed::End => Some(UiNamed::End),
                WinitNamed::PageUp => Some(UiNamed::PageUp),
                WinitNamed::PageDown => Some(UiNamed::PageDown),
                WinitNamed::Delete => Some(UiNamed::Delete),
                WinitNamed::Insert => Some(UiNamed::Insert),
                WinitNamed::Space => Some(UiNamed::Space),
                _ => None,
            };
            match mapped {
                Some(n) => UiLogical::Named(n),
                None => UiLogical::Unidentified,
            }
        }
        Key::Character(ch) => UiLogical::Character(ch.clone()),
        _ => UiLogical::Unidentified,
    };

    let mut ui_mods = UiMods::empty();
    if mods.shift_key() {
        ui_mods |= UiMods::SHIFT;
    }
    if mods.control_key() {
        ui_mods |= UiMods::CTRL;
    }
    if mods.alt_key() {
        ui_mods |= UiMods::ALT;
    }
    if mods.super_key() {
        ui_mods |= UiMods::META;
    }

    KeyDescriptor {
        physical: UiPhysical(0),
        logical,
        state,
        modifiers: ui_mods,
        repeat: key.repeat,
    }
}

async fn install_managed_python_kernel(
    progress: UnboundedSender<ProgressEvent>,
) -> Result<InstalledEntry, InstallError> {
    let _ = progress.send(ProgressEvent::Started);
    let install_dir = neoism_extensions::paths::install_dir_for(NEOISM_PYTHON_KERNEL_ID);
    let venv_dir = install_dir.join("venv");
    tokio::fs::create_dir_all(&install_dir).await?;
    let python = venv_python(&venv_dir);

    if validate_managed_python_kernel(&python).await.is_err() {
        let _ = progress.send(ProgressEvent::Waiting {
            status: "repairing managed Python kernel".to_string(),
        });
        remove_managed_python_kernelspec()
            .await
            .map_err(|err| emit_install_failure(&progress, err))?;
        if venv_dir.exists() {
            tokio::fs::remove_dir_all(&venv_dir)
                .await
                .map_err(|err| emit_install_failure(&progress, InstallError::Io(err)))?;
        }
    }

    if !python.exists() {
        let _ = progress.send(ProgressEvent::Waiting {
            status: "creating managed Python venv".to_string(),
        });
        run_kernel_install_command(
            "python3",
            &["-m", "venv", venv_dir.to_string_lossy().as_ref()],
            "create Python virtual environment",
        )
        .await
        .map_err(|err| emit_install_failure(&progress, err))?;
    }

    let _ = progress.send(ProgressEvent::Waiting {
        status: "upgrading managed Python pip".to_string(),
    });
    run_kernel_install_command(
        python.to_string_lossy().as_ref(),
        &[
            "-m",
            "pip",
            "install",
            "--upgrade",
            "pip",
            "setuptools",
            "wheel",
        ],
        "upgrade managed Python pip",
    )
    .await
    .map_err(|err| emit_install_failure(&progress, err))?;

    let _ = progress.send(ProgressEvent::Waiting {
        status: "installing ipykernel".to_string(),
    });
    run_kernel_install_command(
        python.to_string_lossy().as_ref(),
        &["-m", "pip", "install", "--upgrade", "ipykernel"],
        "install ipykernel",
    )
    .await
    .map_err(|err| emit_install_failure(&progress, err))?;

    validate_managed_python_kernel(&python)
        .await
        .map_err(|err| emit_install_failure(&progress, err))?;
    let _ = progress.send(ProgressEvent::Linking);
    write_managed_python_kernelspec(&python)
        .await
        .map_err(|err| emit_install_failure(&progress, err))?;

    let entry = InstalledEntry {
        id: NEOISM_PYTHON_KERNEL_ID.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        install_kind: "managed-python-kernel".to_string(),
        bin_path: Some(python),
        installed_at: now_millis_i64(),
    };
    neoism_extensions::record_installed(entry.clone())
        .await
        .map_err(|error| emit_install_failure(&progress, error))?;
    let _ = progress.send(ProgressEvent::Done);
    Ok(entry)
}

async fn install_rust_jupyter_kernel(
    progress: UnboundedSender<ProgressEvent>,
) -> Result<InstalledEntry, InstallError> {
    let _ = progress.send(ProgressEvent::Started);
    let _ = progress.send(ProgressEvent::Waiting {
        status: "installing evcxr_jupyter with Cargo".to_string(),
    });
    run_kernel_install_command(
        "cargo",
        &["install", "evcxr_jupyter"],
        "install evcxr_jupyter",
    )
    .await
    .map_err(|err| emit_install_failure(&progress, err))?;

    let evcxr = resolve_evcxr_jupyter_bin();
    let _ = progress.send(ProgressEvent::Waiting {
        status: "registering Rust Jupyter kernelspec".to_string(),
    });
    run_kernel_install_command(
        evcxr.to_string_lossy().as_ref(),
        &["--install"],
        "register Rust Jupyter kernelspec",
    )
    .await
    .map_err(|err| emit_install_failure(&progress, err))?;

    let entry = InstalledEntry {
        id: EVCXR_JUPYTER_KERNEL_ID.to_string(),
        version: "latest".to_string(),
        install_kind: "cargo-jupyter-kernel".to_string(),
        bin_path: Some(evcxr),
        installed_at: now_millis_i64(),
    };
    neoism_extensions::record_installed(entry.clone())
        .await
        .map_err(|error| emit_install_failure(&progress, error))?;
    let _ = progress.send(ProgressEvent::Done);
    Ok(entry)
}

fn resolve_evcxr_jupyter_bin() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(evcxr_jupyter_bin_name());
            if candidate.exists() {
                return candidate;
            }
        }
    }

    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        return std::path::PathBuf::from(cargo_home)
            .join("bin")
            .join(evcxr_jupyter_bin_name());
    }

    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~"))
        .join(".cargo")
        .join("bin")
        .join(evcxr_jupyter_bin_name())
}

fn evcxr_jupyter_bin_name() -> &'static str {
    if cfg!(windows) {
        "evcxr_jupyter.exe"
    } else {
        "evcxr_jupyter"
    }
}

async fn write_managed_python_kernelspec(
    python: &std::path::Path,
) -> Result<(), InstallError> {
    let kernel_dir = managed_python_kernelspec_dir();
    tokio::fs::create_dir_all(&kernel_dir).await?;
    let env = managed_python_kernel_env();
    let kernel_json = serde_json::json!({
        "argv": [
            python,
            "-m",
            "ipykernel_launcher",
            "-f",
            "{connection_file}"
        ],
        "display_name": "Python 3 (Neoism)",
        "language": "python",
        "metadata": {
            "debugger": true,
            "neoism_managed": true
        },
        "env": env
    });
    let bytes = serde_json::to_vec_pretty(&kernel_json)
        .map_err(|err| InstallError::Network(format!("encode kernel.json: {err}")))?;
    tokio::fs::write(kernel_dir.join("kernel.json"), bytes).await?;
    Ok(())
}

async fn remove_managed_python_kernelspec() -> Result<(), InstallError> {
    let kernel_dir = managed_python_kernelspec_dir();
    remove_managed_python_kernelspec_dir(&kernel_dir).await
}

async fn remove_managed_python_kernelspec_dir(
    kernel_dir: &std::path::Path,
) -> Result<(), InstallError> {
    match tokio::fs::remove_dir_all(kernel_dir).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(InstallError::Io(err)),
    }
}

fn managed_python_kernelspec_dir() -> PathBuf {
    neoism_extensions::paths::extensions_dir()
        .join("jupyter")
        .join("share")
        .join("jupyter")
        .join("kernels")
        .join("python3")
}

async fn validate_managed_python_kernel(
    python: &std::path::Path,
) -> Result<(), InstallError> {
    let env = managed_python_kernel_env();
    let mut command = tokio::process::Command::new(python);
    command
        .args(["-c", "import ipykernel"])
        .envs(&env)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(KERNEL_VALIDATE_TIMEOUT, command.output())
        .await
        .map_err(|_| InstallError::TimedOut {
            tool: "managed Python kernel validation".to_string(),
            seconds: KERNEL_VALIDATE_TIMEOUT.as_secs(),
        })?
        .map_err(InstallError::Io)?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallError::CommandFailed {
        command: format!("{} -c import ipykernel", python.display()),
        status: output.status.code().unwrap_or(-1),
        stderr: install_command_output_detail(&output.stdout, &output.stderr),
    })
}

fn emit_install_failure(
    progress: &UnboundedSender<ProgressEvent>,
    err: InstallError,
) -> InstallError {
    let _ = progress.send(ProgressEvent::Failed {
        message: err.to_string(),
    });
    err
}

async fn run_kernel_install_command(
    program: &str,
    args: &[&str],
    label: &str,
) -> Result<(), InstallError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(KERNEL_INSTALL_TIMEOUT, command.output())
        .await
        .map_err(|_| InstallError::TimedOut {
            tool: label.to_string(),
            seconds: KERNEL_INSTALL_TIMEOUT.as_secs(),
        })?
        .map_err(InstallError::Io)?;
    if output.status.success() {
        return Ok(());
    }
    let detail = install_command_output_detail(&output.stdout, &output.stderr);
    Err(InstallError::CommandFailed {
        command: format!("{program} {}", args.join(" ")),
        status: output.status.code().unwrap_or(-1),
        stderr: detail,
    })
}

fn install_command_output_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        format!("stderr:\n{stderr}\nstdout:\n{stdout}")
    }
}

fn venv_python(venv_dir: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

#[cfg(test)]
mod tests;
