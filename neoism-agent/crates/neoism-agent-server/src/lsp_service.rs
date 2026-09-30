use std::{
    collections::{BTreeMap, HashMap},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Context;
use serde_json::{json, Value};

use super::{
    lsp_adapters::{
        best_route_in, LanguageAdapter, ResolvedLanguageRoute, ResolvedLspTransport,
    },
    lsp_client::{InitializeResult, LspClient},
    lsp_parse::{
        parse_call_hierarchy_calls, parse_call_hierarchy_items, parse_completion,
        parse_diagnostics, parse_document_highlights, parse_document_symbols,
        parse_hover, parse_inlay_hints, parse_locations, parse_signature_help,
        parse_workspace_symbols,
    },
    lsp_scan::server_root_for_file,
    path_to_file_uri, DIAGNOSTIC_TIMEOUT, DOCUMENT_TIMEOUT, SYMBOL_TIMEOUT,
    TOUCH_DIAGNOSTIC_TIMEOUT,
};

pub(super) struct LspService {
    pub(super) services: neoism_agent_service_api::AgentServices,
    pub(super) diagnostics_bus: tokio::sync::broadcast::Sender<super::DiagnosticsEvent>,
    pub(super) adapter_cache: super::lsp_adapters::AdapterCache,
    pub(super) generation_config: Option<Arc<neoism_agent_core::AgentConfigDocument>>,
    pub(super) cargo_roots: Mutex<super::lsp_scan::CargoRootCache>,
    typescript_projects:
        Mutex<HashMap<TypeScriptProjectCacheKey, TypeScriptProjectCacheEntry>>,
    self_weak: std::sync::Weak<LspService>,
    clients: Mutex<HashMap<LspClientKey, Arc<Mutex<PersistentLspClient>>>>,
    /// Per-client initialization gates. Spawning and initializing happens
    /// outside `clients`, so unrelated servers never block each other, while
    /// concurrent cold opens for the same server still share exactly one
    /// initialization handshake.
    initialization_gates: Mutex<HashMap<LspClientKey, std::sync::Weak<Mutex<()>>>>,
    diagnostics: Mutex<HashMap<DiagnosticOwnerKey, Vec<super::LspDiagnostic>>>,
    /// Latest document version successfully sent with didOpen/didChange.
    document_versions: Mutex<HashMap<DiagnosticOwnerKey, i32>>,
    /// Latest versioned publishDiagnostics payload accepted per document/server.
    diagnostic_versions: Mutex<HashMap<DiagnosticOwnerKey, i32>>,
    broken: Mutex<HashMap<LspClientKey, BrokenClientState>>,
}

impl Default for LspService {
    fn default() -> Self {
        let (diagnostics_bus, _) = tokio::sync::broadcast::channel(512);
        Self::new(
            crate::standard_services(),
            diagnostics_bus,
            std::sync::Weak::new(),
            None,
        )
    }
}

impl LspService {
    pub(super) fn new(
        services: neoism_agent_service_api::AgentServices,
        diagnostics_bus: tokio::sync::broadcast::Sender<super::DiagnosticsEvent>,
        self_weak: std::sync::Weak<LspService>,
        generation_config: Option<Arc<neoism_agent_core::AgentConfigDocument>>,
    ) -> Self {
        Self {
            services,
            diagnostics_bus,
            adapter_cache: Default::default(),
            generation_config,
            cargo_roots: Default::default(),
            typescript_projects: Default::default(),
            self_weak,
            clients: Mutex::new(HashMap::new()),
            initialization_gates: Mutex::new(HashMap::new()),
            diagnostics: Mutex::new(HashMap::new()),
            document_versions: Mutex::new(HashMap::new()),
            diagnostic_versions: Mutex::new(HashMap::new()),
            broken: Mutex::new(HashMap::new()),
        }
    }
}

const LSP_RECONNECT_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct BrokenClientState {
    reason: String,
    retry_after: Instant,
}

/// Ownership identity for one server's diagnostics publication. A language id
/// alone is not unique: users may attach multiple servers for one language,
/// and the same absolute file can be viewed through nested workspace roots.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DiagnosticOwnerKey {
    root: PathBuf,
    file: PathBuf,
    server_id: String,
    language: String,
}

