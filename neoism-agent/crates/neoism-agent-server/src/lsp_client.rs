use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::OsStr,
    io::{self, BufRead, BufReader, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use std::fs;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::managed_lsp_path::managed_lsp_path;

use super::{
    capability_enabled,
    lsp_adapters::ResolvedLanguageRoute,
    lsp_position::{
        byte_column_to_protocol, client_value_to_protocol,
        client_value_to_protocol_for_file, document_end_byte_position,
        server_value_to_bytes, value_document_path, PositionEncoding,
    },
    path_to_file_uri, INITIALIZE_TIMEOUT, SHUTDOWN_TIMEOUT, TCP_CONNECT_TIMEOUT,
};

pub(crate) struct InitializeResult {
    pub(crate) workspace_symbol_provider: bool,
    pub(crate) completion_provider: bool,
    pub(crate) completion_resolve_provider: bool,
    pub(crate) completion_trigger_characters: Vec<String>,
    pub(crate) hover_provider: bool,
    pub(crate) definition_provider: bool,
    pub(crate) references_provider: bool,
    pub(crate) implementation_provider: bool,
    pub(crate) call_hierarchy_provider: bool,
    pub(crate) document_symbol_provider: bool,
    pub(crate) formatting_provider: bool,
    pub(crate) code_action_provider: bool,
    pub(crate) code_action_resolve_provider: bool,
    pub(crate) rename_provider: bool,
    pub(crate) diagnostic_provider: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TextDocumentSyncKind {
    #[default]
    None,
    Full,
    Incremental,
}

impl TextDocumentSyncKind {
    fn from_number(kind: u64) -> Self {
        match kind {
            1 => Self::Full,
            2 => Self::Incremental,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TextDocumentSyncCapabilities {
    change: TextDocumentSyncKind,
    open_close: bool,
    save: bool,
    save_include_text: bool,
}

impl TextDocumentSyncCapabilities {
    fn from_server_capability(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        if let Some(kind) = value.as_u64() {
            let change = TextDocumentSyncKind::from_number(kind);
            // The legacy numeric form predates TextDocumentSyncOptions. A
            // non-None kind still establishes a didOpen/didClose lifecycle,
            // but it does not advertise didSave.
            return Self {
                change,
                open_close: change != TextDocumentSyncKind::None,
                save: false,
                save_include_text: false,
            };
        }
        let Some(options) = value.as_object() else {
            return Self::default();
        };
        let change = options
            .get("change")
            .and_then(Value::as_u64)
            .map(TextDocumentSyncKind::from_number)
            .unwrap_or_default();
        let open_close = options
            .get("openClose")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (save, save_include_text) = match options.get("save") {
            Some(Value::Bool(enabled)) => (*enabled, false),
            Some(Value::Object(save)) => (
                true,
                save.get("includeText")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            _ => (false, false),
        };
        Self {
            change,
            open_close,
            save,
            save_include_text,
        }
    }
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

enum LspTransportHandle {
    Stdio {
        child: Child,
        stderr_tail: Arc<Mutex<VecDeque<String>>>,
    },
    Tcp {
        socket: TcpStream,
        address: String,
    },
}

#[derive(Default)]
struct TransportHealth {
    failure: Mutex<Option<String>>,
}

/// One JSON-RPC/LSP client independent of its byte transport. Both spawned
/// stdio servers and externally-owned TCP servers use the same router,
/// request ids, server-request replies, diagnostics dispatch, and lifecycle.
pub(crate) struct LspClient {
    transport: LspTransportHandle,
    writer: SharedWriter,
    responses: Receiver<Value>,
    notifications: Receiver<Value>,
    configuration: Arc<Mutex<Option<Value>>>,
    /// Current full document snapshots. Neoism/editor positions are UTF-8 byte
    /// columns, while servers may negotiate UTF-8, UTF-16, or UTF-32. Keeping
    /// the live text here lets both transport threads convert every nested LSP
    /// Position at the protocol boundary without consulting stale disk state.
    documents: Arc<Mutex<HashMap<PathBuf, String>>>,
    position_encoding: Arc<Mutex<PositionEncoding>>,
    text_document_sync: TextDocumentSyncCapabilities,
    health: Arc<TransportHealth>,
    next_id: i64,
    label: String,
}

/// Compatibility name used by the one-shot query helpers. The implementation
/// is transport-neutral; new persistent callers should use [`LspClient`].
#[cfg(test)]
pub(crate) type StdioLspClient = LspClient;

impl LspClient {
    #[cfg(test)]
    pub(crate) fn spawn(root: &Path, command: &[String]) -> Result<Self> {
        Self::spawn_with_env(root, root, "", "", &[], command, &BTreeMap::new())
    }

    pub(crate) fn spawn_with_env(
        project_root: &Path,
        workspace_root: &Path,
        server_id: &str,
        adapter_id: &str,
        routes: &[ResolvedLanguageRoute],
        command: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| anyhow!("LSP command is empty"))?;
        let mut command_proc = Command::new(program);
        command_proc.args(args).current_dir(project_root).envs(env);
        if !env.contains_key("PATH") {
            if let Some(path) = managed_lsp_path() {
                command_proc.env("PATH", path);
            }
        }
        let label = command.join(" ");
        let spawn_started = crate::perf::now();
        tracing::info!(
            target: "neoism_agent::perf",
            command = %program,
            args = ?args,
            root = %project_root.display(),
            "lsp process spawning"
        );
        let mut child = command_proc
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!("failed to spawn LSP command `{}`", command.join(" "))
            })?;
        tracing::info!(
            target: "neoism_agent::perf",
            lsp = %label,
            pid = child.id(),
            spawn_ms = crate::perf::elapsed_ms(spawn_started),
            "lsp process spawned"
        );

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture LSP stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture LSP stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("failed to capture LSP stderr"))?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(20)));
        let stderr_label = label.clone();
        let stderr_tail_writer = Arc::clone(&stderr_tail);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut tail) = stderr_tail_writer.lock() {
                    if tail.len() == 20 {
                        tail.pop_front();
                    }
                    tail.push_back(line.clone());
                }
                if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                    eprintln!("neoism::lsp stderr[{stderr_label}]: {line}");
                }
            }
        });

        Self::from_transport(
            project_root,
            workspace_root,
            server_id,
            adapter_id,
            routes,
            label,
            Box::new(BufReader::new(stdout)),
            Box::new(stdin),
            LspTransportHandle::Stdio { child, stderr_tail },
        )
    }

    /// Connect to an externally-owned language server over TCP. Connection
    /// loss is recorded by the shared reader and causes the service to evict
    /// this client; the next operation creates a fresh connection.
    pub(crate) fn connect_tcp(
        project_root: &Path,
        workspace_root: &Path,
        server_id: &str,
        adapter_id: &str,
        routes: &[ResolvedLanguageRoute],
        host: &str,
        port: u16,
    ) -> Result<Self> {
        let address_label = format!("{host}:{port}");
        let addresses = (host, port).to_socket_addrs().with_context(|| {
            format!("failed to resolve LSP TCP endpoint {address_label}")
        })?;
        let mut last_error = None;
        let mut connected = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, TCP_CONNECT_TIMEOUT) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let stream = connected.ok_or_else(|| {
            anyhow!(
                "failed to connect to LSP TCP endpoint {address_label}: {}",
                last_error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "endpoint resolved to no addresses".to_string())
            )
        })?;
        stream.set_nodelay(true).with_context(|| {
            format!("failed to configure LSP TCP endpoint {address_label}")
        })?;
        let reader = stream.try_clone().with_context(|| {
            format!("failed to clone LSP TCP reader for {address_label}")
        })?;
        let writer = stream.try_clone().with_context(|| {
            format!("failed to clone LSP TCP writer for {address_label}")
        })?;
        let label = format!("tcp://{address_label}");
        tracing::info!(
            target: "neoism_agent::perf",
            lsp = %label,
            root = %project_root.display(),
            "lsp tcp connection established"
        );
        Self::from_transport(
            project_root,
            workspace_root,
            server_id,
            adapter_id,
            routes,
            label,
            Box::new(BufReader::new(reader)),
            Box::new(writer),
            LspTransportHandle::Tcp {
                socket: stream,
                address: address_label,
            },
        )
    }

    fn from_transport(
        project_root: &Path,
        workspace_root: &Path,
        server_id: &str,
        adapter_id: &str,
        routes: &[ResolvedLanguageRoute],
        label: String,
        mut reader: Box<dyn BufRead + Send>,
        writer: Box<dyn Write + Send>,
        transport: LspTransportHandle,
    ) -> Result<Self> {
        let writer = Arc::new(Mutex::new(writer));
        let configuration = Arc::new(Mutex::new(None));
        let documents = Arc::new(Mutex::new(HashMap::new()));
        // LSP 3.17 requires clients to support UTF-16 and defines it as the
        // default when a server omits capabilities.positionEncoding.
        let position_encoding = Arc::new(Mutex::new(PositionEncoding::Utf16));
        let health = Arc::new(TransportHealth::default());
        let (response_sender, responses) = mpsc::channel();
        // Diagnostics payloads are dispatched into the version-guarded cache
        // on the reader thread. This channel is only a wake-up signal for an
        // explicit diagnostics wait, so one slot is sufficient and prevents
        // an unbounded queue during long editing sessions.
        let (notification_sender, notifications) = mpsc::sync_channel(1);
        let reader_root = workspace_root.to_path_buf();
        let reader_server_id = server_id.to_string();
        let reader_routes = routes.to_vec();
        let reader_root_uri = super::path_to_file_uri(project_root);
        let reader_root_name = project_root
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("workspace")
            .to_string();
        let reader_adapter_id = adapter_id.to_string();
        let reader_writer = Arc::clone(&writer);
        let reader_configuration = Arc::clone(&configuration);
        let reader_documents = Arc::clone(&documents);
        let reader_position_encoding = Arc::clone(&position_encoding);
        let reader_health = Arc::clone(&health);
        let reader_label = label.clone();
        thread::spawn(move || {
            loop {
                match read_lsp_message(&mut reader) {
                    Ok(Some(mut message)) => {
                        let method = message
                            .get("method")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        // Requests initiated by the server are not ordinary
                        // notifications and must receive a JSON-RPC response.
                        // In particular rust-analyzer waits for
                        // `workspace/configuration` before loading/checking a
                        // workspace. The old single-queue implementation let
                        // the next client request silently discard these
                        // messages, so hover could work from syntax data while
                        // compiler diagnostics remained empty forever.
                        if method.is_some() && message.get("id").is_some() {
                            let response = server_request_response(
                                &message,
                                &reader_root_uri,
                                &reader_root_name,
                                reader_configuration
                                    .lock()
                                    .ok()
                                    .and_then(|settings| settings.clone()),
                            );
                            let write_result = reader_writer
                                .lock()
                                .map_err(|_| io::Error::other("LSP stdin lock poisoned"))
                                .and_then(|mut stdin| {
                                    write_lsp_message(&mut *stdin, &response)
                                });
                            if let Err(error) = write_result {
                                tracing::debug!(
                                    error = %error,
                                    method = method.as_deref().unwrap_or_default(),
                                    "failed to answer LSP server request"
                                );
                                record_transport_failure(
                                    &reader_health,
                                    format!(
                                        "LSP transport `{reader_label}` write failed: {error}"
                                    ),
                                );
                                break;
                            }
                            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                                eprintln!(
                                    "neoism::lsp client[{reader_adapter_id}]: answered server request {}",
                                    method.as_deref().unwrap_or_default()
                                );
                            }
                            continue;
                        }
                        // Normalize server notifications before diagnostics or
                        // any notification consumer sees them. Responses are
                        // normalized by the request waiter, which knows the
                        // originating document for URI-less ranges (hover,
                        // formatting edits, call hierarchy, and so on).
                        if method.is_some() {
                            let encoding = reader_position_encoding
                                .lock()
                                .map(|encoding| *encoding)
                                .unwrap_or_default();
                            if let Ok(mut documents) = reader_documents.lock() {
                                server_value_to_bytes(
                                    &mut message,
                                    None,
                                    &mut documents,
                                    encoding,
                                );
                            }
                        }
                        // Tap `publishDiagnostics` for the real-time event bus
                        // before handing the message to notification waiters.
                        if method.as_deref() == Some("textDocument/publishDiagnostics") {
                            if !reader_server_id.is_empty() {
                                if let Some(params) = message.get("params") {
                                    crate::lsp::dispatch_diagnostics(
                                        &reader_root,
                                        &reader_server_id,
                                        &reader_routes,
                                        params,
                                    );
                                }
                            }
                        }
                        // Surface indexing/loading progress so it's clear
                        // rust-analyzer is warming up (completions/diagnostics
                        // only start once the workspace is loaded — minutes on a
                        // big project, especially first build-script/proc-macro
                        // compile).
                        if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                            if method.as_deref() == Some("$/progress") {
                                if let Some(value) = message.pointer("/params/value") {
                                    let kind = value
                                        .get("kind")
                                        .and_then(Value::as_str)
                                        .unwrap_or("");
                                    let title = value
                                        .get("title")
                                        .or_else(|| value.get("message"))
                                        .and_then(Value::as_str)
                                        .unwrap_or("");
                                    let pct =
                                        value.get("percentage").and_then(Value::as_u64);
                                    eprintln!(
                                        "neoism::lsp progress[{reader_adapter_id}]: {kind} {title} {pct:?}"
                                    );
                                }
                            } else if method.as_deref() == Some("window/logMessage")
                                || method.as_deref() == Some("window/showMessage")
                            {
                                if let Some(text) = message
                                    .pointer("/params/message")
                                    .and_then(Value::as_str)
                                {
                                    eprintln!(
                                        "neoism::lsp server[{reader_adapter_id}]: {text}"
                                    );
                                }
                            }
                        }
                        // Responses and notifications have separate queues.
                        // A request waiter must never consume an unrelated
                        // notification (or vice versa).
                        if method.as_deref() == Some("textDocument/publishDiagnostics") {
                            match notification_sender.try_send(message) {
                                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                                Err(mpsc::TrySendError::Disconnected(_)) => break,
                            }
                        } else if message.get("id").is_some()
                            && response_sender.send(message).is_err()
                        {
                            break;
                        }
                    }
                    Ok(None) => {
                        record_transport_failure(
                            &reader_health,
                            format!("LSP transport `{reader_label}` closed"),
                        );
                        break;
                    }
                    Err(error) => {
                        tracing::debug!(error = %error, "failed to read LSP message");
                        record_transport_failure(
                            &reader_health,
                            format!(
                                "LSP transport `{reader_label}` read failed: {error}"
                            ),
                        );
                        break;
                    }
                }
            }
        });

        Ok(Self {
            transport,
            writer,
            responses,
            notifications,
            configuration,
            documents,
            position_encoding,
            text_document_sync: TextDocumentSyncCapabilities::default(),
            health,
            next_id: 1,
            label,
        })
    }

    #[cfg(test)]
    pub(crate) fn initialize(&mut self, root: &Path) -> Result<InitializeResult> {
        self.initialize_with_configuration(root, None, None)
    }

    pub(crate) fn initialize_with_configuration(
        &mut self,
        root: &Path,
        initialization_options: Option<Value>,
        settings: Option<Value>,
    ) -> Result<InitializeResult> {
        *self
            .configuration
            .lock()
            .map_err(|_| anyhow!("LSP configuration lock poisoned"))? = settings.clone();
        let root_uri = path_to_file_uri(root);
        let mut params = json!({
            "processId": null,
            "rootPath": root.display().to_string(),
            "rootUri": root_uri,
            "capabilities": {
                "general": {
                    // Preference order, not a unilateral choice. LSP requires
                    // UTF-16 support, and servers may select any advertised
                    // encoding in capabilities.positionEncoding.
                    "positionEncodings": ["utf-8", "utf-16", "utf-32"]
                },
                "workspace": {
                    // These requests are implemented by the reader-thread
                    // server-request router below. Advertising them is what
                    // allows servers to rely on the returned settings/folder
                    // instead of silently falling back to an empty workspace.
                    "configuration": true,
                    "workspaceFolders": true,
                    "executeCommand": {
                        "dynamicRegistration": false
                    },
                    "symbol": {
                        "dynamicRegistration": false,
                        "symbolKind": {
                            "valueSet": (1..=26).collect::<Vec<u8>>()
                        }
                    }
                },
                "textDocument": {
                    "completion": {
                        "dynamicRegistration": false,
                        "contextSupport": true,
                        "completionItem": {
                            // Snippets remain disabled until Neoism owns a real
                            // tab-stop session. Range edits, lazy resolve and
                            // list defaults are fully supported independently.
                            "snippetSupport": false,
                            "insertReplaceSupport": true,
                            "labelDetailsSupport": true,
                            "documentationFormat": ["markdown", "plaintext"],
                            "deprecatedSupport": true,
                            "preselectSupport": true,
                            "resolveSupport": {
                                "properties": [
                                    "documentation",
                                    "detail",
                                    "additionalTextEdits",
                                    "textEdit",
                                    "insertText",
                                    "insertTextFormat"
                                ]
                            }
                        },
                        "completionItemKind": {
                            "valueSet": (1..=25).collect::<Vec<u8>>()
                        },
                        "completionList": {
                            "itemDefaults": ["editRange", "insertTextFormat", "data"]
                        }
                    },
                    "hover": {
                        "dynamicRegistration": false,
                        "contentFormat": ["markdown", "plaintext"]
                    },
                    // Push diagnostics are a client capability, independent
                    // of the 3.17 pull-diagnostic request capability below.
                    // Servers such as typescript-language-server deliberately
                    // disable their publishDiagnostics pipeline when this is
                    // absent even though the transport is otherwise healthy.
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "tagSupport": { "valueSet": [1, 2] },
                        "versionSupport": true,
                        "codeDescriptionSupport": true,
                        "dataSupport": true
                    },
                    "definition": {
                        "dynamicRegistration": false,
                        "linkSupport": true
                    },
                    "references": {
                        "dynamicRegistration": false
                    },
                    "implementation": {
                        "dynamicRegistration": false,
                        "linkSupport": true
                    },
                    "callHierarchy": {
                        "dynamicRegistration": false
                    },
                    "documentSymbol": {
                        "dynamicRegistration": false,
                        "hierarchicalDocumentSymbolSupport": true,
                        "symbolKind": {
                            "valueSet": (1..=26).collect::<Vec<u8>>()
                        }
                    },
                    "formatting": {
                        "dynamicRegistration": false
                    },
                    "rename": {
                        // Neoism applies the returned WorkspaceEdit directly,
                        // but does not issue textDocument/prepareRename yet.
                        "dynamicRegistration": false,
                        "prepareSupport": false,
                        "honorsChangeAnnotations": false
                    },
                    "codeAction": {
                        "dynamicRegistration": false,
                        // The picker preserves and displays all of these
                        // fields, and resolves edit/command lazily against the
                        // exact server that produced the action.
                        "isPreferredSupport": true,
                        "disabledSupport": true,
                        "dataSupport": true,
                        "resolveSupport": {
                            "properties": ["edit", "command"]
                        },
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": [
                                    "",
                                    "quickfix",
                                    "refactor",
                                    "refactor.extract",
                                    "refactor.inline",
                                    "refactor.rewrite",
                                    "source",
                                    "source.organizeImports",
                                    "source.fixAll"
                                ]
                            }
                        }
                    },
                    "synchronization": {
                        // Open/change/close are mandatory pre-3.x behavior;
                        // TextDocumentSyncClientCapabilities only defines
                        // dynamicRegistration, willSave, willSaveWaitUntil,
                        // and didSave. Do not send invented capability keys.
                        "dynamicRegistration": false,
                        "didSave": true
                    },
                    "diagnostic": {
                        "dynamicRegistration": false,
                        "relatedDocumentSupport": false
                    }
                }
            },
            "workspaceFolders": [{
                "uri": root_uri,
                "name": root.file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("workspace")
            }]
        });
        if let Some(options) = initialization_options {
            params["initializationOptions"] = options;
        }
        let initialized_started = crate::perf::now();
        let result = self.request("initialize", params, INITIALIZE_TIMEOUT)?;
        let negotiated_encoding = PositionEncoding::from_server_name(
            result
                .pointer("/capabilities/positionEncoding")
                .and_then(Value::as_str),
        );
        *self
            .position_encoding
            .lock()
            .map_err(|_| anyhow!("LSP position encoding lock poisoned"))? =
            negotiated_encoding;
        self.text_document_sync = TextDocumentSyncCapabilities::from_server_capability(
            result.pointer("/capabilities/textDocumentSync"),
        );
        self.notify("initialized", json!({}))?;
        if let Some(settings) = settings {
            self.notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": settings }),
            )?;
        }

        let workspace_symbol_provider = result
            .pointer("/capabilities/workspaceSymbolProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        // completionProvider is an object (or absent) — a bare bool is never
        // used, so presence of a non-null value means the server completes.
        let completion_provider = result
            .pointer("/capabilities/completionProvider")
            .map(|value| !value.is_null())
            .unwrap_or(false);
        let completion_resolve_provider = result
            .pointer("/capabilities/completionProvider/resolveProvider")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let completion_trigger_characters = result
            .pointer("/capabilities/completionProvider/triggerCharacters")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let hover_provider = result
            .pointer("/capabilities/hoverProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let definition_provider = result
            .pointer("/capabilities/definitionProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let references_provider = result
            .pointer("/capabilities/referencesProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let implementation_provider = result
            .pointer("/capabilities/implementationProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let call_hierarchy_provider = result
            .pointer("/capabilities/callHierarchyProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let document_symbol_provider = result
            .pointer("/capabilities/documentSymbolProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let formatting_provider = result
            .pointer("/capabilities/documentFormattingProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let code_action_provider = result
            .pointer("/capabilities/codeActionProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        let code_action_resolve_provider = result
            .pointer("/capabilities/codeActionProvider/resolveProvider")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let rename_provider = result
            .pointer("/capabilities/renameProvider")
            .map(capability_enabled)
            .unwrap_or(false);
        // Pull diagnostics (`textDocument/diagnostic`, LSP 3.17) — only advertised
        // by servers that opt in (rust-analyzer, gopls, …). Absent on most others,
        // so default false and fall back to the push wait when missing.
        let diagnostic_provider = result
            .pointer("/capabilities/diagnosticProvider")
            .map(capability_enabled)
            .unwrap_or(false);

        tracing::info!(
            target: "neoism_agent::perf",
            lsp = %self.label,
            initialize_ms = crate::perf::elapsed_ms(initialized_started),
            workspace_symbol_provider,
            hover_provider,
            definition_provider,
            references_provider,
            implementation_provider,
            document_symbol_provider,
            formatting_provider,
            code_action_provider,
            code_action_resolve_provider,
            rename_provider,
            diagnostic_provider,
            position_encoding = negotiated_encoding.protocol_name(),
            text_document_sync = ?self.text_document_sync,
            "lsp initialized"
        );

        Ok(InitializeResult {
            workspace_symbol_provider,
            completion_provider,
            completion_resolve_provider,
            completion_trigger_characters,
            hover_provider,
            definition_provider,
            references_provider,
            implementation_provider,
            call_hierarchy_provider,
            document_symbol_provider,
            formatting_provider,
            code_action_provider,
            code_action_resolve_provider,
            rename_provider,
            diagnostic_provider,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_document(&mut self, file: &Path, language_id: &str) -> Result<()> {
        let text = fs::read_to_string(file)
            .with_context(|| format!("failed to read document {}", file.display()))?;
        self.open_document_with_text(file, language_id, &text)
    }

    pub(crate) fn open_document_with_text(
        &mut self,
        file: &Path,
        language_id: &str,
        text: &str,
    ) -> Result<()> {
        self.documents
            .lock()
            .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?
            .insert(file.to_path_buf(), text.to_string());
        if self.text_document_sync.open_close {
            self.notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": path_to_file_uri(file),
                        "languageId": language_id,
                        "version": 0,
                        "text": text,
                    }
                }),
            )?;
        }
        Ok(())
    }

    pub(crate) fn change_document(
        &mut self,
        file: &Path,
        version: i32,
        text: &str,
    ) -> Result<()> {
        let previous = self
            .documents
            .lock()
            .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?
            .get(file)
            .cloned()
            .unwrap_or_default();
        match self.text_document_sync.change {
            TextDocumentSyncKind::None => {}
            TextDocumentSyncKind::Full => {
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {
                            "uri": path_to_file_uri(file),
                            "version": version,
                        },
                        "contentChanges": [{ "text": text }],
                    }),
                )?;
            }
            TextDocumentSyncKind::Incremental => {
                let encoding = self
                    .position_encoding
                    .lock()
                    .map(|encoding| *encoding)
                    .map_err(|_| anyhow!("LSP position encoding lock poisoned"))?;
                let (end_line, end_byte) = document_end_byte_position(&previous);
                let end_character =
                    byte_column_to_protocol(&previous, end_line, end_byte, encoding);
                // A whole-document replacement with an explicit range is a
                // valid incremental change and avoids an invalid range-less
                // payload for servers that advertise Incremental (2).
                self.notify_raw(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {
                            "uri": path_to_file_uri(file),
                            "version": version,
                        },
                        "contentChanges": [{
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": {
                                    "line": end_line,
                                    "character": end_character,
                                },
                            },
                            "text": text,
                        }],
                    }),
                )?;
            }
        }
        self.documents
            .lock()
            .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?
            .insert(file.to_path_buf(), text.to_string());
        Ok(())
    }

    pub(crate) fn save_document(&mut self, file: &Path) -> Result<()> {
        if self.text_document_sync.save {
            let mut params = json!({ "textDocument": { "uri": path_to_file_uri(file) } });
            if self.text_document_sync.save_include_text {
                let text = self
                    .documents
                    .lock()
                    .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?
                    .get(file)
                    .cloned()
                    .unwrap_or_default();
                params["text"] = Value::from(text);
            }
            self.notify("textDocument/didSave", params)?;
        }
        Ok(())
    }

    pub(crate) fn close_document(&mut self, file: &Path) -> Result<()> {
        if self.text_document_sync.open_close {
            self.notify(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": path_to_file_uri(file) } }),
            )?;
        }
        self.documents
            .lock()
            .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?
            .remove(file);
        Ok(())
    }

    /// Pull-model diagnostics (`textDocument/diagnostic`, LSP 3.17). Returns the
    /// report's `items` array so the caller can reuse `parse_diagnostics`. This is
    /// the fast opencode-style path: it returns whatever the server has ready
    /// *right now* instead of blocking on a `publishDiagnostics` push that, for
    /// servers like rust-analyzer, only arrives after `cargo check` finishes.
    pub(crate) fn pull_diagnostics(
        &mut self,
        file: &Path,
        timeout: Duration,
    ) -> Result<Value> {
        let report = self.request(
            "textDocument/diagnostic",
            json!({ "textDocument": { "uri": path_to_file_uri(file) } }),
            timeout,
        )?;
        // RelatedFullDocumentDiagnosticReport: { kind: "full", items: [...] }. An
        // "unchanged" report (or a server that omits items) yields an empty list,
        // which the caller treats as "nothing new ready yet".
        let items = report
            .get("items")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        Ok(json!({
            "uri": path_to_file_uri(file),
            "diagnostics": items,
        }))
    }

    pub(crate) fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        self.request_with_default_file(method, params, None, timeout)
    }

    /// Send a document-scoped request whose params do not themselves carry a
    /// textDocument URI. CompletionItem/resolve is the important example: its
    /// opaque item may contain edit ranges, so those ranges still need the
    /// originating document to round-trip between Neoism byte columns and the
    /// server's negotiated position encoding.
    pub(crate) fn request_for_file(
        &mut self,
        method: &str,
        params: Value,
        file: &Path,
        timeout: Duration,
    ) -> Result<Value> {
        self.request_with_default_file(method, params, Some(file), timeout)
    }

    fn request_with_default_file(
        &mut self,
        method: &str,
        mut params: Value,
        fallback_file: Option<&Path>,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let default_file =
            value_document_path(&params).or_else(|| fallback_file.map(Path::to_path_buf));
        let encoding = self
            .position_encoding
            .lock()
            .map(|encoding| *encoding)
            .map_err(|_| anyhow!("LSP position encoding lock poisoned"))?;
        {
            let mut documents = self
                .documents
                .lock()
                .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?;
            client_value_to_protocol_for_file(
                &mut params,
                default_file.as_deref(),
                &mut documents,
                encoding,
            );
        }
        let params_bytes = params.to_string().len();
        let started = crate::perf::now();
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .with_context(|| format!("failed to send LSP request `{method}`"))?;
        let response = wait_for_response(&self.responses, id, timeout)
            .with_context(|| format!("LSP request `{method}` failed"))
            .and_then(|mut response| {
                let mut documents = self
                    .documents
                    .lock()
                    .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?;
                server_value_to_bytes(
                    &mut response,
                    default_file.as_deref(),
                    &mut documents,
                    encoding,
                );
                Ok(response)
            });
        tracing::info!(
            target: "neoism_agent::perf",
            lsp = %self.label,
            method,
            id,
            timeout_ms = timeout.as_millis(),
            params_bytes,
            elapsed_ms = crate::perf::elapsed_ms(started),
            ok = response.is_ok(),
            error = response.as_ref().err().map(|error| error.to_string()),
            "lsp request completed"
        );
        response
    }

    pub(crate) fn wait_for_notification(
        &self,
        method: &str,
        timeout: Duration,
    ) -> Result<Option<Value>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            match self.notifications.recv_timeout(remaining) {
                Ok(message) => {
                    if message.get("method").and_then(Value::as_str) == Some(method) {
                        return Ok(Some(
                            message.get("params").cloned().unwrap_or(Value::Null),
                        ));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    fn notify(&mut self, method: &str, mut params: Value) -> Result<()> {
        let encoding = self
            .position_encoding
            .lock()
            .map(|encoding| *encoding)
            .map_err(|_| anyhow!("LSP position encoding lock poisoned"))?;
        {
            let mut documents = self
                .documents
                .lock()
                .map_err(|_| anyhow!("LSP document snapshot lock poisoned"))?;
            client_value_to_protocol(&mut params, &mut documents, encoding);
        }
        self.notify_raw(method, params)
    }

    fn notify_raw(&self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .with_context(|| format!("failed to send LSP notification `{method}`"))
    }

    pub(crate) fn shutdown(&mut self) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "shutdown",
            "params": null,
        }))?;
        let _ = wait_for_response(&self.responses, id, SHUTDOWN_TIMEOUT);
        let result = self.notify("exit", Value::Null);
        if let LspTransportHandle::Tcp { socket, .. } = &self.transport {
            let _ = socket.shutdown(Shutdown::Both);
        }
        result
    }

    pub(crate) fn exit_reason(&mut self) -> Option<String> {
        let transport_failure = self.transport_failure();
        match &mut self.transport {
            LspTransportHandle::Stdio { child, stderr_tail } => match child.try_wait() {
                Ok(None) => transport_failure,
                Ok(Some(status)) => {
                    let stderr = stderr_tail
                        .lock()
                        .ok()
                        .map(|lines| {
                            lines.iter().cloned().collect::<Vec<_>>().join(" | ")
                        })
                        .unwrap_or_default();
                    Some(if stderr.is_empty() {
                        format!("language server `{}` exited with {status}", self.label)
                    } else {
                        format!(
                            "language server `{}` exited with {status}: {stderr}",
                            self.label
                        )
                    })
                }
                Err(error) => Some(format!(
                    "failed to inspect language server `{}`: {error}",
                    self.label
                )),
            },
            LspTransportHandle::Tcp { socket, address } => {
                if let Some(reason) = transport_failure {
                    return Some(reason);
                }
                match socket.take_error() {
                    Ok(Some(error)) => {
                        Some(format!("LSP TCP endpoint `{address}` failed: {error}"))
                    }
                    Ok(None) => None,
                    Err(error) => Some(format!(
                        "failed to inspect LSP TCP endpoint `{address}`: {error}"
                    )),
                }
            }
        }
    }

    fn transport_failure(&self) -> Option<String> {
        self.health
            .failure
            .lock()
            .ok()
            .and_then(|failure| failure.clone())
    }

    fn write_message(&self, message: &Value) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("LSP transport writer lock poisoned"))?;
        write_lsp_message(&mut *writer, message)
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        match &mut self.transport {
            LspTransportHandle::Stdio { child, .. } => {
                if let Ok(None) = child.try_wait() {
                    tracing::warn!(
                        target: "neoism_agent::perf",
                        lsp = %self.label,
                        pid = child.id(),
                        "lsp process still running on drop; killing"
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            LspTransportHandle::Tcp { socket, .. } => {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
    }
}

fn record_transport_failure(health: &TransportHealth, reason: String) {
    if let Ok(mut failure) = health.failure.lock() {
        if failure.is_none() {
            *failure = Some(reason);
        }
    }
}

/// Build a standards-compliant response for a request initiated by the
/// language server. These requests are handled on the reader thread so they
/// can never deadlock behind the synchronous client request that caused the
/// server to ask them.
fn server_request_response(
    request: &Value,
    root_uri: &str,
    root_name: &str,
    configuration: Option<Value>,
) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").unwrap_or(&Value::Null);

    if method == "workspace/configuration"
        && !params
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(Value::is_object))
    {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32602,
                "message": "workspace/configuration requires an array of ConfigurationItem objects",
            },
        });
    }

    // Workspace folder change subscriptions are the one dynamically
    // registerable operation implied by workspaceFolders=true despite having
    // no separate dynamicRegistration flag. A Neoism LSP client has one
    // immutable workspace root, so acknowledging this subscription is enough:
    // there can be no change notification until the client itself gains a
    // mutable folder set.
    if method == "client/registerCapability"
        && registrations_only_target_method(
            params,
            "registrations",
            "workspace/didChangeWorkspaceFolders",
        )
    {
        return json!({ "jsonrpc": "2.0", "id": id, "result": null });
    }
    if method == "client/unregisterCapability"
        && registrations_only_target_method(
            params,
            // LSP 3.x standardized this historical misspelling.
            "unregisterations",
            "workspace/didChangeWorkspaceFolders",
        )
    {
        return json!({ "jsonrpc": "2.0", "id": id, "result": null });
    }

    // Every other dynamically-registerable feature we advertise declares
    // dynamicRegistration=false. A null success here would falsely tell the
    // server that a registration took effect, after which it can stop using
    // its static capability.
    if matches!(
        method,
        "client/registerCapability" | "client/unregisterCapability"
    ) {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32803,
                "message": "dynamic capability registration is not supported",
            },
        });
    }

    // A refresh response is only successful after the client has scheduled
    // the corresponding recomputation. Neoism does not advertise any of
    // these refreshSupport capabilities yet, so a no-op success is dishonest.
    if matches!(
        method,
        "window/workDoneProgress/create"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/inlineValue/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh"
    ) {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32803,
                "message": format!("refresh capability was not advertised for `{method}`"),
            },
        });
    }

    let result = match method {
        "workspace/configuration" => {
            let values = params
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            configuration_for_section(
                                configuration.as_ref(),
                                item.get("section").and_then(Value::as_str),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Some(Value::Array(values))
        }
        "workspace/workspaceFolders" => Some(json!([{
            "uri": root_uri,
            "name": root_name,
        }])),
        "workspace/applyEdit" => Some(json!({
            "applied": false,
            "failureReason": "Neoism does not apply unsolicited workspace edits",
        })),
        "window/showDocument" => Some(json!({ "success": false })),
        "window/showMessageRequest" => Some(Value::Null),
        _ => None,
    };

    match result {
        Some(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        None => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("unsupported server request `{method}`"),
            },
        }),
    }
}

