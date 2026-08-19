//! Config + extensions read handlers for the websocket `Config`
//! envelope.
//!
//! Settings writes call the exact same `neoism_backend::config`
//! functions the desktop GUI settings panel uses
//! (`write_setting` / `write_keybind`), so a web client persists into
//! the daemon host's `config.json` with identical semantics — the
//! host's own fs-watcher hot-reloads any desktop app pointed at the
//! same file.
//!
//! `ListExtensions` is the read-only web Extensions catalog: bundled
//! MCP registry + `installed.json`, one row per engine language-server
//! adapter with live connect state (the daemon runs the actual
//! Rust-owned LSP engine), the managed notebook kernels, and the
//! compiled-in tree-sitter grammars. No install flows are exposed —
//! installs stay a desktop-host action.

use neoism_protocol::config::{
    ConfigClientMessage, ConfigServerMessage, ExtensionStatusSummary, ExtensionSummary,
};

use crate::files as files_handler;

/// Permission gate: reads ride the file-read grant, writes the
/// file-write grant (same split the files plane uses).
pub fn required_permission(
    message: &ConfigClientMessage,
) -> neoism_protocol::pairing::Permission {
    match message {
        ConfigClientMessage::GetConfig | ConfigClientMessage::ListExtensions => {
            neoism_protocol::pairing::Permission::ReadFiles
        }
        ConfigClientMessage::SetSetting { .. }
        | ConfigClientMessage::SetKeybind { .. } => {
            neoism_protocol::pairing::Permission::WriteFiles
        }
    }
}

pub async fn handle(message: ConfigClientMessage) -> Vec<ConfigServerMessage> {
    match message {
        ConfigClientMessage::GetConfig => {
            let value = tokio::task::spawn_blocking(
                neoism_backend::config::load_config_json_value,
            )
            .await
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            vec![ConfigServerMessage::Config { value }]
        }
        ConfigClientMessage::SetSetting { key, value } => {
            let write_key = key.clone();
            let result = tokio::task::spawn_blocking(move || {
                neoism_backend::config::write_setting(&write_key, value)
            })
            .await;
            match result {
                Ok(Ok(())) => vec![ConfigServerMessage::SettingWritten { key }],
                Ok(Err(err)) => vec![ConfigServerMessage::Error {
                    message: format!("write_setting {key}: {err}"),
                }],
                Err(err) => vec![ConfigServerMessage::Error {
                    message: format!("write_setting {key} task: {err}"),
                }],
            }
        }
        ConfigClientMessage::SetKeybind { action, key, with } => {
            let write_action = action.clone();
            let result = tokio::task::spawn_blocking(move || {
                neoism_backend::config::write_keybind(&write_action, &key, &with)
            })
            .await;
            match result {
                Ok(Ok(())) => vec![ConfigServerMessage::KeybindWritten { action }],
                Ok(Err(err)) => vec![ConfigServerMessage::Error {
                    message: format!("write_keybind {action}: {err}"),
                }],
                Err(err) => vec![ConfigServerMessage::Error {
                    message: format!("write_keybind {action} task: {err}"),
                }],
            }
        }
        ConfigClientMessage::ListExtensions => {
            let root = files_handler::workspace_root();
            let entries =
                tokio::task::spawn_blocking(move || collect_extension_entries(&root))
                    .await
                    .unwrap_or_default();
            vec![ConfigServerMessage::Extensions { entries }]
        }
    }
}

// ── Extensions inventory ────────────────────────────────────────────
//
// A read-only mirror of the desktop's `load_bundled_extension_entries`
// (desktop/src/screen/bridges/extensions.rs) minus everything install-
// related: bundled MCP registry rows, the built-in Neoism MCP servers,
// managed kernels, live language-server adapter rows, and compiled-in
// grammar rows.

