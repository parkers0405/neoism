use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::Context;
use neoism_agent_core::LspConfig;
use serde_json::{json, Value};

use super::{
    lsp_client::{InitializeResult, StdioLspClient},
    lsp_languages::LanguageSpec,
    lsp_parse::{
        parse_call_hierarchy_calls, parse_call_hierarchy_items, parse_completion,
        parse_diagnostics, parse_document_symbols, parse_hover, parse_locations,
        parse_workspace_symbols,
    },
    lsp_query::language_id_for_path,
    path_to_file_uri, DIAGNOSTIC_TIMEOUT, DOCUMENT_TIMEOUT, SYMBOL_TIMEOUT,
    TOUCH_DIAGNOSTIC_TIMEOUT,
};

#[derive(Default)]
pub(super) struct LspService {
    clients: Mutex<HashMap<LspClientKey, Arc<Mutex<PersistentLspClient>>>>,
    diagnostics: Mutex<HashMap<PathBuf, Vec<super::LspDiagnostic>>>,
    broken: Mutex<HashMap<LspClientKey, String>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LspClientKey {
    root: PathBuf,
    id: String,
    language: String,
    command: Vec<String>,
    env: Vec<(String, String)>,
    initialization_options: Option<String>,
}

struct PersistentLspClient {
    client: StdioLspClient,
    initialized: InitializeResult,
    language: String,
    open_versions: HashMap<PathBuf, i32>,
    /// Hash of the last text we sent the server per file, so an unchanged
    /// buffer never re-sends a `didChange`. This lets the pill/diagnostics
    /// poll run at a snappy cadence without forcing needless re-analysis
    /// every tick (a full-text `didChange` on every poll made rust-analyzer
    /// churn and kept diagnostics ~2s stale).
    synced_hashes: HashMap<PathBuf, u64>,
}

struct LspLaunchConfig {
    id: String,
    language: String,
    command: Vec<String>,
    env: BTreeMap<String, String>,
    initialization_options: Option<Value>,
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
        let clients = self.clients.lock().expect("lsp client map lock poisoned");
        let mut languages = std::collections::BTreeSet::new();
        for key in clients.keys().filter(|key| key.root == root) {
            languages.insert(key.language.clone());
            // typescript-language-server is one workspace server for both
            // TS and JS. Expose both aliases so either filetype's status pill
            // reports the shared client as attached.
            if key.language == "typescript" {
                languages.insert("javascript".to_string());
            }
        }
        languages
    }

