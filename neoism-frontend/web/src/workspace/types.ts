// TypeScript mirrors of `neoism-protocol`.
//
// Serde defaults are externally-tagged enums for non-unit variants, which
// serialize as `{ "<VariantName>": { ...fields } }`. These types match
// exactly. If `neoism-protocol` adds or renames a variant, update both
// sides together.
//
// Pty messages mirror `neoism-protocol/src/pty.rs`.
// Files messages mirror `neoism-protocol/src/files.rs`.
// Git messages mirror `neoism-protocol/src/git.rs`.
//
// Files/Git requests are wrapped on the wire with a `request_id` so the
// JS side can correlate replies to outstanding promises. The wrappers
// piggyback on the existing externally-tagged top-level `ClientMessage`
// / `ServerMessage` enums via `Files`/`FilesReply` and `Git`/`GitReply`
// variants — the daemon will adopt the matching shape when the wire
// surface for files/git is exposed there.

export interface CreatePtyArgs {
  cwd: string | null;
  cols: number;
  rows: number;
  /// Optional explicit shell path. Omit (or set to `null`) to let the
  /// daemon fall back to `$SHELL` / `/bin/sh`.
  shell?: string | null;
}

export interface PtyInputArgs {
  session_id: string;
  bytes: number[];
}

export interface ResizeArgs {
  session_id: string;
  cols: number;
  rows: number;
}

export interface ClosePtyArgs {
  session_id: string;
}

export type ClientMessage =
  | { CreatePty: CreatePtyArgs }
  | { PtyInput: PtyInputArgs }
  | { Resize: ResizeArgs }
  | { ClosePty: ClosePtyArgs }
  | { Files: FilesEnvelope }
  | { Git: GitEnvelope }
  | { Editor: EditorEnvelope }
  | { Agent: AgentEnvelope }
  | { Search: SearchEnvelope }
  | { Workspace: WorkspaceEnvelope }
  | { Diagnostics: DiagnosticsEnvelope }
  | { CursorOverlay: CursorOverlayEnvelope }
  | { Crdt: CrdtEnvelope };

export interface PtyCreatedArgs {
  session_id: string;
  workspace_root?: string | null;
}

export interface PtyOutputArgs {
  session_id: string;
  bytes: number[];
}

export interface PtyClosedArgs {
  session_id: string;
  exit_code: number | null;
}

export interface SessionCwdArgs {
  session_id: string;
  cwd: string;
}

export interface ErrorArgs {
  message: string;
}

export type ServerMessage =
  | { PtyCreated: PtyCreatedArgs }
  | { PtyOutput: PtyOutputArgs }
  | { PtyClosed: PtyClosedArgs }
  | { SessionCwd: SessionCwdArgs }
  | { Error: ErrorArgs }
  | { FilesReply: FilesReplyEnvelope }
  | { GitReply: GitReplyEnvelope }
  | { EditorReply: EditorReplyEnvelope }
  | { AgentReply: AgentReplyEnvelope }
  | { SearchReply: SearchReplyEnvelope }
  | { WorkspaceReply: WorkspaceReplyEnvelope }
  | { DiagnosticsReply: DiagnosticsReplyEnvelope }
  | { CursorOverlayReply: CursorOverlayReplyEnvelope }
  | { CrdtReply: CrdtReplyEnvelope };

// Files messages ------------------------------------------------------
// Mirrors `neoism-protocol/src/files.rs`.

export interface DirEntry {
  name: string;
  is_dir: boolean;
  size: number | null;
}

export interface TreeEntry {
  path: string;
  is_dir: boolean;
  depth: number;
}

export type FilesClientMessage =
  | { ListDir: { path: string } }
  | { Stat: { path: string } }
  | { ReadFile: { path: string } }
  | { WriteFile: { path: string; bytes: number[] } }
  | { WalkTree: { path: string; max_depth: number | null } }
  | { CreateFile: { dir: string; name: string } }
  | { CreateDir: { dir: string; name: string } }
  | { Rename: { from: string; to: string } }
  | { Delete: { path: string } }
  | { ReadShellHistory: { max_entries: number | null } };

export type FilesServerMessage =
  | { DirListing: { path: string; entries: DirEntry[] } }
  | { Stat: { path: string; entry: DirEntry } }
  | { FileContent: { path: string; bytes: number[] } }
  | { FileWritten: { path: string; bytes_written: number } }
  | { TreeListing: { path: string; entries: TreeEntry[] } }
  | { FileCreated: { path: string; is_dir: boolean } }
  | { Renamed: { from: string; to: string } }
  | { Deleted: { path: string; was_dir: boolean } }
  | { ShellHistory: { entries: string[] } }
  | { Error: { message: string } };

export interface FilesEnvelope {
  request_id: number;
  workspace_root?: string | null;
  message: FilesClientMessage;
}

export interface FilesReplyEnvelope {
  request_id: number;
  message: FilesServerMessage;
}

// Git messages --------------------------------------------------------
// Mirrors `neoism-protocol/src/git.rs`.

export type GitFileStatus =
  | "Modified"
  | "Added"
  | "Deleted"
  | "Renamed"
  | "Untracked"
  | "Conflicted";

export interface GitStatusEntry {
  path: string;
  status: GitFileStatus;
}

export interface DiffHunk {
  path: string;
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  patch: string;
}

export interface CommitSummary {
  sha: string;
  short_sha: string;
  author: string;
  message: string;
  timestamp: number;
}

export type GitClientMessage =
  | "Status"
  | { Diff: { path: string | null } }
  | { Log: { max_count: number | null } };

export type GitServerMessage =
  | { Status: { entries: GitStatusEntry[] } }
  | { Diff: { hunks: DiffHunk[] } }
  | { Log: { commits: CommitSummary[] } }
  | { Branch: { name: string | null } }
  | { Changes: { added: number; deleted: number } }
  | { Error: { message: string } };

export interface GitEnvelope {
  request_id: number;
  message: GitClientMessage;
}

export interface GitReplyEnvelope {
  request_id: number;
  message: GitServerMessage;
}

// Editor (nvim proxy) messages -----------------------------------------
// Mirrors `neoism-protocol/src/editor.rs`. The daemon spawns
// `nvim --embed` per session and pipes parsed `ext_linegrid` redraw
// events back over the existing socket.