fn collect_extension_entries(workspace_root: &std::path::Path) -> Vec<ExtensionSummary> {
    let installed = neoism_extensions::InstalledIndex::load().ok();
    let installed_version = |id: &str| -> Option<String> {
        installed
            .as_ref()
            .and_then(|index| index.get(id))
            .map(|entry| entry.version.clone())
    };

    let mut entries: Vec<ExtensionSummary> = Vec::new();

    // Built-in Neoism MCP integrations — always present, no lifecycle.
    for (id, name, description, extra_category) in [
        (
            "neoism-notes-mcp",
            "Neoism Notes",
            "Built-in MCP-style notes access for Neoism agents, scoped to linked project notes when available.",
            "Notes",
        ),
        (
            "neoism-memory-mcp",
            "Neoism Memory",
            "Built-in MCP-style persistent memory for Neoism agents. Stores MEMORY.md indexes and topic files in Neoism Notes vaults.",
            "Memory",
        ),
        (
            "neoism-docs-mcp",
            "Neoism Docs",
            "Read-only access to Neoism's bundled product documentation.",
            "Documentation",
        ),
    ] {
        entries.push(ExtensionSummary {
            id: id.to_string(),
            name: name.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: description.to_string(),
            author: "Neoism".to_string(),
            downloads: None,
            categories: vec![
                "MCP Server".to_string(),
                "Built-in".to_string(),
                extra_category.to_string(),
            ],
            languages: Vec::new(),
            status: ExtensionStatusSummary::BuiltIn,
            installed_version: None,
            repository_url: None,
            lsp_source: None,
        });
    }

    // Managed notebook kernels.
    for (id, name, description, language, author, repo) in [
        (
            "neoism-python-kernel",
            "Neoism Python Kernel",
            "Managed Python ipykernel runtime for Neoism notebooks.",
            "Python",
            "Neoism",
            None,
        ),
        (
            "evcxr-jupyter",
            "Rust Jupyter Kernel",
            "Evcxr Jupyter kernel for Rust notebooks.",
            "Rust",
            "Evcxr",
            Some("https://github.com/evcxr/evcxr"),
        ),
    ] {
        let installed_version = installed_version(id);
        entries.push(ExtensionSummary {
            id: id.to_string(),
            name: name.to_string(),
            version: installed_version.clone().unwrap_or_default(),
            description: description.to_string(),
            author: author.to_string(),
            downloads: None,
            categories: vec!["Kernel".to_string(), "Notebook".to_string()],
            languages: vec![language.to_string()],
            status: if installed_version.is_some() {
                ExtensionStatusSummary::Installed
            } else {
                ExtensionStatusSummary::NotInstalled
            },
            installed_version,
            repository_url: repo.map(str::to_string),
            lsp_source: None,
        });
    }

    // Bundled MCP registry — tagged so the panel's McpServers tab
    // filter (substring "mcp") surfaces them, same as desktop.
    let mut mcp_manifests =
        neoism_extensions::parse_bundled_mcp_registry().unwrap_or_default();
    for manifest in mcp_manifests.iter_mut() {
        if !manifest
            .categories
            .iter()
            .any(|category| category.eq_ignore_ascii_case("MCP Server"))
        {
            manifest.categories.insert(0, "MCP Server".to_string());
        }
    }
    for manifest in mcp_manifests {
        let installed_version = installed_version(&manifest.id);
        entries.push(ExtensionSummary {
            id: manifest.id,
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            author: manifest.author,
            downloads: manifest.downloads,
            categories: manifest.categories,
            languages: manifest.languages,
            status: if installed_version.is_some() {
                ExtensionStatusSummary::Installed
            } else {
                ExtensionStatusSummary::NotInstalled
            },
            installed_version,
            repository_url: manifest.repository_url,
            lsp_source: None,
        });
    }

    entries.extend(language_server_entries(workspace_root, &installed));
    entries.extend(built_in_grammar_entries());
    entries
}