impl DiagnosticOwnerKey {
    fn new(root: &Path, file: &Path, server_id: &str, language: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            file: file.to_path_buf(),
            server_id: server_id.to_string(),
            language: language.to_string(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LspClientKey {
    /// Neoism workspace that owns UI/cache/event state.
    root: PathBuf,
    /// Nearest adapter root used for process cwd, initialize.rootUri, and one
    /// distinct server instance per nested project.
    project_root: PathBuf,
    id: String,
    adapter_id: String,
    endpoint: LspEndpointKey,
    initialization_options: Option<String>,
    configured_initialization_options: Option<String>,
    settings: Option<String>,
    /// Identity of a project runtime loaded by the server (currently the
    /// TypeScript SDK). A generated SDK can change in place without changing
    /// its path or user configuration, and must replace the existing client.
    runtime_identity: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum LspEndpointKey {
    Stdio {
        command: Vec<String>,
        env: Vec<(String, String)>,
    },
    Tcp {
        host: String,
        port: u16,
    },
}

struct PersistentLspClient {
    service: std::sync::Weak<LspService>,
    client: LspClient,
    initialized: InitializeResult,
    root: PathBuf,
    server_id: String,
    adapter_id: String,
    routes: Vec<ResolvedLanguageRoute>,
    open_versions: HashMap<PathBuf, i32>,
    /// Hash of the last text we sent the server per file, so duplicate
    /// editor/collaboration snapshots and read-only queries never re-send a
    /// `didChange` or force needless analysis.
    synced_hashes: HashMap<PathBuf, u64>,
}

struct LspLaunchConfig {
    id: String,
    adapter_id: String,
    routes: Vec<ResolvedLanguageRoute>,
    endpoint: LspEndpoint,
    initialization_options: Option<Value>,
    configured_initialization_options: Option<String>,
    settings: Option<Value>,
    runtime_identity: Option<String>,
    degraded_missing_sdk: bool,
}

const TYPESCRIPT_PROJECT_CACHE_TTL: Duration = Duration::from_secs(2);
const MAX_TYPESCRIPT_PROJECT_CACHE_ENTRIES: usize = 128;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TypeScriptProjectCacheKey {
    workspace_root: PathBuf,
    project_root: PathBuf,
    initialization_options: Option<String>,
}

#[derive(Clone)]
struct TypeScriptProjectCacheEntry {
    checked_at: Instant,
    config: TypeScriptProjectConfig,
}

#[derive(Clone, Debug)]
enum LspEndpoint {
    Stdio {
        command: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Tcp {
        host: String,
        port: u16,
    },
}

impl LspService {
    fn runtime(&self) -> super::LspRuntime {
        super::LspRuntime {
            service: self
                .self_weak
                .upgrade()
                .expect("LSP service must be owned by an LspRuntime"),
        }
    }

    /// Language ids that currently have a live (spawned + initialized) client
    /// under `root`. Lets the status pill show a server as "attached" rather
    /// than merely "available" once the engine has actually connected it.
    pub(super) fn live_languages(
        &self,
        root: &Path,
    ) -> std::collections::BTreeSet<String> {
        let clients = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .iter()
            .filter(|(key, _)| key.root == root)
            .map(|(key, client)| (key.clone(), Arc::clone(client)))
            .collect::<Vec<_>>();
        let mut languages = std::collections::BTreeSet::new();
        let mut dead = Vec::new();
        for (key, client) in clients {
            let (exit_reason, client_languages) = client
                .lock()
                .map(|mut client| {
                    (
                        client.client.exit_reason(),
                        client
                            .routes
                            .iter()
                            .map(|route| route.id.to_string())
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_else(|_| {
                    (Some("LSP client lock poisoned".to_string()), Vec::new())
                });
            if let Some(reason) = exit_reason {
                dead.push((key, reason));
                continue;
            }
            if client_languages.is_empty() {
                languages.insert(key.adapter_id.clone());
            } else {
                languages.extend(client_languages);
            }
        }
        for (key, reason) in dead {
            self.record_broken(&key, reason);
            self.evict_client(&key);
        }
        languages
    }

    pub(super) fn broken_reason(
        &self,
        root: &Path,
        spec: &LanguageAdapter,
    ) -> Option<String> {
        self.broken_reason_at(root, root, spec)
    }

    pub(super) fn broken_reason_for_file(
        &self,
        workspace_root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
    ) -> Option<String> {
        let project_root =
            server_root_for_file(&self.runtime(), workspace_root, file, spec);
        self.broken_reason_at(workspace_root, &project_root, spec)
    }

    /// Whether the exact workspace/project/adapter endpoint represented by
    /// this status row has a live initialized transport. Language-level
    /// liveness is intentionally insufficient: two nested projects can route
    /// the same language to separate processes.
    pub(super) fn client_connected_at(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        spec: &LanguageAdapter,
    ) -> bool {
        let launch = launch_config(&self.runtime(), workspace_root, project_root, spec);
        let key = LspClientKey::new(workspace_root, project_root, &launch);
        let exact = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .get(&key)
            .cloned();
        let (active_key, client) = match exact {
            Some(client) => (key.clone(), client),
            None if launch.degraded_missing_sdk => {
                let Some(existing) = self.existing_client_for_scope(&key) else {
                    return false;
                };
                existing
            }
            None => return false,
        };
        let Some(reason) = client
            .lock()
            .ok()
            .and_then(|mut client| client.client.exit_reason())
        else {
            return true;
        };
        self.record_broken(&active_key, reason);
        self.evict_client(&active_key);
        false
    }

    fn existing_client_for_scope(
        &self,
        replacement: &LspClientKey,
    ) -> Option<(LspClientKey, Arc<Mutex<PersistentLspClient>>)> {
        self.clients
            .lock()
            .expect("lsp client map lock poisoned")
            .iter()
            .find(|(key, _)| can_reuse_for_degraded_sdk(key, replacement))
            .map(|(key, client)| (key.clone(), Arc::clone(client)))
    }

    fn broken_reason_at(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        spec: &LanguageAdapter,
    ) -> Option<String> {
        let launch = launch_config(&self.runtime(), workspace_root, project_root, spec);
        let key = LspClientKey::new(workspace_root, project_root, &launch);
        let client = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .get(&key)
            .cloned();
        if let Some(client) = client {
            if let Some(reason) = client
                .lock()
                .ok()
                .and_then(|mut client| client.client.exit_reason())
            {
                self.record_broken(&key, reason);
                self.evict_client(&key);
            }
        }
        self.broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .get(&key)
            .map(|state| state.reason.clone())
    }

    pub(super) fn workspace_symbols(
        &self,
        root: &Path,
        query: &str,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::WorkspaceSymbol>> {
        self.with_client(root, root, spec, |client| {
            if !client.initialized.workspace_symbol_provider {
                return Ok(Vec::new());
            }
            let result = client.client.request(
                "workspace/symbol",
                json!({ "query": query }),
                SYMBOL_TIMEOUT,
            )?;
            Ok(parse_workspace_symbols(root, &spec.id, result))
        })
    }

    pub(super) fn hover(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspHover>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.hover_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/hover",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_hover(root, file, &language, result)
                .into_iter()
                .collect())
        })
    }

    pub(super) fn signature_help(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspSignatureHelp>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.signature_help_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/signatureHelp",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_signature_help(root, file, &language, result)
                .into_iter()
                .collect())
        })
    }

    pub(super) fn inlay_hints(
        &self,
        root: &Path,
        file: &Path,
        start_line: u32,
        end_line: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspInlayHint>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.inlay_hint_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            // The inclusive zero-based line range widens to the start of the
            // following line so hints anywhere on `end_line` are included.
            let result = client.client.request(
                "textDocument/inlayHint",
                json!({
                    "textDocument": { "uri": path_to_file_uri(file) },
                    "range": {
                        "start": { "line": start_line, "character": 0 },
                        "end": {
                            "line": end_line.saturating_add(1),
                            "character": 0
                        }
                    }
                }),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_inlay_hints(root, file, &language, result))
        })
    }

    pub(super) fn document_highlights(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspDocumentHighlight>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.document_highlight_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/documentHighlight",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_document_highlights(root, file, &language, result))
        })
    }

    pub(super) fn completion(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        text: Option<&str>,
        trigger_character: Option<&str>,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspCompletionItem>> {
        let log = std::env::var_os("NEOISM_LSP_LOG").is_some();
        self.with_file_client(root, file, spec, |client| {
            if log {
                let sample: String = text
                    .and_then(|t| t.lines().nth(line as usize))
                    .map(|l| l.chars().take(60).collect())
                    .unwrap_or_else(|| "<no line>".to_string());
                eprintln!(
                    "neoism::lsp ENGINE completion: spec={} completion_provider={} pos=({line},{character}) text_len={:?} line_text={sample:?}",
                    spec.id,
                    client.initialized.completion_provider,
                    text.map(|t| t.len()),
                );
            }
            if !client.initialized.completion_provider {
                if log {
                    eprintln!(
                        "neoism::lsp ENGINE completion BAILED: server did not advertise completionProvider (0 items)"
                    );
                }
                return Ok(Vec::new());
            }
            // Sync the LIVE buffer text (didChange) so completion is computed
            // against what the user is actually typing, not the disk version.
            match client.ensure_open(file, text) {
                Ok(()) => {
                    if log {
                        eprintln!(
                            "neoism::lsp ENGINE completion: synced doc (version={:?})",
                            client.open_versions.get(file)
                        );
                    }
                }
                Err(error) => {
                    if log {
                        eprintln!("neoism::lsp ENGINE completion: ensure_open FAILED: {error}");
                    }
                    return Err(error);
                }
            }
            let mut params = text_document_position_params(file, line, character);
            params["context"] = completion_request_context(
                &client.initialized.completion_trigger_characters,
                trigger_character,
            );
            let result = match client.client.request(
                "textDocument/completion",
                params,
                DOCUMENT_TIMEOUT,
            ) {
                Ok(result) => result,
                Err(error) => {
                    if log {
                        eprintln!("neoism::lsp ENGINE completion REQUEST FAILED: {error}");
                    }
                    return Err(error);
                }
            };
            let mut items = parse_completion(result.clone());
            for item in &mut items {
                item.server_id = Some(client.server_id.clone());
            }
            if log {
                let raw = serde_json::to_string(&result).unwrap_or_default();
                let head: String = raw.chars().take(400).collect();
                let raw_count = result
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|a| a.len())
                    .or_else(|| result.as_array().map(|a| a.len()));
                eprintln!(
                    "neoism::lsp ENGINE completion RESULT: raw_items={raw_count:?} parsed={} raw_head={head}",
                    items.len(),
                );
            }
            Ok(items)
        })
    }

    pub(super) fn resolve_completion(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
        item: Value,
    ) -> anyhow::Result<Value> {
        self.with_file_client(root, file, spec, |client| {
            client.ensure_open(file, None)?;
            if !client.initialized.completion_resolve_provider {
                return Ok(item);
            }
            let resolved = client.client.request_for_file(
                "completionItem/resolve",
                item.clone(),
                file,
                DOCUMENT_TIMEOUT,
            )?;
            Ok(merge_completion_item(item, resolved))
        })
    }

    pub(super) fn completion_trigger_characters(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<String>> {
        self.with_file_client(root, file, spec, |client| {
            Ok(client.initialized.completion_trigger_characters.clone())
        })
    }

    pub(super) fn definitions(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspLocation>> {
        self.position_locations(
            root,
            file,
            line,
            character,
            spec,
            "definition",
            "textDocument/definition",
        )
    }

    pub(super) fn references(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspLocation>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.references_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let mut params = text_document_position_params(file, line, character);
            params["context"] = json!({ "includeDeclaration": true });
            let result = client.client.request(
                "textDocument/references",
                params,
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_locations(root, &language, result))
        })
    }

    pub(super) fn implementations(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspLocation>> {
        self.position_locations(
            root,
            file,
            line,
            character,
            spec,
            "implementation",
            "textDocument/implementation",
        )
    }

    pub(super) fn prepare_call_hierarchy(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspCallHierarchyItem>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.call_hierarchy_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/prepareCallHierarchy",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_call_hierarchy_items(root, &language, result))
        })
    }

    pub(super) fn call_hierarchy_calls(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
        incoming: bool,
    ) -> anyhow::Result<Vec<super::LspCallHierarchyCall>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.call_hierarchy_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let prepared = client.client.request(
                "textDocument/prepareCallHierarchy",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            let method = if incoming {
                "callHierarchy/incomingCalls"
            } else {
                "callHierarchy/outgoingCalls"
            };
            let mut calls = Vec::new();
            if let Value::Array(items) = prepared {
                for item in items {
                    let result = client.client.request(
                        method,
                        json!({ "item": item }),
                        DOCUMENT_TIMEOUT,
                    )?;
                    calls.extend(parse_call_hierarchy_calls(
                        root, &language, result, incoming,
                    ));
                    if calls.len() >= super::MAX_SYMBOLS {
                        calls.truncate(super::MAX_SYMBOLS);
                        break;
                    }
                }
            }
            Ok(calls)
        })
    }

    pub(super) fn diagnostics(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspDiagnostic>> {
        self.with_file_client(root, file, spec, |client| {
            client.ensure_open(file, None)?;
            // The reader thread already routed every publication through the
            // version guard. This wait is only a bounded barrier; consuming
            // and inserting the queued payload here used to let stale results
            // overwrite the current cache and caused count oscillation.
            let _ = client.client.wait_for_notification(
                "textDocument/publishDiagnostics",
                DIAGNOSTIC_TIMEOUT,
            )?;
            Ok(())
        })?;
        Ok(self.cached_diagnostics(root, file))
    }

    /// Overwrite the cached diagnostics for `file` from a real-time
    /// `publishDiagnostics` push, so cache readers stay fresh without a
    /// pull/`touch`.
    #[cfg(test)]
    pub(super) fn store_diagnostics(
        &self,
        root: &Path,
        file: &Path,
        server_id: &str,
        language: &str,
        diagnostics: Vec<super::LspDiagnostic>,
    ) {
        let _ = self.store_versioned_diagnostics(
            root,
            file,
            server_id,
            language,
            None,
            diagnostics,
        );
    }

    /// Store a diagnostics publication unless it predates either the current
    /// in-memory document or a newer publication already accepted. LSP servers
    /// are allowed to finish analysis out of order; without this guard an old
    /// empty result can erase current errors and make the UI flicker to zero.
    pub(super) fn store_versioned_diagnostics(
        &self,
        root: &Path,
        file: &Path,
        server_id: &str,
        language: &str,
        version: Option<i32>,
        diagnostics: Vec<super::LspDiagnostic>,
    ) -> bool {
        let key = DiagnosticOwnerKey::new(root, file, server_id, language);
        if let Some(version) = version {
            let current_document_version = self
                .document_versions
                .lock()
                .expect("lsp document-version lock poisoned")
                .get(&key)
                .copied();
            let current_diagnostic_version = self
                .diagnostic_versions
                .lock()
                .expect("lsp diagnostic-version lock poisoned")
                .get(&key)
                .copied();
            if current_document_version.is_some_and(|current| version < current)
                || current_diagnostic_version.is_some_and(|current| version < current)
            {
                return false;
            }
            self.diagnostic_versions
                .lock()
                .expect("lsp diagnostic-version lock poisoned")
                .insert(key.clone(), version);
        }
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .insert(key, diagnostics);
        true
    }

    pub(super) fn record_document_version(
        &self,
        root: &Path,
        file: &Path,
        server_id: &str,
        language: &str,
        version: i32,
    ) {
        self.document_versions
            .lock()
            .expect("lsp document-version lock poisoned")
            .insert(
                DiagnosticOwnerKey::new(root, file, server_id, language),
                version,
            );
    }

    pub(super) fn cached_diagnostics(
        &self,
        root: &Path,
        file: &Path,
    ) -> Vec<super::LspDiagnostic> {
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .iter()
            .filter(|(key, _)| key.root == root && key.file == file)
            .flat_map(|(_, diagnostics)| diagnostics.iter().cloned())
            .collect()
    }

    fn cached_diagnostics_for_server(
        &self,
        root: &Path,
        file: &Path,
        server_id: &str,
    ) -> Vec<super::LspDiagnostic> {
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .iter()
            .filter(|(key, _)| {
                key.root == root && key.file == file && key.server_id == server_id
            })
            .flat_map(|(_, diagnostics)| diagnostics.iter().cloned())
            .collect()
    }

    /// Clear all server-owned diagnostic snapshots for one document inside one
    /// workspace. No other root or file is affected.
    pub(super) fn clear_diagnostics(&self, root: &Path, file: &Path) {
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .retain(|key, _| key.root != root || key.file != file);
        self.diagnostic_versions
            .lock()
            .expect("lsp diagnostic-version lock poisoned")
            .retain(|key, _| key.root != root || key.file != file);
    }

    pub(super) fn close_document(&self, root: &Path, file: &Path) -> anyhow::Result<()> {
        let clients = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .iter()
            .filter(|(key, _)| key.root == root)
            .map(|(_, client)| Arc::clone(client))
            .collect::<Vec<_>>();
        let mut first_error = None;
        for client in clients {
            let result = client
                .lock()
                .map_err(|_| anyhow::anyhow!("LSP client lock poisoned"))
                .and_then(|mut client| client.close_document(file));
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        self.clear_document_state(root, file);
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn clear_document_state(&self, root: &Path, file: &Path) {
        self.clear_diagnostics(root, file);
        self.document_versions
            .lock()
            .expect("lsp document-version lock poisoned")
            .retain(|key, _| key.root != root || key.file != file);
    }

    pub(super) fn cached_diagnostics_snapshot(
        &self,
        root: &Path,
    ) -> Vec<(PathBuf, Vec<super::LspDiagnostic>)> {
        let diagnostics = self
            .diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned");
        let mut by_file: BTreeMap<PathBuf, Vec<super::LspDiagnostic>> = BTreeMap::new();
        for (key, items) in diagnostics.iter().filter(|(key, _)| key.root == root) {
            by_file
                .entry(key.file.clone())
                .or_default()
                .extend(items.iter().cloned());
        }
        by_file.into_iter().collect()
    }

    pub(super) fn formatting(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Value> {
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.formatting_provider {
                return Ok(Value::Array(Vec::new()));
            }
            client.ensure_open(file, None)?;
            client.client.request(
                "textDocument/formatting",
                json!({
                    "textDocument": { "uri": path_to_file_uri(file) },
                    "options": {
                        "tabSize": 4,
                        "insertSpaces": true,
                        "trimTrailingWhitespace": true,
                        "insertFinalNewline": true,
                        "trimFinalNewlines": true
                    }
                }),
                DOCUMENT_TIMEOUT,
            )
        })
    }

    pub(super) fn code_actions(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Value> {
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.code_action_provider {
                return Ok(Value::Array(Vec::new()));
            }
            client.ensure_open(file, None)?;
            let diagnostics = self
                .cached_diagnostics_for_server(root, file, &spec.id)
                .iter()
                .filter(|diagnostic| {
                    diagnostic_contains_position(diagnostic, line, character)
                })
                .filter_map(|diagnostic| diagnostic_to_wire_value(root, diagnostic))
                .collect::<Vec<_>>();
            client.client.request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": path_to_file_uri(file) },
                    "range": {
                        "start": { "line": line, "character": character },
                        "end": { "line": line, "character": character }
                    },
                    "context": { "diagnostics": diagnostics }
                }),
                DOCUMENT_TIMEOUT,
            )
        })
    }

    pub(super) fn resolve_code_action(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
        action: Value,
    ) -> anyhow::Result<Value> {
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.code_action_resolve_provider {
                return Ok(action);
            }
            client
                .client
                .request("codeAction/resolve", action, DOCUMENT_TIMEOUT)
        })
    }

    pub(super) fn execute_command(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
        command: Value,
    ) -> anyhow::Result<Value> {
        self.with_file_client(root, file, spec, |client| {
            client
                .client
                .request("workspace/executeCommand", command, DOCUMENT_TIMEOUT)
        })
    }

    pub(super) fn rename(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        new_name: &str,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Value> {
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.rename_provider {
                return Ok(Value::Null);
            }
            client.ensure_open(file, None)?;
            client.client.request(
                "textDocument/rename",
                json!({
                    "textDocument": { "uri": path_to_file_uri(file) },
                    "position": { "line": line, "character": character },
                    "newName": new_name,
                }),
                DOCUMENT_TIMEOUT,
            )
        })
    }

    /// Open (didOpen) or update (didChange) the document WITHOUT waiting for
    /// diagnostics. The server re-analyzes and pushes `publishDiagnostics`
    /// asynchronously, which the reader thread fans onto the event bus — the
    /// event-driven (golden) path. Spawns/initializes the client on first call.
    pub(super) fn sync(
        &self,
        root: &Path,
        file: &Path,
        text: Option<&str>,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<()> {
        self.with_file_client(root, file, spec, |client| {
            client.ensure_open(file, text)?;
            Ok(())
        })
    }

    /// Deliver didSave for an already-synchronized document when the server
    /// negotiated save notifications. This is lifecycle-wide rather than
    /// diagnostics-specific: format/completion/hover-only adapters can also
    /// depend on save to refresh project state.
    pub(super) fn save(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<()> {
        self.with_file_client(root, file, spec, |client| {
            client.ensure_open(file, None)?;
            client.client.save_document(file)
        })
    }

    /// Synchronize a diagnostic touch without synthesizing an editor save.
    /// Unlike navigation's ensure_open(None), a touch without live text means
    /// the caller changed the file on disk, including when it is already open.
    pub(super) fn touch(
        &self,
        root: &Path,
        file: &Path,
        text: Option<&str>,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspDiagnostic>> {
        let disk_text;
        let text = match text {
            Some(text) => text,
            None => {
                disk_text = fs::read_to_string(file).with_context(|| {
                    format!("failed to read touched document {}", file.display())
                })?;
                &disk_text
            }
        };
        let pulled = self.with_file_client(root, file, spec, |client| {
            client.ensure_open(file, Some(text))?;
            // didSave belongs exclusively to save(): an agent diagnostic touch
            // must not trigger a redundant check-on-save build.
            // Preserve pull diagnostics when supported and asynchronous pushes
            // otherwise; neither guarantees a fresh build-backed snapshot.
            if client.initialized.diagnostic_provider {
                if let Ok(report) = client
                    .client
                    .pull_diagnostics(file, TOUCH_DIAGNOSTIC_TIMEOUT)
                {
                    let language = client.logical_language_for_path(file)?.to_string();
                    return Ok(Some((
                        client.server_id.clone(),
                        language.clone(),
                        parse_diagnostics(root, file, &language, report),
                    )));
                }
            }
            // Push-only diagnostics are captured by the reader thread in
            // dispatch_diagnostics. Do not consume the request channel here:
            // it may contain an older publish queued before this touch, which
            // would overwrite the newer real-time cache and make the UI flash
            // between stale and current counts.
            Ok(None)
        })?;
        if let Some((server_id, language, diagnostics)) = pulled {
            self.diagnostics
                .lock()
                .expect("lsp diagnostics cache lock poisoned")
                .insert(
                    DiagnosticOwnerKey::new(root, file, &server_id, &language),
                    diagnostics,
                );
        }
        Ok(self.cached_diagnostics(root, file))
    }

    pub(super) fn document_symbols(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspDocumentSymbol>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            if !client.initialized.document_symbol_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": path_to_file_uri(file) } }),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_document_symbols(root, file, &language, result))
        })
    }

    pub(super) fn shutdown_all(&self) {
        let clients = std::mem::take(
            &mut *self.clients.lock().expect("lsp client map lock poisoned"),
        );
        for client in clients.into_values() {
            if let Ok(mut client) = client.lock() {
                let _ = client.client.shutdown();
            }
        }
        self.typescript_projects
            .lock()
            .expect("TypeScript project cache lock poisoned")
            .clear();
    }

    pub(super) fn shutdown_root(&self, root: &Path) {
        let root = crate::lsp::normalized_root(root);
        let clients = {
            let mut clients = self.clients.lock().expect("lsp client map lock poisoned");
            let keys = clients
                .keys()
                .filter(|key| key.root == root)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| clients.remove(&key))
                .collect::<Vec<_>>()
        };
        for client in clients {
            if let Ok(mut client) = client.lock() {
                let _ = client.client.shutdown();
            }
        }
        self.diagnostics
            .lock()
            .expect("lsp diagnostics lock poisoned")
            .retain(|key, _| key.root != root);
        self.document_versions
            .lock()
            .expect("lsp document version lock poisoned")
            .retain(|key, _| key.root != root);
        self.diagnostic_versions
            .lock()
            .expect("lsp diagnostic version lock poisoned")
            .retain(|key, _| key.root != root);
        self.initialization_gates
            .lock()
            .expect("lsp initialization gate lock poisoned")
            .retain(|key, _| key.root != root);
        self.broken
            .lock()
            .expect("lsp broken map lock poisoned")
            .retain(|key, _| key.root != root);
        self.typescript_projects
            .lock()
            .expect("TypeScript project cache lock poisoned")
            .retain(|key, _| key.workspace_root != root);
        *self
            .cargo_roots
            .lock()
            .expect("lsp cargo root cache lock poisoned") = Default::default();
    }

    fn position_locations(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageAdapter,
        capability: &str,
        method: &str,
    ) -> anyhow::Result<Vec<super::LspLocation>> {
        let language = spec
            .logical_language_for_path(file)
            .unwrap_or(&spec.id)
            .to_string();
        self.with_file_client(root, file, spec, |client| {
            let supported = match capability {
                "definition" => client.initialized.definition_provider,
                "implementation" => client.initialized.implementation_provider,
                _ => true,
            };
            if !supported {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                method,
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_locations(root, &language, result))
        })
    }

    fn with_file_client<T>(
        &self,
        workspace_root: &Path,
        file: &Path,
        spec: &LanguageAdapter,
        operation: impl FnOnce(&mut PersistentLspClient) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let project_root =
            server_root_for_file(&self.runtime(), workspace_root, file, spec);
        self.with_client(workspace_root, &project_root, spec, operation)
    }

    fn with_client<T>(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        spec: &LanguageAdapter,
        operation: impl FnOnce(&mut PersistentLspClient) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let launch = launch_config(&self.runtime(), workspace_root, project_root, spec);
        let key = LspClientKey::new(workspace_root, project_root, &launch);
        // `client()` records only real connect/spawn/initialize failures. Do
        // not record its backoff sentinel here: resetting `retry_after` on
        // every UI poll can otherwise postpone reconnect forever.
        let client = self.client(workspace_root, project_root, &key, &launch)?;
        let mut guard = client.lock().expect("lsp client lock poisoned");
        if let Some(reason) = guard.client.exit_reason() {
            drop(guard);
            self.record_broken(&key, reason.clone());
            self.evict_client(&key);
            anyhow::bail!(reason);
        }
        let result = operation(&mut guard);
        // A valid JSON-RPC error (for example RequestFailed, ContentModified,
        // or a method-specific validation error) does not mean the transport
        // died. Only retire the client when the process/socket itself reports
        // an exit; otherwise subsequent edits and requests must reuse it.
        let exit_reason = guard.client.exit_reason();
        drop(guard);
        if let Some(reason) = exit_reason {
            self.record_broken(&key, reason);
            self.evict_client(&key);
        }
        result
    }

    fn client(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        key: &LspClientKey,
        launch: &LspLaunchConfig,
    ) -> anyhow::Result<Arc<Mutex<PersistentLspClient>>> {
        // A running tsserver has already loaded its SDK. If that SDK is
        // temporarily absent during Yarn regeneration, keep the useful client
        // instead of tearing it down merely to start a degraded fallback.
        if launch.degraded_missing_sdk {
            if let Some((_, client)) = self.existing_client_for_scope(key) {
                return Ok(client);
            }
        }
        // Endpoint, environment, initializationOptions, and settings are part
        // of the key. If configuration changes any of them, retire the old
        // transport for this workspace+project+adapter before creating its
        // replacement; sibling nested projects retain their own clients.
        self.evict_superseded_clients(key);

        // `clients` cannot remain locked during process startup/initialize,
        // but that otherwise permits two simultaneous didOpen paths to spawn
        // duplicate processes. A weak per-key gate serializes only that cold
        // path and is discarded automatically after the last waiter leaves.
        let initialization_gate = self.initialization_gate(key);
        let _initialization_guard = initialization_gate
            .lock()
            .expect("lsp initialization gate lock poisoned");
        let existing = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .get(key)
            .cloned();
        if let Some(client) = existing {
            let reason = client
                .lock()
                .ok()
                .and_then(|mut client| client.client.exit_reason());
            if reason.is_none() {
                self.broken
                    .lock()
                    .expect("lsp broken-client map lock poisoned")
                    .remove(key);
                return Ok(client);
            }
            self.record_broken(
                key,
                reason.unwrap_or_else(|| "language server disconnected".to_string()),
            );
            self.evict_client(key);
        }

        if let Some(state) = self
            .broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .get(key)
            .filter(|state| Instant::now() < state.retry_after)
            .cloned()
        {
            anyhow::bail!("{}; reconnecting after a short backoff", state.reason);
        }

        // Spawn/connect and initialize outside the global clients lock. One
        // slow server must never block unrelated language servers or status
        // reads in the same process.
        let spawned = match &launch.endpoint {
            LspEndpoint::Stdio { command, env } => LspClient::spawn_with_env(
                &self.services,
                self.self_weak.clone(),
                project_root,
                workspace_root,
                &launch.id,
                &launch.adapter_id,
                &launch.routes,
                command,
                env,
            ),
            LspEndpoint::Tcp { host, port } => {
                if *port == 0 {
                    anyhow::bail!(
                        "LSP TCP endpoint for `{}` needs a non-zero `port`",
                        launch.id
                    );
                }
                LspClient::connect_tcp(
                    self.self_weak.clone(),
                    project_root,
                    workspace_root,
                    &launch.id,
                    &launch.adapter_id,
                    &launch.routes,
                    host,
                    *port,
                )
            }
        };
        let mut client = match spawned {
            Ok(client) => client,
            Err(error) => {
                self.record_broken(key, error.to_string());
                return Err(error);
            }
        };
        let initialized = match client.initialize_with_configuration(
            project_root,
            launch.initialization_options.clone(),
            launch.settings.clone(),
        ) {
            Ok(initialized) => initialized,
            Err(error) => {
                self.record_broken(key, error.to_string());
                let _ = client.shutdown();
                return Err(error);
            }
        };
        let persistent = Arc::new(Mutex::new(PersistentLspClient {
            service: self.self_weak.clone(),
            client,
            initialized,
            root: workspace_root.to_path_buf(),
            server_id: launch.id.clone(),
            adapter_id: launch.adapter_id.clone(),
            routes: launch.routes.clone(),
            open_versions: HashMap::new(),
            synced_hashes: HashMap::new(),
        }));
        let persistent = {
            let mut clients = self.clients.lock().expect("lsp client map lock poisoned");
            if let Some(existing) = clients.get(key) {
                existing.clone()
            } else {
                // A replacement client starts each document at didOpen version
                // 0. Remove stale version floors from the prior transport while
                // preserving its last diagnostic snapshot until the new server
                // publishes, avoiding both rejection and UI flicker.
                self.reset_client_versions(workspace_root, &launch.id);
                clients.insert(key.clone(), persistent.clone());
                persistent
            }
        };
        self.broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .remove(key);
        Ok(persistent)
    }

    fn initialization_gate(&self, key: &LspClientKey) -> Arc<Mutex<()>> {
        let mut gates = self
            .initialization_gates
            .lock()
            .expect("lsp initialization-gate map lock poisoned");
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(key).and_then(std::sync::Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(key.clone(), Arc::downgrade(&gate));
        gate
    }

    fn evict_client(&self, key: &LspClientKey) {
        let removed = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .remove(key);
        if let Some(client) = removed {
            if let Ok(mut client) = client.lock() {
                let _ = client.client.shutdown();
            }
        }
    }

    fn evict_superseded_clients(&self, replacement: &LspClientKey) {
        let removed = {
            let mut removed = Vec::new();
            self.clients
                .lock()
                .expect("lsp client map lock poisoned")
                .retain(|key, client| {
                    let superseded = key != replacement
                        && key.root == replacement.root
                        && key.project_root == replacement.project_root
                        && key.adapter_id == replacement.adapter_id;
                    if superseded {
                        removed.push(Arc::clone(client));
                    }
                    !superseded
                });
            removed
        };
        self.broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .retain(|key, _| {
                key == replacement
                    || key.root != replacement.root
                    || key.project_root != replacement.project_root
                    || key.adapter_id != replacement.adapter_id
            });
        for client in removed {
            if let Ok(mut client) = client.lock() {
                let _ = client.client.shutdown();
            }
        }
    }

    fn record_broken(&self, key: &LspClientKey, reason: String) {
        self.broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .insert(
                key.clone(),
                BrokenClientState {
                    reason,
                    retry_after: Instant::now() + LSP_RECONNECT_BACKOFF,
                },
            );
    }

    fn reset_client_versions(&self, root: &Path, server_id: &str) {
        self.document_versions
            .lock()
            .expect("lsp document-version lock poisoned")
            .retain(|key, _| key.root != root || key.server_id != server_id);
        self.diagnostic_versions
            .lock()
            .expect("lsp diagnostic-version lock poisoned")
            .retain(|key, _| key.root != root || key.server_id != server_id);
    }
}