export interface GridCell {
  row: number;
  col: number;
  /** Cell text — usually one grapheme; `""` for the trailing half of
   *  a double-width glyph. */
  ch: string;
  /** Resolved fg as `0x00RRGGBB`. */
  fg: number;
  /** Resolved bg as `0x00RRGGBB`. */
  bg: number;
  /** Bitfield: 0 bold, 1 italic, 2 underline, 3 undercurl,
   *  4 strikethrough, 5 reverse. */
  attrs: number;
}

export interface GridPos {
  row: number;
  col: number;
}

export interface PopupMenuItem {
  word: string;
  kind: string;
  menu: string;
  info: string;
}

export type DiagnosticSeverity = "Error" | "Warn" | "Info" | "Hint";

export interface DiagnosticRelatedInformation {
  path: string;
  line: number;
  col: number;
  end_line: number;
  end_col: number;
  message: string;
}

export interface DiagnosticItem {
  severity: DiagnosticSeverity;
  message: string;
  source: string | null;
  line: number;
  col: number;
  end_line: number;
  end_col: number;
  lnum: number;
  code?: string | null;
  code_description?: string | null;
  tags?: string[];
  related_information?: DiagnosticRelatedInformation[];
}

export interface LspSnapshotServer {
  name: string;
  binary: string;
  filetype: string;
  state: string;
  source?: string | null;
  message?: string | null;
  level?: string | null;
}

export interface HighlightAttrs {
  fg?: number | null;
  bg?: number | null;
  sp?: number | null;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  undercurl: boolean;
  strikethrough: boolean;
  reverse: boolean;
}

export type EditorLspAction =
  | "hover"
  | "definition"
  | "references"
  | "implementation"
  | "document_symbols"
  | "workspace_symbols"
  | "info"
  | "format"
  | "code_actions"
  | "rename"
  | "toggle_inlay_hints";

export interface EditorLspCodeAction {
  server_id: string;
  file_path: string;
  document_revision: string;
  title: string;
  kind?: string | null;
  preferred: boolean;
  disabled_reason?: string | null;
  payload: unknown;
}

export interface EditorLspCompletionItem {
  server_id?: string | null;
  file_path: string;
  document_revision: string;
  label: string;
  kind: string;
  detail?: string | null;
  documentation?: string | null;
  insert_text: string;
  filter_text?: string | null;
  sort_text?: string | null;
  preselect: boolean;
  payload?: unknown;
}

export type EditorClientMessage =
  | { OpenBuffer: { path: string; surface_id?: string | null } }
  | { SendKeys: { bytes: number[]; surface_id?: string | null } }
  | {
      MouseInput: {
        button: string;
        action: string;
        modifier: string;
        grid: number;
        row: number;
        col: number;
        count: number;
        surface_id?: string | null;
      };
    }
  | { Resize: { width: number; height: number; surface_id?: string | null } }
  | {
      LspAction: {
        action: EditorLspAction;
        text?: string | null;
        surface_id?: string | null;
      };
    }
  | {
      ApplyLspCodeAction: {
        action: EditorLspCodeAction;
        surface_id?: string | null;
      };
    }
  | {
      LspComplete: {
        seq: number;
        trigger_character?: string | null;
        surface_id?: string | null;
      };
    }
  | {
      ApplyLspCompletion: {
        item: EditorLspCompletionItem;
        replace_prefix: string;
        surface_id?: string | null;
      };
    }
  | {
      CancelLspCompletion: {
        surface_id?: string | null;
      };
    }
  | {
      LspHoverAt: {
        seq: number;
        grid: number;
        row: number;
        col: number;
        surface_id?: string | null;
      };
    }
  | "Close";

export type EditorServerMessage =
  | {
      Batch: {
        surface_id?: string | null;
        messages: EditorServerMessage[];
      };
    }
  | {
      GridUpdate: {
        surface_id?: string | null;
        grid_id: number;
        width: number;
        height: number;
        cells: GridCell[];
        cursor: GridPos | null;
        mode: string | null;
      };
    }
  | {
      GridResize: {
        surface_id?: string | null;
        grid_id: number;
        width: number;
        height: number;
      };
    }
  | {
      GridClear: {
        surface_id?: string | null;
        grid_id: number;
      };
    }
  | {
      GridScroll: {
        surface_id?: string | null;
        grid_id: number;
        top: number;
        bot: number;
        left: number;
        right: number;
        rows: number;
        cols: number;
      };
    }
  | {
      CursorGoto: {
        surface_id?: string | null;
        grid_id: number;
        row: number;
        col: number;
      };
    }
  | {
      HighlightDefined: {
        surface_id?: string | null;
        hl_id: number;
        attrs: HighlightAttrs;
      };
    }
  | {
      WinViewport: {
        surface_id?: string | null;
        grid_id: number;
        topline: number;
        botline: number;
        line_count: number;
        scroll_delta: number;
        /** Buffer-coordinate cursor (0-based) — what presence
         *  publishes so remote screens draw the true line. Absent on
         *  older daemons. */
        curline?: number;
        curcol?: number;
        /** Gutter width in cells. */
        textoff?: number;
      };
    }
  | {
      DefaultColors: {
        surface_id?: string | null;
        rgb_fg: number;
        rgb_bg: number;
        rgb_sp: number;
      };
    }
  | {
      PopupMenu: {
        surface_id?: string | null;
        items: PopupMenuItem[];
        selected: number | null;
        anchor: GridPos;
        grid_id: number;
      };
    }
  | { PopupMenuSelect: { surface_id?: string | null; selected: number | null } }
  | { PopupHide: { surface_id?: string | null } }
  | { MouseMode: { surface_id?: string | null; enabled: boolean } }
  | {
      Diagnostics: {
        surface_id?: string | null;
        error?: number;
        warn?: number;
        info?: number;
        hint?: number;
        file_path?: string | null;
        items: DiagnosticItem[];
      };
    }
  | {
      LspStatus: {
        surface_id?: string | null;
        state: string;
        name?: string | null;
        binary?: string | null;
        filetype?: string | null;
      };
    }
  | {
      LspSnapshot: {
        surface_id?: string | null;
        file_path?: string | null;
        filetype: string;
        servers: LspSnapshotServer[];
      };
    }
  | {
      LspMessage: {
        surface_id?: string | null;
        server: string;
        text: string;
        level: string;
      };
    }
  | {
      LspActionResult: {
        surface_id?: string | null;
        action: EditorLspAction;
        line: number;
        character: number;
        summary: string;
        hover?: string | null;
        locations: Array<{ uri: string; line: number; character: number }>;
        symbol_count: number;
        symbols: Array<{
          name: string;
          kind: string;
          detail?: string | null;
          uri: string;
          line: number;
          character: number;
          depth: number;
        }>;
        code_actions: EditorLspCodeAction[];
      };
    }
  | {
      LspCompletions: {
        surface_id?: string | null;
        seq: number;
        replace_prefix: string;
        items: EditorLspCompletionItem[];
      };
    }
  | {
      LspHoverResult: {
        surface_id?: string | null;
        seq: number;
        line: number;
        character: number;
        contents: string;
      };
    }
  | { ModeChange: { surface_id?: string | null; mode: string; mode_idx: number } }
  | {
      BufferOpened: {
        surface_id?: string | null;
        path: string;
        line_count: number;
      };
    }
  | {
      BufferModified: {
        surface_id?: string | null;
        path: string;
        modified: boolean;
      };
    }
  | { Notification: { surface_id?: string | null; message: string; level: string } }
  | {
      YankFlash: {
        surface_id?: string | null;
        row_top: number;
        row_bot: number;
        col_left?: number | null;
        col_right?: number | null;
      };
    }
  | { Closed: { surface_id?: string | null; reason: string | null } }
  | { Error: { surface_id?: string | null; message: string } };

