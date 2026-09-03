pub mod sync;

use crate::clipboard::ClipboardType;
use crate::error::RioError;
use neoism_terminal_core::ansi::graphics::UpdateQueues;
use neoism_terminal_core::colors::ColorRgb;
use neoism_terminal_core::crosswords::grid::Scroll;
use neoism_terminal_core::crosswords::pos::{Direction, Pos};
use neoism_terminal_core::crosswords::search::{Match, RegexSearch};
use neoism_window::event::Event as RioWindowEvent;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;
use teletypewriter::WinsizeBuilder;

use neoism_window::event_loop::EventLoopProxy;

pub type WindowId = neoism_window::window::WindowId;

#[derive(Debug, Clone)]
pub enum RioEventType {
    Rio(RioEvent),
    Frame,
    // Message(Message),
}

#[derive(Debug)]
pub enum Msg {
    /// Data that should be written to the PTY.
    Input(Cow<'static, [u8]>),

    #[allow(dead_code)]
    Shutdown,

    Resize(WinsizeBuilder),

    /// Re-home this PTY's parser driver onto a different host window.
    /// Sent when a workspace is detached/moved into another OS window so
    /// the live session keeps emitting `RioEvent`s tagged with the new
    /// `WindowId` instead of restarting the shell.
    RebindWindow(WindowId),
}

#[derive(Debug, Eq, PartialEq)]
pub enum ClickState {
    None,
    Click,
    DoubleClick,
    TripleClick,
}

// Phase 3b: `TerminalDamage` (and its `LineDamage` payload) moved
// into `neoism_terminal_core::damage`. External callers should import
// them from `neoism_terminal_core::damage::{LineDamage,
// TerminalDamage}` — no compatibility re-export here.

#[derive(Clone)]
pub enum RioEvent {
    PrepareRender(u64),
    PrepareRenderOnRoute(u64, usize),
    PrepareUpdateConfig,
    /// New terminal content available.
    Render,
    /// New terminal content available per route.
    RenderRoute(usize),
    /// Terminal content changed — lightweight notification (no damage payload).
    /// Damage stays in the terminal; renderer extracts it when it locks.
    TerminalDamaged(usize),
    /// Graphics update available from terminal.
    UpdateGraphics {
        route_id: usize,
        queues: UpdateQueues,
    },
    Paste,
    Copy(String),
    UpdateFontSize(u8),
    Scroll(Scroll),
    ToggleFullScreen,
    ToggleAppearanceTheme,
    Minimize(bool),
    Hide,
    HideOtherApplications,
    UpdateConfig,
    CreateWindow(Option<std::path::PathBuf>),
    CreateWindowWithOptions {
        working_dir: Option<std::path::PathBuf>,
        open_paths: Vec<std::path::PathBuf>,
    },
    CloseWindow,
    CreateNativeTab(Option<String>),
    CreateConfigEditor,
    SelectNativeTabByIndex(usize),
    SelectNativeTabLast,
    SelectNativeTabNext,
    SelectNativeTabPrev,

    ReportToAssistant(RioError),

    /// Grid has changed possibly requiring a mouse cursor shape change.
    MouseCursorDirty,

    /// Window title change.
    Title(String),

    /// Window title change.
    TitleWithSubtitle(String, String),

    /// Reset to the default window title.
    ResetTitle,

    /// Request to store a text string in the clipboard.
    ClipboardStore(ClipboardType, String),

    /// Rio-owned IDE tool installer finished on a worker thread.
    IdeToolInstallFinished {
        tool: String,
        success: bool,
        message: String,
    },

    /// Request from a terminal pane to open a path, or a fresh empty
    /// editor buffer, as a Neoism editor tab in the current workspace.
    OpenEditorTab {
        route_id: usize,
        path: Option<std::path::PathBuf>,
    },

    /// Request to write the contents of the clipboard to the PTY.
    ///
    /// The attached function is a formatter which will correctly transform the clipboard content
    /// into the expected escape sequence format.
    ClipboardLoad(
        usize,
        ClipboardType,
        Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ),

