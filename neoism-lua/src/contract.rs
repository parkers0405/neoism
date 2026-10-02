use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ExecutionScope, HostAction, PluginOwner};

/// Events emitted by the Rust host. Compatibility strings are accepted only
/// after they resolve through this registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginEventKind {
    Startup,
    Command,
    CommandResult,
    AsyncResult,
    ConfigReloaded,
    LspResult,
    BufferChanged,
    WorkspaceChanged,
    TabChanged,
    PanelChanged,
    FileTreeChanged,
    NotesChanged,
    AgentChanged,
    TerminalChanged,
    GitChanged,
    ThemeChanged,
    PluginsChanged,
    ConfigChanged,
    DocumentOpened,
    DocumentClosed,
    DocumentFocused,
    DocumentChanged,
    SelectionChanged,
    DiagnosticsChanged,
    PaneFocused,
    PaneChanged,
}

impl PluginEventKind {
    pub const ALL: [Self; 26] = [
        Self::Startup, Self::Command, Self::CommandResult, Self::AsyncResult, Self::ConfigReloaded, Self::LspResult,
        Self::BufferChanged, Self::WorkspaceChanged, Self::TabChanged,
        Self::PanelChanged, Self::FileTreeChanged, Self::NotesChanged,
        Self::AgentChanged, Self::TerminalChanged, Self::GitChanged,
        Self::ThemeChanged, Self::PluginsChanged, Self::ConfigChanged,
        Self::DocumentOpened, Self::DocumentClosed, Self::DocumentFocused,
        Self::DocumentChanged, Self::SelectionChanged, Self::DiagnosticsChanged,
        Self::PaneFocused, Self::PaneChanged,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "Startup",
            Self::Command => "Command",
            Self::CommandResult => "CommandResult",
            Self::AsyncResult => "AsyncResult",
            Self::ConfigReloaded => "ConfigReloaded",
            Self::LspResult => "LspResult",
            Self::BufferChanged => "BufferChanged",
            Self::WorkspaceChanged => "WorkspaceChanged",
            Self::TabChanged => "TabChanged",
            Self::PanelChanged => "PanelChanged",
            Self::FileTreeChanged => "FileTreeChanged",
            Self::NotesChanged => "NotesChanged",
            Self::AgentChanged => "AgentChanged",
            Self::TerminalChanged => "TerminalChanged",
            Self::GitChanged => "GitChanged",
            Self::ThemeChanged => "ThemeChanged",
            Self::PluginsChanged => "PluginsChanged",
            Self::ConfigChanged => "ConfigChanged",
            Self::DocumentOpened => "DocumentOpened",
            Self::DocumentClosed => "DocumentClosed",
            Self::DocumentFocused => "DocumentFocused",
            Self::DocumentChanged => "DocumentChanged",
            Self::SelectionChanged => "SelectionChanged",
            Self::DiagnosticsChanged => "DiagnosticsChanged",
            Self::PaneFocused => "PaneFocused",
            Self::PaneChanged => "PaneChanged",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventContract {
    pub kind: PluginEventKind,
    pub name: String,
    pub scope: ExecutionScope,
    pub payload_schema: String,
    pub owner_scoped: bool,
}

pub fn event_contract(name: &str) -> Result<EventContract, String> {
    let kind = PluginEventKind::ALL.into_iter().find(|kind| kind.name() == name)
        .ok_or_else(|| format!("unregistered Lua host event `{name}`"))?;
    let (payload_schema, owner_scoped) = match kind {
        PluginEventKind::LspResult => ("LuaLspCompletion", true),
        PluginEventKind::CommandResult => ("PluginCommandCompletion", true),
        PluginEventKind::AsyncResult => ("PluginAsyncCompletion", true),
        PluginEventKind::Startup => ("null", false),
        PluginEventKind::Command => ("CommandEvent", false),
        PluginEventKind::DocumentOpened | PluginEventKind::DocumentClosed
        | PluginEventKind::DocumentFocused | PluginEventKind::DocumentChanged
        | PluginEventKind::SelectionChanged | PluginEventKind::DiagnosticsChanged => ("DocumentEvent", false),
        PluginEventKind::PaneFocused | PluginEventKind::PaneChanged => ("PaneEvent", false),
        _ => ("snapshot", false),
    };
    Ok(EventContract {
        kind,
        name: name.into(),
        scope: ExecutionScope::Local,
        payload_schema: payload_schema.into(),
        owner_scoped,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsyncResultKind {
    Lsp,
    Command,
    Host,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncResultContract {
    pub kind: AsyncResultKind,
    pub event: PluginEventKind,
    pub payload_schema: String,
    pub cancellation: CancellationPolicy,
    pub exact_owner: bool,
    pub request_identity: bool,
}

pub fn async_result_contract(kind: AsyncResultKind) -> AsyncResultContract {
    match kind {
        AsyncResultKind::Lsp => AsyncResultContract {
            kind,
            event: PluginEventKind::LspResult,
            payload_schema: "LuaLspCompletion".into(),
            cancellation: CancellationPolicy::Logical,
            exact_owner: true,
            request_identity: true,
        },
        AsyncResultKind::Command => AsyncResultContract {
            kind,
            event: PluginEventKind::CommandResult,
            payload_schema: "PluginCommandCompletion".into(),
            cancellation: CancellationPolicy::Logical,
            exact_owner: true,
            request_identity: true,
        },
        AsyncResultKind::Host => AsyncResultContract {
            kind,
            event: PluginEventKind::AsyncResult,
            payload_schema: "PluginAsyncCompletion".into(),
            cancellation: CancellationPolicy::Logical,
            exact_owner: true,
            request_identity: true,
        },
    }
}

pub fn validate_async_result(
    kind: AsyncResultKind,
    owner: &PluginOwner,
    request_id: &str,
) -> Result<AsyncResultContract, String> {
    if owner.plugin_id.is_empty() || owner.revision.0.is_empty() {
        return Err("asynchronous result is missing an exact plugin owner revision".into());
    }
    if request_id.trim().is_empty() {
        return Err("asynchronous result is missing a request identity".into());
    }
    Ok(async_result_contract(kind))
}

/// Stable identifier for an operation at the Lua/host boundary. The strings
/// used by the compatibility API are parsed into this enum before capability
/// checks or application dispatch; privileged code never dispatches an
/// arbitrary `(namespace, operation)` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOperation {
    QuerySnapshot,
    QueryCurrent,
    QueryList,
    QueryGet,
    QueryStatus,
    QueryDiff,
    QuerySessions,
    DocumentText,
    DocumentLines,
    DocumentRange,
    DocumentSelections,
    DocumentCursor,
    DocumentMetadata,
    DocumentLanguage,
    DocumentDirty,
    DocumentRevision,
    DocumentEdit,
    DocumentSetSelections,
    DocumentMoveCursor,
    DocumentFocus,
    DocumentOpen,
    DocumentClose,
    DocumentSave,
    DocumentReload,
    NamespaceCreate,
    NamespaceClear,
    NamespaceDelete,
    AnchorCreate,
    AnchorDelete,
    DecorationCreate,
    DecorationDelete,
    DecorationClear,
    DiagnosticPublish,
    DiagnosticClear,
    DiagnosticActionExecute,
    StateGet,
    StateList,
    StateSet,
    StateDelete,
    StateClear,
    SchedulerCreate,
    SchedulerCancel,
    RegisterGet,
    RegisterList,
    RegisterSet,
    ClipboardGet,
    ClipboardSet,
    MarkGet,
    MarkList,
    MarkSet,
    MarkDelete,
    JumplistList,
    JumplistJump,
    MacroGet,
    MacroList,
    MacroSet,
    MacroPlay,
    AsyncCancel,
    JobSpawn,
    JobStdin,
    JobCloseStdin,
    JobCancel,
    NotificationShow,
    ProgressCreate,
    ProgressUpdate,
    ProgressFinish,
    PromptRequest,
    PromptCancel,
    ResultListCreate,
    ResultListReplace,
    ResultListAppend,
    ResultListClear,
    ResultListDelete,
    ResultListOpen,
    ResultListQuery,
    ClipboardRead,
    WatchCreate,
    WatchCancel,
    NetworkRequest,
    CredentialStatus,
    CompletionRequest,
    CompletionResolve,
    CompletionCancel,
    SnippetApply,
    VirtualDocumentOpen,
    VirtualDocumentUpdate,
    VirtualDocumentClose,
    SyntaxQuery,
    TaskRun,
    TaskCancel,
    TestRun,
    TestCancel,
    DebugStart,
    DebugControl,
    DebugStop,
    PtyCreate,
    PtySend,
    PtyResize,
    PtyStatus,
    PtyClose,
    GitQuery,
    GitMutation,
    AgentQuery,
    AgentMutation,
    ExtensionProcessStart,
    ExtensionProcessStop,
    ExtensionNativeLoad,
    LspRequest,
    LspCancel,
    LspNativeAction,
    CommandExecute,
    CommandCancel,
    ResourceOpen,
    ResourceReveal,
    ResourceCreate,
    ResourceCreateDirectory,
    ResourceRename,
    ResourceMove,
    ResourceDelete,
    PanelVisibility,
    WorkspaceFocus,
    WorkspaceSplit,
    TabCreate,
    TabClose,
    TabFocus,
    TabMove,
    AgentSend,
    TerminalSend,
    TerminalRun,
    GitStage,
    GitUnstage,
    GitCommit,
    GitRefresh,
    ConfigSet,
    EffectEmit,
    GenericRegisteredAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationPolicy {
    NotCancellable,
    Logical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    None,
    ActiveView,
    Document,
    Pane,
    Tab,
    Workspace,
    HostResource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostContract {
    pub operation: HostOperation,
    pub namespace: String,
    pub name: String,
    pub capability: String,
    pub scope: ExecutionScope,
    pub target: TargetKind,
    pub cancellation: CancellationPolicy,
    pub result_schema: String,
}

impl HostContract {
    fn new(
        operation: HostOperation,
        namespace: &str,
        name: &str,
        access: &str,
        scope: ExecutionScope,
        target: TargetKind,
        cancellation: CancellationPolicy,
        result_schema: &str,
    ) -> Self {
        Self {
            operation,
            namespace: namespace.into(),
            name: name.into(),
            capability: format!("{namespace}.{access}"),
            scope,
            target,
            cancellation,
            result_schema: result_schema.into(),
        }
    }
}

/// Resolve a compatibility query name to its typed contract.
pub fn query_contract(namespace: &str, operation: &str) -> Result<HostContract, String> {
    use HostOperation::*;
    let typed = match (namespace, operation) {
        ("document", "text") => (DocumentText, TargetKind::Document, "string"),
        ("document", "lines") => (DocumentLines, TargetKind::Document, "string[]"),
        ("document", "range") => (DocumentRange, TargetKind::Document, "string"),
        ("document", "selections") => (DocumentSelections, TargetKind::Document, "Selection[]"),
        ("document", "cursor") => (DocumentCursor, TargetKind::Document, "Position"),
        ("document", "metadata") => (DocumentMetadata, TargetKind::Document, "DocumentMetadata"),
        ("document", "language") => (DocumentLanguage, TargetKind::Document, "string"),
        ("document", "dirty") => (DocumentDirty, TargetKind::Document, "boolean"),
        ("document", "revision") => (DocumentRevision, TargetKind::Document, "integer"),
        ("state", "get") => (StateGet, TargetKind::None, "unknown"),
        ("state", "list") => (StateList, TargetKind::None, "object"),
        ("register", "get") => (RegisterGet, TargetKind::Document, "VimRegisterValue"),
        ("register", "list") => (RegisterList, TargetKind::Document, "VimRegisterMap"),
        ("clipboard", "get") => (ClipboardGet, TargetKind::ActiveView, "ClipboardValue"),
        ("mark", "get") => (MarkGet, TargetKind::Document, "EditorMark"),
        ("mark", "list") => (MarkList, TargetKind::Document, "EditorMarkMap"),
        ("jumplist", "list") => (JumplistList, TargetKind::Document, "EditorPositionList"),
        ("macro", "get") => (MacroGet, TargetKind::Document, "MacroValue"),
        ("macro", "list") => (MacroList, TargetKind::Document, "MacroMap"),
        (_, "snapshot") => (QuerySnapshot, TargetKind::ActiveView, "snapshot"),
        (_, "current") => (QueryCurrent, TargetKind::ActiveView, "snapshot"),
        (_, "list") => (QueryList, TargetKind::ActiveView, "snapshot[]"),
        (_, "get") => (QueryGet, TargetKind::ActiveView, "snapshot"),
        (_, "status") => (QueryStatus, TargetKind::ActiveView, "snapshot"),
        (_, "diff") => (QueryDiff, TargetKind::ActiveView, "snapshot"),
        (_, "sessions") => (QuerySessions, TargetKind::ActiveView, "snapshot[]"),
        _ => return Err(format!("unregistered Lua host query `{namespace}.{operation}`")),
    };
    Ok(HostContract::new(
        typed.0,
        namespace,
        operation,
        "read",
        ExecutionScope::Local,
        typed.1,
        CancellationPolicy::NotCancellable,
        typed.2,
    ))
}

/// Resolve and validate an action. This is the single privileged registry used
/// by the runtime, capability wrapper, queue, and application dispatcher.
pub fn action_contract(action: &HostAction) -> Result<HostContract, String> {
    use HostOperation::*;
    let ns = action.namespace.as_str();
    let name = action.action.as_str();
    let (operation, access, scope, target, cancellation, result) = match (ns, name) {
        ("document", "edit") => (DocumentEdit, "write", ExecutionScope::SharedBuffer, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "set_selections") => (DocumentSetSelections, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "move_cursor") => (DocumentMoveCursor, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "focus") => (DocumentFocus, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "open") => (DocumentOpen, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "close") => (DocumentClose, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "save") => (DocumentSave, "write", ExecutionScope::SharedBuffer, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("document", "reload") => (DocumentReload, "write", ExecutionScope::SharedBuffer, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("namespace", "create") => (NamespaceCreate, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "PluginNamespaceHandle"),
        ("namespace", "clear") => (NamespaceClear, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("namespace", "delete") => (NamespaceDelete, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("anchor", "create") => (AnchorCreate, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "PluginAnchorHandle"),
        ("anchor", "delete") => (AnchorDelete, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("decoration", "create" | "set") => (DecorationCreate, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "PluginDecorationHandle"),
        ("decoration", "delete") => (DecorationDelete, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("decoration", "clear") => (DecorationClear, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("diagnostic", "publish" | "update") => (DiagnosticPublish, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "PluginDiagnosticHandle"),
        ("diagnostic", "clear") => (DiagnosticClear, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("diagnostic", "execute_action") => (DiagnosticActionExecute, "execute", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "CommandResult"),
        ("state", "set") => (StateSet, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("state", "delete") => (StateDelete, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("state", "clear") => (StateClear, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("scheduler", "after" | "every") => (SchedulerCreate, "execute", ExecutionScope::Local, TargetKind::None, CancellationPolicy::Logical, "PluginTimerHandle"),
        ("scheduler", "cancel") => (SchedulerCancel, "execute", ExecutionScope::Local, TargetKind::None, CancellationPolicy::Logical, "QueuedMutation"),
        ("register", "get") => (RegisterGet, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "VimRegisterValue"),
        ("register", "list") => (RegisterList, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "VimRegisterMap"),
        ("register", "set") => (RegisterSet, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("clipboard", "get") => (ClipboardGet, "read", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "ClipboardValue"),
        ("clipboard", "set") => (ClipboardSet, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("mark", "get") => (MarkGet, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "EditorMark"),
        ("mark", "list") => (MarkList, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "EditorMarkMap"),
        ("mark", "set") => (MarkSet, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("mark", "delete") => (MarkDelete, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("jumplist", "list") => (JumplistList, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "EditorPositionList"),
        ("jumplist", "jump") => (JumplistJump, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("macro", "get") => (MacroGet, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "MacroValue"),
        ("macro", "list") => (MacroList, "editor.read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "MacroMap"),
        ("macro", "set") => (MacroSet, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("macro", "play") => (MacroPlay, "write", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("async", "cancel") => (AsyncCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("job", "spawn") => (JobSpawn, "execute", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginJobHandle"),
        ("job", "stdin") => (JobStdin, "execute", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("job", "close_stdin") => (JobCloseStdin, "execute", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("job", "cancel") => (JobCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("notification", "show") => (NotificationShow, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("progress", "create") => (ProgressCreate, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::Logical, "PluginProgressHandle"),
        ("progress", "update") => (ProgressUpdate, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("progress", "finish") => (ProgressFinish, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("prompt", "input" | "confirm" | "select") => (PromptRequest, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::Logical, "PluginPromptHandle"),
        ("prompt", "cancel") => (PromptCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("result_list", "create") => (ResultListCreate, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "PluginResultListHandle"),
        ("result_list", "replace") => (ResultListReplace, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("result_list", "append") => (ResultListAppend, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("result_list", "clear") => (ResultListClear, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("result_list", "delete") => (ResultListDelete, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("result_list", "open") => (ResultListOpen, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "NativeUi"),
        ("result_list", "query") => (ResultListQuery, "read", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("clipboard", "read") => (ClipboardRead, "read", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("watcher", "watch") => (WatchCreate, "read", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginWatcherHandle"),
        ("watcher", "cancel") => (WatchCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("network", "request") => (NetworkRequest, "execute", ExecutionScope::Local, TargetKind::None, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("credential", "status") => (CredentialStatus, "read", ExecutionScope::Local, TargetKind::None, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("completion", "request") => (CompletionRequest, "read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("completion", "resolve") => (CompletionResolve, "read", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("completion", "cancel") => (CompletionCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("snippet", "apply") => (SnippetApply, "write", ExecutionScope::SharedBuffer, TargetKind::Document, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("virtual_document", "open") => (VirtualDocumentOpen, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("virtual_document", "update") => (VirtualDocumentUpdate, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("virtual_document", "close") => (VirtualDocumentClose, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("syntax" | "tree_sitter", "query") => (SyntaxQuery, "read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("task", "run") => (TaskRun, "execute", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("task", "cancel") => (TaskCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("test", "run" | "watch" | "rerun") => (TestRun, "execute", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("test", "cancel") => (TestCancel, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("debug", "start") => (DebugStart, "execute", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("debug", "continue" | "pause" | "step_in" | "step_out" | "next" | "evaluate" | "breakpoints") => (DebugControl, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("debug", "stop") => (DebugStop, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("pty", "create") => (PtyCreate, "execute", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("pty", "send") => (PtySend, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("pty", "resize") => (PtyResize, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("pty", "status") => (PtyStatus, "read", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("pty", "close") => (PtyClose, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("git", "status" | "diff" | "blame" | "branches" | "history" | "worktrees") => (GitQuery, "read", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("git", "stage_hunk" | "unstage_hunk" | "checkout" | "branch" | "worktree") => (GitMutation, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("agent", "sessions" | "messages" | "checkpoints" | "status") => (AgentQuery, "read", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("agent", "approve" | "checkpoint" | "subagent" | "workflow") => (AgentMutation, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("extension_host", "start") => (ExtensionProcessStart, "execute", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("extension_host", "stop") => (ExtensionProcessStop, "cancel", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "PluginAsyncCompletion"),
        ("extension_host", "load_native") => (ExtensionNativeLoad, "native", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("lsp", "request") => {
            let access = match action.arguments.get("operation").and_then(Value::as_str).unwrap_or_default() {
                "rename" | "format" | "code_actions" | "apply_code_action" => "edit",
                _ => "read",
            };
            (LspRequest, access, ExecutionScope::Local, TargetKind::Document, CancellationPolicy::Logical, "LspResult")
        }
        ("lsp", "cancel") => (LspCancel, "cancel", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::Logical, "LspResult"),
        ("lsp", "register_server" | "unregister_server") => (GenericRegisteredAction, "register", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("lsp", _) => (LspNativeAction, "read", ExecutionScope::Local, TargetKind::Document, CancellationPolicy::NotCancellable, "NativeUi"),
        ("command", "execute") => (CommandExecute, "execute", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::Logical, "PluginCommandRequest"),
        ("command", "cancel") => (CommandCancel, "execute", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::Logical, "QueuedMutation"),
        ("buffer" | "file_tree" | "notes", "open") => (ResourceOpen, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("buffer" | "file_tree" | "notes", "reveal") => (ResourceReveal, "write", ExecutionScope::Local, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("buffer", "edit") => (GenericRegisteredAction, "write", ExecutionScope::SharedBuffer, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("buffer", "save") => (GenericRegisteredAction, "write", ExecutionScope::SharedBuffer, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("file_tree" | "notes", "create") => (ResourceCreate, "write", ExecutionScope::Workspace, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("file_tree" | "notes", "create_dir") => (ResourceCreateDirectory, "write", ExecutionScope::Workspace, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("file_tree" | "notes", "rename") => (ResourceRename, "write", ExecutionScope::Workspace, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("file_tree" | "notes", "move") => (ResourceMove, "write", ExecutionScope::Workspace, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("file_tree" | "notes", "delete") => (ResourceDelete, "write", ExecutionScope::Workspace, TargetKind::HostResource, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("panel", "show" | "hide" | "toggle" | "focus" | "open" | "close") => (PanelVisibility, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("workspace", "open") => (WorkspaceFocus, "write", ExecutionScope::Workspace, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("workspace", "focus") => (WorkspaceFocus, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("workspace", "split") => (WorkspaceSplit, "write", ExecutionScope::Workspace, TargetKind::Pane, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("tab", "create" | "open") => (TabCreate, "write", ExecutionScope::Workspace, TargetKind::Pane, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("tab", "close") => (TabClose, "write", ExecutionScope::Workspace, TargetKind::Tab, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("tab", "focus" | "select") => (TabFocus, "write", ExecutionScope::Local, TargetKind::Tab, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("tab", "move") => (TabMove, "write", ExecutionScope::Workspace, TargetKind::Tab, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("agent", "send") => (AgentSend, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("agent", "open" | "create") => (GenericRegisteredAction, "write", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("terminal", "send") => (TerminalSend, "write", ExecutionScope::Local, TargetKind::Pane, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("terminal", "run") => (TerminalRun, "write", ExecutionScope::Local, TargetKind::Pane, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("git", "stage") => (GitStage, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("git", "unstage") => (GitUnstage, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("git", "commit") => (GitCommit, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("git", "refresh") => (GitRefresh, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("git", "open" | "toggle") => (GenericRegisteredAction, "write", ExecutionScope::Local, TargetKind::Workspace, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("config" | "theme", "set" | "apply") => (ConfigSet, "write", ExecutionScope::Local, TargetKind::None, CancellationPolicy::NotCancellable, "QueuedMutation"),
        ("effect", "emit") => (EffectEmit, "emit", ExecutionScope::Local, TargetKind::ActiveView, CancellationPolicy::NotCancellable, "QueuedMutation"),
        _ => return Err(format!("unregistered Lua host action `{ns}.{name}`")),
    };
    if action.scope != scope {
        return Err(format!(
            "Lua host action `{ns}.{name}` requires scope `{scope:?}`, got `{:?}`",
            action.scope
        ));
    }
    Ok(HostContract::new(operation, ns, name, access, scope, target, cancellation, result))
}

pub fn require_exact_owner(action: &HostAction) -> Result<&PluginOwner, String> {
    action.owner.as_ref().filter(|owner| !owner.plugin_id.is_empty() && !owner.revision.0.is_empty())
        .ok_or_else(|| format!("Lua host action `{}.{}` is missing an exact plugin owner revision", action.namespace, action.action))
}

/// Registry projection used by docs/introspection. It is data-only and safe to
/// expose to sandboxed plugins.
pub fn contract_schema() -> Value {
    let events = PluginEventKind::ALL.into_iter()
        .map(|kind| event_contract(kind.name()).expect("registered event"))
        .collect::<Vec<_>>();
    let async_results = [AsyncResultKind::Lsp, AsyncResultKind::Command, AsyncResultKind::Host].into_iter()
        .map(async_result_contract)
        .collect::<Vec<_>>();
    serde_json::json!({
        "version": crate::API_VERSION,
        "coordinateEncoding": "zero-based-utf8-bytes",
        "owner": ["pluginId", "revision"],
        "handles": ["document", "pane", "tab", "workspace", "selection", "cursor", "range"],
        "events": events,
        "asyncResults": async_results,
        "platform": crate::platform_capabilities(),
        "security": { "luaOnRenderThread": false, "opaqueHostResources": true, "defaultTier": "sandboxed_lua" }
    })
}

pub fn registered_host_operations() -> Vec<String> {
    [
        "document.edit", "document.set_selections", "document.move_cursor", "document.focus", "document.open",
        "document.close", "document.save", "document.reload", "namespace.create", "namespace.clear",
        "namespace.delete", "anchor.create", "anchor.delete", "decoration.create", "decoration.set",
        "decoration.delete", "decoration.clear", "diagnostic.publish", "diagnostic.update",
        "diagnostic.clear", "diagnostic.execute_action", "state.set", "state.delete", "state.clear",
        "scheduler.after", "scheduler.every", "scheduler.cancel", "register.get", "register.list", "register.set",
        "clipboard.get", "clipboard.set", "clipboard.read", "mark.get", "mark.list", "mark.set", "mark.delete",
        "jumplist.list", "jumplist.jump", "macro.get", "macro.list", "macro.set", "macro.play", "async.cancel",
        "job.spawn", "job.stdin", "job.close_stdin", "job.cancel", "notification.show", "progress.create",
        "progress.update", "progress.finish",
        "watcher.watch", "watcher.cancel", "network.request", "credential.status", "prompt.input",
        "prompt.confirm", "prompt.select", "prompt.cancel", "result_list.create", "result_list.replace",
        "result_list.append", "result_list.clear", "result_list.delete", "result_list.open", "result_list.query",
        "completion.request", "completion.resolve", "completion.cancel", "snippet.apply", "virtual_document.open",
        "virtual_document.update", "virtual_document.close", "syntax.query", "tree_sitter.query", "task.run",
        "task.cancel", "test.run", "test.watch", "test.rerun", "test.cancel", "debug.start", "debug.continue",
        "debug.pause", "debug.step_in", "debug.step_out", "debug.next", "debug.evaluate", "debug.breakpoints",
        "debug.stop", "pty.create", "pty.send", "pty.resize", "pty.status", "pty.close", "git.status", "git.diff", "git.blame",
        "git.branches", "git.history", "git.worktrees", "agent.sessions", "agent.messages", "agent.checkpoints",
        "agent.status", "git.stage_hunk", "git.unstage_hunk", "git.checkout", "git.branch", "git.worktree",
        "agent.approve", "agent.checkpoint", "agent.subagent", "agent.workflow", "extension_host.start",
        "extension_host.stop", "extension_host.load_native", "lsp.request", "lsp.cancel", "lsp.register_server",
        "lsp.unregister_server", "lsp.definition", "lsp.references", "lsp.format", "lsp.hover",
        "lsp.code_actions", "lsp.signature_help", "lsp.document_symbols", "lsp.workspace_symbols",
        "lsp.diagnostics", "lsp.clients", "lsp.apply_code_action", "command.execute", "command.cancel",
        "buffer.open", "buffer.reveal", "buffer.edit", "buffer.save", "file_tree.open", "file_tree.reveal",
        "file_tree.create", "file_tree.create_dir", "file_tree.rename", "file_tree.move", "file_tree.delete",
        "notes.open", "notes.reveal", "notes.create", "notes.create_dir", "notes.rename", "notes.move",
        "notes.delete", "panel.show", "panel.hide", "panel.toggle", "panel.focus", "panel.open", "panel.close",
        "workspace.open", "workspace.focus", "workspace.split", "tab.create", "tab.open", "tab.close",
        "tab.focus", "tab.select", "tab.move", "agent.send", "agent.open", "agent.create", "terminal.send",
        "terminal.run", "git.stage", "git.unstage", "git.commit", "git.refresh", "git.open", "git.toggle",
        "config.set", "config.apply", "theme.set", "theme.apply", "effect.emit",
    ].into_iter().map(str::to_owned).collect()
}

/// Namespace tables and compatibility methods installed by `runtime.rs`.
/// Keeping these lists beside the annotation generator prevents the runtime
/// surface and generated language-server model from drifting independently.
pub(crate) const RUNTIME_HOST_NAMESPACES: &[&str] = &[
    "buffer", "document", "pane", "workspace", "tab", "file_tree", "notes", "agent", "terminal", "git",
    "config", "theme", "plugins", "lsp", "namespace", "anchor", "decoration", "diagnostic", "state", "scheduler",
    "register", "clipboard", "mark", "jumplist", "changelist", "macro", "async", "job", "notification",
    "progress", "prompt", "result_list", "watcher", "network", "credential", "completion", "snippet", "syntax",
    "tree_sitter", "task", "test", "debug", "pty", "virtual_document", "extension_host", "effect",
];

pub(crate) const RUNTIME_QUERY_METHODS: &[&str] = &[
    "current", "list", "get", "snapshot", "status", "diff", "sessions", "text", "lines", "range",
    "selections", "cursor", "metadata", "language", "dirty", "revision",
];

pub(crate) const RUNTIME_ACTION_METHODS: &[&str] = &[
    "open", "close", "show", "hide", "toggle", "focus", "edit", "save", "create", "send", "refresh",
    "reveal", "split", "move", "pin", "stage", "commit", "set", "run", "create_dir", "rename", "delete",
    "jump", "play", "spawn", "stdin", "close_stdin", "input", "confirm", "select", "finish", "append",
    "replace", "read", "query", "watch", "request", "set_selections", "move_cursor", "reload", "clear",
    "publish", "update", "after", "every", "cancel", "definition", "references", "format", "hover",
    "code_actions", "signature_help", "document_symbols", "workspace_symbols", "diagnostics", "clients",
    "register_server", "unregister_server", "apply_code_action", "emit",
];

/// Deterministic Lua-language-server annotations generated from the same typed
/// registries used at runtime. Keep the type declarations here: unlike a
/// handwritten projection they are verified against `docs/lua-api.lua`.
pub fn generate_lua_api_annotations() -> String {
    let event_names = PluginEventKind::ALL.into_iter()
        .map(|kind| format!("'{}'", kind.name()))
        .collect::<Vec<_>>()
        .join("|");
    let host_operations = registered_host_operations().into_iter()
        .map(|operation| format!("'{operation}'"))
        .collect::<Vec<_>>()
        .join("|");
    let host_action_names = registered_host_operations().into_iter()
        .filter_map(|operation| operation.split_once('.').map(|(_, name)| name.to_owned()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join("|");
    let query_methods = RUNTIME_QUERY_METHODS.iter()
        .map(|method| format!("---@field {method} fun(args?: unknown): unknown Compatibility query; unsupported namespace/query combinations are rejected by the host."))
        .collect::<Vec<_>>()
        .join("\n");
    let action_methods = RUNTIME_ACTION_METHODS.iter()
        .map(|method| format!("---@field {method} fun(args?: unknown): unknown Compatibility action; unsupported namespace/action combinations are rejected by the host."))
        .collect::<Vec<_>>()
        .join("\n");
    let namespace_fields = RUNTIME_HOST_NAMESPACES.iter()
        .filter(|namespace| !matches!(**namespace,
            "document" | "namespace" | "anchor" | "decoration" | "diagnostic" | "state" | "scheduler" |
            "register" | "clipboard" | "mark" | "jumplist" | "macro" | "async" | "job" | "notification" |
            "progress" | "prompt" | "result_list"))
        .map(|namespace| format!("---@field {namespace} NeoismHostNamespaceApi"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(r#"---@meta neoism

-- Generated by neoism_lua::generate_lua_api_annotations. Do not edit by hand.

---@alias NeoismHandle string Host-owned identity. Never parse or construct it.
---@alias NeoismEventName {event_names}
---@alias NeoismHostOperation {host_operations}
---@alias NeoismHostActionName {host_action_names} Unqualified action accepted by NeoismHostNamespaceApi.call; the namespace/action pair must occur in NeoismHostOperation.

---@class NeoismHostNamespaceApi
---@field call fun(action: NeoismHostActionName, args?: unknown, scope?: 'local'|'workspace'|'shared-buffer'|'presence'): unknown Dispatch an action by its unqualified name. Valid namespace/action pairs are listed by NeoismHostOperation.
{query_methods}
{action_methods}

---@class NeoismPosition
---@field line integer Zero-based line.
---@field character integer Zero-based UTF-8 byte column.

---@class NeoismRange
---@field handle NeoismHandle
---@field start NeoismPosition
---@field end NeoismPosition

---@class NeoismSelection
---@field handle NeoismHandle
---@field anchor NeoismPosition
---@field active NeoismPosition

---@class NeoismDocumentMetadata
---@field title string
---@field hostPath string Opaque host-native display path; do not apply guest path semantics.
---@field language string
---@field kind string
---@field remote boolean
---@field dirty boolean
---@field revision integer
---@field lineCount integer

---@class NeoismDocument
---@field handle NeoismHandle
---@field pane NeoismHandle
---@field tab NeoismHandle
---@field workspace NeoismHandle
---@field revision integer
---@field text string
---@field cursor {{ handle: NeoismHandle, position: NeoismPosition }}
---@field selections NeoismSelection[]
---@field metadata NeoismDocumentMetadata

---@class NeoismDocumentEdit
---@field range NeoismRange
---@field text string

---@class NeoismDocumentApi: NeoismHostNamespaceApi
---@field current fun(): NeoismDocument
---@field text fun(args: {{ document: NeoismHandle }}): string
---@field lines fun(args: {{ document: NeoismHandle, startLine?: integer, endLine?: integer }}): string[]
---@field range fun(args: {{ document: NeoismHandle, range: NeoismRange }}): string
---@field cursor fun(args: {{ document: NeoismHandle }}): {{ handle: NeoismHandle, position: NeoismPosition }}
---@field selections fun(args: {{ document: NeoismHandle }}): NeoismSelection[]
---@field metadata fun(args: {{ document: NeoismHandle }}): NeoismDocumentMetadata
---@field language fun(args: {{ document: NeoismHandle }}): string
---@field dirty fun(args: {{ document: NeoismHandle }}): boolean
---@field revision fun(args: {{ document: NeoismHandle }}): integer
---@field edit fun(args: {{ document: NeoismHandle, expectedRevision: integer, edits: NeoismDocumentEdit[] }}): {{ id: string }}
---@field move_cursor fun(args: {{ document: NeoismHandle, expectedRevision: integer, position: NeoismPosition, extendSelection?: boolean }}): {{ id: string }}
---@field set_selections fun(args: {{ document: NeoismHandle, expectedRevision: integer, selections: NeoismSelection[] }}): {{ id: string }}
---@field focus fun(args: {{ document: NeoismHandle, expectedRevision: integer }}): {{ id: string }}
---@field save fun(args: {{ document: NeoismHandle, expectedRevision: integer }}): {{ id: string }}
---@field close fun(args: {{ document: NeoismHandle, expectedRevision: integer }}): {{ id: string }}

---@class NeoismAnchorApi: NeoismHostNamespaceApi
---@field create fun(args: {{ namespace: string, document: NeoismHandle, expectedRevision: integer, position: NeoismPosition, bias: 'before'|'after' }}): {{ id: string }}
---@field delete fun(args: {{ namespace: string, resource: string }}): {{ id: string }}

---@class NeoismDecorationApi: NeoismHostNamespaceApi
---@field create fun(args: {{ namespace: string, start: string, end: string, layer: 'highlight'|'gutter_sign'|'virtual_text'|'virtual_line'|'inline_widget'|'code_lens'|'fold'|'conceal'|'diagnostic', text?: string, severity?: 'error'|'warning'|'information'|'hint', style?: table }}): {{ id: string }}
---@field delete fun(args: {{ namespace: string, resource: string }}): {{ id: string }}
---@field clear fun(args: {{ namespace: string }}): {{ id: string }}

---@class NeoismNamespaceApi: NeoismHostNamespaceApi
---@field create fun(args?: {{ name?: string }}): {{ id: string }}
---@field clear fun(args: {{ namespace: string }}): {{ id: string }}
---@field delete fun(args: {{ namespace: string }}): {{ id: string }}

---@class NeoismStateApi: NeoismHostNamespaceApi
---@field get fun(args: {{ scope: 'plugin'|'document'|'pane'|'tab'|'workspace', target?: NeoismHandle, persistent?: boolean, key: string }}): unknown
---@field list fun(args: {{ scope: 'plugin'|'document'|'pane'|'tab'|'workspace', target?: NeoismHandle, persistent?: boolean }}): table<string, unknown>
---@field set fun(args: {{ scope: 'plugin'|'document'|'pane'|'tab'|'workspace', target?: NeoismHandle, persistent?: boolean, key: string, value: unknown }}): {{ id: string }}
---@field delete fun(args: {{ scope: 'plugin'|'document'|'pane'|'tab'|'workspace', target?: NeoismHandle, persistent?: boolean, key: string }}): {{ id: string }}
---@field clear fun(args: {{ scope: 'plugin'|'document'|'pane'|'tab'|'workspace', target?: NeoismHandle, persistent?: boolean, key?: string }}): {{ id: string }}

---@class NeoismCommandRequest
---@field command string Command id or alias.
---@field arguments? unknown
---@field range? {{ startLine: integer, endLine: integer }}
---@field count? integer
---@field bang? boolean

---@class NeoismCommandApi
---@field register fun(id: string, callback: fun(request: NeoismCommandRequest): unknown, options?: table) Register a Lua command contribution.
---@field execute fun(id: string, arguments?: unknown): {{ id: string }} Compatibility invocation.
---@field invoke fun(request: NeoismCommandRequest): {{ id: string }}
---@field cancel fun(request: {{ id: string }}): {{ id: string }}

---@class NeoismOptionApi
---@field set fun(args: {{ name: 'wrap'|'tab_width'|'use_tabs'|'input_mode', value: unknown, scope: 'document'|'pane'|'tab'|'workspace', target?: NeoismHandle, priority?: integer }})

---@class NeoismInputRegistration
---@field keys string[] One or more normalized single- or multi-key sequences.
---@field mode? 'normal'|'insert'|'visual'|'editor'|'global'
---@field when? string
---@field priority? integer
---@field fallback? boolean Allow the failing key to continue through native input after a prefix mismatch.

---@class NeoismInputApi
---@field register fun(id: string, callback: fun(event: NeoismEvent): unknown, options: NeoismInputRegistration): string

---@class NeoismRegisterApi: NeoismHostNamespaceApi
---@field get fun(args: {{ document: NeoismHandle, name: string }}): {{ text: string, linewise: boolean, blockwise: boolean }}|nil
---@field list fun(args: {{ document: NeoismHandle }}): table<string, table>
---@field set fun(args: {{ document: NeoismHandle, name: string, text: string, linewise?: boolean, blockwise?: boolean }}): {{ id: string }}

---@class NeoismMarkApi: NeoismHostNamespaceApi
---@field get fun(args: {{ document: NeoismHandle, name: string }}): NeoismPosition|nil
---@field list fun(args: {{ document: NeoismHandle }}): table<string, NeoismPosition>
---@field set fun(args: {{ document: NeoismHandle, name: string, line: integer, character: integer }}): {{ id: string }}
---@field delete fun(args: {{ document: NeoismHandle, name: string }}): {{ id: string }}

---@class NeoismClipboardApi: NeoismHostNamespaceApi
---@field set fun(args: {{ text: string }}): {{ id: string }}
---@field read fun(): {{ id: string }} Asynchronous; emits AsyncResult.

---@class NeoismJumpListApi: NeoismHostNamespaceApi
---@field list fun(args: {{ document: NeoismHandle }}): table[]
---@field jump fun(args: {{ document: NeoismHandle, direction: 'back'|'forward' }}): {{ id: string }}

---@class NeoismMacroApi: NeoismHostNamespaceApi
---@field get fun(args: {{ document: NeoismHandle, name: string }}): string|nil
---@field list fun(args: {{ document: NeoismHandle }}): table<string, string>
---@field set fun(args: {{ document: NeoismHandle, name: string, keys: string }}): {{ id: string }}
---@field play fun(args: {{ document: NeoismHandle, name: string, count?: integer }}): {{ id: string }}

---@class NeoismAsyncApi: NeoismHostNamespaceApi
---@field cancel fun(args: {{ id: string }}): {{ id: string }}

---@class NeoismJobApi: NeoismHostNamespaceApi
---@field spawn fun(args: {{ program: string, arguments?: string[], cwd?: string, env?: table<string,string>, timeoutMillis?: integer, maxOutputBytes?: integer }}): {{ id: string }}
---@field stdin fun(args: {{ job: string, data: string }}): {{ id: string }}
---@field close_stdin fun(args: {{ job: string }}): {{ id: string }}
---@field cancel fun(args: {{ job: string }}): {{ id: string }}

---@class NeoismNotificationApi: NeoismHostNamespaceApi
---@field show fun(args: {{ title?: string, message: string, level?: 'info'|'warning'|'error' }}): {{ id: string }}

---@class NeoismProgressApi: NeoismHostNamespaceApi
---@field create fun(args: {{ message?: string }}): {{ id: string }}
---@field update fun(args: {{ progress: string, message?: string, percentage?: number }}): {{ id: string }}
---@field finish fun(args: {{ progress: string, message?: string }}): {{ id: string }}

---@class NeoismPromptApi: NeoismHostNamespaceApi
---@field input fun(args: {{ title: string, message?: string, default?: string }}): {{ id: string }}
---@field confirm fun(args: {{ title: string, message?: string }}): {{ id: string }}
---@field select fun(args: {{ title: string, message?: string, options: string[] }}): {{ id: string }}
---@field cancel fun(args: {{ id: string }}): {{ id: string }}

---@class NeoismResultEntry
---@field document NeoismHandle
---@field position NeoismPosition
---@field label string
---@field detail? string
---@field severity? string

---@class NeoismResultListApi: NeoismHostNamespaceApi
---@field create fun(args: {{ title?: string, kind?: 'quickfix'|'location'|'result', entries?: NeoismResultEntry[] }}): {{ id: string }}
---@field replace fun(args: {{ list: string, title?: string, kind?: string, entries: NeoismResultEntry[] }}): {{ id: string }}
---@field append fun(args: {{ list: string, entries: NeoismResultEntry[] }}): {{ id: string }}
---@field clear fun(args: {{ list: string }}): {{ id: string }}
---@field delete fun(args: {{ list: string }}): {{ id: string }}
---@field query fun(args: {{ list: string }}): {{ id: string }}
---@field open fun(args: {{ list: string, index?: integer }}): {{ id: string }}

---@class NeoismSchedulerApi: NeoismHostNamespaceApi
---@field after fun(args: {{ delayMillis: integer, command: string, arguments?: unknown }}): {{ id: string }}
---@field every fun(args: {{ delayMillis: integer, intervalMillis?: integer, command: string, arguments?: unknown }}): {{ id: string }}
---@field cancel fun(args: {{ timer: string }}): {{ id: string }}

---@class NeoismEvent
---@field name NeoismEventName
---@field payload unknown
---@field scope 'local'
---@field origin? string

---@class NeoismLspResult Async results are matched by exact plugin revision and request id.
---@field id string
---@field ok boolean
---@field cancelled boolean
---@field result? unknown
---@field error? {{ code: string, message: string }}

---@class NeoismCommandResult Async command results are matched by exact plugin revision and request id.
---@field id string
---@field command string Canonical command id.
---@field ok boolean
---@field cancelled boolean
---@field result? unknown
---@field error? {{ code: string, message: string }}

---@class NeoismUiApi
---@field style fun(selector: string, value: table)
---@field contribute fun(value: table)
---@field view fun(id: string, value: table)
---@field surface fun(id: string, value: table)
---@field item fun(id: string, value: table)

---@class NeoismExtensionApi
---@field register fun(value: table)
---@field capabilities table

---@class NeoismKeymapApi
---@field set fun(mode: string, key: string, target: string|fun(): unknown, options?: {{ when?: string, priority?: integer, fallback?: boolean }})

---@class NeoismPanelApi: NeoismHostNamespaceApi
---@field register fun(id: string, value: table)

---@class NeoismApi
---@field api_version integer
---@field contract table Typed host-contract introspection snapshot.
---@field setup fun(config: table)
---@field ui NeoismUiApi
---@field extension NeoismExtensionApi
---@field keymap NeoismKeymapApi
---@field augroup fun(name: string, options?: {{ clear?: boolean, delete?: boolean }}): string
---@field autocmd fun(event: NeoismEventName|'*', callback: fun(event: NeoismEvent): unknown, options?: {{ pattern?: string, once?: boolean, scope?: 'local'|'workspace'|'shared-buffer'|'presence', group?: string, nested?: boolean, priority?: integer }})
---@field panel NeoismPanelApi
---@field document NeoismDocumentApi
---@field namespace NeoismNamespaceApi
---@field anchor NeoismAnchorApi
---@field decoration NeoismDecorationApi
---@field diagnostic NeoismDecorationApi
---@field state NeoismStateApi
---@field command NeoismCommandApi
---@field option NeoismOptionApi
---@field motion NeoismInputApi
---@field operator NeoismInputApi
---@field text_object NeoismInputApi
---@field input NeoismInputApi
---@field register NeoismRegisterApi
---@field clipboard NeoismClipboardApi
---@field mark NeoismMarkApi
---@field jumplist NeoismJumpListApi
---@field macro NeoismMacroApi
---@field async NeoismAsyncApi
---@field job NeoismJobApi
---@field notification NeoismNotificationApi
---@field progress NeoismProgressApi
---@field prompt NeoismPromptApi
---@field result_list NeoismResultListApi
---@field scheduler NeoismSchedulerApi
{namespace_fields}

---@type NeoismApi
neoism = neoism
"#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PluginRevision, PluginOwner};

    fn action(namespace: &str, name: &str, scope: ExecutionScope) -> HostAction {
        HostAction {
            namespace: namespace.into(),
            action: name.into(),
            arguments: Value::Null,
            scope,
            invocation_id: None,
            owner: Some(PluginOwner { plugin_id: "dev.test".into(), revision: PluginRevision("r1".into()) }),
        }
    }

    #[test]
    fn rejects_unregistered_and_wrong_scope_actions() {
        assert!(action_contract(&action("renderer", "draw", ExecutionScope::Local)).is_err());
        assert!(action_contract(&action("document", "edit", ExecutionScope::Local)).is_err());
        assert_eq!(
            action_contract(&action("document", "edit", ExecutionScope::SharedBuffer)).unwrap().operation,
            HostOperation::DocumentEdit
        );
    }

    #[test]
    fn privileged_actions_require_exact_revision_owner() {
        let mut value = action("document", "edit", ExecutionScope::SharedBuffer);
        assert!(require_exact_owner(&value).is_ok());
        value.owner.as_mut().unwrap().revision.0.clear();
        assert!(require_exact_owner(&value).is_err());
    }

    #[test]
    fn event_and_async_registries_reject_untyped_or_unowned_values() {
        assert_eq!(event_contract("LspResult").unwrap().kind, PluginEventKind::LspResult);
        assert_eq!(event_contract("CommandResult").unwrap().kind, PluginEventKind::CommandResult);
        assert_eq!(event_contract("AsyncResult").unwrap().kind, PluginEventKind::AsyncResult);
        assert!(event_contract("RenderFrame").is_err());
        let owner = PluginOwner { plugin_id: "dev.test".into(), revision: PluginRevision("r1".into()) };
        assert!(validate_async_result(AsyncResultKind::Lsp, &owner, "request-1").is_ok());
        assert!(validate_async_result(AsyncResultKind::Command, &owner, "request-2").is_ok());
        assert!(validate_async_result(AsyncResultKind::Host, &owner, "request-3").is_ok());
        assert!(validate_async_result(AsyncResultKind::Lsp, &PluginOwner::default(), "request-1").is_err());
        assert!(validate_async_result(AsyncResultKind::Lsp, &owner, "").is_err());
    }

    #[test]
    fn checked_in_lua_annotations_match_the_registry_generator() {
        let checked_in = include_str!("../../docs/lua-api.lua");
        assert_eq!(checked_in, generate_lua_api_annotations());
    }

    #[test]
    fn registered_host_operations_are_unique_and_resolve_to_action_contracts() {
        let operations = registered_host_operations();
        let unique = operations.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), operations.len(), "registered host operations contain duplicates");

        for operation in &operations {
            let (namespace, name) = operation.split_once('.').expect("qualified host operation");
            let resolved = [
                ExecutionScope::Local,
                ExecutionScope::Workspace,
                ExecutionScope::SharedBuffer,
                ExecutionScope::Presence,
            ].into_iter().any(|scope| action_contract(&action(namespace, name, scope)).is_ok());
            assert!(resolved, "registered host operation `{operation}` has no action contract");
        }

        for required in [
            "pty.status", "git.stage_hunk", "git.unstage_hunk", "git.checkout", "git.branch", "git.worktree",
            "agent.approve", "agent.checkpoint", "agent.subagent", "agent.workflow",
        ] {
            assert!(operations.iter().any(|operation| operation == required), "missing `{required}`");
        }
    }

    #[test]
    fn generated_annotations_cover_runtime_namespaces_and_compatibility_methods() {
        let annotations = generate_lua_api_annotations();
        for namespace in RUNTIME_HOST_NAMESPACES {
            assert!(
                annotations.contains(&format!("---@field {namespace} ")),
                "generated API omits runtime namespace `{namespace}`"
            );
        }
        for method in RUNTIME_QUERY_METHODS.iter().chain(RUNTIME_ACTION_METHODS) {
            assert!(
                annotations.contains(&format!("---@field {method} fun(")),
                "generated generic namespace API omits runtime method `{method}`"
            );
        }
        for operation in registered_host_operations() {
            assert!(annotations.contains(&format!("'{operation}'")), "generated API omits `{operation}`");
        }
    }
}