fn can_reuse_for_degraded_sdk(
    existing: &LspClientKey,
    replacement: &LspClientKey,
) -> bool {
    existing.root == replacement.root
        && existing.project_root == replacement.project_root
        && existing.id == replacement.id
        && existing.adapter_id == replacement.adapter_id
        && existing.endpoint == replacement.endpoint
        && existing.configured_initialization_options
            == replacement.configured_initialization_options
        && existing.settings == replacement.settings
}

/// Servers normally echo the full CompletionItem from `completionItem/resolve`,
/// but the protocol permits them to fill only advertised lazy properties in
/// practice. Preserve every original field that the response omitted so edit
/// ranges, data and the display label cannot disappear at acceptance time.
fn merge_completion_item(original: Value, resolved: Value) -> Value {
    match (original, resolved) {
        (Value::Object(original), Value::Object(mut resolved)) => {
            for (key, value) in original {
                resolved.entry(key).or_insert(value);
            }
            Value::Object(resolved)
        }
        (_, resolved) => resolved,
    }
}

/// Public diagnostics carry display-friendly one-based byte coordinates,
/// severity names, and Neoism-only ownership fields. Code-action context must
/// contain actual LSP Diagnostics: zero-based positions, numeric severity, and
/// no `path`/`language` metadata.
fn diagnostic_contains_position(
    diagnostic: &super::LspDiagnostic,
    line: u32,
    character: u32,
) -> bool {
    let Some(range) = diagnostic.range.as_ref() else {
        return false;
    };
    let start = (
        range.start.line.saturating_sub(1),
        range.start.character.saturating_sub(1),
    );
    let end = (
        range.end.line.saturating_sub(1),
        range.end.character.saturating_sub(1),
    );
    start <= (line, character) && (line, character) <= end
}