export interface EditorEnvelope {
  request_id: number;
  workspace_root?: string | null;
  message: EditorClientMessage;
}

export interface EditorReplyEnvelope {
  request_id: number;
  message: EditorServerMessage;
}

// CRDT / presence messages -------------------------------------------
// Mirrors `neoism-protocol/src/crdt.rs`. K3 uses only presence
// variants in the web frontend; sync payloads are typed so frames do
// not get dropped when K4 broadcasts them over the same service.

export interface CrdtCursorPosition {
  line: number;
  column: number;
  offset?: number | null;
}

export interface CrdtSelectionRange {
  anchor: CrdtCursorPosition;
  head: CrdtCursorPosition;
}

export interface CrdtPresenceColor {
  r: number;
  g: number;
  b: number;
}

export interface CrdtPeerPresence {
  buffer_id: string;
  peer_id: string;
  display_name: string;
  color: CrdtPresenceColor;
  cursor: CrdtCursorPosition;
  selection?: CrdtSelectionRange | null;
  /** Peer is in insert/replace mode → thin beam; normal → block. */
  insert?: boolean;
  /** Peer's cursor uses the animated rainbow preset — receivers
   *  ignore `color` and animate the rainbow locally. */
  rainbow?: boolean;
  updated_at_ms: number;
}

export interface CrdtBufferUpdate {
  buffer_id: string;
  origin_client_id: number;
  update_v1: number[];
  state_vector_v1?: number[];
}

export interface CrdtSyncEnvelope {
  buffer_id: string;
  origin_client_id: number;
  update_v1?: number[];
  state_vector_v1?: number[];
}

export type CrdtPresenceUpdate =
  | { Upsert: CrdtPeerPresence }
  | { Remove: { buffer_id: string; peer_id: string } };

export type CrdtClientMessage =
  | { OpenBuffer: { buffer_id: string; initial_text?: string } }
  | { RequestSnapshot: { buffer_id: string; state_vector_v1?: number[] } }
  | { ApplyUpdate: { update: CrdtBufferUpdate } }
  | { ApplySync: { envelope: CrdtSyncEnvelope } }
  | { PublishPresence: { presence: CrdtPeerPresence } }
  | { ClearPresence: { buffer_id: string; peer_id: string } }
  | {
      RequestPresenceSnapshot: {
        buffer_id: string;
        exclude_peer_id?: string | null;
      };
    }
  | { RequestCompactionStatus: { buffer_id: string } }
  | { SaveBuffer: { buffer_id: string } };

export type CrdtServerMessage =
  | { Snapshot: { buffer_id: string; update_v1: number[]; state_vector_v1: number[] } }
  | {
      SnapshotFallback: {
        buffer_id: string;
        update_v1: number[];
        state_vector_v1: number[];
        compacted_through_state_vector_v1: number[];
        reason: string;
      };
    }
  | { Update: { update: CrdtBufferUpdate } }
  | { Sync: { envelope: CrdtSyncEnvelope } }
  | { Presence: { update: CrdtPresenceUpdate } }
  | { PresenceSnapshot: { buffer_id: string; peers: CrdtPeerPresence[] } }
  | { CompactionStatus: unknown }
  | { Saved: { buffer_id: string; bytes_written: number } }
  | { Error: { buffer_id?: string | null; message: string } };

export interface CrdtEnvelope {
  request_id: number;
  message: CrdtClientMessage;
}

export interface CrdtReplyEnvelope {
  request_id: number;
  message: CrdtServerMessage;
}

// Agent (Claude API proxy) messages ----------------------------------
// Mirrors `neoism-protocol/src/agent.rs`. The daemon runs the streaming
// SSE pump against `api.anthropic.com` and emits server messages back
// over the existing socket.

export type Role = "User" | "Assistant" | "System";

export type ContentKind =
  | "Text"
  | "Reasoning"
  | { Tool: { name: string } };

export type PermissionDecision = "Yes" | "Always" | "No";

export interface Attachment {
  kind: string;
  path: string | null;
  bytes: number[];
}

export type StreamingState =
  | "Idle"
  | "Thinking"
  | "Working"
  | "Generating"
  | "Compacting"
  | "WaitingSubagents";

export type NoticeLevel = "Info" | "Warn" | "Error";

export type ToolStatus =
  | "Pending"
  | "Running"
  | "Completed"
  | "Failed"
  | "Cancelled";

export type SubagentStatus = "Running" | "Blocked" | "Completed" | "Failed";

export type CompactionPhase = "Started" | "Delta" | "Ended";

export type HistoryMessageKind =
  | "User"
  | "Assistant"
  | "Reasoning"
  | "Tool"
  | "System"
  | "Subtask"
  | "Compaction";