/// One row per engine language-server adapter, with the LIVE state the
/// daemon's Rust-owned LSP engine reports (connected / managed /
/// $PATH / missing). Read-only mirror of the desktop builder.
fn language_server_entries(
    workspace_root: &std::path::Path,
    installed: &Option<neoism_extensions::InstalledIndex>,
) -> Vec<ExtensionSummary> {
    use neoism_agent_server::language_server::{
        command_source, language_server_adapters_for, live_languages, LspAdapterTransport,
        LspCommandSource,
    };

    let adapters = language_server_adapters_for(workspace_root);
    let live = live_languages(workspace_root);

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
            let mut categories = vec!["Language Server".to_string(), "LSP".to_string()];
            if adapter.capabilities.formatting {
                categories.push("Formatting".to_string());
            }

            match &adapter.transport {
                LspAdapterTransport::Tcp {
                    default_host,
                    default_port,
                    ..
                } => {
                    categories.push("Built-in".to_string());
                    ExtensionSummary {
                        id: format!("builtin-lsp-{}", adapter.id),
                        name: format!("{} Language Server", adapter.name),
                        version: String::new(),
                        description: format!(
                            "Built-in connection adapter for {routed_languages}; the host application must provide its language server at {default_host}:{default_port}."
                        ),
                        author: "Neoism".to_string(),
                        downloads: None,
                        categories,
                        languages: vec![adapter.name],
                        status: ExtensionStatusSummary::BuiltIn,
                        installed_version: None,
                        repository_url: None,
                        lsp_source: Some(
                            if connected { "connected" } else { "built-in/socket" }
                                .to_string(),
                        ),
                    }
                }
                LspAdapterTransport::Stdio { command } => {
                    let executable = command.first().cloned().unwrap_or_default();
                    let source = command_source(&adapter.id, command.clone());
                    let package_id = adapter
                        .catalog_packages
                        .first()
                        .map(|package| package.package_id.clone());
                    let id = package_id
                        .unwrap_or_else(|| format!("lsp-{}", adapter.id));
                    let installed_version = installed
                        .as_ref()
                        .and_then(|index| index.get(&id))
                        .map(|entry| entry.version.clone());
                    let status = if adapter.configuration_error.is_some() {
                        ExtensionStatusSummary::Unavailable
                    } else {
                        match source {
                            LspCommandSource::Extension => {
                                ExtensionStatusSummary::Installed
                            }
                            LspCommandSource::Path
                            | LspCommandSource::Config
                            | LspCommandSource::BuiltIn => {
                                ExtensionStatusSummary::Detected
                            }
                            LspCommandSource::Missing => {
                                ExtensionStatusSummary::NotInstalled
                            }
                        }
                    };
                    let mut description = format!(
                        "Language server for {routed_languages}; the Neoism LSP engine runs `{executable}` over stdio."
                    );
                    if adapter.capabilities.formatting {
                        description.push_str(" Provides document formatting.");
                    }
                    if let Some(error) = &adapter.configuration_error {
                        description.push_str(&format!(" Configuration error: {error}"));
                    }
                    let lsp_source = if connected {
                        "connected".to_string()
                    } else {
                        match source {
                            LspCommandSource::BuiltIn => "built-in/socket",
                            LspCommandSource::Extension => "extension",
                            LspCommandSource::Config => "config",
                            LspCommandSource::Path => "path",
                            LspCommandSource::Missing => "missing",
                        }
                        .to_string()
                    };
                    ExtensionSummary {
                        id,
                        name: format!("{} Language Server", adapter.name),
                        version: installed_version.clone().unwrap_or_default(),
                        description,
                        author: "Neoism".to_string(),
                        downloads: None,
                        categories,
                        languages: vec![adapter.name],
                        status: if matches!(status, ExtensionStatusSummary::Installed) {
                            ExtensionStatusSummary::Installed
                        } else {
                            status
                        },
                        installed_version,
                        repository_url: None,
                        lsp_source: Some(lsp_source),
                    }
                }
                LspAdapterTransport::Invalid => ExtensionSummary {
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
                    status: ExtensionStatusSummary::Unavailable,
                    installed_version: None,
                    repository_url: None,
                    lsp_source: Some("missing".to_string()),
                },
            }
        })
        .collect()
}

/// Cards for the tree-sitter grammars compiled into Neoism itself.
fn built_in_grammar_entries() -> Vec<ExtensionSummary> {
    neoism_ui::syntax::built_in_grammars()
        .iter()
        .map(|(grammar_id, language)| ExtensionSummary {
            id: format!("grammar-{grammar_id}"),
            name: format!("{language} Syntax"),
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
            status: ExtensionStatusSummary::BuiltIn,
            installed_version: None,
            repository_url: None,
            lsp_source: None,
        })
        .collect()
}