    /// Request to write the RGB value of a color to the PTY.
    ///
    /// The attached function is a formatter which will correctly transform the RGB color into the
    /// expected escape sequence format.
    ColorRequest(
        usize,
        usize,
        Arc<dyn Fn(ColorRgb) -> String + Sync + Send + 'static>,
    ),

    /// Write some text to the PTY.
    PtyWrite(usize, String),

    /// Request to write the text area size.
    TextAreaSizeRequest(
        usize,
        Arc<dyn Fn(WinsizeBuilder) -> String + Sync + Send + 'static>,
    ),

    /// Cursor blinking state has changed.
    CursorBlinkingChange,

    CursorBlinkingChangeOnRoute(usize),

    /// Progress bar report from OSC 9;4 sequence
    ProgressReport(ProgressReport),

    /// Terminal bell ring.
    Bell,

    /// Desktop notification from OSC 9 or OSC 777.
    DesktopNotification {
        title: String,
        body: String,
    },

    /// Shutdown request.
    Exit,

    /// Quit request.
    Quit,

    /// Leave current terminal.
    CloseTerminal(usize),

    BlinkCursor(u64, usize),

    /// Selection scroll tick — auto-scroll while dragging outside viewport.
    SelectionScrollTick,

    /// Notebook execution status tick — refresh running-cell elapsed/status UI.
    NotebookStatusTick,

    /// Git state watcher fired; debounce before refreshing the tree.
    PrepareRefreshFileTreeGitStatus,

    /// Refresh visible file-tree git badges after a debounced git event.
    RefreshFileTreeGitStatus,

    /// Worktree watcher fired; debounce before refreshing the visible tree.
    PrepareRefreshFileTree,

    /// Refresh visible file-tree entries after a debounced worktree event.
    RefreshFileTree,

    /// Background file-tree git refresh completed; apply the cached
    /// result on the UI thread.
    ApplyFileTreeGitStatus,
    /// The process-global native LSP worker published one or more complete
    /// document diagnostic snapshots. The application drains the mailbox on
    /// the UI thread and fans the snapshots to every window.
    CodeDiagnosticsReady,
    /// Background code-buffer git analysis completed. The application drains
    /// revision-tagged results and installs only still-current marks.
    CodeGitMarksReady,
    /// Delayed liveness check for a REMOTE (joined-workspace) file
    /// tree: fired a few seconds after a directory listing was
    /// dispatched over the daemon files plane. The handler re-issues
    /// the listing if the tree is still empty — covering dropped
    /// requests and reconnect races without polling.
    RemoteFileTreeCheck,

    /// Background ACP worker has new session/file/debug events to drain.
    AcpWake,

    /// Background Markdown note indexing finished; drain the workspace
    /// note-index result channel on the UI thread.
    WorkspaceNotesWake,

    /// Update window titles.
    UpdateTitles,

    /// A post-launch background check found a newer desktop release.
    UpdateAvailable {
        version: String,
    },

    /// Status from the existing `neoism update` command while GUI-driven.
    SelfUpdateProgress {
        percent: Option<u8>,
        message: String,
        ready_to_restart: bool,
        failed: bool,
    },

    /// Update terminal screen colors.
    ///
    /// The first usize is the route_id, the second is the color index to change.
    /// Color index: 0 for foreground, 1 for background, 2 for cursor color.
    ColorChange(usize, usize, Option<ColorRgb>),