export interface TodoItem {
  status: string;
  content: string;
}

export interface Usage {
  input: number;
  output: number;
  reasoning?: number;
  cache_read?: number;
  cache_write?: number;
  total?: number;
  cost_micros?: number;
  context_limit?: number | null;
}

export interface ThreadSummary {
  session_id: string;
  title: string;
  directory?: string | null;
  model?: string | null;
  agent?: string | null;
  updated_at?: number;
  message_count?: number;
  busy?: boolean;
}

export interface HistoryMessage {
  id: string;
  role: Role;
  kind: HistoryMessageKind;
  title?: string;
  text?: string;
  status?: string;
  tool?: string;
  lang?: string;
  line_offset?: number | null;
  detail?: string;
  todos?: TodoItem[];
  usage?: Usage | null;
  created_at?: number;
}

export interface ModelInfo {
  id: string;
  name: string;
  context_limit?: number | null;
}

export interface ProviderInfo {
  id: string;
  name: string;
  models?: ModelInfo[];
}

export interface AgentInfo {
  name: string;
  description: string;
  mode?: string | null;
}

export interface SkillInfo {
  name: string;
  description: string;
  path?: string | null;
}

export type AgentClientMessage =
  // -- Original direct-proxy surface (preserved verbatim) ----------
  | { SendMessage: { text: string; attachments: Attachment[] } }
  | "Cancel"
  | "NewThread"
  | {
      ReplyPermission: {
        request_id: number;
        decision: PermissionDecision;
      };
    }
  // -- Session lifecycle -------------------------------------------
  | {
      CreateThread: {
        title?: string | null;
        directory?: string | null;
        agent?: string | null;
        model?: string | null;
      };
    }
  | { SwitchThread: { session_id: string } }
  | { DeleteThread: { session_id: string } }
  | {
      ListThreads: {
        directory?: string | null;
        limit?: number | null;
      };
    }
  | {
      GetHistory: {
        session_id: string;
        cursor?: string | null;
        limit?: number | null;
      };
    }
  | { ResumeStream: { session_id: string } }
  | { StopStream: { session_id: string } }
  // -- Prompt / submission -----------------------------------------
  | {
      SubmitPrompt: {
        session_id: string;
        text: string;
        attachments?: Attachment[];
        mode?: string | null;
        model?: string | null;
        thinking?: string | null;
      };
    }
  | { CancelInflight: { session_id: string } }
  | { EnqueuePrompt: { session_id: string; text: string } }
  | { ClearQueue: { session_id: string } }
  | { RetryLast: { session_id: string } }
  // -- Tool / permission gating ------------------------------------
  | {
      ApproveTool: {
        request_id: string;
        session_id: string;
        decision: PermissionDecision;
      };
    }
  | {
      DenyTool: {
        request_id: string;
        session_id: string;
      };
    }
  // -- Edit proposals ----------------------------------------------
  | { ApplyEdit: { session_id: string; edit_id: string } }
  | { RejectEdit: { session_id: string; edit_id: string } }
  // -- Provider / model / agent configuration ----------------------
  | { SetProvider: { session_id: string; provider_id: string } }
  | {
      SetModel: {
        session_id: string;
        model: string;
        thinking?: string | null;
      };
    }
  | { SetAgent: { session_id: string; agent: string } }
  | { SetThinkingMode: { session_id: string; thinking: string } }
  | "ListProviders"
  // -- Local context -----------------------------------------------
  | { ListAgents: { directory?: string | null } }
  | { ListSkills: { directory?: string | null } }
  | {
      StartSubagent: {
        session_id: string;
        agent: string;
        prompt?: string | null;
      };
    }
  // -- Maintenance -------------------------------------------------
  | { Compact: { session_id: string } }
  | { SetTitle: { session_id: string; title: string } }
  | "Ping";

export type AgentServerMessage =
  // -- Original direct-proxy surface -------------------------------
  | { Disabled: { reason: string } }
  | { MessageStart: { session_id: string; role: Role; message_id: string } }
  | {
      ContentDelta: {
        session_id: string;
        message_id: string;
        kind: ContentKind;
        text: string;
      };
    }
  | {
      MessageEnd: {
        session_id: string;
        message_id: string;
        stop_reason: string;
      };
    }
  | {
      PermissionRequest: {
        request_id: number;
        tool: string;
        args: unknown;
      };
    }
  | { Error: { message: string } }
  // -- Session lifecycle -------------------------------------------
  | {
      ThreadCreated: {
        session_id: string;
        title?: string | null;
        directory?: string | null;
        agent?: string | null;
        model?: string | null;
      };
    }
  | { ThreadSwitched: { session_id: string } }
  | { ThreadDeleted: { session_id: string } }
  | { ThreadList: { threads: ThreadSummary[] } }
  | {
      HistoryChunk: {
        session_id: string;
        messages: HistoryMessage[];
        next_cursor?: string | null;
      };
    }
  | {
      SessionEvent: {
        session_id: string;
        kind: string;
        properties: unknown;
      };
    }
  | {
      MessageUpdated: {
        session_id: string;
        message: HistoryMessage;
      };
    }
  | { PartRemoved: { session_id: string; part_id: string } }
  | { SessionIdle: { session_id: string } }
  | {
      StreamingState: {
        session_id: string;
        state: StreamingState;
        label?: string | null;
      };
    }
  | {
      Notice: {
        session_id: string;
        title: string;
        body: string;
        level: NoticeLevel;
      };
    }
  // -- Tool / permission gating ------------------------------------
  | {
      ToolUseRequest: {
        session_id: string;
        request_id: string;
        tool: string;
        title: string;
        patterns?: string[];
        args?: unknown;
        source_agent?: string | null;
      };
    }
  | {
      ToolUseResult: {
        session_id: string;
        tool_use_id: string;
        tool: string;
        status: ToolStatus;
        output?: string | null;
        error?: string | null;
      };
    }
  // -- Edit proposals ----------------------------------------------
  | {
      EditProposed: {
        session_id: string;
        edit_id: string;
        path: string;
        patch: string;
        tool?: string | null;
      };
    }
  | {
      EditApplied: {
        session_id: string;
        edit_id: string;
        path: string;
        bytes_written: number;
      };
    }
  | {
      EditRejected: {
        session_id: string;
        edit_id: string;
        path: string;
        reason?: string | null;
      };
    }
  // -- Provider / model / agent state ------------------------------
  | {
      ProviderState: {
        session_id: string;
        provider_id?: string | null;
        model?: string | null;
        agent?: string | null;
        thinking?: string | null;
        context_limit?: number | null;
      };
    }
  | { ProviderCatalog: { providers: ProviderInfo[] } }
  | { AgentCatalog: { agents: AgentInfo[] } }
  | { SkillCatalog: { skills: SkillInfo[] } }
  | { UsageUpdate: { session_id: string; usage: Usage } }
  | { TodoUpdate: { session_id: string; todos: TodoItem[] } }
  | {
      QueueUpdate: {
        session_id: string;
        count: number;
        preview?: string | null;
        started_at?: number | null;
      };
    }
  | {
      SubagentUpdate: {
        session_id: string;
        status: SubagentStatus;
        title?: string | null;
        agent?: string | null;
        current_tool?: string | null;
        started_at?: number | null;
      };
    }
  | {
      Compaction: {
        session_id: string;
        phase: CompactionPhase;
        text?: string | null;
        reason?: string | null;
      };
    }
  | "Pong";