    pub(super) fn workspace_symbols(
        &self,
        root: &Path,
        query: &str,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::WorkspaceSymbol>> {
        self.with_client(root, spec, |client| {
            if !client.initialized.workspace_symbol_provider {
                return Ok(Vec::new());
            }
            let result = client.client.request(
                "workspace/symbol",
                json!({ "query": query }),
                SYMBOL_TIMEOUT,
            )?;
            Ok(parse_workspace_symbols(root, spec.id, result))
        })
    }

    pub(super) fn hover(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspHover>> {
        self.with_client(root, spec, |client| {
            if !client.initialized.hover_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/hover",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_hover(root, file, spec.id, result)
                .into_iter()
                .collect())
        })
    }

    pub(super) fn completion(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        text: Option<&str>,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspCompletionItem>> {
        let log = std::env::var_os("NEOISM_LSP_LOG").is_some();
        self.with_client(root, spec, |client| {
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
            // Invoked (1): the user (or client heuristic) explicitly asked, as
            // opposed to a trigger-character insertion. Verbatim identifier
            // completion works the same either way for our plain-text inserts.
            params["context"] = json!({ "triggerKind": 1 });
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
            let items = parse_completion(result.clone());
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

    pub(super) fn completion_trigger_characters(
        &self,
        root: &Path,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<String>> {
        self.with_client(root, spec, |client| {
            Ok(client.initialized.completion_trigger_characters.clone())
        })
    }

    pub(super) fn definitions(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageSpec,
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
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspLocation>> {
        self.with_client(root, spec, |client| {
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
            Ok(parse_locations(root, spec.id, result))
        })
    }

    pub(super) fn implementations(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageSpec,
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
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspCallHierarchyItem>> {
        self.with_client(root, spec, |client| {
            if !client.initialized.call_hierarchy_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/prepareCallHierarchy",
                text_document_position_params(file, line, character),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_call_hierarchy_items(root, spec.id, result))
        })
    }

    pub(super) fn call_hierarchy_calls(
        &self,
        root: &Path,
        file: &Path,
        line: u32,
        character: u32,
        spec: &LanguageSpec,
        incoming: bool,
    ) -> anyhow::Result<Vec<super::LspCallHierarchyCall>> {
        self.with_client(root, spec, |client| {
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
                        root, spec.id, result, incoming,
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
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspDiagnostic>> {
        let diagnostics = self.with_client(root, spec, |client| {
            client.ensure_open(file, None)?;
            let result = client
                .client
                .wait_for_notification(
                    "textDocument/publishDiagnostics",
                    DIAGNOSTIC_TIMEOUT,
                )?
                .unwrap_or(Value::Null);
            Ok(parse_diagnostics(root, file, spec.id, result))
        })?;
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .insert(file.to_path_buf(), diagnostics.clone());
        Ok(diagnostics)
    }

    /// Overwrite the cached diagnostics for `file` from a real-time
    /// `publishDiagnostics` push, so cache readers stay fresh without a
    /// pull/`touch`.
    pub(super) fn store_diagnostics(
        &self,
        file: &Path,
        diagnostics: Vec<super::LspDiagnostic>,
    ) {
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .insert(file.to_path_buf(), diagnostics);
    }

    pub(super) fn cached_diagnostics(&self, file: &Path) -> Vec<super::LspDiagnostic> {
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .get(file)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn cached_diagnostics_snapshot(
        &self,
    ) -> Vec<(PathBuf, Vec<super::LspDiagnostic>)> {
        self.diagnostics
            .lock()
            .expect("lsp diagnostics cache lock poisoned")
            .iter()
            .map(|(path, diagnostics)| (path.clone(), diagnostics.clone()))
            .collect()
    }

    pub(super) fn formatting(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Value> {
        self.with_client(root, spec, |client| {
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
        spec: &LanguageSpec,
    ) -> anyhow::Result<Value> {
        self.with_client(root, spec, |client| {
            if !client.initialized.code_action_provider {
                return Ok(Value::Array(Vec::new()));
            }
            client.ensure_open(file, None)?;
            client.client.request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": path_to_file_uri(file) },
                    "range": {
                        "start": { "line": line, "character": character },
                        "end": { "line": line, "character": character }
                    },
                    "context": { "diagnostics": self.cached_diagnostics(file) }
                }),
                DOCUMENT_TIMEOUT,
            )
        })
    }

    pub(super) fn resolve_code_action(
        &self,
        root: &Path,
        spec: &LanguageSpec,
        action: Value,
    ) -> anyhow::Result<Value> {
        self.with_client(root, spec, |client| {
            if !client.initialized.code_action_provider {
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
        spec: &LanguageSpec,
        command: Value,
    ) -> anyhow::Result<Value> {
        self.with_client(root, spec, |client| {
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
        spec: &LanguageSpec,
    ) -> anyhow::Result<Value> {
        self.with_client(root, spec, |client| {
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
        spec: &LanguageSpec,
    ) -> anyhow::Result<()> {
        self.with_client(root, spec, |client| {
            client.ensure_open(file, text)?;
            Ok(())
        })
    }

    pub(super) fn touch(
        &self,
        root: &Path,
        file: &Path,
        text: Option<&str>,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspDiagnostic>> {
        self.with_client(root, spec, |client| {
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
                    return Ok(parse_diagnostics(root, file, spec.id, report));
                }
            }
            // Fallback for push-only servers: wait briefly for a fresh
            // `publishDiagnostics`. Bounded by TOUCH_DIAGNOSTIC_TIMEOUT so an edit
            // never eats the full 2s; late diagnostics land in the cache and are
            // surfaced on the next tool call.
            let diagnostics = client
                .client
                .wait_for_notification(
                    "textDocument/publishDiagnostics",
                    TOUCH_DIAGNOSTIC_TIMEOUT,
                )?
                .map(|result| parse_diagnostics(root, file, spec.id, result))
                .unwrap_or_default();
            Ok(diagnostics)
        })
        .inspect(|diagnostics| {
            self.diagnostics
                .lock()
                .expect("lsp diagnostics cache lock poisoned")
                .insert(file.to_path_buf(), diagnostics.clone());
        })
    }

    pub(super) fn document_symbols(
        &self,
        root: &Path,
        file: &Path,
        spec: &LanguageSpec,
    ) -> anyhow::Result<Vec<super::LspDocumentSymbol>> {
        self.with_client(root, spec, |client| {
            if !client.initialized.document_symbol_provider {
                return Ok(Vec::new());
            }
            client.ensure_open(file, None)?;
            let result = client.client.request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": path_to_file_uri(file) } }),
                DOCUMENT_TIMEOUT,
            )?;
            Ok(parse_document_symbols(root, file, spec.id, result))
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
        spec: &LanguageSpec,
        capability: &str,
        method: &str,
    ) -> anyhow::Result<Vec<super::LspLocation>> {
        self.with_client(root, spec, |client| {
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
            Ok(parse_locations(root, spec.id, result))
        })
    }

    fn with_client<T>(
        &self,
        root: &Path,
        spec: &LanguageSpec,
        operation: impl FnOnce(&mut PersistentLspClient) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let launch = launch_config(root, spec);
        let key = LspClientKey::new(root, &launch);
        let client = self.client(root, &key, &launch).inspect_err(|error| {
            self.record_broken(&key, error.to_string());
        })?;
        let mut guard = client.lock().expect("lsp client lock poisoned");
        if !guard.client.is_running() {
            drop(guard);
            self.evict_client(&key);
            anyhow::bail!("persistent LSP server {} is no longer running", key.id);
        }
        let result = operation(&mut guard);
        drop(guard);
        if let Err(error) = &result {
            self.record_broken(&key, error.to_string());
            self.evict_client(&key);
        }
        result
    }

    fn client(
        &self,
        root: &Path,
        key: &LspClientKey,
        launch: &LspLaunchConfig,
    ) -> anyhow::Result<Arc<Mutex<PersistentLspClient>>> {
        let mut clients = self.clients.lock().expect("lsp client map lock poisoned");
        if let Some(client) = clients.get(key) {
            self.broken
                .lock()
                .expect("lsp broken-client map lock poisoned")
                .remove(key);
            return Ok(client.clone());
        }
        let mut client = StdioLspClient::spawn_with_env(
            root,
            &launch.language,
            &key.command,
            &launch.env,
        )?;
        let initialized = client
            .initialize_with_options(root, launch.initialization_options.clone())?;
        let persistent = Arc::new(Mutex::new(PersistentLspClient {
            client,
            initialized,
            language: launch.language.clone(),
            open_versions: HashMap::new(),
            synced_hashes: HashMap::new(),
        }));
        clients.insert(key.clone(), persistent.clone());
        self.broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .remove(key);
        Ok(persistent)
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

    fn record_broken(&self, key: &LspClientKey, reason: String) {
        self.broken
            .lock()
            .expect("lsp broken-client map lock poisoned")
            .insert(key.clone(), reason);
    }
}

impl PersistentLspClient {
    fn ensure_open(&mut self, file: &Path, text: Option<&str>) -> anyhow::Result<()> {
        let language_id = language_id_for_path(&self.language, file);
        match self.open_versions.get_mut(file) {
            Some(version) => {
                // Already open. Only re-sync when the caller supplies fresh
                // text (a live buffer edit). With `None` we must NOT re-read
                // the file from disk — that would clobber the live in-memory
                // text with the stale on-disk version, so hover/completion/etc.
                // would query the wrong content and return nothing.
                if let Some(text) = text {
                    // Skip the didChange when the buffer is byte-identical to
                    // what the server already has — otherwise the poll re-sends
                    // the full text every tick and forces a needless
                    // re-analysis. Deduping here is what makes a fast poll safe.
                    let hash = text_hash(text);
                    if self.synced_hashes.get(file) != Some(&hash) {
                        *version += 1;
                        self.client.change_document(file, *version, text)?;
                        self.synced_hashes.insert(file.to_path_buf(), hash);
                    }
                }
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
                    .open_document_with_text(file, language_id, &text)?;
                self.open_versions.insert(file.to_path_buf(), 0);
                self.synced_hashes
                    .insert(file.to_path_buf(), text_hash(&text));
                Ok(())
            }
        }
    }
}

impl LspClientKey {
    fn new(root: &Path, launch: &LspLaunchConfig) -> Self {
        Self {
            root: root.to_path_buf(),
            id: launch.id.clone(),
            language: launch.language.clone(),
            command: launch.command.clone(),
            env: launch
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            initialization_options: launch
                .initialization_options
                .as_ref()
                .map(|value| value.to_string()),
        }
    }
}

fn launch_config(root: &Path, spec: &LanguageSpec) -> LspLaunchConfig {
    // JavaScript and TypeScript are protocols of the same
    // typescript-language-server process. Canonicalizing the built-in key
    // prevents one workspace from spawning two servers (and two tsserver
    // trees) merely because the user opened one `.js` and one `.ts` file.
    // Explicit per-language config below still gets its own id/key.
    let builtin_family = if spec.id == "javascript" {
        "typescript"
    } else {
        spec.id
    };
    let mut launch = LspLaunchConfig {
        id: builtin_family.to_string(),
        language: builtin_family.to_string(),
        command: spec
            .command
            .iter()
            .map(|part| (*part).to_string())
            .collect(),
        env: BTreeMap::new(),
        initialization_options: default_initialization_options(spec.id),
    };
    if let Ok(loaded) = crate::config::load(&root.to_string_lossy()) {
        if let LspConfig::Servers(servers) = loaded.info.lsp {
            if let Some((id, object)) = servers.into_iter().find_map(|(id, value)| {
                configured_server_object(spec.id, &id, value).map(|object| (id, object))
            }) {
                launch.id = id.clone();
                launch.language = object
                    .get("language")
                    .or_else(|| object.get("languageId"))
                    .and_then(Value::as_str)
                    .unwrap_or(spec.id)
                    .to_string();
                if let Some(command) =
                    object.get("command").and_then(configured_command_value)
                {
                    launch.command = command;
                }
                launch.env = object
                    .get("env")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect();
                if let Some(options) = object
                    .get("initialization")
                    .or_else(|| object.get("initializationOptions"))
                    .cloned()
                {
                    launch.initialization_options = Some(options);
                }
            }
        }
    }
    // ONE authoritative, traceable record of which binary services this server
    // and why it was picked. There are commonly several rust-analyzers on a host
    // (a rustup shim on PATH, a neoism-managed extension binary, a nix copy);
    // without this line a silent fall-through to a non-working PATH shim is
    // invisible. Logged under NEOISM_LSP_LOG alongside the rest of the attach.
    let (resolved, source) = crate::lsp::resolve_lsp_command(&launch.id, launch.command);
    launch.command = resolved;
    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
        let bin = launch
            .command
            .first()
            .map(String::as_str)
            .unwrap_or("<none>");
        eprintln!(
            "neoism::lsp resolve[{}]: source={:?} bin={}",
            launch.id, source, bin,
        );
        if source == crate::lsp::LspCommandSource::Path {
            eprintln!(
                "neoism::lsp resolve[{}]: WARNING resolved from PATH — a bare `rust-analyzer` on \
                 PATH is often a rustup proxy that is NOT a working LSP server (prints \
                 \"unavailable for the active toolchain\" and exits). Install it from the \
                 extensions page so a managed binary is used instead.",
                launch.id,
            );
        }
    }
    launch
}

fn default_initialization_options(language: &str) -> Option<Value> {
    if language == "rust" {
        // Match Zed's out-of-the-box rust-analyzer behavior: do not inject a
        // hard-coded initializationOptions block. rust-analyzer's own defaults
        // stay authoritative, and users/project config can still provide
        // explicit options via the `initialization_options` /
        // `initializationOptions` server config below.
    }
    None
}

fn configured_server_object(
    language: &str,
    id: &str,
    value: Value,
) -> Option<serde_json::Map<String, Value>> {
    if value.as_bool() == Some(false) {
        return None;
    }
    let object = value.as_object()?.clone();
    if object.get("enabled").and_then(Value::as_bool) == Some(false)
        || object.get("disabled").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let configured_language = object
        .get("language")
        .or_else(|| object.get("languageId"))
        .and_then(Value::as_str)
        .unwrap_or(id);
    (id == language || configured_language == language).then_some(object)
}

fn configured_command_value(value: &Value) -> Option<Vec<String>> {
    if let Some(command) = value.as_str() {
        let parts = command
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        return (!parts.is_empty()).then_some(parts);
    }
    let parts = value
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!parts.is_empty()).then_some(parts)
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