fn diagnostic_to_wire_value(
    root: &Path,
    diagnostic: &super::LspDiagnostic,
) -> Option<Value> {
    let range = diagnostic.range.as_ref()?;
    let mut value = json!({
        "range": {
            "start": {
                "line": range.start.line.saturating_sub(1),
                "character": range.start.character.saturating_sub(1),
            },
            "end": {
                "line": range.end.line.saturating_sub(1),
                "character": range.end.character.saturating_sub(1),
            },
        },
        "message": diagnostic.message,
    });
    if let Some(severity) = match diagnostic.severity.as_str() {
        "error" => Some(1),
        "warning" => Some(2),
        "information" => Some(3),
        "hint" => Some(4),
        _ => None,
    } {
        value["severity"] = Value::from(severity);
    }
    if let Some(code) = &diagnostic.code {
        value["code"] = Value::from(code.clone());
    }
    if let Some(href) = &diagnostic.code_description {
        value["codeDescription"] = json!({ "href": href });
    }
    if let Some(source) = &diagnostic.source {
        value["source"] = Value::from(source.clone());
    }
    if !diagnostic.tags.is_empty() {
        value["tags"] = Value::Array(
            diagnostic
                .tags
                .iter()
                .filter_map(|tag| match tag.as_str() {
                    "unnecessary" => Some(Value::from(1)),
                    "deprecated" => Some(Value::from(2)),
                    _ => None,
                })
                .collect(),
        );
    }
    if !diagnostic.related_information.is_empty() {
        value["relatedInformation"] = Value::Array(
            diagnostic
                .related_information
                .iter()
                .filter_map(|related| {
                    let range = related.range.as_ref()?;
                    let related_path = Path::new(&related.path);
                    let related_path = if related_path.is_absolute() {
                        related_path.to_path_buf()
                    } else {
                        root.join(related_path)
                    };
                    Some(json!({
                        "location": {
                            "uri": path_to_file_uri(&related_path),
                            "range": {
                                "start": {
                                    "line": range.start.line.saturating_sub(1),
                                    "character": range.start.character.saturating_sub(1),
                                },
                                "end": {
                                    "line": range.end.line.saturating_sub(1),
                                    "character": range.end.character.saturating_sub(1),
                                },
                            },
                        },
                        "message": related.message,
                    }))
                })
                .collect(),
        );
    }
    if let Some(data) = &diagnostic.data {
        value["data"] = data.clone();
    }
    Some(value)
}