export interface AgentEnvelope {
  request_id: number;
  message: AgentClientMessage;
}

export interface AgentReplyEnvelope {
  request_id: number;
  message: AgentServerMessage;
}

// Search messages -----------------------------------------------------
// Mirrors `neoism-protocol/src/search.rs`. The daemon spawns `rg`,
// `fff_search::FilePicker`, and `git status` directly on the host and
// pipes hits back over the WebSocket. Every variant carries a `req_id`
// the JS side uses to correlate replies into the `JsSearchService`
// pending-correlation slot on the wasm side.

export type SearchFileMode = "Fuzzy" | "Exact";
export type SearchGrepMode = "Fuzzy" | "Exact" | "Regex";
export type SearchGitStatus =
  | "Modified"
  | "Staged"
  | "Mixed"
  | "Added"
  | "Deleted"
  | "Renamed"
  | "Untracked"
  | "Conflict";

export interface SearchFileHit {
  score: number;
  path: string;
}

export interface SearchGrepHit {
  score: number;
  path: string;
  line: number;
  column: number;
  text: string;
}

export interface SearchGitHit {
  path: string;
  status: SearchGitStatus;
  line: number;
  text: string;
}

export type SearchClientMessage =
  | { CollectFiles: { req_id: number; cwd: string } }
  | {
      SearchFiles: {
        req_id: number;
        query: string;
        cwd: string;
        mode: SearchFileMode;
      };
    }
  | {
      SearchGrep: {
        req_id: number;
        query: string;
        cwd: string;
        mode: SearchGrepMode;
        case_sensitive: boolean | null;
        file_patterns: string[];
      };
    }
  | { SearchGitChanges: { req_id: number; cwd: string } }
  | { GitRepoRoot: { req_id: number; cwd: string } }
  | { CancelSearch: { req_id: number } };

export type SearchServerMessage =
  | { CollectFilesResult: { req_id: number; paths: string[] } }
  | { SearchFilesResult: { req_id: number; hits: SearchFileHit[] } }
  | { SearchGrepResult: { req_id: number; hits: SearchGrepHit[] } }
  | { SearchGitChangesResult: { req_id: number; hits: SearchGitHit[] } }
  | { GitRepoRootResult: { req_id: number; path: string | null } }
  | { SearchProgress: { req_id: number; found_so_far: number } }
  | { SearchError: { req_id: number; message: string } };

export interface SearchEnvelope {
  request_id: number;
  message: SearchClientMessage;
}

export interface SearchReplyEnvelope {
  request_id: number;
  message: SearchServerMessage;
}

// Workspace messages --------------------------------------------------
// Mirrors `neoism-protocol/src/workspace.rs`. Workspace identity,
// project registry, per-session cwd, and the active-session pointer.

export interface ProjectRootSummary {
  id: string;
  name: string;
  path: string;
  last_opened: number;
}

export interface HostSummary {
  id: string;
  label: string;
  online?: boolean;
  peer_identity?: string | null;
  last_seen?: number;
  /** Canonical dialable daemon endpoint, e.g. `ws://100.64.0.2:7878/session`.
   *  Used to resolve a workspace's `running_on_host_id` to a URL for re-dial
   *  when it is promoted/demoted between hosts. Absent for hosts with no
   *  known address. Mirrors `HostSummary.daemon_url` on the Rust side. */
  daemon_url?: string | null;
  /** The workspace this host is currently sitting in (daemon-filled
   *  from its active pointer). Web boot auto-attaches here so opening
   *  a browser lands where the desktop already is. */
  active_workspace_id?: string | null;
}

export interface WorkspaceSummary {
  id: string;
  host_id: string;
  title: string;
  host_kind?: "local" | "tailscale" | "docker_sandbox" | "cloud_sandbox";
  visibility?: "private" | "shared" | "team";
  main_session_id?: string | null;
  root_dir?: string | null;
  active_tab_id?: string | null;
  running_on_host_id?: string | null;
  controlled_by_host_id?: string | null;
  layout_snapshot?: string | null;
  last_active?: number;
}

export interface WorkspaceTabSummary {
  id: string;
  workspace_id: string;
  title: string;
  kind?: string | null;
  session_id?: string | null;
  surface_id?: string | null;
  cwd?: string | null;
  active?: boolean;
  last_active?: number;
}

export type InitialWorkspaceReason =
  | "client_remembered"
  | "host_active"
  | "most_recent"
  | "created_default";

export interface HostWorkspaceTreeSummary {
  hosts: HostSummary[];
  workspaces: WorkspaceSummary[];
  tabs: WorkspaceTabSummary[];
}

export interface SessionSummary {
  id: string;
  workspace_id: string;
  cwd: string;
  label: string | null;
  last_active: number;
}

export interface EditorSurfaceSummary {
  surface_id: string;
  workspace_id: string;
  session_id: string;
  path: string | null;
  // Daemon-assigned PTY / diagnostics route id for this surface.
  // The chrome uses this as the `route_id` it passes to
  // `SubscribeDiagnostics`, replacing the hard-coded `1` it used
  // while every web client ran a single embedded nvim. Optional so
  // older daemons (which don't assign route ids) stay
  // wire-compatible; newer daemons always populate it.
  route_id?: number | null;
  last_active: number;
}