    // No operation
    Noop,
}

impl Debug for RioEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RioEvent::ClipboardStore(ty, text) => {
                write!(f, "ClipboardStore({ty:?}, {text})")
            }
            RioEvent::IdeToolInstallFinished {
                tool,
                success,
                message,
            } => write!(f, "IdeToolInstallFinished({tool}, {success}, {message})"),
            RioEvent::OpenEditorTab { route_id, path } => {
                write!(f, "OpenEditorTab({path:?} on route {route_id})")
            }
            RioEvent::ClipboardLoad(route_id, ty, _) => {
                write!(f, "ClipboardLoad({ty:?} on route {route_id})")
            }
            RioEvent::TextAreaSizeRequest(route_id, _) => {
                write!(f, "TextAreaSizeRequest on route {route_id}")
            }
            RioEvent::ColorRequest(route_id, index, _) => {
                write!(f, "ColorRequest({index}) on route {route_id}")
            }
            RioEvent::PtyWrite(route_id, text) => {
                write!(f, "PtyWrite({text}) on route {route_id}")
            }
            RioEvent::Title(title) => write!(f, "Title({title})"),
            RioEvent::TitleWithSubtitle(title, subtitle) => {
                write!(f, "TitleWithSubtitle({title}, {subtitle})")
            }
            RioEvent::Minimize(cond) => write!(f, "Minimize({cond})"),
            RioEvent::Hide => write!(f, "Hide)"),
            RioEvent::HideOtherApplications => write!(f, "HideOtherApplications)"),
            RioEvent::CursorBlinkingChange => write!(f, "CursorBlinkingChange"),
            RioEvent::CursorBlinkingChangeOnRoute(route_id) => {
                write!(f, "CursorBlinkingChangeOnRoute {route_id}")
            }
            RioEvent::ProgressReport(report) => {
                write!(f, "ProgressReport({:?})", report)
            }
            RioEvent::MouseCursorDirty => write!(f, "MouseCursorDirty"),
            RioEvent::ResetTitle => write!(f, "ResetTitle"),
            RioEvent::PrepareUpdateConfig => write!(f, "PrepareUpdateConfig"),
            RioEvent::PrepareRender(millis) => write!(f, "PrepareRender({millis})"),
            RioEvent::PrepareRenderOnRoute(millis, route) => {
                write!(f, "PrepareRender({millis} on route {route})")
            }
            RioEvent::Render => write!(f, "Render"),
            RioEvent::RenderRoute(route) => write!(f, "Render route {route}"),
            RioEvent::TerminalDamaged(route_id) => {
                write!(f, "TerminalDamaged route {route_id}")
            }
            RioEvent::Scroll(scroll) => write!(f, "Scroll {scroll:?}"),
            RioEvent::Bell => write!(f, "Bell"),
            RioEvent::DesktopNotification { title, body } => {
                write!(f, "DesktopNotification({title}, {body})")
            }
            RioEvent::Exit => write!(f, "Exit"),
            RioEvent::Quit => write!(f, "Quit"),
            RioEvent::CloseTerminal(route) => write!(f, "CloseTerminal {route}"),
            RioEvent::CreateWindow(_) => write!(f, "CreateWindow"),
            RioEvent::CreateWindowWithOptions {
                working_dir,
                open_paths,
            } => write!(
                f,
                "CreateWindowWithOptions(working_dir={working_dir:?}, open_paths={open_paths:?})"
            ),
            RioEvent::CloseWindow => write!(f, "CloseWindow"),
            RioEvent::CreateNativeTab(_) => write!(f, "CreateNativeTab"),
            RioEvent::SelectNativeTabByIndex(tab_index) => {
                write!(f, "SelectNativeTabByIndex({tab_index})")
            }
            RioEvent::SelectNativeTabLast => write!(f, "SelectNativeTabLast"),
            RioEvent::SelectNativeTabNext => write!(f, "SelectNativeTabNext"),
            RioEvent::SelectNativeTabPrev => write!(f, "SelectNativeTabPrev"),
            RioEvent::CreateConfigEditor => write!(f, "CreateConfigEditor"),
            RioEvent::UpdateConfig => write!(f, "ReloadConfiguration"),
            RioEvent::ReportToAssistant(error_report) => {
                write!(f, "ReportToAssistant({})", error_report.report)
            }
            RioEvent::ToggleFullScreen => write!(f, "FullScreen"),
            RioEvent::ToggleAppearanceTheme => write!(f, "ToggleAppearanceTheme"),
            RioEvent::BlinkCursor(timeout, route_id) => {
                write!(f, "BlinkCursor {timeout} {route_id}")
            }
            RioEvent::SelectionScrollTick => write!(f, "SelectionScrollTick"),
            RioEvent::NotebookStatusTick => write!(f, "NotebookStatusTick"),
            RioEvent::PrepareRefreshFileTreeGitStatus => {
                write!(f, "PrepareRefreshFileTreeGitStatus")
            }
            RioEvent::RefreshFileTreeGitStatus => {
                write!(f, "RefreshFileTreeGitStatus")
            }
            RioEvent::PrepareRefreshFileTree => {
                write!(f, "PrepareRefreshFileTree")
            }
            RioEvent::RefreshFileTree => {
                write!(f, "RefreshFileTree")
            }
            RioEvent::ApplyFileTreeGitStatus => {
                write!(f, "ApplyFileTreeGitStatus")
            }
            RioEvent::CodeDiagnosticsReady => write!(f, "CodeDiagnosticsReady"),
            RioEvent::CodeGitMarksReady => write!(f, "CodeGitMarksReady"),
            RioEvent::RemoteFileTreeCheck => {
                write!(f, "RemoteFileTreeCheck")
            }
            RioEvent::AcpWake => write!(f, "AcpWake"),
            RioEvent::WorkspaceNotesWake => write!(f, "WorkspaceNotesWake"),
            RioEvent::UpdateTitles => write!(f, "UpdateTitles"),
            RioEvent::UpdateAvailable { version } => {
                write!(f, "UpdateAvailable({version})")
            }
            RioEvent::SelfUpdateProgress {
                percent,
                message,
                ready_to_restart,
                failed,
            } => write!(
                f,
                "SelfUpdateProgress({percent:?}, {message}, ready={ready_to_restart}, failed={failed})"
            ),
            RioEvent::Noop => write!(f, "Noop"),
            RioEvent::Copy(_) => write!(f, "Copy"),
            RioEvent::Paste => write!(f, "Paste"),
            RioEvent::UpdateFontSize(action) => write!(f, "UpdateFontSize({action:?})"),
            RioEvent::UpdateGraphics { route_id, .. } => {
                write!(f, "UpdateGraphics({route_id})")
            }
            RioEvent::ColorChange(route_id, color, rgb) => {
                write!(f, "ColorChange({route_id}, {color:?}, {rgb:?})")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventPayload {
    /// Event payload.
    pub payload: RioEventType,
    pub window_id: WindowId,
}

impl EventPayload {
    pub fn new(payload: RioEventType, window_id: WindowId) -> Self {
        Self { payload, window_id }
    }
}

impl From<EventPayload> for RioWindowEvent<EventPayload> {
    fn from(event: EventPayload) -> Self {
        RioWindowEvent::UserEvent(event)
    }
}

pub trait OnResize {
    fn on_resize(&mut self, window_size: WinsizeBuilder);
}

/// Event Loop for notifying the renderer about terminal events.
pub trait EventListener {
    fn event(&self) -> (Option<RioEvent>, bool);

    fn send_event(&self, _event: RioEvent, _id: WindowId) {}

    fn send_event_with_high_priority(&self, _event: RioEvent, _id: WindowId) {}

    fn send_redraw(&self, _id: WindowId) {}

    fn send_global_event(&self, _event: RioEvent) {}
}

#[derive(Clone)]
pub struct VoidListener;

impl From<RioEvent> for RioEventType {
    fn from(rio_event: RioEvent) -> Self {
        Self::Rio(rio_event)
    }
}

impl EventListener for VoidListener {
    fn event(&self) -> (std::option::Option<RioEvent>, bool) {
        (None, false)
    }
}

#[derive(Debug, Clone)]
pub struct EventProxy {
    proxy: EventLoopProxy<EventPayload>,
}

impl EventProxy {
    pub fn new(proxy: EventLoopProxy<EventPayload>) -> Self {
        Self { proxy }
    }

    pub fn send_event(&self, event: RioEventType, id: WindowId) {
        let _ = self.proxy.send_event(EventPayload::new(event, id));
    }
}

impl EventListener for EventProxy {
    fn event(&self) -> (std::option::Option<RioEvent>, bool) {
        (None, false)
    }

    fn send_event(&self, event: RioEvent, id: WindowId) {
        let _ = self.proxy.send_event(EventPayload::new(event.into(), id));
    }
}

/// Regex search state.
pub struct SearchState {
    /// Search direction.
    pub direction: Direction,

    /// Current position in the search history.
    pub history_index: Option<usize>,

    /// Change in display offset since the beginning of the search.
    pub display_offset_delta: i32,

    /// Search origin in viewport coordinates relative to original display offset.
    pub origin: Pos,

    /// Focused match during active search.
    pub focused_match: Option<Match>,

    /// Search regex and history.
    ///
    /// During an active search, the first element is the user's current input.
    ///
    /// While going through history, the [`SearchState::history_index`] will point to the element
    /// in history which is currently being previewed.
    pub history: VecDeque<String>,

    /// Compiled search automatons.
    pub dfas: Option<RegexSearch>,
}

impl SearchState {
    /// Search regex text if a search is active.
    pub fn regex(&self) -> Option<&String> {
        self.history_index.and_then(|index| self.history.get(index))
    }

    /// Direction of the search from the search origin.
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// Focused match during vi-less search.
    pub fn focused_match(&self) -> Option<&Match> {
        self.focused_match.as_ref()
    }

    /// Clear the focused match.
    pub fn clear_focused_match(&mut self) {
        self.focused_match = None;
    }

    /// Active search dfas.
    pub fn dfas_mut(&mut self) -> Option<&mut RegexSearch> {
        self.dfas.as_mut()
    }

    /// Active search dfas.
    pub fn dfas(&self) -> Option<&RegexSearch> {
        self.dfas.as_ref()
    }

    /// Search regex text if a search is active.
    pub fn regex_mut(&mut self) -> Option<&mut String> {
        self.history_index
            .and_then(move |index| self.history.get_mut(index))
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            direction: Direction::Right,
            display_offset_delta: Default::default(),
            focused_match: Default::default(),
            history_index: Default::default(),
            history: Default::default(),
            origin: Default::default(),
            dfas: Default::default(),
        }
    }
}

/// Progress bar state for OSC 9;4 ConEmu/Windows Terminal progress reporting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    /// Remove/hide the progress bar (state 0)
    Remove,
    /// Set progress with a specific percentage (state 1)
    Set,
    /// Show error state (state 2)
    Error,
    /// Indeterminate/pulsing progress (state 3)
    Indeterminate,
    /// Paused progress (state 4)
    Pause,
}

