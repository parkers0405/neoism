use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
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

#[derive(Default)]
pub(super) struct LspService {
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
    settings: Option<String>,
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
    settings: Option<Value>,
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

pub(super) fn service() -> &'static LspService {
    static SERVICE: OnceLock<LspService> = OnceLock::new();
    SERVICE.get_or_init(LspService::default)
}

impl LspService {
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
        let project_root = server_root_for_file(workspace_root, file, spec);
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
        let launch = launch_config(spec);
        let key = LspClientKey::new(workspace_root, project_root, &launch);
        let client = self
            .clients
            .lock()
            .expect("lsp client map lock poisoned")
            .get(&key)
            .cloned();
        let Some(client) = client else {
            return false;
        };
        let reason = client
            .lock()
            .ok()
            .and_then(|mut client| client.client.exit_reason());
        if let Some(reason) = reason {
            self.record_broken(&key, reason);
            self.evict_client(&key);
            false
        } else {
            true
        }
    }

    fn broken_reason_at(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        spec: &LanguageAdapter,
    ) -> Option<String> {
        let launch = launch_config(spec);
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

    pub(super) fn touch(
        &self,
        root: &Path,
        file: &Path,
        text: Option<&str>,
        spec: &LanguageAdapter,
    ) -> anyhow::Result<Vec<super::LspDiagnostic>> {
        let pulled = self.with_file_client(root, file, spec, |client| {
            client.ensure_open(file, text)?;
            client.client.save_document(file)?;
            // Fast path: pull diagnostics (`textDocument/diagnostic`) when the
            // server supports it. This returns whatever is ready immediately
            // instead of stalling on a flycheck push — the opencode model.
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
            // it may contain an older publish queued before this save, which
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
        let project_root = server_root_for_file(workspace_root, file, spec);
        self.with_client(workspace_root, &project_root, spec, operation)
    }

    fn with_client<T>(
        &self,
        workspace_root: &Path,
        project_root: &Path,
        spec: &LanguageAdapter,
        operation: impl FnOnce(&mut PersistentLspClient) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let launch = launch_config(spec);
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
        let service = LspService::default();
        let root = Path::new("/tmp/neoism-broken-lsp-status");
        let spec = LanguageAdapter::from_builtin(
            &super::super::lsp_languages::LANGUAGE_SPECS[0],
        );
        let launch = launch_config(&spec);
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
                        service().record_document_version(
                            &self.root,
                            file,
                            &self.server_id,
                            &logical_language,
                            next_version,
                        );
                        if let Err(error) =
                            self.client.change_document(file, next_version, text)
                        {
                            service().record_document_version(
                                &self.root,
                                file,
                                &self.server_id,
                                &logical_language,
                                *version,
                            );
                            return Err(error);
                        }
                        *version = next_version;
                        self.synced_hashes.insert(file.to_path_buf(), hash);
                    }
                }
                service().record_document_version(
                    &self.root,
                    file,
                    &self.server_id,
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
                service().record_document_version(
                    &self.root,
                    file,
                    &self.server_id,
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
            settings: launch.settings.as_ref().map(Value::to_string),
        }
    }
}

fn launch_config(adapter: &LanguageAdapter) -> LspLaunchConfig {
    let endpoint = match &adapter.transport {
        ResolvedLspTransport::Stdio { command, env } => {
            let (command, source) =
                crate::lsp::resolve_lsp_command(&adapter.id, command.clone());
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
    LspLaunchConfig {
        id: adapter.id.clone(),
        adapter_id: adapter.id.clone(),
        routes: adapter.routes.clone(),
        endpoint,
        initialization_options: adapter.initialization_options.clone(),
        settings: adapter.settings.clone(),
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