export type WorkspaceAction =
  | "CreateNeoismNote";

export interface ClipboardPayload {
  mime_type: string;
  text?: string | null;
  bytes?: number[];
  filename?: string | null;
}

// Pane-layout mutation primitives — mirror of
// `neoism_protocol::workspace::PaneLayoutOp`. Used by the
// `PaneLayoutOp` client message (phone agent drives the layout) and
// echoed inside the daemon's `PaneLayoutChanged` broadcast so paired
// surfaces converge on the same layout state.
export type PaneSplitAxis = "Horizontal" | "Vertical";

export type PaneSplitPlacement = "Before" | "After";

export type PaneFocusDir = "Left" | "Right" | "Up" | "Down";

export type PaneLayoutOp =
  | { Split: { axis: PaneSplitAxis; placement: PaneSplitPlacement } }
  | { Focus: { dir: PaneFocusDir } }
  | "Close"
  | { ResizeRatio: { delta: number } }
  | { MoveTab: { from: number; to: number } };

export type WorkspaceClientMessage =
  | {
      OpenProjectRoot: {
        path: string;
        init_if_missing?: boolean;
      };
    }
  | { CloseProjectRoot: { id: string } }
  | "ListProjectRoots"
  | { SwitchProjectRoot: { id: string } }
  | { GetProjectRootInfo: { id: string } }
  | { RenameProjectRoot: { id: string; name: string } }
  | { ForgetProjectRoot: { id: string } }
  | "ListHosts"
  | { UpsertHost: { host: HostSummary } }
  | { ListHostWorkspaces: { host_id?: string | null } }
  | { ListWorkspaceTabs: { workspace_id: string } }
  | "RequestHostWorkspaceTree"
  | { ResolveInitialWorkspace: { preferred_host_id?: string | null } }
  | {
      CreateHostWorkspace: {
        host_id: string;
        workspace_id?: string | null;
        title?: string | null;
        root_dir?: string | null;
      };
    }
  | {
      CreateWorkspace: {
        workspace_id?: string | null;
        title?: string | null;
        root_dir?: string | null;
      };
    }
  | { CloseHostWorkspace: { workspace_id: string } }
  | { SwitchHostWorkspace: { workspace_id: string } }
  | { SetWorkspaceRoot: { workspace_id: string; root_dir: string } }
  | { ShareWorkspace: { workspace_id: string } }
  | { StopSharingWorkspace: { workspace_id: string } }
  | { SendWorkspaceToDockerSandbox: { workspace_id: string } }
  | { SendWorkspaceToCloud: { workspace_id: string } }
  | { SubscribeWorkspace: { workspace_id: string } }
  | { UnsubscribeWorkspace: { workspace_id: string } }
  | {
      ControlWorkspace: {
        workspace_id: string;
        controller_host_id: string;
      };
    }
  | {
      ReleaseWorkspaceControl: {
        workspace_id: string;
        controller_host_id: string;
      };
    }
  | {
      MoveWorkspaceToHost: {
        workspace_id: string;
        target_host_id: string;
      };
    }
  | {
      MoveTabToWorkspace: {
        tab_id: string;
        target_workspace_id: string;
      };
    }
  | {
      MoveTabToHostWorkspace: {
        tab_id: string;
        target_host_id: string;
        target_workspace_id: string;
      };
    }
  | {
      PublishWorkspaceTabs: {
        workspace_id: string;
        tabs: WorkspaceTabSummary[];
      };
    }
  | "ListSessions"
  | { RequestFullSnapshot: { since_offset?: Record<number, number> | null } }
  | { SwitchSession: { session_id: string } }
  | {
      NewSession: {
        cwd?: string | null;
        label?: string | null;
      };
    }
  | { CloseSession: { session_id: string } }
  | { GetSessionState: { session_id: string } }
  | { SetCwd: { session_id: string; path: string } }
  | { RenameSession: { session_id: string; label: string } }
  | {
      BindEditorSurface: {
        surface_id: string;
        session_id: string;
        path?: string | null;
      };
    }
  | "ListEditorSurfaces"
  | { CloseEditorSurface: { surface_id: string } }
  | { RunWorkspaceAction: { action: WorkspaceAction } }
  | { StoreClipboard: { payload: ClipboardPayload } }
  | "LoadClipboard"
  | {
      MaterializeClipboardImage: {
        payload: ClipboardPayload;
        // Opaque correlation token echoed back in the
        // `ClipboardImageMaterialized` reply. Multi-pane frontends use
        // it to route the materialised path to the pane that initiated
        // the paste (the focused surface at reply time may not be the
        // one that pasted). `undefined`/missing opts out of correlation
        // — single-pane callers can omit it.
        request_id?: string | null;
      };
    }
  | {
      // Phone-driven pane-layout mutation targeting the editor surface
      // identified by the integer external id used by the chrome's
      // pane overlay. The daemon validates the op against the active
      // workspace and broadcasts a sibling `PaneLayoutChanged` to
      // every connected client so paired surfaces converge on the
      // same layout.
      PaneLayoutOp: {
        pane_external_id: number;
        op: PaneLayoutOp;
      };
    }
  | {
      // First frame the chrome ships after the websocket opens.
      // `token` is the operator's pairing secret for the focused
      // daemon (omit / null for the legacy "trust local" path); the
      // daemon checks it against its `PairingTokenStore` when
      // `NEOISM_REQUIRE_AUTH=1` is set and replies with `HelloAck`.
      // `client_name` is a human-facing label used only for log /
      // audit lines — it never participates in the auth decision.
      Hello: {
        token?: string | null;
        client_name?: string | null;
      };
    }
  | {
      // F3: fetch the daemon-persisted UI preferences (theme, font
      // size, sidebar widths, last session-layout snapshot) for
      // `workspace_id`. The daemon replies with a sibling
      // `WorkplacePreferences` frame even if nothing has been
      // persisted yet (defaulted shape — no `Error` for first-run).
      GetWorkplacePreferences: { workspace_id: string };
    }
  | {
      // F3: replace the daemon-persisted preferences for
      // `workspace_id`. The daemon fans a `WorkplacePreferencesChanged`
      // out to every connected client (the submitter included) so
      // paired surfaces re-apply theme / sidebar widths without
      // polling — this is fire-and-forget; no direct reply is sent.
      SetWorkplacePreferences: {
        workspace_id: string;
        prefs: WorkplacePreferencesWire;
      };
    };