#[cfg(test)]
mod diagnostic_cache_tests {
    use super::*;

    fn diagnostic(message: &str) -> super::super::LspDiagnostic {
        super::super::LspDiagnostic {
            path: "project.gd".into(),
            range: None,
            severity: "error".into(),
            message: message.into(),
            source: None,
            code: None,
            code_description: None,
            tags: Vec::new(),
            related_information: Vec::new(),
            data: None,
            language: None,
        }
    }

    #[test]
    fn runtimes_isolate_diagnostics_events_and_all_mutable_caches() {
        let first = super::super::LspRuntime::new(crate::standard_services());
        let second = super::super::LspRuntime::new(crate::standard_services());
        let root = Path::new("/tmp/lsp-runtime-isolation");
        let file = Path::new("src/main.rs");
        let mut first_events = first.subscribe_diagnostics();
        let mut second_events = second.subscribe_diagnostics();

        first.service.store_diagnostics(
            root,
            file,
            "server-a",
            "rust",
            vec![diagnostic("first only")],
        );
        let _ = first
            .service
            .diagnostics_bus
            .send(super::super::DiagnosticsEvent {
                root: root.to_path_buf(),
                server_id: "server-a".into(),
                language: "rust".into(),
                file: file.display().to_string(),
                diagnostics: first.service.cached_diagnostics(root, file),
            });

        assert_eq!(first.service.cached_diagnostics(root, file).len(), 1);
        assert!(second.service.cached_diagnostics(root, file).is_empty());
        assert_eq!(first_events.try_recv().unwrap().diagnostics.len(), 1);
        assert!(matches!(
            second_events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(!std::ptr::eq(
            &first.service.adapter_cache,
            &second.service.adapter_cache,
        ));
        assert!(!std::ptr::eq(
            &first.service.cargo_roots,
            &second.service.cargo_roots,
        ));
        assert!(first.service.clients.lock().unwrap().is_empty());
        assert!(second.service.clients.lock().unwrap().is_empty());
    }

    #[test]
    fn code_action_context_preserves_server_diagnostic_identity() {
        let root = Path::new("/workspace");
        let diagnostic = super::super::LspDiagnostic {
            path: "src/main.rs".into(),
            range: Some(super::super::LspRange {
                start: super::super::LspPosition {
                    line: 3,
                    character: 5,
                },
                end: super::super::LspPosition {
                    line: 3,
                    character: 9,
                },
            }),
            severity: "error".into(),
            code: Some("E0425".into()),
            code_description: Some("https://example.invalid/E0425".into()),
            source: Some("fixture-lsp".into()),
            message: "missing name".into(),
            tags: vec!["unnecessary".into()],
            related_information: vec![super::super::LspDiagnosticRelatedInformation {
                path: "src/lib.rs".into(),
                range: Some(super::super::LspRange {
                    start: super::super::LspPosition {
                        line: 8,
                        character: 2,
                    },
                    end: super::super::LspPosition {
                        line: 8,
                        character: 6,
                    },
                }),
                message: "declared here".into(),
            }],
            data: Some(json!({ "fixId": 17 })),
            language: Some("fixture".into()),
        };

        let wire = diagnostic_to_wire_value(root, &diagnostic).expect("wire diagnostic");
        assert_eq!(wire["range"]["start"]["line"], 2);
        assert_eq!(wire["range"]["start"]["character"], 4);
        assert_eq!(wire["code"], "E0425");
        assert_eq!(
            wire["codeDescription"]["href"],
            "https://example.invalid/E0425"
        );
        assert_eq!(wire["tags"], json!([1]));
        assert_eq!(wire["relatedInformation"][0]["message"], "declared here");
        assert_eq!(
            wire["relatedInformation"][0]["location"]["range"]["start"]["line"],
            7
        );
        assert_eq!(wire["data"]["fixId"], 17);
    }

    #[test]
    fn code_action_context_is_scoped_to_origin_server_and_cursor_range() {
        let service = LspService::default();
        let root = Path::new("/tmp/workspace");
        let file = Path::new("src/main.rs");
        let mut at_cursor = diagnostic("at cursor");
        at_cursor.range = Some(super::super::LspRange {
            start: super::super::LspPosition {
                line: 8,
                character: 3,
            },
            end: super::super::LspPosition {
                line: 8,
                character: 9,
            },
        });
        let mut other_server = at_cursor.clone();
        other_server.message = "other server".into();
        service.store_diagnostics(
            root,
            file,
            "server-a",
            "rust",
            vec![at_cursor.clone()],
        );
        service.store_diagnostics(root, file, "server-b", "rust", vec![other_server]);

        let owned = service.cached_diagnostics_for_server(root, file, "server-a");
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].message, "at cursor");
        assert!(diagnostic_contains_position(&at_cursor, 7, 2));
        assert!(diagnostic_contains_position(&at_cursor, 7, 8));
        assert!(!diagnostic_contains_position(&at_cursor, 7, 9));
        assert!(!diagnostic_contains_position(&at_cursor, 6, 2));
    }