/// Progress report from OSC 9;4 sequence
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressReport {
    /// The progress bar state
    pub state: ProgressState,
    /// Optional progress percentage (0-100), only used with Set, Error, and Pause states
    pub progress: Option<u8>,
}

impl From<ProgressState> for neoism_terminal_core::ProgressState {
    fn from(value: ProgressState) -> Self {
        match value {
            ProgressState::Remove => Self::Remove,
            ProgressState::Set => Self::Set,
            ProgressState::Error => Self::Error,
            ProgressState::Indeterminate => Self::Indeterminate,
            ProgressState::Pause => Self::Pause,
        }
    }
}

impl From<neoism_terminal_core::ProgressState> for ProgressState {
    fn from(value: neoism_terminal_core::ProgressState) -> Self {
        match value {
            neoism_terminal_core::ProgressState::Remove => Self::Remove,
            neoism_terminal_core::ProgressState::Set => Self::Set,
            neoism_terminal_core::ProgressState::Error => Self::Error,
            neoism_terminal_core::ProgressState::Indeterminate => Self::Indeterminate,
            neoism_terminal_core::ProgressState::Pause => Self::Pause,
        }
    }
}

impl From<ProgressReport> for neoism_terminal_core::ProgressReport {
    fn from(value: ProgressReport) -> Self {
        Self {
            state: value.state.into(),
            progress: value.progress,
        }
    }
}

impl From<neoism_terminal_core::ProgressReport> for ProgressReport {
    fn from(value: neoism_terminal_core::ProgressReport) -> Self {
        Self {
            state: value.state.into(),
            progress: value.progress,
        }
    }
}