/** Wire shape for the F3 per-workplace UI preferences blob. Mirrors
 *  `neoism_protocol::workspace::WorkplacePreferences` 1:1. All fields
 *  are optional so partial updates from older clients round-trip
 *  cleanly — the daemon merges-by-replace, not patch. */
export interface WorkplacePreferencesWire {
  theme?: string | null;
  font_size?: number | null;
  sidebar_widths?: Record<string, number>;
  session_tree?: string | null;
}

export type WorkspaceServerMessage =
  | { ProjectRootList: { project_roots: ProjectRootSummary[] } }
  | {
      ProjectRootInfo: {
        id: string;
        name: string;
        path: string;
        sessions: SessionSummary[];
        active: boolean;
      };
    }
  | { ProjectRootOpened: { project_root: ProjectRootSummary } }
  | { ProjectRootClosed: { id: string } }
  | { ProjectRootChanged: { id: string | null } }
  | { HostList: { hosts: HostSummary[] } }
  | { HostWorkspaceList: { workspaces: WorkspaceSummary[] } }
  | { WorkspaceTabList: { tabs: WorkspaceTabSummary[] } }
  | { HostWorkspaceTree: HostWorkspaceTreeSummary }
  | {
      InitialWorkspaceResolved: {
        workspace: WorkspaceSummary;
        reason: InitialWorkspaceReason;
      };
    }
  | { HostWorkspaceUpserted: { workspace: WorkspaceSummary } }
  | { HostWorkspaceChanged: { host_id: string; workspace_id?: string | null } }
  | { WorkspaceControlChanged: { workspace: WorkspaceSummary } }
  | { WorkspaceTabMoved: { tab: WorkspaceTabSummary } }
  | { SessionList: { sessions: SessionSummary[] } }
  | {
      SessionState: {
        id: string;
        workspace_id: string;
        cwd: string;
        label: string | null;
        last_active: number;
      };
    }
  | { SessionChanged: { session_id: string | null } }
  | { SessionCreated: { session: SessionSummary } }
  | { SessionClosed: { session_id: string } }
  | { EditorSurfaceList: { surfaces: EditorSurfaceSummary[] } }
  | { EditorSurfaceChanged: { surface: EditorSurfaceSummary } }
  | { EditorSurfaceClosed: { surface_id: string } }
  | {
      WorkspaceActionCompleted: {
        action: WorkspaceAction;
        path: string | null;
        message: string;
      };
    }
  | { ClipboardPayload: { payload: ClipboardPayload | null } }
  | {
      ClipboardImageMaterialized: {
        path: string;
        mime_type: string;
        filename: string | null;
        // Echoed correlation token; `null`/missing when the original
        // request didn't supply one. See `MaterializeClipboardImage`
        // above for the routing rationale.
        request_id?: string | null;
      };
    }
  | {
      // Broadcast notification: a `PaneLayoutOp` mutation landed
      // successfully. The daemon fans this out to every connected
      // client (not just the submitter) so paired surfaces (laptop
      // chrome + phone agent + web) converge on the same layout.
      // `new_layout_snapshot` is the daemon's serialized
      // `PaneLayoutSnapshot` (the authoritative N-ary pane tree). The
      // web lowers it through the shared
      // `SessionLayout::from_pane_layout_snapshot` mirror in the wasm
      // bridge so it renders the same split intent the desktop does.
      PaneLayoutChanged: {
        pane_external_id: number;
        op: PaneLayoutOp;
        new_layout_snapshot?: string | null;
      };
    }
  | {
      // Daemon reply to the chrome's `Hello` frame. `accepted=false`
      // means the daemon is about to drop the websocket — the client
      // should surface `reason` (if present) and clean up. `accepted=true`
      // optionally carries a `peer_identity` string the daemon
      // resolved via `tailscale whois`, suitable for rendering
      // "connected to laptop-A (you@tailnet)" in the chrome.
      HelloAck: {
        accepted: boolean;
        reason?: string | null;
        peer_identity?: string | null;
      };
    }
  | {
      // F3 reply to `GetWorkplacePreferences`. `prefs` is the
      // daemon's persisted shape; first-run / never-set workspaces
      // round-trip as the default (all-empty) struct rather than an
      // `Error`.
      WorkplacePreferences: {
        workspace_id: string;
        prefs: WorkplacePreferencesWire;
      };
    }
  | {
      // F3 broadcast — fans out of the daemon on every successful
      // `SetWorkplacePreferences`. Chrome listeners use this to
      // re-apply theme / sidebar widths without polling. Sent to
      // every connected client including the submitter, so a
      // local mutation appears here on the round-trip.
      WorkplacePreferencesChanged: {
        workspace_id: string;
        prefs: WorkplacePreferencesWire;
      };
    }
  | { Error: { message: string } };

export interface WorkspaceEnvelope {
  message: WorkspaceClientMessage;
}

export interface WorkspaceReplyEnvelope {
  message: WorkspaceServerMessage;
}

// Diagnostics messages ------------------------------------------------
// Mirrors `neoism-protocol/src/diagnostics.rs`. The web frontend has no
// local nvim, so the daemon forwards LSP diagnostics across the
// WebSocket per-route.

export type LspState =
  | "Starting"
  | "Ready"
  | "Indexing"
  | "Stopped"
  | { Failed: { message: string } };

export interface LspDiagnosticItem {
  line: number;
  col: number;
  end_line: number;
  end_col: number;
  severity: number;
  message: string;
  source: string | null;
  code?: string | null;
  code_description?: string | null;
  tags?: string[];
  related_information?: DiagnosticRelatedInformation[];
}

export type DiagnosticsClientMessage =
  | { SubscribeDiagnostics: { route_id: number } }
  | { UnsubscribeDiagnostics: { route_id: number } };

export type DiagnosticsServerMessage =
  | {
      DiagnosticsPush: {
        route_id: number;
        items: LspDiagnosticItem[];
      };
    }
  | { DiagnosticsCleared: { route_id: number } }
  | { LspStatusUpdate: { server: string; state: LspState } };