    #[test]
    fn diagnostics_from_multiple_servers_are_merged_per_file() {
        let service = LspService::default();
        let root = Path::new("/tmp/workspace");
        let file = Path::new("project.gd");
        service.store_diagnostics(
            root,
            file,
            "gdscript-lsp",
            "gdscript",
            vec![diagnostic("parse")],
        );
        service.store_diagnostics(
            root,
            file,
            "godot-lsp",
            "gdscript",
            vec![diagnostic("type")],
        );

        let merged = service.cached_diagnostics(root, file);
        assert_eq!(merged.len(), 2);

        service.store_diagnostics(root, file, "gdscript-lsp", "gdscript", Vec::new());
        let merged = service.cached_diagnostics(root, file);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].message, "type");

        let other_root = Path::new("/tmp/nested-workspace");
        service.store_diagnostics(
            other_root,
            file,
            "godot-lsp",
            "gdscript",
            vec![diagnostic("nested")],
        );
        assert_eq!(service.cached_diagnostics(root, file).len(), 1);
        assert_eq!(service.cached_diagnostics(other_root, file).len(), 1);
    }

    #[test]
    fn stale_versioned_diagnostics_cannot_replace_current_document_results() {
        let service = LspService::default();
        let root = Path::new("/tmp/workspace");
        let file = Path::new("src/main.rs");
        service.record_document_version(root, file, "rust-analyzer", "rust", 4);

        assert!(!service.store_versioned_diagnostics(
            root,
            file,
            "rust-analyzer",
            "rust",
            Some(3),
            vec![diagnostic("stale")],
        ));
        assert!(service.cached_diagnostics(root, file).is_empty());

        assert!(service.store_versioned_diagnostics(
            root,
            file,
            "rust-analyzer",
            "rust",
            Some(4),
            vec![diagnostic("current")],
        ));
        assert!(!service.store_versioned_diagnostics(
            root,
            file,
            "rust-analyzer",
            "rust",
            Some(2),
            Vec::new(),
        ));
        let cached = service.cached_diagnostics(root, file);
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].message, "current");
    }

    #[test]
    fn rapid_fix_keeps_empty_current_snapshot_and_rejects_late_results() {
        let service = LspService::default();
        let root = Path::new("/tmp/workspace");
        let file = Path::new("src/main.rs");

        service.record_document_version(root, file, "server", "rust", 1);
        assert!(service.store_versioned_diagnostics(
            root,
            file,
            "server",
            "rust",
            Some(1),
            vec![diagnostic("broken-v1")],
        ));
        service.record_document_version(root, file, "server", "rust", 2);
        service.record_document_version(root, file, "server", "rust", 3);
        assert!(service.store_versioned_diagnostics(
            root,
            file,
            "server",
            "rust",
            Some(3),
            Vec::new(),
        ));

        assert!(!service.store_versioned_diagnostics(
            root,
            file,
            "server",
            "rust",
            Some(2),
            vec![diagnostic("late-v2")],
        ));
        assert!(service.cached_diagnostics(root, file).is_empty());
    }

    #[test]
    fn closing_previous_buffer_never_clears_active_buffer_diagnostics() {
        let service = LspService::default();
        let root = Path::new("/tmp/workspace");
        let previous = Path::new("src/previous.rs");
        let active = Path::new("src/active.rs");
        service.store_diagnostics(
            root,
            previous,
            "server",
            "rust",
            vec![diagnostic("previous")],
        );
        service.store_diagnostics(
            root,
            active,
            "server",
            "rust",
            vec![diagnostic("active")],
        );

        service
            .close_document(root, previous)
            .expect("close previous buffer");

        assert!(service.cached_diagnostics(root, previous).is_empty());
        let active_diagnostics = service.cached_diagnostics(root, active);
        assert_eq!(active_diagnostics.len(), 1);
        assert_eq!(active_diagnostics[0].message, "active");
    }

    #[test]
    fn failed_client_reason_is_available_to_status_reporting() {
        use neoism_agent_service_api::{
            LanguageCapabilitySnapshot, LanguageRootPolicy, LanguageRouteCapability,
            LanguageServerCapability, LanguageServerOperations, LanguageServerTransport,
            StaticLanguageCapabilityService,
        };
        let capability = LanguageServerCapability {
            id: "fake-language".to_string(),
            name: "Fake Language".to_string(),
            catalog_packages: Vec::new(),
            transport: LanguageServerTransport::Stdio {
                command: vec!["fake-language-server".to_string()],
            },
            routes: vec![LanguageRouteCapability {
                id: "fake-language".to_string(),
                document_language_id: "fake-language".to_string(),
                extensions: vec!["fake".to_string()],
                filename_patterns: Vec::new(),
            }],
            markers: Vec::new(),
            root_policy: LanguageRootPolicy::NearestMarker,
            capabilities: LanguageServerOperations {
                workspace_symbols: true,
                completion: true,
                hover: true,
                definition: true,
                references: true,
                implementation: true,
                call_hierarchy: true,
                diagnostics: true,
                document_symbols: true,
                formatting: true,
                code_actions: true,
                rename: true,
            },
        };
        let services =
            crate::standard_services().with_language_capabilities(std::sync::Arc::new(
                StaticLanguageCapabilityService::new(LanguageCapabilitySnapshot {
                    generation: 1,
                    languages: std::sync::Arc::from(vec![capability.clone()]),
                }),
            ));
        let runtime = super::super::LspRuntime::new(services);
        let service = runtime.service.as_ref();
        let root = Path::new("/tmp/neoism-broken-lsp-status");
        let spec = LanguageAdapter::from_capability(&capability);
        let launch = launch_config(&runtime, root, root, &spec);
        let key = LspClientKey::new(root, root, &launch);
        service.record_broken(&key, "server exited during initialize".to_string());

        assert_eq!(
            service.broken_reason(root, &spec).as_deref(),
            Some("server exited during initialize")
        );
    }
}

impl PersistentLspClient {
    fn ensure_open(&mut self, file: &Path, text: Option<&str>) -> anyhow::Result<()> {
        let service = self.service.clone();
        let root = self.root.clone();
        let server_id = self.server_id.clone();
        let (language_id, logical_language) = self.language_ids_for_path(file)?;
        match self.open_versions.get_mut(file) {
            Some(version) => {
                // Already open. Only re-sync when the caller supplies fresh
                // text (a live buffer edit). With `None` we must NOT re-read
                // the file from disk — that would clobber the live in-memory
                // text with the stale on-disk version, so hover/completion/etc.
                // would query the wrong content and return nothing.
                if let Some(text) = text {
                    // Skip didChange when the snapshot is byte-identical to
                    // what the server already has. Collaborative views and
                    // read-only queries can legitimately submit the same
                    // authoritative text more than once.
                    let hash = text_hash(text);
                    if self.synced_hashes.get(file) != Some(&hash) {
                        let next_version = version.saturating_add(1);
                        // Advance the acceptance floor before writing
                        // didChange. The reader thread can receive a very fast
                        // (or late prior-revision) publish concurrently with
                        // this call; recording afterward leaves a window where
                        // that stale payload is accepted and briefly flickers
                        // back into the UI.
                        record_document_version(
                            &service,
                            &root,
                            file,
                            &server_id,
                            &logical_language,
                            next_version,
                        );
                        if let Err(error) =
                            self.client.change_document(file, next_version, text)
                        {
                            record_document_version(
                                &service,
                                &root,
                                file,
                                &server_id,
                                &logical_language,
                                *version,
                            );
                            return Err(error);
                        }
                        *version = next_version;
                        self.synced_hashes.insert(file.to_path_buf(), hash);
                    }
                }
                record_document_version(
                    &service,
                    &root,
                    file,
                    &server_id,
                    &logical_language,
                    *version,
                );
                Ok(())
            }
            None => {
                // First open: use the live text if provided, else disk.
                let text = match text {
                    Some(text) => text.to_string(),
                    None => fs::read_to_string(file).with_context(|| {
                        format!("failed to read document {}", file.display())
                    })?,
                };
                self.client
                    .open_document_with_text(file, &language_id, &text)?;
                self.open_versions.insert(file.to_path_buf(), 0);
                self.synced_hashes
                    .insert(file.to_path_buf(), text_hash(&text));
                record_document_version(
                    &service,
                    &root,
                    file,
                    &server_id,
                    &logical_language,
                    0,
                );
                Ok(())
            }
        }
    }

    fn close_document(&mut self, file: &Path) -> anyhow::Result<()> {
        if self.open_versions.remove(file).is_some() {
            self.synced_hashes.remove(file);
            self.client.close_document(file)?;
        }
        Ok(())
    }

    fn language_ids_for_path(&self, file: &Path) -> anyhow::Result<(String, String)> {
        let route = best_route_in(&self.routes, file).with_context(|| {
            format!(
                "LSP adapter `{}` has no document route for {}",
                self.adapter_id,
                file.display()
            )
        })?;
        Ok((route.document_language_id.to_string(), route.id.to_string()))
    }

    fn logical_language_for_path(&self, file: &Path) -> anyhow::Result<&str> {
        best_route_in(&self.routes, file)
            .map(|route| route.id.as_str())
            .with_context(|| {
                format!(
                    "LSP adapter `{}` has no document route for {}",
                    self.adapter_id,
                    file.display()
                )
            })
    }
}

fn record_document_version(
    service: &std::sync::Weak<LspService>,
    root: &Path,
    file: &Path,
    server_id: &str,
    language: &str,
    version: i32,
) {
    if let Some(service) = service.upgrade() {
        service.record_document_version(root, file, server_id, language, version);
    }
}

impl LspClientKey {
    fn new(root: &Path, project_root: &Path, launch: &LspLaunchConfig) -> Self {
        let endpoint = match &launch.endpoint {
            LspEndpoint::Stdio { command, env } => LspEndpointKey::Stdio {
                command: command.clone(),
                env: env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            },
            LspEndpoint::Tcp { host, port } => LspEndpointKey::Tcp {
                host: host.clone(),
                port: *port,
            },
        };
        Self {
            root: root.to_path_buf(),
            project_root: project_root.to_path_buf(),
            id: launch.id.clone(),
            adapter_id: launch.adapter_id.clone(),
            endpoint,
            initialization_options: launch
                .initialization_options
                .as_ref()
                .map(|value| value.to_string()),
            configured_initialization_options: launch
                .configured_initialization_options
                .clone(),
            settings: launch.settings.as_ref().map(Value::to_string),
            runtime_identity: launch.runtime_identity.clone(),
        }
    }
}

fn launch_config(
    runtime: &super::LspRuntime,
    workspace_root: &Path,
    project_root: &Path,
    adapter: &LanguageAdapter,
) -> LspLaunchConfig {
    let endpoint = match &adapter.transport {
        ResolvedLspTransport::Stdio { command, env } => {
            let (command, source) =
                runtime.resolve_lsp_command(&adapter.id, command.clone());
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                eprintln!(
                    "neoism::lsp resolve[{}]: source={source:?} endpoint=stdio:{}",
                    adapter.id,
                    command.join(" "),
                );
            }
            LspEndpoint::Stdio {
                command,
                env: env.clone(),
            }
        }
        ResolvedLspTransport::Tcp { host, port, .. } => {
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                eprintln!(
                    "neoism::lsp resolve[{}]: endpoint=tcp://{host}:{port}",
                    adapter.id,
                );
            }
            LspEndpoint::Tcp {
                host: host.clone(),
                port: *port,
            }
        }
        ResolvedLspTransport::Invalid => LspEndpoint::Stdio {
            command: Vec::new(),
            env: BTreeMap::new(),
        },
    };
    let project =
        runtime
            .service
            .typescript_project_config(workspace_root, project_root, adapter);
    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
        if let Some(info) = &project.runtime {
            eprintln!(
                "neoism::lsp runtime[{}]: source={:?} path={} version={}",
                adapter.id,
                info.source,
                info.path.as_deref().unwrap_or("<server default>"),
                info.version.as_deref().unwrap_or("unknown"),
            );
        }
    }
    LspLaunchConfig {
        id: adapter.id.clone(),
        adapter_id: adapter.id.clone(),
        routes: adapter.routes.clone(),
        endpoint,
        initialization_options: project.initialization_options,
        configured_initialization_options: adapter
            .initialization_options
            .as_ref()
            .map(Value::to_string),
        settings: adapter.settings.clone(),
        runtime_identity: project.identity,
        degraded_missing_sdk: project.degraded_missing_sdk,
    }
}

#[derive(Clone, Default)]
pub(super) struct TypeScriptProjectConfig {
    initialization_options: Option<Value>,
    identity: Option<String>,
    pub(super) runtime: Option<super::LspRuntimeInfo>,
    pub(super) warning: Option<String>,
    degraded_missing_sdk: bool,
}