/// Resolve a `workspace/configuration` item. LSP servers usually request a
/// named section (for example `rust-analyzer`). An omitted section requests
/// the whole settings object; an unavailable named section must return null.
fn configuration_for_section(
    configuration: Option<&Value>,
    section: Option<&str>,
) -> Value {
    let Some(configuration) = configuration else {
        return Value::Null;
    };
    let Some(section) = section.filter(|section| !section.is_empty()) else {
        return configuration.clone();
    };

    if let Some(value) = configuration.get(section) {
        return value.clone();
    }
    let mut value = configuration;
    let mut matched = true;
    for part in section.split('.') {
        match value.get(part) {
            Some(next) => value = next,
            None => {
                matched = false;
                break;
            }
        }
    }
    if matched {
        value.clone()
    } else {
        Value::Null
    }
}

fn registrations_only_target_method(
    params: &Value,
    list_key: &str,
    target_method: &str,
) -> bool {
    let Some(registrations) = params.get(list_key).and_then(Value::as_array) else {
        return false;
    };
    !registrations.is_empty()
        && registrations.iter().all(|registration| {
            registration.get("method").and_then(Value::as_str) == Some(target_method)
        })
}

fn wait_for_response(
    messages: &Receiver<Value>,
    id: i64,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("timed out waiting for response id {id}");
        }

        let message = messages
            .recv_timeout(remaining)
            .with_context(|| format!("timed out waiting for response id {id}"))?;
        if message.get("id").and_then(Value::as_i64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            bail!("server returned error for response id {id}: {error}");
        }
        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn write_lsp_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

