// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use crate::notebook_runtime::managed_python_kernel_env;
use crate::workspace::extensions::{ExtensionEntry, ExtensionStatus};
use neoism_extensions::{
    ExtensionManifest, InstallError, InstalledEntry, InstalledIndex, ProgressEvent,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const NEOISM_NOTES_MCP_ID: &str = "neoism-notes";
const NEOISM_MEMORY_MCP_ID: &str = "neoism-memory";
const NEOISM_PYTHON_KERNEL_ID: &str = "neoism-python-kernel";
const EVCXR_JUPYTER_KERNEL_ID: &str = "evcxr-jupyter-kernel";
const TREESITTER_EXTENSION_PREFIX: &str = "treesitter-";
use tokio::task::JoinHandle;

/// Dedicated tokio runtime for extension installs. The desktop UI
/// thread runs on winit's event loop, not tokio, so `tokio::spawn`
/// from here panics ("no reactor running"). Per-install threads can't
/// own the runtime either because `JoinHandle` outlives the function
/// that spawned it (we poll completion on the main thread, see
/// `pump_install_progress`). One process-wide multi-threaded runtime
/// resolves both concerns: spawn calls succeed, handles stay valid,
/// and the runtime lives until process exit.
static EXT_RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Flipped to `true` by the detached `kick_mason_seed_if_needed` task
/// once the Mason registry has been fetched (or refreshed from the
/// 24h-stale cache). The per-frame `render_neoism_extensions_panels`
/// observes the flag, re-seeds the visible Extensions pane with the
/// newly available LSP rows, and clears it back to `false`. This is the
/// "show LSPs on first launch without blocking the UI" hand-off — on
/// cache hit the flag flips before the pane even opens, so no re-seed
/// happens; on cache miss the flag flips a few seconds in and the
/// pane updates in place.
static MASON_CACHE_FRESH: AtomicBool = AtomicBool::new(false);

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
/// missing-LSP modal also needs to close the busy modal, open a
/// success/failure modal, and ask nvim to retry the LSP attach.
#[derive(Clone, Debug)]
pub(crate) enum InstallSource {
    ExtensionsPanel,
    PythonKernelModal {
        retry_notebook_cell: Option<(PathBuf, usize)>,
    },
    /// Triggered by `maybe_open_lsp_missing_modal` → "Install <server>"
    /// button. `server` is the lsp.lua server name (e.g. `rust-analyzer`,
    /// `ts_ls`). The `display` string is the user-facing label.
    MissingLspModal {
        server: String,
        display: String,
    },
    TreeSitterParser {
        lang: String,
    },
}

/// One install: the tokio task driving the runner + the mpsc receiver
/// carrying its progress stream + the most recent event so we can paint
/// even when no new events landed this frame.
pub(crate) struct InstallJob {
    pub join_handle: JoinHandle<Result<InstalledEntry, InstallError>>,
    pub progress_rx: UnboundedReceiver<ProgressEvent>,
    /// `None` until the runner emits its first event; once set we keep
    /// the latest snapshot here so the pane row reflects current state
    /// across frames where no new events arrived.
    pub last_percent: u8,
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
fn extension_manifest_to_entry(
    manifest: ExtensionManifest,
    installed_version: Option<String>,
) -> ExtensionEntry {
    let status = match installed_version {
        Some(version) => ExtensionStatus::Installed { version },
        None => ExtensionStatus::NotInstalled,
    };
    let lsp_source = lsp_source_label(&manifest);
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
        lsp_source,
    }
}

/// For a language-server manifest, ask the Rust LSP engine where it would
/// resolve the binary (`extension`/`path`/`config`/`missing`) so the row
/// can show a source badge that matches runtime reality — e.g. a server
/// already on `$PATH` reads as usable even when Mason never installed it.
/// `None` for non-LSP manifests (MCP servers, parsers, kernels).
fn lsp_source_label(manifest: &ExtensionManifest) -> Option<String> {
    let is_lsp = manifest.categories.iter().any(|category| {
        let category = category.to_lowercase();
        category.contains("lsp") || category.contains("language server")
    });
    if !is_lsp {
        return None;
    }
    let command = manifest
        .run
        .as_ref()
        .map(|run| run.command.clone())
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| vec![manifest.id.clone()]);
    use neoism_agent_server::rust_lsp::LspCommandSource;
    let label = match neoism_agent_server::rust_lsp::command_source(&manifest.id, command)
    {
        LspCommandSource::Extension => "extension",
        LspCommandSource::Config => "config",
        LspCommandSource::Path => "path",
        LspCommandSource::Missing => "missing",
    };
    Some(label.to_string())
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

fn treesitter_extension_id(lang: &str) -> String {
    format!("{TREESITTER_EXTENSION_PREFIX}{lang}")
}

fn treesitter_lang_from_extension_id(id: &str) -> Option<&str> {
    let lang = id.strip_prefix(TREESITTER_EXTENSION_PREFIX)?;
    crate::neoism::ide_tools::treesitter_install_spec(lang).map(|_| lang)
}

fn treesitter_parser_path(lang: &str) -> PathBuf {
    neoism_backend::performer::nvim::rio_nvim_parser_dir().join(format!("{lang}.so"))
}

fn treesitter_query_dir(lang: &str) -> PathBuf {
    neoism_backend::performer::nvim::rio_nvim_runtime_dir()
        .join("queries")
        .join(lang)
}

fn treesitter_parser_installed(lang: &str) -> bool {
    treesitter_parser_path(lang).is_file()
        && treesitter_query_dir(lang).join("highlights.scm").is_file()
}

fn treesitter_parser_manifest(lang: &str) -> Option<ExtensionManifest> {
    let spec = crate::neoism::ide_tools::treesitter_install_spec(lang)?;
    Some(ExtensionManifest {
        id: treesitter_extension_id(spec.lang),
        name: format!("{} Syntax Parser", spec.display_name),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: format!(
            "Tree-sitter parser and highlight queries for {} files in Neoism's embedded editor.",
            spec.display_name
        ),
        author: "Tree-sitter".to_string(),
        downloads: None,
        categories: vec![
            "Tree-sitter Parser".to_string(),
            "Syntax Parser".to_string(),
            "Syntax".to_string(),
        ],
        languages: vec![spec.display_name.to_string()],
        repository_url: Some(spec.repo.to_string()),
        homepage: Some(spec.repo.to_string()),
        install: neoism_extensions::InstallKind::Cargo {
            crate_name: format!("tree-sitter-{}", spec.lang),
            version: "managed".to_string(),
            features: Vec::new(),
        },
        run: None,
        env_keys: Vec::new(),
    })
}

fn treesitter_parser_manifests() -> Vec<ExtensionManifest> {
    crate::neoism::ide_tools::treesitter_install_specs()
        .iter()
        .filter_map(|spec| treesitter_parser_manifest(spec.lang))
        .collect()
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
        let _ = progress.send(ProgressEvent::Progress {
            percent: 8,
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
        let _ = progress.send(ProgressEvent::Progress {
            percent: 12,
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

    let _ = progress.send(ProgressEvent::Progress {
        percent: 35,
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

    let _ = progress.send(ProgressEvent::Progress {
        percent: 65,
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

    let _ = progress.send(ProgressEvent::Done);
    Ok(InstalledEntry {
        id: NEOISM_PYTHON_KERNEL_ID.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        install_kind: "managed-python-kernel".to_string(),
        bin_path: Some(python),
        installed_at: now_millis_i64(),
    })
}

async fn install_rust_jupyter_kernel(
    progress: UnboundedSender<ProgressEvent>,
) -> Result<InstalledEntry, InstallError> {
    let _ = progress.send(ProgressEvent::Started);
    let _ = progress.send(ProgressEvent::Progress {
        percent: 10,
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
    let _ = progress.send(ProgressEvent::Progress {
        percent: 85,
        status: "registering Rust Jupyter kernelspec".to_string(),
    });
    run_kernel_install_command(
        evcxr.to_string_lossy().as_ref(),
        &["--install"],
        "register Rust Jupyter kernelspec",
    )
    .await
    .map_err(|err| emit_install_failure(&progress, err))?;

    let _ = progress.send(ProgressEvent::Done);
    Ok(InstalledEntry {
        id: EVCXR_JUPYTER_KERNEL_ID.to_string(),
        version: "latest".to_string(),
        install_kind: "cargo-jupyter-kernel".to_string(),
        bin_path: Some(evcxr),
        installed_at: now_millis_i64(),
    })
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
    let output = tokio::process::Command::new(python)
        .args(["-c", "import ipykernel"])
        .envs(&env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
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
    _label: &str,
) -> Result<(), InstallError> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
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