impl LspService {
    pub(super) fn typescript_project_config(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        adapter: &LanguageAdapter,
    ) -> TypeScriptProjectConfig {
        if adapter.id != "typescript" {
            return uncached_typescript_project_config(
                workspace_root,
                project_root,
                adapter,
            );
        }
        let key = TypeScriptProjectCacheKey {
            workspace_root: workspace_root.to_path_buf(),
            project_root: project_root.to_path_buf(),
            initialization_options: adapter
                .initialization_options
                .as_ref()
                .map(Value::to_string),
        };
        if let Some(config) = self
            .typescript_projects
            .lock()
            .expect("TypeScript project cache lock poisoned")
            .get(&key)
            .filter(|entry| entry.checked_at.elapsed() < TYPESCRIPT_PROJECT_CACHE_TTL)
            .map(|entry| entry.config.clone())
        {
            return config;
        }

        // Filesystem probing and SDK package parsing happen only on this
        // bounded refresh path, never on each document operation. SDK changes
        // become visible within the short TTL or on runtime recreation.
        let config =
            uncached_typescript_project_config(workspace_root, project_root, adapter);
        let mut cache = self
            .typescript_projects
            .lock()
            .expect("TypeScript project cache lock poisoned");
        if cache.len() >= MAX_TYPESCRIPT_PROJECT_CACHE_ENTRIES
            && !cache.contains_key(&key)
        {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.checked_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            key,
            TypeScriptProjectCacheEntry {
                checked_at: Instant::now(),
                config: config.clone(),
            },
        );
        config
    }
}

fn uncached_typescript_project_config(
    workspace_root: &Path,
    project_root: &Path,
    adapter: &LanguageAdapter,
) -> TypeScriptProjectConfig {
    let mut result = TypeScriptProjectConfig {
        initialization_options: adapter.initialization_options.clone(),
        ..TypeScriptProjectConfig::default()
    };
    if adapter.id != "typescript" {
        return result;
    }

    if let Some(configured) = configured_typescript_sdk(&result.initialization_options) {
        let path = absolute_sdk_path(project_root, Path::new(configured));
        result.identity = sdk_identity(&path);
        result.runtime = Some(typescript_runtime_info(
            super::LspRuntimeSource::Configured,
            &path,
        ));
        return result;
    }

    let mut pnp_root = None;
    for directory in bounded_project_ancestors(workspace_root, project_root) {
        let sdk = directory.join(".yarn/sdks/typescript/lib");
        if let Some(probe) = probe_sdk(&sdk) {
            if merge_typescript_sdk_path(&mut result.initialization_options, &sdk) {
                result.identity = Some(probe.identity);
                result.runtime = Some(super::LspRuntimeInfo {
                    source: super::LspRuntimeSource::YarnSdk,
                    path: Some(sdk.display().to_string()),
                    version: probe.version,
                });
                return result;
            }
            result.warning = Some("Yarn Plug'n'Play TypeScript SDK was found, but the configured `initializationOptions.tsserver` value prevents Neoism from selecting it. Use an object with a valid string `path`, or remove `path` to enable automatic selection.".to_string());
            break;
        }
        if directory.join(".pnp.cjs").is_file() {
            pnp_root = Some(directory.to_path_buf());
            break;
        }
    }

    if let Some(pnp_root) = pnp_root {
        let sdk = pnp_root.join(".yarn/sdks/typescript/lib");
        result.runtime = Some(super::LspRuntimeInfo {
            source: super::LspRuntimeSource::MissingYarnSdk,
            path: Some(sdk.display().to_string()),
            version: None,
        });
        result.warning = Some(format!(
            "Yarn Plug'n'Play was detected at {}, but the patched TypeScript SDK is missing. TypeScript is using its normal fallback; run `corepack yarn dlx @yarnpkg/sdks base` once at that project root for complete PnP resolution.",
            pnp_root.display()
        ));
        result.degraded_missing_sdk = true;
    } else {
        result.runtime = Some(super::LspRuntimeInfo {
            source: super::LspRuntimeSource::LanguageServerDefault,
            path: None,
            version: None,
        });
    }
    result
}

fn bounded_project_ancestors<'a>(
    workspace_root: &'a Path,
    project_root: &'a Path,
) -> impl Iterator<Item = &'a Path> {
    let bounded = project_root.starts_with(workspace_root);
    project_root.ancestors().take_while(move |directory| {
        if bounded {
            directory.starts_with(workspace_root)
        } else {
            *directory == project_root
        }
    })
}