export interface DiagnosticsEnvelope {
  message: DiagnosticsClientMessage;
}

export interface DiagnosticsReplyEnvelope {
  message: DiagnosticsServerMessage;
}

// Cursor-overlay messages -------------------------------------------
// Mirrors `neoism-protocol/src/cursor.rs`. The daemon translates
// nvim's `grid_cursor_goto`, `mode_change`, and `TextYankPost` events
// into these push-style envelopes; the dispatcher in
// `terminal/TerminalPanel.ts` translates cell coordinates to physical
// pixels via the chrome bridge's `cell_metrics()` before forwarding to
// the matching `setTrailCursor` / `setCustomCursor` /
// `setCursorlineOverlay` / `setYankFlash` bridge setter.

export type CursorShape = "Block" | "Beam" | "Underline" | "Hidden";

export interface YankFlashRegion {
  row_top: number;
  row_bot: number;
  col_left?: number | null;
  col_right?: number | null;
}

export type CursorOverlayServerMessage =
  | {
      TrailCursor: {
        /** 0-based cell column. */
        col: number;
        /** 0-based cell row. */
        row: number;
        /** `null` defaults to `Block`. */
        shape: CursorShape | null;
        no_jump?: boolean;
        reset?: boolean;
        snap?: boolean;
      };
    }
  | {
      CustomCursor: {
        /**
         * Pointer position, physical pixels. `null` / missing on
         * visibility-only frames (e.g. daemon `mouse_on` / `mouse_off`
         * relays from nvim); the dispatcher preserves the last known
         * coordinate when omitted so the sprite doesn't snap to (0,0).
         */
        x?: number | null;
        y?: number | null;
        visible?: boolean;
      };
    }
  | {
      CursorlineOverlay: {
        /** 0-based pane / rich-text id. */
        rich_text_id: number;
        /** 0-based row of the highlighted line (cell coordinates). */
        target_row: number;
        snap?: boolean;
        forget?: boolean;
      };
    }
  | {
      YankFlash: {
        regions: YankFlashRegion[];
      };
    };

export type CursorOverlayClientMessage =
  | {
      CustomCursor: {
        /** Pointer position, physical pixels. */
        x: number;
        y: number;
        visible?: boolean;
      };
    };

export interface CursorOverlayEnvelope {
  request_id?: number;
  message: CursorOverlayClientMessage;
}

export interface CursorOverlayReplyEnvelope {
  request_id: number;
  message: CursorOverlayServerMessage;
}

// Narrowing helpers ---------------------------------------------------

export function isPtyCreated(
  msg: ServerMessage,
): msg is { PtyCreated: PtyCreatedArgs } {
  return Object.prototype.hasOwnProperty.call(msg, "PtyCreated");
}

export function isPtyOutput(
  msg: ServerMessage,
): msg is { PtyOutput: PtyOutputArgs } {
  return Object.prototype.hasOwnProperty.call(msg, "PtyOutput");
}

export function isPtyClosed(
  msg: ServerMessage,
): msg is { PtyClosed: PtyClosedArgs } {
  return Object.prototype.hasOwnProperty.call(msg, "PtyClosed");
}

export function isSessionCwd(
  msg: ServerMessage,
): msg is { SessionCwd: SessionCwdArgs } {
  return Object.prototype.hasOwnProperty.call(msg, "SessionCwd");
}

export function isServerError(
  msg: ServerMessage,
): msg is { Error: ErrorArgs } {
  return Object.prototype.hasOwnProperty.call(msg, "Error");
}

export function isFilesReply(
  msg: ServerMessage,
): msg is { FilesReply: FilesReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "FilesReply");
}

export function isGitReply(
  msg: ServerMessage,
): msg is { GitReply: GitReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "GitReply");
}

export function isEditorReply(
  msg: ServerMessage,
): msg is { EditorReply: EditorReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "EditorReply");
}

export function isAgentReply(
  msg: ServerMessage,
): msg is { AgentReply: AgentReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "AgentReply");
}

export function isSearchReply(
  msg: ServerMessage,
): msg is { SearchReply: SearchReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "SearchReply");
}

export function isWorkspaceReply(
  msg: ServerMessage,
): msg is { WorkspaceReply: WorkspaceReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "WorkspaceReply");
}

export function isDiagnosticsReply(
  msg: ServerMessage,
): msg is { DiagnosticsReply: DiagnosticsReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "DiagnosticsReply");
}

export function isCursorOverlayReply(
  msg: ServerMessage,
): msg is { CursorOverlayReply: CursorOverlayReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "CursorOverlayReply");
}

export function isCrdtReply(
  msg: ServerMessage,
): msg is { CrdtReply: CrdtReplyEnvelope } {
  return Object.prototype.hasOwnProperty.call(msg, "CrdtReply");
}

// Constructors --------------------------------------------------------

export const ClientMessage = {
  createPty(args: CreatePtyArgs): ClientMessage {
    return { CreatePty: args };
  },
  ptyInput(args: PtyInputArgs): ClientMessage {
    return { PtyInput: args };
  },
  resize(args: ResizeArgs): ClientMessage {
    return { Resize: args };
  },
  closePty(args: ClosePtyArgs): ClientMessage {
    return { ClosePty: args };
  },
  files(envelope: FilesEnvelope): ClientMessage {
    return { Files: envelope };
  },
  git(envelope: GitEnvelope): ClientMessage {
    return { Git: envelope };
  },
  editor(envelope: EditorEnvelope): ClientMessage {
    return { Editor: envelope };
  },
  agent(envelope: AgentEnvelope): ClientMessage {
    return { Agent: envelope };
  },
  search(envelope: SearchEnvelope): ClientMessage {
    return { Search: envelope };
  },
  workspace(envelope: WorkspaceEnvelope): ClientMessage {
    return { Workspace: envelope };
  },
  diagnostics(envelope: DiagnosticsEnvelope): ClientMessage {
    return { Diagnostics: envelope };
  },
  cursorOverlay(envelope: CursorOverlayEnvelope): ClientMessage {
    return { CursorOverlay: envelope };
  },
  crdt(envelope: CrdtEnvelope): ClientMessage {
    return { Crdt: envelope };
  },
};