pub(crate) fn read_lsp_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid length: {error}"),
                )
            })?);
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message missing Content-Length",
        )
    })?;
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod sync_capability_tests {
    use super::*;

    #[test]
    fn initialization_and_connect_budgets_are_separate_and_waits_stay_bounded() {
        assert_eq!(INITIALIZE_TIMEOUT, Duration::from_secs(30));
        assert_eq!(TCP_CONNECT_TIMEOUT, Duration::from_secs(3));
        let (_sender, receiver) = mpsc::channel();
        let started = Instant::now();
        let error = wait_for_response(&receiver, 7, Duration::from_millis(10))
            .expect_err("missing response must time out");
        assert!(error.to_string().contains("timed out"));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "bounded response wait exceeded one second"
        );
    }

    #[test]
    fn parses_numeric_and_object_text_document_sync_capabilities() {
        assert_eq!(
            TextDocumentSyncCapabilities::from_server_capability(Some(&json!(0))),
            TextDocumentSyncCapabilities::default()
        );
        assert_eq!(
            TextDocumentSyncCapabilities::from_server_capability(Some(&json!(1))),
            TextDocumentSyncCapabilities {
                change: TextDocumentSyncKind::Full,
                open_close: true,
                save: false,
                save_include_text: false,
            }
        );
        assert_eq!(
            TextDocumentSyncCapabilities::from_server_capability(Some(&json!({
                "openClose": false,
                "change": 2,
                "save": { "includeText": true }
            }))),
            TextDocumentSyncCapabilities {
                change: TextDocumentSyncKind::Incremental,
                open_close: false,
                save: true,
                save_include_text: true,
            }
        );
        assert_eq!(
            TextDocumentSyncCapabilities::from_server_capability(Some(&json!({
                "openClose": true,
                "change": 0,
                "save": false
            }))),
            TextDocumentSyncCapabilities {
                change: TextDocumentSyncKind::None,
                open_close: true,
                save: false,
                save_include_text: false,
            }
        );
        assert_eq!(
            TextDocumentSyncCapabilities::from_server_capability(Some(&json!({
                "save": { "includeText": false }
            }))),
            TextDocumentSyncCapabilities {
                change: TextDocumentSyncKind::None,
                open_close: false,
                save: true,
                save_include_text: false,
            }
        );
    }

    #[test]
    fn server_configuration_returns_exact_nested_whole_and_missing_values() {
        let settings = json!({
            "rust-analyzer": {
                "cargo": { "allFeatures": true }
            }
        });
        let response = server_request_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "workspace/configuration",
                "params": {
                    "items": [
                        { "section": "rust-analyzer" },
                        { "section": "rust-analyzer.cargo" },
                        { "section": "missing" },
                        {}
                    ]
                }
            }),
            "file:///workspace",
            "workspace",
            Some(settings.clone()),
        );
        assert_eq!(
            response["result"],
            json!([
                { "cargo": { "allFeatures": true } },
                { "allFeatures": true },
                null,
                settings
            ])
        );

        let malformed = server_request_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "workspace/configuration",
                "params": { "items": "not-an-array" }
            }),
            "file:///workspace",
            "workspace",
            None,
        );
        assert_eq!(malformed["error"]["code"], -32602);
    }

    #[test]
    fn server_registration_only_accepts_workspace_folder_change_subscriptions() {
        let folder_registration = server_request_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 20,
                "method": "client/registerCapability",
                "params": { "registrations": [{
                    "id": "folders",
                    "method": "workspace/didChangeWorkspaceFolders"
                }] }
            }),
            "file:///workspace",
            "workspace",
            None,
        );
        assert_eq!(folder_registration["result"], Value::Null);
        assert!(folder_registration.get("error").is_none());

        for registrations in [
            json!([{ "id": "hover", "method": "textDocument/hover" }]),
            json!([
                { "id": "folders", "method": "workspace/didChangeWorkspaceFolders" },
                { "id": "hover", "method": "textDocument/hover" }
            ]),
        ] {
            let rejected = server_request_response(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 21,
                    "method": "client/registerCapability",
                    "params": { "registrations": registrations }
                }),
                "file:///workspace",
                "workspace",
                None,
            );
            assert_eq!(rejected["error"]["code"], -32803);
        }

        let folder_unregistration = server_request_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 22,
                "method": "client/unregisterCapability",
                "params": { "unregisterations": [{
                    "id": "folders",
                    "method": "workspace/didChangeWorkspaceFolders"
                }] }
            }),
            "file:///workspace",
            "workspace",
            None,
        );
        assert_eq!(folder_unregistration["result"], Value::Null);
        assert!(folder_unregistration.get("error").is_none());
    }

    #[test]
    fn unadvertised_progress_and_refresh_requests_never_report_false_success() {
        for method in [
            "window/workDoneProgress/create",
            "workspace/semanticTokens/refresh",
            "workspace/inlayHint/refresh",
            "workspace/inlineValue/refresh",
            "workspace/codeLens/refresh",
            "workspace/diagnostic/refresh",
        ] {
            let response = server_request_response(
                &json!({ "jsonrpc": "2.0", "id": 30, "method": method }),
                "file:///workspace",
                "workspace",
                None,
            );
            assert_eq!(response["error"]["code"], -32803, "method={method}");
            assert!(response.get("result").is_none(), "method={method}");
        }
    }
}

#[cfg(test)]
#[path = "lsp_tcp_e2e_tests.rs"]
mod tcp_e2e_tests;