fn configured_typescript_sdk(options: &Option<Value>) -> Option<&str> {
    options
        .as_ref()?
        .get("tsserver")?
        .get("path")?
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

fn merge_typescript_sdk_path(options: &mut Option<Value>, sdk: &Path) -> bool {
    if options.is_none() {
        *options = Some(json!({ "tsserver": { "path": sdk.display().to_string() } }));
        return true;
    }
    let Some(object) = options.as_mut().and_then(Value::as_object_mut) else {
        return false;
    };
    let tsserver = object
        .entry("tsserver")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(tsserver) = tsserver.as_object_mut() else {
        return false;
    };
    match tsserver.entry("path") {
        serde_json::map::Entry::Vacant(entry) => {
            entry.insert(Value::String(sdk.display().to_string()));
            true
        }
        // A valid configured string was handled before automatic selection.
        // Preserve every other user value rather than silently replacing it.
        serde_json::map::Entry::Occupied(_) => false,
    }
}

fn absolute_sdk_path(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn typescript_runtime_info(
    source: super::LspRuntimeSource,
    sdk: &Path,
) -> super::LspRuntimeInfo {
    let version = sdk
        .parent()
        .and_then(|typescript| {
            std::fs::read_to_string(typescript.join("package.json")).ok()
        })
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .and_then(|package| {
            package
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    super::LspRuntimeInfo {
        source,
        path: Some(sdk.display().to_string()),
        version,
    }
}

struct SdkProbe {
    identity: String,
    version: Option<String>,
}

fn probe_sdk(sdk: &Path) -> Option<SdkProbe> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sdk.hash(&mut hasher);
    let typescript = fs::read(sdk.join("typescript.js")).ok()?;
    let tsserver = fs::read(sdk.join("tsserver.js")).ok()?;
    typescript.hash(&mut hasher);
    tsserver.hash(&mut hasher);
    let package = sdk
        .parent()
        .and_then(|typescript| fs::read(typescript.join("package.json")).ok());
    package.hash(&mut hasher);
    let version = package
        .as_deref()
        .and_then(|contents| serde_json::from_slice::<Value>(contents).ok())
        .and_then(|package| {
            package
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Some(SdkProbe {
        identity: format!("{:016x}", hasher.finish()),
        version,
    })
}

fn sdk_identity(sdk: &Path) -> Option<String> {
    probe_sdk(sdk).map(|probe| probe.identity)
}

#[cfg(test)]
mod typescript_project_tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use neoism_agent_service_api::{
        LanguageRootPolicy, LanguageRouteCapability, LanguageServerCapability,
        LanguageServerOperations, LanguageServerTransport,
    };
    use serde_json::json;

    use super::*;

    struct Project {
        root: PathBuf,
    }

    impl Project {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "neoism-typescript-project-{name}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create project");
            Self { root }
        }

        fn add_pnp(&self) {
            fs::write(self.root.join(".pnp.cjs"), "module.exports = {};")
                .expect("write PnP marker");
        }

        fn add_yarn_sdk(&self, version: &str) -> PathBuf {
            let typescript = self.root.join(".yarn/sdks/typescript");
            let lib = typescript.join("lib");
            fs::create_dir_all(&lib).expect("create SDK");
            fs::write(lib.join("typescript.js"), "typescript wrapper")
                .expect("write typescript wrapper");
            fs::write(lib.join("tsserver.js"), "tsserver wrapper")
                .expect("write tsserver wrapper");
            fs::write(
                typescript.join("package.json"),
                json!({ "name": "typescript", "version": version }).to_string(),
            )
            .expect("write SDK package");
            lib
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn adapter(options: Option<Value>) -> LanguageAdapter {
        let mut adapter = LanguageAdapter::from_capability(&LanguageServerCapability {
            id: "typescript".to_string(),
            name: "TypeScript".to_string(),
            catalog_packages: Vec::new(),
            transport: LanguageServerTransport::Stdio {
                command: vec![
                    "typescript-language-server".to_string(),
                    "--stdio".to_string(),
                ],
            },
            routes: vec![LanguageRouteCapability {
                id: "typescript".to_string(),
                document_language_id: "typescript".to_string(),
                extensions: vec!["ts".to_string()],
                filename_patterns: Vec::new(),
            }],
            markers: vec!["package.json".to_string(), ".pnp.cjs".to_string()],
            root_policy: LanguageRootPolicy::NearestMarker,
            capabilities: LanguageServerOperations {
                workspace_symbols: true,
                completion: true,
                hover: true,
                definition: true,
                references: true,
                implementation: true,
                call_hierarchy: true,
                diagnostics: true,
                document_symbols: true,
                formatting: true,
                code_actions: true,
                rename: true,
            },
        });
        adapter.initialization_options = options;
        adapter
    }

    #[test]
    fn yarn_pnp_sdk_is_selected_and_merged_with_user_options() {
        let project = Project::new("sdk-merge");
        project.add_pnp();
        let sdk = project.add_yarn_sdk("5.7.3-sdk");
        let adapter = adapter(Some(json!({
            "provideFormatter": false,
            "tsserver": { "maxTsServerMemory": 4096 }
        })));

        let config =
            uncached_typescript_project_config(&project.root, &project.root, &adapter);

        assert_eq!(config.warning, None);
        assert_eq!(
            config
                .initialization_options
                .as_ref()
                .and_then(|value| value.pointer("/tsserver/path"))
                .and_then(Value::as_str),
            Some(sdk.to_str().expect("UTF-8 path"))
        );
        assert_eq!(
            config
                .initialization_options
                .as_ref()
                .and_then(|value| value.pointer("/tsserver/maxTsServerMemory")),
            Some(&json!(4096))
        );
        assert_eq!(
            config
                .initialization_options
                .as_ref()
                .and_then(|value| value.get("provideFormatter")),
            Some(&json!(false))
        );
        let runtime = config.runtime.expect("runtime info");
        assert_eq!(runtime.source, super::super::LspRuntimeSource::YarnSdk);
        assert_eq!(runtime.version.as_deref(), Some("5.7.3-sdk"));
        assert!(config.identity.is_some());
    }

    #[test]
    fn yarn_pnp_without_sdk_has_actionable_non_mutating_warning() {
        let project = Project::new("missing-sdk");
        project.add_pnp();

        let config = uncached_typescript_project_config(
            &project.root,
            &project.root,
            &adapter(None),
        );

        assert!(config.warning.as_deref().is_some_and(|message| {
            message.contains("corepack yarn dlx @yarnpkg/sdks base")
                && message.contains("normal fallback")
                && message.contains(project.root.to_str().expect("UTF-8 path"))
        }));
        assert_eq!(
            config.runtime.expect("runtime info").source,
            super::super::LspRuntimeSource::MissingYarnSdk
        );
        assert!(!project.root.join(".yarn").exists());
    }

    #[test]
    fn explicit_user_sdk_path_takes_precedence_over_generated_sdk() {
        let project = Project::new("explicit-sdk");
        project.add_pnp();
        project.add_yarn_sdk("5.7.3-sdk");
        let explicit = project.root.join("custom/typescript/lib");
        fs::create_dir_all(&explicit).expect("create explicit SDK");
        fs::write(explicit.join("typescript.js"), "custom").expect("write explicit SDK");
        let adapter = adapter(Some(json!({
            "tsserver": { "path": "custom/typescript/lib", "logVerbosity": "verbose" },
            "locale": "en"
        })));

        let config =
            uncached_typescript_project_config(&project.root, &project.root, &adapter);

        assert_eq!(config.warning, None);
        assert_eq!(
            config.initialization_options,
            adapter.initialization_options
        );
        let runtime = config.runtime.expect("runtime info");
        assert_eq!(runtime.source, super::super::LspRuntimeSource::Configured);
        assert_eq!(
            runtime.path.as_deref(),
            Some(explicit.to_str().expect("UTF-8 path"))
        );
    }

    #[test]
    fn sdk_appearance_and_change_update_lsp_client_key_identity() {
        let project = Project::new("restart-identity");
        project.add_pnp();
        let adapter = adapter(None);
        let runtime = super::super::LspRuntime::new(crate::standard_services());
        let first = launch_config(&runtime, &project.root, &project.root, &adapter);
        let first_key = LspClientKey::new(&project.root, &project.root, &first);

        let sdk = project.add_yarn_sdk("5.7.3-sdk");
        runtime
            .service
            .typescript_projects
            .lock()
            .expect("cache lock")
            .clear();
        let second = launch_config(&runtime, &project.root, &project.root, &adapter);
        let second_key = LspClientKey::new(&project.root, &project.root, &second);
        assert_ne!(
            first_key, second_key,
            "SDK appearance must replace the client"
        );

        assert_eq!("tsserver wrapper".len(), "changed! wrapper".len());
        fs::write(sdk.join("tsserver.js"), "changed! wrapper")
            .expect("same-size SDK replacement");
        runtime
            .service
            .typescript_projects
            .lock()
            .expect("cache lock")
            .clear();
        let third = launch_config(&runtime, &project.root, &project.root, &adapter);
        let third_key = LspClientKey::new(&project.root, &project.root, &third);

        assert_ne!(
            second_key, third_key,
            "SDK replacement must restart the client"
        );

        fs::remove_file(sdk.join("tsserver.js")).expect("remove SDK wrapper");
        let deleted =
            uncached_typescript_project_config(&project.root, &project.root, &adapter);
        assert_eq!(
            deleted.runtime.expect("fallback runtime").source,
            super::super::LspRuntimeSource::MissingYarnSdk
        );
        runtime
            .service
            .typescript_projects
            .lock()
            .expect("cache lock")
            .clear();
        let fallback = launch_config(&runtime, &project.root, &project.root, &adapter);
        let fallback_key = LspClientKey::new(&project.root, &project.root, &fallback);
        assert_ne!(third_key, fallback_key);
        assert!(can_reuse_for_degraded_sdk(&third_key, &fallback_key));
    }

    #[test]
    fn nested_package_discovers_workspace_root_pnp_sdk() {
        let project = Project::new("nested-monorepo");
        project.add_pnp();
        let sdk = project.add_yarn_sdk("5.8.2-sdk");
        let nested = project.root.join("packages/app");
        fs::create_dir_all(nested.join("src")).expect("create nested package");
        fs::write(nested.join("package.json"), "{}").expect("write nested package");
        fs::write(nested.join("src/index.ts"), "export {};").expect("write source");

        let config =
            uncached_typescript_project_config(&project.root, &nested, &adapter(None));

        assert_eq!(
            config
                .initialization_options
                .as_ref()
                .and_then(|value| value.pointer("/tsserver/path"))
                .and_then(Value::as_str),
            Some(sdk.to_str().expect("UTF-8 path"))
        );
        assert_eq!(
            config.runtime.expect("runtime").source,
            super::super::LspRuntimeSource::YarnSdk
        );
    }

    #[test]
    fn pnp_discovery_never_escapes_opened_workspace() {
        let project = Project::new("bounded-monorepo");
        project.add_pnp();
        project.add_yarn_sdk("5.8.2-sdk");
        let opened_workspace = project.root.join("packages/app");
        fs::create_dir_all(&opened_workspace).expect("create opened workspace");
        fs::write(opened_workspace.join("package.json"), "{}").expect("write package");

        let config = uncached_typescript_project_config(
            &opened_workspace,
            &opened_workspace,
            &adapter(None),
        );

        assert_eq!(
            config.runtime.expect("runtime").source,
            super::super::LspRuntimeSource::LanguageServerDefault
        );
        assert_eq!(config.initialization_options, None);
    }

    #[test]
    fn malformed_tsserver_options_are_preserved_and_warn() {
        let project = Project::new("malformed-options");
        project.add_pnp();
        project.add_yarn_sdk("5.8.2-sdk");
        for options in [
            json!({ "tsserver": "invalid" }),
            json!({ "tsserver": { "path": 42, "maxTsServerMemory": 1024 } }),
        ] {
            let adapter = adapter(Some(options.clone()));
            let config = uncached_typescript_project_config(
                &project.root,
                &project.root,
                &adapter,
            );
            assert_eq!(config.initialization_options, Some(options));
            assert!(config.warning.as_deref().is_some_and(|warning| {
                warning.contains("initializationOptions.tsserver")
            }));
            assert_eq!(
                config.runtime.expect("fallback runtime").source,
                super::super::LspRuntimeSource::LanguageServerDefault
            );
        }
    }

    #[test]
    fn project_probe_is_cached_between_document_operations() {
        let project = Project::new("cache");
        project.add_yarn_sdk("5.8.2-sdk");
        let runtime = super::super::LspRuntime::new(crate::standard_services());
        let adapter = adapter(None);
        let first = runtime.service.typescript_project_config(
            &project.root,
            &project.root,
            &adapter,
        );
        fs::remove_dir_all(project.root.join(".yarn")).expect("remove SDK");
        let cached = runtime.service.typescript_project_config(
            &project.root,
            &project.root,
            &adapter,
        );
        assert_eq!(first.identity, cached.identity);
        assert_eq!(
            cached.runtime.expect("cached runtime").source,
            super::super::LspRuntimeSource::YarnSdk
        );
    }
}

/// Cheap content fingerprint used to skip redundant `didChange` syncs.
fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn text_document_position_params(file: &Path, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {
            "uri": path_to_file_uri(file)
        },
        "position": {
            "line": line,
            "character": character
        }
    })
}

fn completion_request_context(
    advertised_triggers: &[String],
    typed_character: Option<&str>,
) -> Value {
    match typed_character.filter(|trigger| {
        advertised_triggers
            .iter()
            .any(|advertised| advertised == *trigger)
    }) {
        Some(trigger) => json!({
            "triggerKind": 2,
            "triggerCharacter": trigger,
        }),
        None => json!({ "triggerKind": 1 }),
    }
}

#[cfg(test)]
mod completion_context_tests {
    use super::*;

    #[test]
    fn only_server_advertised_character_uses_trigger_character_context() {
        let advertised = vec![".".to_string(), ":".to_string()];
        assert_eq!(
            completion_request_context(&advertised, Some(".")),
            json!({"triggerKind": 2, "triggerCharacter": "."})
        );
        assert_eq!(
            completion_request_context(&advertised, Some("d")),
            json!({"triggerKind": 1})
        );
        assert_eq!(
            completion_request_context(&advertised, None),
            json!({"triggerKind": 1})
        );
    }

    #[test]
    fn completion_resolve_keeps_original_edit_and_adds_lazy_fields() {
        let merged = merge_completion_item(
            json!({
                "label": "details",
                "textEdit": {
                    "range": {
                        "start": {"line": 1, "character": 2},
                        "end": {"line": 1, "character": 5}
                    },
                    "newText": "details"
                },
                "data": {"id": 4}
            }),
            json!({
                "label": "details",
                "documentation": "Resolved docs",
                "additionalTextEdits": []
            }),
        );
        assert_eq!(
            merged.pointer("/textEdit/newText").and_then(Value::as_str),
            Some("details")
        );
        assert_eq!(merged.pointer("/data/id").and_then(Value::as_u64), Some(4));
        assert_eq!(
            merged.get("documentation").and_then(Value::as_str),
            Some("Resolved docs")
        );
    }
}

#[cfg(test)]
#[path = "lsp_service_tcp_e2e_tests.rs"]
mod tcp_e2e_tests;

#[cfg(test)]
#[path = "lsp_service_e2e_tests.rs"]
mod e2e_tests;
