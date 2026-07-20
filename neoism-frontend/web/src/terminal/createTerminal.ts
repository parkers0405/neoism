/**
 * Factory for the web terminal bridge. Normal dev must use the rendered
 * wasm/Sugarloaf path; non-rendered adapters are diagnostic-only and require
 * `VITE_NEOISM_ALLOW_TERMINAL_STUB=1`.
 *
 * Three implementations cover the same baseline surface (feed, resize,
 * takePtyWrites, drainEffects, snapshot, isReal):
 *
 *   - `StubAdapter`        — pure TS diagnostic placeholder; opt-in only.
 *   - `RealAdapter`        — wraps the wasm-exported `Terminal` (data-only,
 *                            no GPU). Diagnostic-only because it diverges
 *                            from the shared Sugarloaf renderer.
 *   - `RenderedAdapter`    — wraps the wasm-exported `RenderedTerminal`,
 *                            which owns the canvas via sugarloaf and
 *                            paints cells through WebGPU/WebGL. Adds a
 *                            `render()` call that the panel drives from
 *                            its RAF loop.
 *
 * `ChromeAdapter` / `RenderedAdapter` are the only default paths. Callers
 * discriminate via `isRendered()`.
 */

import { WasmTerminalStub } from "./WasmTerminalStub.js";
import { computeSizeContract, type SizeContract } from "./sizeContract.js";

export interface TerminalAdapter {
    feed(bytes: Uint8Array): void;
    /** Resize the grid. `scale` is `window.devicePixelRatio` and is only
     *  meaningful to the rendered adapter; the others ignore it. */
    resize(
        cols: number,
        rows: number,
        scale: number,
        widthPx?: number,
        heightPx?: number,
    ): void;
    takePtyWrites(): Uint8Array;
    drainEffects(): unknown[];
    snapshot(): unknown;
    /** True if any wasm bundle is loaded (vs the pure-TS stub). */
    isReal(): boolean;
    /** True if this adapter owns the canvas via sugarloaf and exposes
     *  `render()`. Implies `isReal()`. */
    isRendered(): boolean;
    /** True when the adapter also drives neoism-ui chrome. */
    isChrome(): boolean;
    /** Paint one frame of cells. No-op for stub / data-only adapters. */
    render(): void;
    handleUiEvent?(event: unknown): void;
    serviceReply?(requestId: number, payload: unknown): void;
    setClipboardValue?(text: string | null): void;
    setChromeCallbacks?(callbacks: ChromeCallbacks): void;
    refreshFileTree?(): void;
    setFileTreeEntries?(entriesJson: string): void;
    drainFileTreeOpens?(): unknown;
    /** Hit-test the file tree at a window-space coordinate. Returns the
     *  row's path + parent dir + label, or `null` when no row was hit.
     *  Selection is nudged onto the row as a side-effect so subsequent
     *  keyboard shortcuts (F2 / Delete) target the same entry. */
    fileTreeContextTarget?(x: number, y: number): FileTreeContextTarget | null;
    /** Return the absolute path of the currently-selected file tree
     *  row (null when nothing is selected or selected row is a virtual
     *  entry without a backing path). */
    fileTreeSelectedPath?(): string | null;
    /** Return the workspace root the chrome was constructed with —
     *  used as the default creation target for "New File / New Folder"
     *  when the user invokes the menu without targeting a row. */
    fileTreeWorkspaceRoot?(): string | null;
    setWorkspaceRoot?(workspaceRoot: string): void;
    /** True when the file tree currently owns chrome focus. Gates the
     *  F2 / Delete keyboard shortcuts so they don't fire from other
     *  surfaces (terminal, editor, palette). */
    fileTreeFocused?(): boolean;
    /** Drain pending tab-strip click intents queued by the chrome.
     *  Returns `{ activate: number | null, close: number[] }`. */
    drainBufferTabIntents?(): BufferTabIntents | null;
    /** Drain Rust-owned chrome requests to open/focus the Neoism Agent tab. */
    drainAgentTabOpens?(): number;
    /** Drain pending finder "open this hit" intents queued when the
     *  user activates a finder row (Enter / click). Each entry is a
     *  `FinderOpenIntent`; the host turns it into a buffer-tab append
     *  + `Editor::OpenBuffer` envelope. */
    drainFinderOpenIntents?(): FinderOpenIntent[] | null;
    /** Drain pending command-palette execute intents queued when the
     *  user picks a row (Enter / click). Each entry is a discriminated
     *  union the host dispatches per `kind`. */
    drainPaletteIntents?(): PaletteIntent[] | null;
    setBufferTabs?(titlesJson: string, active: number): void;
    applyBufferTabPolicy?(
        tabsJson: string,
        active: number,
        operation: string,
        index?: number | null,
    ): unknown;
    applySessionLayoutPolicy?(
        stateJson: string | null,
        operation: string,
        axis?: string | null,
        title?: string | null,
        externalId?: number | null,
    ): unknown;
    mirrorPaneLayoutSnapshot?(snapshotJson: string): unknown;
    setActiveTab?(idx: number): void;
    setTabContent?(idx: number, text: string, path: string): void;
    setTerminalInput?(text: string): void;
    clearTerminalInput?(): void;
    terminalInput?(): string;
    terminalCommandComposerVisible?(): boolean;
    terminalShouldCaptureInput?(): boolean;
    terminalInputInsert?(text: string): void;
    terminalInputKey?(key: string): boolean;
    terminalSubmitPayload?(): Uint8Array;
    recordTerminalSubmit?(command: string): void;
    terminalCommandBlockCount?(): number;
    terminalCommandBlocksJson?(): string;
    dismissTerminalSplash?(): void;
    resetTerminalSplash?(): void;
    toggleFileTree?(): void;
    showFileTree?(): void;
    hideFileTree?(): void;
    showCommandPalette?(): void;
    setCommandPaletteWorkspaceVisibility?(visibility: string): void;
    setWorkspaceIslandTabs?(payloadJson: string): void;
    workspaceIslandClick?(x: number, y: number): boolean;
    workspaceIslandContextClick?(x: number, y: number): boolean;
    drainWorkspaceIslandIntents?(): unknown;
    focusWorkspaceIsland?(): void;
    moveWorkspaceIslandFocus?(previous: boolean): boolean;
    activateWorkspaceIslandFocus?(): boolean;
    bufferTabsFocused?(): boolean;
    workspaceIslandFocused?(): boolean;
    blurWorkspaceIsland?(): void;
    showSearchPalette?(): void;
    showCommandComposer?(): void;
    showGitDiff?(): void;
    toggleGitDiff?(): void;
    /** Rich right-side git diff panel (desktop Alt+G). Returns the new
     *  visibility so the host can kick the daemon data fetch. */
    toggleGitDiffPanel?(): boolean;
    /** Notes sidebar (desktop Alt+N). Returns the new visibility. */
    toggleNotesSidebar?(): boolean;
    /** One-shot flags: the panel just opened and wants daemon data. */
    takeGitPanelRefresh?(): boolean;
    takeNotesRefresh?(): boolean;
    /** Flag the notes vault as changed on disk so the next
     *  `takeNotesRefresh` re-fetches the listing (live add/delete). */
    markNotesDirty?(): void;
    gitPanelSetFiles?(filesJson: string): void;
    gitPanelSetDiff?(path: string, patch: string): void;
    gitPanelSetError?(message: string): void;
    notesSetEntries?(entriesJson: string): void;
    /** Paths activated in the git panel / notes sidebar. */
    drainPanelOpenPaths?(): unknown;
    toggleAgentPane?(): void;
    showFinder?(): void;
    showFinderFiles?(): void;
    showFinderGrep?(): void;
    showFinderGitChanges?(): void;
    hideModals?(): void;
    splashClick?(x: number, y: number): boolean;
    splashMouseMove?(x: number, y: number): void;
    splashMouseLeave?(): void;
    splashWordmarkClick?(x: number, y: number): void;
    chromeLayout?(): ChromeLayout | null;
    drainTopBarAction?(): string | null;
    chromeKeyboardCaptureActive?(): boolean;
    editorInputModalActive?(): boolean;
    focusEditorInput?(): void;
    animationsActive?(): boolean;
    /** Push a daemon-resolved branch name into the status line. */
    setStatusBranch?(branch: string | null): void;
    /** Push the latest working-tree change counts into the status line.
     *  `added` and `deleted` come straight from the daemon's
     *  `git status --porcelain` poll loop. */
    setStatusGitChanges?(added: number, deleted: number): void;
    /** Switch the active IdeTheme by name (e.g. "pastel_dark",
     *  "nvchad_one", "tokyo_night", "catppuccin_mocha"). Drives both
     *  the chrome palette AND sugarloaf's swapchain clear color so the
     *  terminal background, status line, tabs, and modals agree. */
    setIdeTheme?(name: string): void;
    /** User-facing font zoom (Ctrl+= / Ctrl+- / Ctrl+0). `scale` is the
     *  absolute multiplier (1.0 = default cell size); the bridge clamps
     *  to `[0.5, 3.0]`. Adapters that don't drive chrome ignore this. */
    setFontScale?(scale: number): void;
    /** Swap the command palette into font-family browsing mode. */
    enterPaletteFontsMode?(fontsJson: string): void;
    /** Swap the command palette into IDE-theme browsing mode. */
    enterPaletteThemesMode?(themesJson: string): void;
    /** Swap the command palette into shader/filter browsing mode. */
    enterPaletteShadersMode?(shadersJson: string): void;
    /** Swap the command palette into host-buffer browsing mode. */
    enterPaletteBuffersMode?(buffersJson: string): void;
    /** Open the desktop-parity Workspaces modal: the command palette's
     *  grouped host→workspace tree. `payloadJson` is the JSON-encoded
     *  `WorkspacesModalPayload`. Returns `false` when the underlying
     *  bridge doesn't expose the mode (stale wasm pkg) so the host can
     *  fall back to the legacy DOM switcher overlay. */
    openWorkspacesPalette?(payloadJson: string): boolean;
    /** True while the Workspaces modal is currently open, so hosts can
     *  live-refresh the tree as daemon pushes arrive. */
    workspacesPaletteOpen?(): boolean;
    /** The markdown pane's REAL caret (line + UTF-16 column) for the
     *  presence plane. Null when no markdown pane is active. */
    markdownCursor?(): { line: number; columnUtf16: number; insert?: boolean } | null;
    /** False when the served wasm bundle predates the co-editing
     *  exports (host should tell the user to hard-refresh). */
    crdtSupported?(): boolean;
    /** Wave 8D: drain outbound CRDT client messages for the active
     *  markdown doc (JSON array string). Null when nothing queued. */
    crdtPump?(bufferId: string | null): string | null;
    /** Apply one inbound CrdtServerMessage (JSON) to the bound pane;
     *  true when visible text changed. */
    crdtApply?(json: string): boolean;
    /** Queue a daemon-owned save of the active markdown doc. */
    markdownRequestSave?(): boolean;
    /** Swap a fresh host→workspace tree into the already-open
     *  Workspaces modal without resetting query/selection. */
    refreshWorkspacesPalette?(payloadJson: string): void;
    /** Toggle terminal vi mode in the rendered bridge. */
    toggleViMode?(): void;
    /** Hand an `AgentServerMessage` JSON envelope to the bridge for
     *  translation into pane state mutations. */
    agentEvent?(eventJson: string): void;
    agentSetInput?(text: string): void;
    agentInput?(): string;
    agentClearInput?(): void;
    agentHandleKey?(
        key: string,
        code: string,
        text: string,
        shift: boolean,
        control: boolean,
        alt: boolean,
        meta: boolean,
    ): boolean;
    agentHistoryStep?(delta: number): string;
    agentScrollTimeline?(deltaPixels: number): boolean;
    /** Desktop-priority pointer-down chain (picker rows, side panel,
     *  permissions, links, tool-card expand). `copy` carries text for
     *  the clipboard; `link` a target the host should open. */
    agentPointerDown?(
        x: number,
        y: number,
    ): { handled: boolean; copy: string | null; link: string | null } | null;
    /** Position-aware wheel: picker / side panel / diff card / timeline. */
    agentScrollAt?(x: number, y: number, deltaPixels: number): boolean;
    /** Center-modal click router: 0 = unhandled, 1 = row committed,
     *  2 = inside modal chrome/input (raise keyboard for the query). */
    modalPointerDown?(x: number, y: number): number;
    /** Center-modal list scroll (DOM sign). */
    modalScroll?(x: number, y: number, deltaPixels: number): boolean;
    /** Seed composer ArrowUp history (oldest first). */
    terminalSeedHistory?(entriesJson: string): void;
    /** Seed a directory listing for composer Tab completion. */
    terminalSeedCompletionDir?(dir: string, entriesJson: string): void;
    /** Dirs Tab completion is waiting on (absolute paths). */
    drainCompletionDirRequests?(): unknown;
    /** Position-aware touch drag. 0 = unhandled, 1 = overlay/diff card,
     *  2 = timeline (fling allowed on release). */
    agentDragAt?(x: number, y: number, dyPixels: number): number;
    /** 1:1 touch drag — content tracks the finger, no inertia. */
    agentDragTimeline?(deltaPixels: number): boolean;
    /** Launch (non-zero) or stop (0) a kinetic glide; returns whether
     *  the timeline was gliding before the call. */
    agentFlingTimeline?(velocityPxPerSecond: number): boolean;
    /** Agent prompt-input rect in chrome-logical px, or null. Drives
     *  the mobile tap-to-summon-keyboard hit-test — the home screen
     *  centers the input mid-pane rather than docking it bottom. */
    agentInputRect?(): [number, number, number, number] | null;
    agentHasConversation?(): boolean;
    agentHasPendingPermission?(): boolean;
    agentIsStreaming?(): boolean;
    agentMovePermissionSelection?(delta: number): boolean;
    agentSubmitPermission?(): boolean;
    agentReplyPermission?(decision: "Yes" | "Always" | "No"): boolean;
    /** Install the JS-side callback the bridge fires when the chrome
     *  wants to emit an `AgentClientMessage`. Signature is
     *  `(requestId: number, envelopeJson: string)`. */
    setAgentSend?(cb: (requestId: number, envelopeJson: string) => void): void;
    /** Wake/list the daemon-backed agent server without creating a session. */
    agentAttach?(directory?: string | null): void;
    /** Drive the bridge's "send a SendMessage" path; under the hood the
     *  bridge fires `agent_send` with a fresh request id. */
    agentSendMessage?(text: string): void;
    /** Submit an agent prompt with protocol Attachment records. */
    agentSendMessageWithAttachments?(text: string, attachmentsJson: string): void;
    /** Cancel the in-flight Claude request on the daemon side. */
    agentCancel?(): void;
    /** Reset the daemon-side conversation history. */
    agentNewThread?(directory?: string | null): void;
    /** Trigger the Neoism Agent home wordmark click animation. */
    agentWordmarkClick?(x: number, y: number): boolean;
    /** Which surface should consume the next raw keystroke. Returns
     *  `"terminal"` for the shell tab, `"editor"` for file tabs,
     *  and `"agent"` for the Neoism Agent tab. Stub / data-only adapters
     *  can omit this. */
    activeSurface?(): string;
    /** Install the JS-side callback the bridge fires when the terminal
     *  emits PTY response bytes (DSR / OSC / cursor pos). The bridge
     *  auto-calls this after every `feed_pty_output`, so hosts using
     *  the outbox path don't need to poll `takePtyWrites()`. Payload
     *  is a base64 string. */
    setPtyOutbox?(cb: (bytesB64: string) => void): void;
    /** Search service setters — installed by `SearchService.install()`.
     *  Wasm passes `(reqId, envelopeJson)` for every search flavor. */
    setSearchCollectFiles?(cb: (reqId: number, envelopeJson: string) => void): void;
    setSearchFiles?(cb: (reqId: number, envelopeJson: string) => void): void;
    setSearchGrep?(cb: (reqId: number, envelopeJson: string) => void): void;
    setSearchGitChanges?(cb: (reqId: number, envelopeJson: string) => void): void;
    setSearchGitRepoRoot?(cb: (reqId: number, envelopeJson: string) => void): void;
    setSearchCancel?(cb: (reqId: number) => void): void;
    /** Push a daemon-resolved `DiagnosticsServerMessage` JSON envelope
     *  into the bridge. The bridge translates each variant into the
     *  matching `Chrome::set_diagnostics(...)` / status-line mutation. */
    diagnosticsEvent?(eventJson: string): void;
    /** Push a daemon-resolved `WorkspaceServerMessage` JSON envelope
     *  into the bridge. The bridge updates its workspace registry and
     *  refreshes any panels bound to workspace state. */
    workspaceEvent?(eventJson: string): void;
    /** Hand a full LSP diagnostics list (the JSON-serialized array of
     *  `LspDiagnosticItem`) into the active editor's gutter overlay.
     *  Routed from `DiagnosticsPush`. */
    setDiagnostics?(itemsJson: string): void;
    /** Open the diagnostic-detail popup pinned to the cursor at the
     *  given (line, col). */
    showDiagnosticsAt?(line: number, col: number): void;
    /** Hide the diagnostic gutter + popup. Routed from
     *  `DiagnosticsCleared`. */
    hideDiagnostics?(): void;
    /** Routed from `LspStatusUpdate`: write the LSP pill on the
     *  status line. */
    setStatusLspActive?(name: string): void;
    setStatusLspInitializing?(): void;
    setStatusLspMissing?(): void;
    setStatusLspOff?(): void;
    statusLineClick?(x: number, y: number): StatusLineClickIntent | null;
    /** Push breadcrumb segments for the active buffer into the
     *  breadcrumb panel. Argument is JSON ` [{ label, kind }, ... ]`. */
    setBreadcrumbs?(segmentsJson: string): void;
    /** Push the LSP autocomplete entries into the completion popup;
     *  `"[]"` hides it. */
    setCompletionMenu?(itemsJson: string): void;
    /** Push the minimap snapshot for the active buffer. */
    setMinimap?(snapshotJson: string): void;
    /** Push a toast / status notification onto the chrome's
     *  notification stack. */
    pushNotification?(notificationJson: string): void;
    /** Write the active branch name into the standalone branch pill. */
    setGitBranchPill?(branch: string | null): void;
    /** Open the right-click / generic context menu. JSON:
     *  `{ title, x, y, window_w, window_h, items: [{ label, hint, enabled }] }`.
     *  See `ChromeBridge::set_context_menu` for field semantics. */
    setContextMenu?(payloadJson: string): void;
    /** Hide the context menu. Idempotent. */
    hideContextMenu?(): void;
    /** Returns the chrome's current cell metrics in physical pixels
     *  as `[cell_w, cell_h]`. The cursor-overlay dispatcher reads
     *  these to translate daemon-emitted cell coordinates into the
     *  physical pixels the setter JSON expects. Optional because
     *  pre-W3 bridges don't expose it; the dispatcher falls back
     *  to `[8, 16]` (the bridge's resize defaults). */
    cellMetrics?(): [number, number];
    /** Push the trail cursor's latest destination + shape. JSON shape
     *  documented on the Rust `ChromeBridge::set_trail_cursor` setter:
     *  `{ x, y, cell_w, cell_h, shape, no_jump?, reset?, snap? }`.
     *  `shape` is `"block" | "beam" | "underline" | "hidden"`. */
    setTrailCursor?(json: string): void;
    /** Push the custom mouse-cursor sprite position. JSON:
     *  `{ x, y, visible? }`. `visible = false` hides the sprite
     *  without forgetting the last-known position (pointer-leave). */
    setCustomCursor?(json: string): void;
    /** Push the cursorline-overlay target for one editor pane. JSON:
     *  `{ rich_text_id, target_y, snap?, forget? }`. `forget = true`
     *  drops the cached pane state (call when a pane is closed). */
    setCursorlineOverlay?(json: string): void;
    /** Spawn one or more yank-flash regions. JSON:
     *  `{ regions: [{ row_top, row_bot }, ...] }`. Rows are 0-based
     *  screen rows relative to the editor pane top. */
    setYankFlash?(json: string): void;
}

export interface ChromeRect {
    x: number;
    y: number;
    w: number;
    h: number;
}

/** Output of `drainBufferTabIntents`. `activate` is the index of the
 *  tab the user clicked (null when no click since the last drain);
 *  `close` is the list of tabs whose X button was hit since the last
 *  drain, in click order; `newTab` is set when the strip's trailing
 *  "+" button was clicked (host spawns a terminal tab — desktop
 *  TabCreateNew parity). */
export interface BufferTabIntents {
    activate: number | null;
    close: number[];
    newTab: boolean;
}

/** Snapshot of the file-tree row the user right-clicked. `path` is the
 *  row's absolute path (null for virtual rows like the Neoism workspace
 *  header). `parentDir` is the directory that should host New File /
 *  New Folder operations — the row itself when it's a directory, the
 *  row's parent otherwise. */
export interface FileTreeContextTarget {
    path: string | null;
    is_dir: boolean;
    parent_dir: string;
    label: string;
}

export type StatusLineClickIntent =
    | { kind: "toggle_split" }
    | { kind: "toggle_git_diff" }
    | { kind: "diagnostics_opened" }
    | { kind: "diagnostic_jump"; line: number }
    | { kind: "consumed" };

/** One queued "open this finder hit" intent emitted by the wasm bridge.
 *  Wire shape matches the Rust `FinderOpenIntent` serde derivation:
 *  `mode` is `"files" | "grep" | "git_changes"`; `line` is `null` for
 *  files-mode hits and `1`-based for grep / git-changes hits. */
export interface FinderOpenIntent {
    path: string;
    line: number | null;
    mode: "files" | "grep" | "git_changes";
    query: string;
}

/** Discriminated union of palette pick intents emitted by the wasm
 *  bridge. Wire shape matches the Rust `PaletteIntent` `#[serde(tag =
 *  "kind", rename_all = "snake_case")]` derivation. */
export type PaletteIntent =
    | { kind: "action"; action: string }
    | { kind: "ex_command"; command: string }
    | {
          kind: "search";
          query: string;
          match_location: [number, number] | null;
      }
    | { kind: "font"; family: string }
    | { kind: "theme"; name: string }
    | { kind: "shader"; title: string; filter: string | null }
    | { kind: "buffer"; target: PaletteBufferTarget }
    | { kind: "workspace"; workspace_id: string };

export type PaletteBufferTarget =
    | { target: "workspace"; tab_index: number }
    | { target: "pane"; route_id: number; tab_index: number };

/** One workspace row for the wasm Workspaces modal (the command
 *  palette's grouped host→workspace tree). Wire shape matches the
 *  Rust `ChromeBridge::open_workspaces_palette` deserializer, which
 *  maps it onto `PaletteWorkspaceEntry`. */
export interface WorkspacesModalWorkspace {
    title: string;
    detail: string;
    workspace_id: string;
    host_id: string;
    host_label: string;
    host_kind: "local" | "remote" | "cloud";
    workspace_host_kind?: "local" | "tailscale" | "docker_sandbox" | "cloud_sandbox";
    workspace_visibility?: "private" | "shared" | "team";
    daemon_url: string | null;
    host_online: boolean;
}

/** A workspace-less host header (e.g. a discovered tailnet peer) shown
 *  as a drop target in the Workspaces modal. Mirrors the Rust
 *  `PaletteHostEntry`. */
export interface WorkspacesModalPeerHost {
    host_id: string;
    label: string;
    kind: "local" | "remote" | "cloud";
    daemon_url: string | null;
    online: boolean;
}

/** Payload for `TerminalAdapter.openWorkspacesPalette`. */
export interface WorkspacesModalPayload {
    workspaces: WorkspacesModalWorkspace[];
    peer_hosts: WorkspacesModalPeerHost[];
}

function parsePaletteBufferTarget(raw: unknown): PaletteBufferTarget | null {
    if (!raw || typeof raw !== "object") return null;
    const rec = raw as Record<string, unknown>;
    const target = typeof rec.target === "string" ? rec.target : "";
    const tabIndex =
        typeof rec.tab_index === "number" && Number.isFinite(rec.tab_index)
            ? Math.trunc(rec.tab_index)
            : null;
    if (tabIndex === null) return null;
    if (target === "workspace") {
        return { target: "workspace", tab_index: tabIndex };
    }
    if (target === "pane") {
        const routeId =
            typeof rec.route_id === "number" && Number.isFinite(rec.route_id)
                ? Math.trunc(rec.route_id)
                : 0;
        return { target: "pane", route_id: routeId, tab_index: tabIndex };
    }
    return null;
}

export interface ChromeLayout {
    file_tree?: ChromeRect | null;
    buffer_tabs: ChromeRect;
    breadcrumbs?: ChromeRect | null;
    status_line: ChromeRect;
    terminal: ChromeRect;
    command_palette?: ChromeRect | null;
    finder?: ChromeRect | null;
    git_diff?: ChromeRect | null;
    command_composer?: ChromeRect | null;
}

export interface ChromeCallbacks {
    listDir(requestId: number, path: string): void;
    readFile(requestId: number, path: string): void;
    writeFile(requestId: number, path: string, bytes: Uint8Array): void;
    stat(requestId: number, path: string): void;
    clipboardRead(requestId: number): void;
    clipboardWrite(text: string): void;
    /** Shared chrome raised an OS-notification request. Wire shape
     *  mirrors `NotificationService::notify`: title, body, and a
     *  severity hint (`"info" | "warn" | "error"`). The host is
     *  responsible for funneling through `Notification` (with lazy
     *  permission) and falling back to the in-app toast stack when
     *  permission was denied or the API is unavailable. */
    notify(title: string, body: string, level: string): void;
    commandRun(requestId: number, command: string): void;
    gitStatus(requestId: number, repo: string): void;
    gitDiff(requestId: number, repo: string, path: string | null): void;
}

function requestId(value: unknown): number {
    return Math.trunc(Number(value));
}

/**
 * Probe `GL_MAX_TEXTURE_SIZE` so we never feed sugarloaf a swapchain
 * larger than the host GPU/driver can allocate. Modern WebGL2 typically
 * reports 8192 or 16384; older or virtualized GPUs can report 2048 (the
 * pathology the original `renderedScale` workaround flagged).
 *
 * We cap at 8192 by default — going higher gains nothing for our cell
 * grid and burns fillrate. The probe runs on a throwaway off-screen
 * `<canvas>` so we never bind a GL context to the panel canvas before
 * sugarloaf claims it with its own attribute set (alpha,
 * preserveDrawingBuffer, etc.) — a second `getContext` call would
 * silently return the existing context and ignore those attrs. The
 * probed result is cached for the page lifetime since the value is a
 * static property of the GPU/driver.
 */
let cachedTextureCap: number | null = null;
function rendererTextureCap(): number {
    const DEFAULT_CAP = 8192;
    if (cachedTextureCap !== null) return cachedTextureCap;
    if (typeof document === "undefined") {
        cachedTextureCap = DEFAULT_CAP;
        return cachedTextureCap;
    }
    try {
        const probe = document.createElement("canvas");
        probe.width = 1;
        probe.height = 1;
        const gl =
            (probe.getContext("webgl2") as WebGL2RenderingContext | null) ??
            (probe.getContext("webgl") as WebGLRenderingContext | null);
        if (!gl) {
            cachedTextureCap = DEFAULT_CAP;
            return cachedTextureCap;
        }
        const max = gl.getParameter(gl.MAX_TEXTURE_SIZE) as number | null;
        cachedTextureCap =
            typeof max === "number" && max > 0
                ? Math.min(max, DEFAULT_CAP)
                : DEFAULT_CAP;
        // Drop the probe context's references — it will be GC'd along with
        // the throwaway canvas, releasing the underlying WebGL2 resource.
        return cachedTextureCap;
    } catch {
        cachedTextureCap = DEFAULT_CAP;
        return cachedTextureCap;
    }
}

/**
 * Full size contract for the panel canvas at a measured CSS rect:
 * style dims, effective render scale, and physical backing dims, all
 * derived in one place (`computeSizeContract`) so the canvas style,
 * the chrome layout viewport, the sugarloaf scale_factor, and the
 * swapchain can never disagree.
 *
 * Called on every resize so a display swap / zoom change is picked up;
 * `window.devicePixelRatio` is read fresh (fractional values from
 * browser zoom — 1.25, 1.5, 0.8 — pass through unfloored).
 */
export function sizeContractFor(
    // The canvas is accepted (and unused) for API stability — a future
    // multi-monitor / multi-canvas world may want per-canvas caps.
    _canvas: HTMLCanvasElement | null,
    cssWidth: number,
    cssHeight: number,
): SizeContract {
    const devicePixelRatio =
        typeof window !== "undefined" && window.devicePixelRatio
            ? window.devicePixelRatio
            : 1;
    return computeSizeContract(
        cssWidth,
        cssHeight,
        devicePixelRatio,
        rendererTextureCap(),
    );
}

/**
 * Effective DPR for sugarloaf, given the current canvas + CSS-pixel
 * viewport. Clamps `window.devicePixelRatio` so `width * dpr <= cap`
 * AND `height * dpr <= cap` on the GPU's max-texture-size. When the
 * CSS viewport itself exceeds the cap (very old GPU reporting 2048 on
 * a 2560-wide window) the scale drops BELOW 1 so the whole frame still
 * fits — sugarloaf clamping the swapchain while chrome lays out at a
 * bigger scale is exactly the blurry-overflow bug this prevents.
 */
export function renderedScaleFor(
    canvas: HTMLCanvasElement | null,
    cssWidth: number,
    cssHeight: number,
): number {
    return sizeContractFor(canvas, cssWidth, cssHeight).scale;
}

/**
 * Initial-construction DPR. The panel re-resizes immediately with real
 * dimensions, so this only affects the very first frame before the
 * `ResizeObserver` fires. Capped at 4 because the launch viewport is
 * tiny (80x24 cells); a follow-up `renderedScaleFor` call refines it
 * once real CSS dimensions are known.
 */
function renderedScale(): number {
    if (typeof window !== "undefined" && window.devicePixelRatio) {
        return Math.max(1, Math.min(window.devicePixelRatio, 4));
    }
    return 1;
}

function terminalStubFallbackAllowed(): boolean {
    const meta = import.meta as unknown as {
        env?: Record<string, string | boolean | undefined>;
    };
    const value = meta.env?.VITE_NEOISM_ALLOW_TERMINAL_STUB;
    return value === "1" || value === "true" || value === true;
}

function formatInitError(reason: string, err?: unknown): Error {
    const suffix = err === undefined ? "" : ` Original error: ${String(err)}`;
    return new Error(
        `[neoism] ${reason}. Web terminal requires the rendered wasm/Sugarloaf ` +
        "path in normal dev so it stays aligned with shared desktop rendering. " +
        "Rebuild neoism-frontend/wasm, or set VITE_NEOISM_ALLOW_TERMINAL_STUB=1 " +
        `only when intentionally using the diagnostic stub.${suffix}`,
    );
}

class StubAdapter implements TerminalAdapter {
    constructor(private inner: WasmTerminalStub) { }
    feed(bytes: Uint8Array) {
        this.inner.feed(bytes);
    }
    resize(cols: number, rows: number, _scale: number) {
        this.inner.resize(cols, rows);
    }
    takePtyWrites() {
        return new Uint8Array();
    }
    drainEffects() {
        return [];
    }
    snapshot() {
        return this.inner.snapshot();
    }
    isReal() {
        return false;
    }
    isRendered() {
        return false;
    }
    isChrome() {
        return false;
    }
    render() {
        /* no-op: panel draws the stub via canvas2d */
    }
}

interface RealTerminalInstance {
    feed(bytes: Uint8Array): void;
    resize(cols: number, rows: number): void;
    take_pty_writes(): Uint8Array;
    drain_effects_json(): unknown[];
    snapshot(): unknown;
    free?(): void;
}

interface RenderedTerminalInstance {
    feed(bytes: Uint8Array): void;
    resize(cols: number, rows: number, scale: number): void;
    take_pty_writes(): Uint8Array;
    drain_effects_json(): unknown[];
    snapshot(): unknown;
    render(): void;
    free?(): void;
}

interface ChromeBridgeInstance {
    feed_pty_output(bytes: Uint8Array): void;
    set_markdown_remote_cursors?(peers: unknown): void;
    markdown_scroll?(deltaY: number, viewportH: number): boolean;
    markdown_click?(x: number, y: number): boolean;
    markdown_key?(key: string, ctrl: boolean): boolean;
    markdown_in_insert_mode?(): boolean;
    crdt_pump?(bufferId: string | null): string | undefined;
    crdt_apply?(json: string): boolean;
    markdown_request_save?(): boolean;
    resize(
        cols: number,
        rows: number,
        scale: number,
        widthPx: number,
        heightPx: number,
    ): void;
    take_pty_writes(): Uint8Array;
    drain_effects_json(): unknown[];
    snapshot(): unknown;
    render(timeMs: number): void;
    handle_event(eventJson: string): void;
    service_reply(requestId: bigint, payloadJson: string): void;
    set_clipboard_value(text: string | null): void;
    set_list_dir(cb: (requestId: number, path: string) => void): void;
    set_read_file(cb: (requestId: number, path: string) => void): void;
    set_write_file(
        cb: (requestId: number, path: string, bytes: Uint8Array) => void,
    ): void;
    set_stat(cb: (requestId: number, path: string) => void): void;
    set_clipboard_read(cb: (requestId: number) => void): void;
    set_clipboard_write(cb: (text: string) => void): void;
    /** Install the JS callback the bridge fires when shared chrome
     *  raises an OS-notification request (the `NotificationService`
     *  trait). Optional because pre-W3 bridges don't expose it. */
    set_notification_outbox?(
        cb: (title: string, body: string, level: string) => void,
    ): void;
    set_command_run(cb: (requestId: number, command: string) => void): void;
    set_git_status(cb: (requestId: number, repo: string) => void): void;
    set_git_diff(
        cb: (requestId: number, repo: string, path: string | null) => void,
    ): void;
    set_command_palette_workspace_visibility?(visibility: string): void;
    set_workspace_island_tabs?(payloadJson: string): void;
    workspace_island_click?(x: number, y: number): boolean;
    workspace_island_context_click?(x: number, y: number): boolean;
    drain_workspace_island_intents?(): unknown;
    focus_workspace_island?(): void;
    move_workspace_island_focus?(previous: boolean): boolean;
    activate_workspace_island_focus?(): boolean;
    buffer_tabs_focused?(): boolean;
    workspace_island_focused?(): boolean;
    blur_workspace_island?(): void;
    refresh_file_tree(): void;
    set_file_tree_entries(entriesJson: string): void;
    drain_file_tree_opens(): unknown;
    /** Hit-test the file-tree at a window-space pixel. Returns
     *  `{ path, is_dir, parent_dir, label }` for the targeted row,
     *  or `null` outside the panel / past the last row. Side effect:
     *  selection is nudged onto the hit row. Optional — pre-task-68
     *  bundles don't expose it. */
    file_tree_context_target?(x: number, y: number): unknown;
    /** Absolute path of the currently-selected file-tree row, or
     *  `null` when nothing is selected / the row has no backing path. */
    file_tree_selected_path?(): unknown;
    /** Workspace root the chrome was constructed with — used as the
     *  default "New File / New Folder" target when no row is selected. */
    file_tree_workspace_root?(): unknown;
    set_workspace_root?(workspaceRoot: string): void;
    /** True when the file tree currently owns chrome focus. */
    file_tree_focused?(): boolean;
    drain_buffer_tab_intents(): unknown;
    drain_top_bar_action?(): string | undefined;
    drain_agent_tab_opens(): number;
    drain_finder_open_intents(): unknown;
    drain_palette_intents(): unknown;
    set_buffer_tabs(titlesJson: string, active: number): void;
    apply_buffer_tab_policy(
        tabsJson: string,
        active: number,
        operation: string,
        index?: number,
    ): unknown;
    apply_session_layout_policy?(
        stateJson: string | null,
        operation: string,
        axis?: string | null,
        title?: string | null,
        externalId?: number | null,
    ): unknown;
    /** Lower a daemon `PaneLayoutSnapshot` JSON blob into the same
     *  pane-rect result `apply_session_layout_policy` returns, so the web
     *  renders the authoritative desktop split tree. Optional — older
     *  bundles only expose the local policy path. */
    mirror_pane_layout_snapshot?(snapshotJson: string): unknown;
    set_active_tab(idx: number): void;
    set_tab_content(idx: number, text: string, path: string): void;
    set_terminal_input(text: string): void;
    clear_terminal_input(): void;
    terminal_input(): string;
    terminal_command_composer_visible?(): boolean;
    terminal_should_capture_input?(): boolean;
    terminal_input_insert?(text: string): void;
    terminal_input_key?(key: string): boolean;
    terminal_submit_payload?(): Uint8Array;
    record_terminal_submit?(command: string): void;
    terminal_command_block_count?(): number;
    terminal_command_blocks_json?(): string;
    dismiss_terminal_splash(): void;
    reset_terminal_splash(): void;
    toggle_file_tree(): void;
    show_file_tree(): void;
    hide_file_tree(): void;
    show_command_palette(): void;
    show_search_palette?(): void;
    show_command_composer(): void;
    show_git_diff(): void;
    toggle_git_diff(): void;
    toggle_git_diff_panel?(): boolean;
    toggle_notes_sidebar?(): boolean;
    take_git_panel_refresh?(): boolean;
    take_notes_refresh?(): boolean;
    mark_notes_dirty?(): void;
    git_panel_set_files?(filesJson: string): void;
    git_panel_set_diff?(path: string, patch: string): void;
    git_panel_set_error?(message: string): void;
    notes_set_entries?(entriesJson: string): void;
    drain_panel_open_paths?(): unknown;
    toggle_agent_pane(): void;
    show_finder(): void;
    show_finder_files?(): void;
    show_finder_grep?(): void;
    show_finder_git_changes?(): void;
    hide_modals(): void;
    splash_click(x: number, y: number): boolean;
    splash_mouse_move(x: number, y: number): void;
    splash_mouse_leave(): void;
    splash_wordmark_click(x: number, y: number): void;
    layout_json(): unknown;
    keyboard_capture_active?(): boolean;
    editor_input_modal_active?(): boolean;
    focus_editor_input?(): void;
    animations_active?(): boolean;
    set_status_branch(branch: string | null): void;
    set_status_git_changes(added: number, deleted: number): void;
    set_ide_theme(name: string): void;
    set_cursor_style?(colorHex: string | null, style: string): void;
    set_font_scale(scale: number): void;
    enter_palette_fonts_mode?(fontsJson: string): void;
    enter_palette_themes_mode?(themesJson: string): void;
    enter_palette_shaders_mode?(shadersJson: string): void;
    enter_palette_buffers_mode?(buffersJson: string): void;
    open_workspaces_palette?(payloadJson: string): void;
    workspaces_palette_open?(): boolean;
    refresh_workspaces_palette?(payloadJson: string): void;
    markdown_cursor?(): Uint32Array | number[] | undefined;
    toggle_vi_mode?(): void;
    font_scale(): number;
    agent_event(eventJson: string): void;
    agent_set_input(text: string): void;
    agent_input(): string;
    agent_clear_input(): void;
    agent_handle_key?(
        key: string,
        code: string,
        text: string,
        shift: boolean,
        control: boolean,
        alt: boolean,
        meta: boolean,
    ): boolean;
    agent_history_step(delta: number): string;
    agent_scroll_timeline(deltaPixels: number): boolean;
    agent_pointer_down?(x: number, y: number): unknown;
    agent_scroll_at?(x: number, y: number, deltaPixels: number): boolean;
    modal_pointer_down?(x: number, y: number): number;
    modal_scroll?(x: number, y: number, deltaPixels: number): boolean;
    terminal_seed_history?(entriesJson: string): void;
    terminal_seed_completion_dir?(dir: string, entriesJson: string): void;
    drain_completion_dir_requests?(): unknown;
    agent_drag_at?(x: number, y: number, dyPixels: number): number;
    agent_drag_timeline?(deltaPixels: number): boolean;
    agent_fling_timeline?(velocityPxPerSecond: number): boolean;
    agent_input_rect_json?(): unknown;
    agent_has_conversation?(): boolean;
    agent_has_pending_permission(): boolean;
    agent_is_streaming(): boolean;
    agent_move_permission_selection(delta: number): boolean;
    agent_submit_permission(): boolean;
    agent_reply_permission(decision: string): boolean;
    set_agent_send(cb: (requestId: number, envelopeJson: string) => void): void;
    agent_attach?(directory?: string | null): void;
    agent_send_message(text: string): void;
    agent_send_message_with_attachments?(text: string, attachmentsJson: string): void;
    agent_cancel(): void;
    agent_new_thread(directory?: string | null): void;
    agent_wordmark_click?(x: number, y: number): boolean;
    active_surface(): string;
    /** Install the PTY outbox callback. Optional because the wasm
     *  bundle may pre-date the outbox method; JS guards with `?.()`.
     *  When installed, `feed_pty_output` auto-flushes pending PTY
     *  responses through this callback as base64. */
    set_pty_outbox?(cb: (bytesB64: string) => void): void;
    /** Search setters — optional because the wasm bundle may pre-date
     *  the search vocabulary. JS calls them through optional chaining;
     *  the missing methods are a no-op when the chrome doesn't surface
     *  search panels yet. */
    set_search_collect_files?(cb: (reqId: number, envelopeJson: string) => void): void;
    set_search_files?(cb: (reqId: number, envelopeJson: string) => void): void;
    set_search_grep?(cb: (reqId: number, envelopeJson: string) => void): void;
    set_search_git_changes?(cb: (reqId: number, envelopeJson: string) => void): void;
    set_search_git_repo_root?(cb: (reqId: number, envelopeJson: string) => void): void;
    set_search_cancel?(cb: (reqId: number) => void): void;
    diagnostics_event?(eventJson: string): void;
    workspace_event?(eventJson: string): void;
    /** Push the full set of diagnostic items for the active editor
     *  buffer into the chrome's gutter / virtual-text overlay. JSON
     *  array of `{ line, col, severity, message, source }` shaped
     *  records — mirrors `Vec<LspDiagnosticItem>` from the protocol.
     *  Optional because the bridge may pre-date the W3-B push surface. */
    set_diagnostics?(itemsJson: string): void;
    /** Show the diagnostic-detail popup at the cursor / a specific
     *  (line, col) cell. Coordinates are 0-based grid cells. */
    show_diagnostics_at?(line: number, col: number): void;
    /** Hide the diagnostic-detail popup AND drop the gutter/virtual
     *  overlays. Wired to `DiagnosticsCleared`. */
    hide_diagnostics?(): void;
    /** Push the LSP server name into the status-line "LSP <name>"
     *  pill. Wired to `LspStatusUpdate { state: Ready }`. */
    set_status_lsp_active?(name: string): void;
    set_status_lsp_initializing?(): void;
    set_status_lsp_missing?(): void;
    set_status_lsp_off?(): void;
    status_line_click?(x: number, y: number): unknown;
    /** Push breadcrumb segments for the active buffer (file → symbol
     *  path). JSON array of `{ label, kind }` strings. */
    set_breadcrumbs?(segmentsJson: string): void;
    /** Push the completion menu entries (LSP autocomplete). JSON
     *  array of `{ label, kind, detail, doc }`; passing `"[]"` hides
     *  the popup. */
    set_completion_menu?(itemsJson: string): void;
    /** Push the minimap viewport summary (visible-line histogram +
     *  cursor band). Single JSON blob the bridge decodes into the
     *  minimap panel's owned state. */
    set_minimap?(snapshotJson: string): void;
    /** Push a toast / status notification onto the chrome's
     *  notification stack. JSON shaped as `{ kind, title, body, ttl_ms }`. */
    push_notification?(notificationJson: string): void;
    /** Push the active git branch name into the dedicated branch pill
     *  (separate from `set_status_branch`, which writes the inline
     *  status-line segment). */
    set_git_branch_pill?(branch: string | null): void;
    /** Open the right-click / generic context menu. Wire shape
     *  documented on `ChromeBridge::set_context_menu`. Optional —
     *  pre-W3 bundles don't expose it. */
    set_context_menu?(payloadJson: string): void;
    /** Hide the context menu. Idempotent. */
    hide_context_menu?(): void;
    /** Cursor-overlay state-push surfaces. Each accepts a JSON string
     *  whose shape is documented inline on the Rust setter:
     *    - `set_trail_cursor`    : trail destination + shape
     *    - `set_custom_cursor`   : mouse-sprite position + visibility
     *    - `set_cursorline_overlay`: animated cursorline target per pane
     *    - `set_yank_flash`      : transient highlight regions
     *  Optional because pre-W3 bridges may not expose them yet. */
    set_trail_cursor?(json: string): void;
    set_custom_cursor?(json: string): void;
    set_cursorline_overlay?(json: string): void;
    set_yank_flash?(json: string): void;
    /** Returns `[cell_w, cell_h]` in physical pixels. Optional
     *  because pre-W3 bridges don't expose it; the JS dispatcher
     *  falls back to the bridge's resize defaults (8, 16) when
     *  missing. */
    cell_metrics?(): Float32Array | number[];
    free?(): void;
}

interface RealWasmModule {
    default(): Promise<unknown>;
    Terminal: new (cols: number, rows: number) => RealTerminalInstance;
    workspace_chrome_actions?: () => unknown;
    workspace_chrome_actions_for_visibility?: (visibility: string) => unknown;
    island_chrome_spec?: (scale: number) => unknown;
    island_tab_label?: (content: string, program?: string) => string;
    // wasm-bindgen exposes `async fn new` as a STATIC method named `new`
    // on the class. Calling `new SomeClass(...)` produces a JS shell with
    // a null Rust handle — that triggers "null pointer passed to rust" on
    // every subsequent method call. Always invoke as `Klass.new(...)`.
    RenderedTerminal?: {
        new(
            canvas: HTMLCanvasElement,
            cols: number,
            rows: number,
            scale: number,
        ): Promise<RenderedTerminalInstance>;
    };
    ChromeBridge?: {
        new(
            canvas: HTMLCanvasElement,
            cols: number,
            rows: number,
            scale: number,
            workspaceRoot: string,
        ): Promise<ChromeBridgeInstance>;
    };
}

export type WorkspaceChromeActionKind =
    | "share"
    | "stop_sharing"
    | "send_to_docker_sandbox"
    | "send_to_cloud";

export interface WorkspaceChromeActionSpec {
    kind: WorkspaceChromeActionKind;
    label: string;
    shortcut: string;
}

export interface IslandChromeSpec {
    height: number;
    titleFontSize: number;
    tabPaddingX: number;
    tabRadius: number;
    marginRight: number;
    iconGlyph: string;
    iconColor: [number, number, number, number];
}

const DEFAULT_WORKSPACE_CHROME_ACTIONS: WorkspaceChromeActionSpec[] = [
    { kind: "share", label: "Share Workspace", shortcut: "s" },
    { kind: "stop_sharing", label: "Stop Sharing", shortcut: "u" },
    { kind: "send_to_docker_sandbox", label: "Send to Docker Sandbox", shortcut: "d" },
    { kind: "send_to_cloud", label: "Send to Cloud", shortcut: "c" },
];

let wasmWorkspaceChromeActions: WorkspaceChromeActionSpec[] | null = null;
let wasmWorkspaceChromeActionsForVisibility: ((visibility: string) => unknown) | null = null;
let wasmIslandChromeSpec: ((scale: number) => unknown) | null = null;
let wasmIslandTabLabel: ((content: string, program?: string) => string) | null = null;

function normalizeWorkspaceChromeActions(raw: unknown): WorkspaceChromeActionSpec[] | null {
    if (!Array.isArray(raw)) return null;
    const actions = raw.flatMap((item) => {
        if (!item || typeof item !== "object") return [];
        const rec = item as Record<string, unknown>;
        const kind = typeof rec.kind === "string" ? rec.kind : "";
        const label = typeof rec.label === "string" ? rec.label : "";
        const shortcut = typeof rec.shortcut === "string" ? rec.shortcut : "";
        if (
            (kind === "share" ||
                kind === "stop_sharing" ||
                kind === "send_to_docker_sandbox" ||
                kind === "send_to_cloud") &&
            label.length > 0
        ) {
            return [{ kind, label, shortcut } as WorkspaceChromeActionSpec];
        }
        return [];
    });
    return actions.length > 0 ? actions : null;
}

export function workspaceChromeActions(): WorkspaceChromeActionSpec[] {
    return wasmWorkspaceChromeActions ?? DEFAULT_WORKSPACE_CHROME_ACTIONS;
}

export function workspaceChromeActionsForVisibility(visibility: string): WorkspaceChromeActionSpec[] {
    return normalizeWorkspaceChromeActions(wasmWorkspaceChromeActionsForVisibility?.(visibility)) ??
        workspaceChromeActions().filter((action) => {
            if ((visibility === "shared" || visibility === "team") && action.kind === "share") {
                return false;
            }
            if (visibility !== "shared" && visibility !== "team" && action.kind === "stop_sharing") {
                return false;
            }
            return true;
        });
}

function normalizeIslandChromeSpec(raw: unknown): IslandChromeSpec | null {
    if (!raw || typeof raw !== "object") return null;
    const rec = raw as Record<string, unknown>;
    const iconColor = Array.isArray(rec.iconColor) ? rec.iconColor : [];
    if (
        typeof rec.height === "number" &&
        typeof rec.titleFontSize === "number" &&
        typeof rec.tabPaddingX === "number" &&
        typeof rec.tabRadius === "number" &&
        typeof rec.marginRight === "number" &&
        typeof rec.iconGlyph === "string" &&
        iconColor.length === 4 &&
        iconColor.every((value) => typeof value === "number")
    ) {
        return {
            height: rec.height,
            titleFontSize: rec.titleFontSize,
            tabPaddingX: rec.tabPaddingX,
            tabRadius: rec.tabRadius,
            marginRight: rec.marginRight,
            iconGlyph: rec.iconGlyph,
            iconColor: iconColor as [number, number, number, number],
        };
    }
    return null;
}

export function islandChromeSpec(scale = 1): IslandChromeSpec {
    return normalizeIslandChromeSpec(wasmIslandChromeSpec?.(scale)) ?? {
        height: 28 * scale,
        titleFontSize: 11.5 * scale,
        tabPaddingX: 24 * scale,
        tabRadius: 6 * scale,
        marginRight: 8 * scale,
        iconGlyph: "󰉋",
        iconColor: [86, 156, 214, 255],
    };
}

export function islandTabLabel(content: string, program?: string): string {
    return wasmIslandTabLabel?.(content, program) ??
        (content.trim().length > 0
            ? content.replace(/\/+$/, "").split("/").filter(Boolean).at(-1) ?? content
            : program && program.length > 0
              ? program
              : "~");
}

class RealAdapter implements TerminalAdapter {
    constructor(private inner: RealTerminalInstance) { }
    feed(bytes: Uint8Array) {
        this.inner.feed(bytes);
    }
    resize(cols: number, rows: number, _scale: number) {
        this.inner.resize(cols, rows);
    }
    takePtyWrites() {
        return this.inner.take_pty_writes();
    }
    drainEffects() {
        const out = this.inner.drain_effects_json();
        return Array.isArray(out) ? out : [];
    }
    snapshot() {
        return this.inner.snapshot();
    }
    isReal() {
        return true;
    }
    isRendered() {
        return false;
    }
    isChrome() {
        return false;
    }
    render() {
        /* no-op: data-only adapter, host paints */
    }
}

class RenderedAdapter implements TerminalAdapter {
    constructor(private inner: RenderedTerminalInstance) { }
    feed(bytes: Uint8Array) {
        this.inner.feed(bytes);
    }
    resize(cols: number, rows: number, scale: number) {
        this.inner.resize(cols, rows, scale);
    }
    takePtyWrites() {
        return this.inner.take_pty_writes();
    }
    drainEffects() {
        const out = this.inner.drain_effects_json();
        return Array.isArray(out) ? out : [];
    }
    snapshot() {
        return this.inner.snapshot();
    }
    isReal() {
        return true;
    }
    isRendered() {
        return true;
    }
    isChrome() {
        return false;
    }
    render() {
        this.inner.render();
    }
}

class ChromeAdapter implements TerminalAdapter {
    constructor(private inner: ChromeBridgeInstance) { }
    feed(bytes: Uint8Array) {
        this.inner.feed_pty_output(bytes);
    }
    resize(
        cols: number,
        rows: number,
        scale: number,
        widthPx = cols * 8,
        heightPx = rows * 16,
    ) {
        this.inner.resize(
            cols,
            rows,
            scale,
            Math.max(1, Math.floor(widthPx)),
            Math.max(1, Math.floor(heightPx)),
        );
    }
    takePtyWrites() {
        return this.inner.take_pty_writes();
    }
    drainEffects() {
        const out = this.inner.drain_effects_json();
        return Array.isArray(out) ? out : [];
    }
    snapshot() {
        return this.inner.snapshot();
    }
    isReal() {
        return true;
    }
    isRendered() {
        return true;
    }
    isChrome() {
        return true;
    }
    render() {
        this.inner.render(performance.now());
    }
    handleUiEvent(event: unknown) {
        this.inner.handle_event(JSON.stringify(event));
    }
    serviceReply(requestId: number, payload: unknown) {
        this.inner.service_reply(BigInt(requestId), JSON.stringify(payload));
    }
    setClipboardValue(text: string | null) {
        this.inner.set_clipboard_value(text);
    }
    setChromeCallbacks(callbacks: ChromeCallbacks) {
        this.inner.set_list_dir((id, path) => callbacks.listDir(requestId(id), path));
        this.inner.set_read_file((id, path) => callbacks.readFile(requestId(id), path));
        this.inner.set_write_file((id, path, bytes) =>
            callbacks.writeFile(requestId(id), path, bytes),
        );
        this.inner.set_stat((id, path) => callbacks.stat(requestId(id), path));
        this.inner.set_clipboard_read((id) => callbacks.clipboardRead(requestId(id)));
        this.inner.set_clipboard_write((text) => callbacks.clipboardWrite(text));
        // Notification outbox — optional on pre-W3 bridges. The
        // host wires permission negotiation + the `Notification`
        // API in `TerminalPanel`; the bridge just relays.
        this.inner.set_notification_outbox?.((title, body, level) =>
            callbacks.notify(title, body, level),
        );
        this.inner.set_command_run((id, command) =>
            callbacks.commandRun(requestId(id), command),
        );
        this.inner.set_git_status((id, repo) => callbacks.gitStatus(requestId(id), repo));
        this.inner.set_git_diff((id, repo, path) =>
            callbacks.gitDiff(requestId(id), repo, path),
        );
    }
    refreshFileTree() {
        this.inner.refresh_file_tree();
    }
    setWorkspaceRoot(workspaceRoot: string) {
        this.inner.set_workspace_root?.(workspaceRoot);
    }
    /** Wave 7-web: remote collaborator carets for the wasm markdown
     *  pane — `[{name, color:[r,g,b], line, col_utf16}]`. */
    setMarkdownRemoteCursors(peers: unknown) {
        this.inner.set_markdown_remote_cursors?.(peers);
    }
    markdownScroll(deltaY: number, viewportH: number): boolean {
        return this.inner.markdown_scroll?.(deltaY, viewportH) ?? false;
    }
    markdownClick(x: number, y: number): boolean {
        return this.inner.markdown_click?.(x, y) ?? false;
    }
    markdownKey(key: string, ctrl: boolean): boolean {
        return this.inner.markdown_key?.(key, ctrl) ?? false;
    }
    markdownInInsertMode(): boolean {
        return this.inner.markdown_in_insert_mode?.() === true;
    }
    /** True when the loaded wasm bundle exposes the co-editing exports.
     *  False means the served bundle predates Wave 8D — the host warns
     *  the user to hard-refresh instead of silently not syncing. */
    crdtSupported(): boolean {
        return typeof this.inner.crdt_pump === "function";
    }
    /** Wave 8D web outbound co-editing: bind/flush the active markdown
     *  pane's doc and drain queued CRDT client messages (JSON array)
     *  for the host to ship. Pass null when no markdown tab is active. */
    crdtPump(bufferId: string | null): string | null {
        return this.inner.crdt_pump?.(bufferId) ?? null;
    }
    /** Route one inbound CrdtServerMessage (JSON) into the bound pane.
     *  True when visible pane text changed (host redraws). */
    crdtApply(json: string): boolean {
        return this.inner.crdt_apply?.(json) ?? false;
    }
    /** Queue a daemon-owned save of the active markdown doc. */
    markdownRequestSave(): boolean {
        return this.inner.markdown_request_save?.() ?? false;
    }
    setFileTreeEntries(entriesJson: string) {
        this.inner.set_file_tree_entries(entriesJson);
    }
    drainFileTreeOpens(): unknown {
        return this.inner.drain_file_tree_opens();
    }
    fileTreeContextTarget(x: number, y: number): FileTreeContextTarget | null {
        const raw = this.inner.file_tree_context_target?.(x, y);
        if (!raw || typeof raw !== "object") return null;
        const rec = raw as Record<string, unknown>;
        const path = typeof rec.path === "string" ? rec.path : null;
        const isDir = rec.is_dir === true;
        const parentDir =
            typeof rec.parent_dir === "string" ? rec.parent_dir : "";
        const label = typeof rec.label === "string" ? rec.label : "";
        if (parentDir.length === 0) return null;
        return { path, is_dir: isDir, parent_dir: parentDir, label };
    }
    fileTreeSelectedPath(): string | null {
        const raw = this.inner.file_tree_selected_path?.();
        return typeof raw === "string" && raw.length > 0 ? raw : null;
    }
    fileTreeWorkspaceRoot(): string | null {
        const raw = this.inner.file_tree_workspace_root?.();
        return typeof raw === "string" && raw.length > 0 ? raw : null;
    }
    fileTreeFocused(): boolean {
        return this.inner.file_tree_focused?.() === true;
    }
    drainBufferTabIntents(): BufferTabIntents | null {
        const raw = this.inner.drain_buffer_tab_intents();
        if (!raw || typeof raw !== "object") return null;
        const rec = raw as Record<string, unknown>;
        const activate =
            typeof rec.activate === "number" ? rec.activate : null;
        const close = Array.isArray(rec.close)
            ? rec.close.filter((n: unknown): n is number => typeof n === "number")
            : [];
        const newTab = rec.new_tab === true;
        return { activate, close, newTab };
    }
    drainAgentTabOpens(): number {
        return this.inner.drain_agent_tab_opens();
    }
    drainFinderOpenIntents(): FinderOpenIntent[] | null {
        const raw = this.inner.drain_finder_open_intents();
        if (!Array.isArray(raw)) return null;
        const out: FinderOpenIntent[] = [];
        for (const entry of raw) {
            if (!entry || typeof entry !== "object") continue;
            const rec = entry as Record<string, unknown>;
            const path = typeof rec.path === "string" ? rec.path : null;
            if (path === null || path.length === 0) continue;
            const lineRaw = rec.line;
            const line =
                typeof lineRaw === "number" && Number.isFinite(lineRaw)
                    ? Math.trunc(lineRaw)
                    : null;
            const modeRaw = typeof rec.mode === "string" ? rec.mode : "files";
            const mode: FinderOpenIntent["mode"] =
                modeRaw === "grep" || modeRaw === "git_changes" ? modeRaw : "files";
            const query = typeof rec.query === "string" ? rec.query : "";
            out.push({ path, line, mode, query });
        }
        return out;
    }
    drainPaletteIntents(): PaletteIntent[] | null {
        const raw = this.inner.drain_palette_intents();
        if (!Array.isArray(raw)) return null;
        const out: PaletteIntent[] = [];
        for (const entry of raw) {
            if (!entry || typeof entry !== "object") continue;
            const rec = entry as Record<string, unknown>;
            const kind = typeof rec.kind === "string" ? rec.kind : null;
            if (kind === "action" && typeof rec.action === "string") {
                out.push({ kind: "action", action: rec.action });
            } else if (kind === "ex_command" && typeof rec.command === "string") {
                out.push({ kind: "ex_command", command: rec.command });
            } else if (kind === "search" && typeof rec.query === "string") {
                const loc = rec.match_location;
                const matchLocation: [number, number] | null =
                    Array.isArray(loc) &&
                    loc.length === 2 &&
                    typeof loc[0] === "number" &&
                    Number.isFinite(loc[0]) &&
                    typeof loc[1] === "number" &&
                    Number.isFinite(loc[1])
                        ? [Math.trunc(loc[0]), Math.trunc(loc[1])]
                        : null;
                out.push({
                    kind: "search",
                    query: rec.query,
                    match_location: matchLocation,
                });
            } else if (kind === "font" && typeof rec.family === "string") {
                out.push({ kind: "font", family: rec.family });
            } else if (kind === "theme" && typeof rec.name === "string") {
                out.push({ kind: "theme", name: rec.name });
            } else if (kind === "shader" && typeof rec.title === "string") {
                out.push({
                    kind: "shader",
                    title: rec.title,
                    filter: typeof rec.filter === "string" ? rec.filter : null,
                });
            } else if (kind === "buffer") {
                const target = parsePaletteBufferTarget(rec.target);
                if (target) out.push({ kind: "buffer", target });
            } else if (
                kind === "workspace" &&
                typeof rec.workspace_id === "string" &&
                rec.workspace_id.length > 0
            ) {
                out.push({ kind: "workspace", workspace_id: rec.workspace_id });
            }
        }
        return out;
    }
    setBufferTabs(titlesJson: string, active: number) {
        this.inner.set_buffer_tabs(titlesJson, active);
    }
    applyBufferTabPolicy(
        tabsJson: string,
        active: number,
        operation: string,
        index?: number | null,
    ): unknown {
        return this.inner.apply_buffer_tab_policy(tabsJson, active, operation, index ?? undefined);
    }
    applySessionLayoutPolicy(
        stateJson: string | null,
        operation: string,
        axis?: string | null,
        title?: string | null,
        externalId?: number | null,
    ): unknown {
        return this.inner.apply_session_layout_policy?.(
            stateJson,
            operation,
            axis ?? undefined,
            title ?? undefined,
            externalId ?? undefined,
        );
    }
    mirrorPaneLayoutSnapshot(snapshotJson: string): unknown {
        return this.inner.mirror_pane_layout_snapshot?.(snapshotJson);
    }
    setActiveTab(idx: number) {
        this.inner.set_active_tab(idx);
    }
    setTabContent(idx: number, text: string, path: string) {
        this.inner.set_tab_content(idx, text, path);
    }
    setTerminalInput(text: string) {
        this.inner.set_terminal_input(text);
    }
    clearTerminalInput() {
        this.inner.clear_terminal_input();
    }
    terminalInput(): string {
        return this.inner.terminal_input();
    }
    terminalCommandComposerVisible(): boolean {
        return this.inner.terminal_command_composer_visible?.() === true;
    }
    terminalShouldCaptureInput(): boolean {
        return this.inner.terminal_should_capture_input?.() === true;
    }
    terminalInputInsert(text: string) {
        this.inner.terminal_input_insert?.(text);
    }
    terminalInputKey(key: string): boolean {
        return this.inner.terminal_input_key?.(key) === true;
    }
    terminalSubmitPayload(): Uint8Array {
        return this.inner.terminal_submit_payload?.() ?? new Uint8Array();
    }
    recordTerminalSubmit(command: string) {
        this.inner.record_terminal_submit?.(command);
    }
    terminalCommandBlockCount(): number {
        return this.inner.terminal_command_block_count?.() ?? 0;
    }
    terminalCommandBlocksJson(): string {
        return this.inner.terminal_command_blocks_json?.() ?? "[]";
    }
    dismissTerminalSplash() {
        this.inner.dismiss_terminal_splash();
    }
    resetTerminalSplash() {
        this.inner.reset_terminal_splash();
    }
    toggleFileTree() {
        this.inner.toggle_file_tree();
    }
    showFileTree() {
        this.inner.show_file_tree();
    }
    hideFileTree() {
        this.inner.hide_file_tree();
    }
    showCommandPalette() {
        this.inner.show_command_palette();
    }
    setCommandPaletteWorkspaceVisibility(visibility: string) {
        this.inner.set_command_palette_workspace_visibility?.(visibility);
    }
    setWorkspaceIslandTabs(payloadJson: string) {
        this.inner.set_workspace_island_tabs?.(payloadJson);
    }
    workspaceIslandClick(x: number, y: number): boolean {
        return this.inner.workspace_island_click?.(x, y) === true;
    }
    workspaceIslandContextClick(x: number, y: number): boolean {
        return this.inner.workspace_island_context_click?.(x, y) === true;
    }
    drainWorkspaceIslandIntents(): unknown {
        return this.inner.drain_workspace_island_intents?.() ?? [];
    }
    focusWorkspaceIsland() {
        this.inner.focus_workspace_island?.();
    }
    moveWorkspaceIslandFocus(previous: boolean): boolean {
        return this.inner.move_workspace_island_focus?.(previous) === true;
    }
    activateWorkspaceIslandFocus(): boolean {
        return this.inner.activate_workspace_island_focus?.() === true;
    }
    bufferTabsFocused(): boolean {
        return this.inner.buffer_tabs_focused?.() === true;
    }
    workspaceIslandFocused(): boolean {
        return this.inner.workspace_island_focused?.() === true;
    }
    blurWorkspaceIsland() {
        this.inner.blur_workspace_island?.();
    }
    showCommandComposer() {
        this.inner.show_command_composer();
    }
    showSearchPalette() {
        (this.inner.show_search_palette ?? this.inner.show_command_palette).call(this.inner);
    }
    showGitDiff() {
        this.inner.show_git_diff();
    }
    toggleGitDiff() {
        this.inner.toggle_git_diff();
    }
    toggleGitDiffPanel(): boolean {
        return this.inner.toggle_git_diff_panel?.() === true;
    }
    toggleNotesSidebar(): boolean {
        return this.inner.toggle_notes_sidebar?.() === true;
    }
    takeGitPanelRefresh(): boolean {
        return this.inner.take_git_panel_refresh?.() === true;
    }
    takeNotesRefresh(): boolean {
        return this.inner.take_notes_refresh?.() === true;
    }
    markNotesDirty(): void {
        this.inner.mark_notes_dirty?.();
    }
    gitPanelSetFiles(filesJson: string): void {
        this.inner.git_panel_set_files?.(filesJson);
    }
    gitPanelSetDiff(path: string, patch: string): void {
        this.inner.git_panel_set_diff?.(path, patch);
    }
    gitPanelSetError(message: string): void {
        this.inner.git_panel_set_error?.(message);
    }
    notesSetEntries(entriesJson: string): void {
        this.inner.notes_set_entries?.(entriesJson);
    }
    drainPanelOpenPaths(): unknown {
        return this.inner.drain_panel_open_paths?.();
    }
    toggleAgentPane() {
        this.inner.toggle_agent_pane();
    }
    showFinder() {
        this.inner.show_finder();
    }
    showFinderFiles() {
        (this.inner.show_finder_files ?? this.inner.show_finder).call(this.inner);
    }
    showFinderGrep() {
        (this.inner.show_finder_grep ?? this.inner.show_finder).call(this.inner);
    }
    showFinderGitChanges() {
        (this.inner.show_finder_git_changes ?? this.inner.show_finder).call(this.inner);
    }
    hideModals() {
        this.inner.hide_modals();
    }
    splashClick(x: number, y: number): boolean {
        return this.inner.splash_click(x, y);
    }
    splashMouseMove(x: number, y: number): void {
        this.inner.splash_mouse_move(x, y);
    }
    splashMouseLeave(): void {
        this.inner.splash_mouse_leave();
    }
    splashWordmarkClick(x: number, y: number): void {
        this.inner.splash_wordmark_click(x, y);
    }
    chromeLayout(): ChromeLayout | null {
        const layout = this.inner.layout_json();
        return isChromeLayout(layout) ? layout : null;
    }
    drainTopBarAction(): string | null {
        const action = this.inner.drain_top_bar_action?.();
        return typeof action === "string" ? action : null;
    }
    chromeKeyboardCaptureActive(): boolean {
        return this.inner.keyboard_capture_active?.() === true;
    }
    editorInputModalActive(): boolean {
        return this.inner.editor_input_modal_active?.() === true;
    }
    focusEditorInput(): void {
        this.inner.focus_editor_input?.();
    }
    animationsActive(): boolean {
        return this.inner.animations_active?.() === true;
    }
    setStatusBranch(branch: string | null): void {
        this.inner.set_status_branch(branch);
    }
    setStatusGitChanges(added: number, deleted: number): void {
        this.inner.set_status_git_changes(added, deleted);
    }
    setIdeTheme(name: string): void {
        this.inner.set_ide_theme(name);
    }
    /** User cursor style: optional `#RRGGBB` override + preset name
     *  (`"rainbow"` animates and ignores the color). */
    setCursorStyle(colorHex: string | null, style: string): void {
        this.inner.set_cursor_style?.(colorHex, style);
    }
    setFontScale(scale: number): void {
        this.inner.set_font_scale(scale);
    }
    enterPaletteFontsMode(fontsJson: string): void {
        this.inner.enter_palette_fonts_mode?.(fontsJson);
    }
    enterPaletteThemesMode(themesJson: string): void {
        this.inner.enter_palette_themes_mode?.(themesJson);
    }
    enterPaletteShadersMode(shadersJson: string): void {
        this.inner.enter_palette_shaders_mode?.(shadersJson);
    }
    enterPaletteBuffersMode(buffersJson: string): void {
        this.inner.enter_palette_buffers_mode?.(buffersJson);
    }
    openWorkspacesPalette(payloadJson: string): boolean {
        if (!this.inner.open_workspaces_palette) return false;
        this.inner.open_workspaces_palette(payloadJson);
        return true;
    }
    workspacesPaletteOpen(): boolean {
        return this.inner.workspaces_palette_open?.() ?? false;
    }
    refreshWorkspacesPalette(payloadJson: string): void {
        this.inner.refresh_workspaces_palette?.(payloadJson);
    }
    markdownCursor(): { line: number; columnUtf16: number; insert?: boolean } | null {
        const pair = this.inner.markdown_cursor?.();
        if (!pair || pair.length < 2) return null;
        return {
            line: Number(pair[0]),
            columnUtf16: Number(pair[1]),
            insert: pair.length > 2 ? Number(pair[2]) === 1 : undefined,
        };
    }
    toggleViMode(): void {
        this.inner.toggle_vi_mode?.();
    }
    agentEvent(eventJson: string): void {
        this.inner.agent_event(eventJson);
    }
    agentSetInput(text: string): void {
        this.inner.agent_set_input(text);
    }
    agentInput(): string {
        return this.inner.agent_input();
    }
    agentClearInput(): void {
        this.inner.agent_clear_input();
    }
    agentHandleKey(
        key: string,
        code: string,
        text: string,
        shift: boolean,
        control: boolean,
        alt: boolean,
        meta: boolean,
    ): boolean {
        return (
            this.inner.agent_handle_key?.(
                key,
                code,
                text,
                shift,
                control,
                alt,
                meta,
            ) === true
        );
    }
    agentHistoryStep(delta: number): string {
        return this.inner.agent_history_step(delta);
    }
    agentScrollTimeline(deltaPixels: number): boolean {
        return this.inner.agent_scroll_timeline(deltaPixels);
    }
    agentPointerDown(
        x: number,
        y: number,
    ): { handled: boolean; copy: string | null; link: string | null } | null {
        const raw = this.inner.agent_pointer_down?.(x, y);
        if (!raw || typeof raw !== "object") return null;
        const rec = raw as Record<string, unknown>;
        return {
            handled: rec.handled === true,
            copy: typeof rec.copy === "string" ? rec.copy : null,
            link: typeof rec.link === "string" ? rec.link : null,
        };
    }
    agentScrollAt(x: number, y: number, deltaPixels: number): boolean {
        return this.inner.agent_scroll_at?.(x, y, deltaPixels) === true;
    }
    modalPointerDown(x: number, y: number): number {
        return this.inner.modal_pointer_down?.(x, y) ?? 0;
    }
    modalScroll(x: number, y: number, deltaPixels: number): boolean {
        return this.inner.modal_scroll?.(x, y, deltaPixels) === true;
    }
    terminalSeedHistory(entriesJson: string): void {
        this.inner.terminal_seed_history?.(entriesJson);
    }
    terminalSeedCompletionDir(dir: string, entriesJson: string): void {
        this.inner.terminal_seed_completion_dir?.(dir, entriesJson);
    }
    drainCompletionDirRequests(): unknown {
        return this.inner.drain_completion_dir_requests?.();
    }
    agentDragAt(x: number, y: number, dyPixels: number): number {
        return this.inner.agent_drag_at?.(x, y, dyPixels) ?? 0;
    }
    agentDragTimeline(deltaPixels: number): boolean {
        return this.inner.agent_drag_timeline?.(deltaPixels) === true;
    }
    agentFlingTimeline(velocityPxPerSecond: number): boolean {
        return this.inner.agent_fling_timeline?.(velocityPxPerSecond) === true;
    }
    agentInputRect(): [number, number, number, number] | null {
        const raw = this.inner.agent_input_rect_json?.();
        return Array.isArray(raw) && raw.length === 4 &&
            raw.every((v) => typeof v === "number")
            ? (raw as [number, number, number, number])
            : null;
    }
    agentHasConversation(): boolean {
        return this.inner.agent_has_conversation?.() === true;
    }
    agentHasPendingPermission(): boolean {
        return this.inner.agent_has_pending_permission();
    }
    agentIsStreaming(): boolean {
        return this.inner.agent_is_streaming();
    }
    agentMovePermissionSelection(delta: number): boolean {
        return this.inner.agent_move_permission_selection(delta);
    }
    agentSubmitPermission(): boolean {
        return this.inner.agent_submit_permission();
    }
    agentReplyPermission(decision: "Yes" | "Always" | "No"): boolean {
        return this.inner.agent_reply_permission(decision);
    }
    setAgentSend(cb: (requestId: number, envelopeJson: string) => void): void {
        this.inner.set_agent_send(cb);
    }
    agentAttach(directory?: string | null): void {
        this.inner.agent_attach?.(directory ?? undefined);
    }
    agentSendMessage(text: string): void {
        this.inner.agent_send_message(text);
    }
    agentSendMessageWithAttachments(text: string, attachmentsJson: string): void {
        this.inner.agent_send_message_with_attachments?.(text, attachmentsJson);
    }
    agentCancel(): void {
        this.inner.agent_cancel();
    }
    agentNewThread(directory?: string | null): void {
        this.inner.agent_new_thread(directory ?? undefined);
    }
    agentWordmarkClick(x: number, y: number): boolean {
        return this.inner.agent_wordmark_click?.(x, y) === true;
    }
    activeSurface(): string {
        return this.inner.active_surface();
    }
    setPtyOutbox(cb: (bytesB64: string) => void): void {
        this.inner.set_pty_outbox?.(cb);
    }
    setSearchCollectFiles(cb: (reqId: number, envelopeJson: string) => void): void {
        this.inner.set_search_collect_files?.(cb);
    }
    setSearchFiles(cb: (reqId: number, envelopeJson: string) => void): void {
        this.inner.set_search_files?.(cb);
    }
    setSearchGrep(cb: (reqId: number, envelopeJson: string) => void): void {
        this.inner.set_search_grep?.(cb);
    }
    setSearchGitChanges(cb: (reqId: number, envelopeJson: string) => void): void {
        this.inner.set_search_git_changes?.(cb);
    }
    setSearchGitRepoRoot(cb: (reqId: number, envelopeJson: string) => void): void {
        this.inner.set_search_git_repo_root?.(cb);
    }
    setSearchCancel(cb: (reqId: number) => void): void {
        this.inner.set_search_cancel?.(cb);
    }
    diagnosticsEvent(eventJson: string): void {
        this.inner.diagnostics_event?.(eventJson);
    }
    workspaceEvent(eventJson: string): void {
        this.inner.workspace_event?.(eventJson);
    }
    setDiagnostics(itemsJson: string): void {
        this.inner.set_diagnostics?.(itemsJson);
    }
    showDiagnosticsAt(line: number, col: number): void {
        this.inner.show_diagnostics_at?.(line, col);
    }
    hideDiagnostics(): void {
        this.inner.hide_diagnostics?.();
    }
    setStatusLspActive(name: string): void {
        this.inner.set_status_lsp_active?.(name);
    }
    setStatusLspInitializing(): void {
        this.inner.set_status_lsp_initializing?.();
    }
    setStatusLspMissing(): void {
        this.inner.set_status_lsp_missing?.();
    }
    setStatusLspOff(): void {
        this.inner.set_status_lsp_off?.();
    }
    statusLineClick(x: number, y: number): StatusLineClickIntent | null {
        const raw = this.inner.status_line_click?.(x, y);
        if (!raw || typeof raw !== "object") return null;
        const rec = raw as Record<string, unknown>;
        const kind = typeof rec.kind === "string" ? rec.kind : "";
        switch (kind) {
            case "toggle_split":
            case "toggle_git_diff":
            case "diagnostics_opened":
            case "consumed":
                return { kind };
            case "diagnostic_jump": {
                const line =
                    typeof rec.line === "number" && Number.isFinite(rec.line)
                        ? Math.trunc(rec.line)
                        : null;
                return line && line > 0 ? { kind, line } : null;
            }
            default:
                return null;
        }
    }
    setBreadcrumbs(segmentsJson: string): void {
        this.inner.set_breadcrumbs?.(segmentsJson);
    }
    setCompletionMenu(itemsJson: string): void {
        this.inner.set_completion_menu?.(itemsJson);
    }
    setMinimap(snapshotJson: string): void {
        this.inner.set_minimap?.(snapshotJson);
    }
    pushNotification(notificationJson: string): void {
        this.inner.push_notification?.(notificationJson);
    }
    setGitBranchPill(branch: string | null): void {
        this.inner.set_git_branch_pill?.(branch);
    }
    cellMetrics(): [number, number] {
        const raw = this.inner.cell_metrics?.();
        if (raw && (raw as ArrayLike<number>).length >= 2) {
            const cw = Number((raw as ArrayLike<number>)[0]);
            const ch = Number((raw as ArrayLike<number>)[1]);
            if (Number.isFinite(cw) && Number.isFinite(ch) && cw > 0 && ch > 0) {
                return [cw, ch];
            }
        }
        // Match `ChromeAdapter.resize` defaults so the dispatcher
        // lands on the same pixel positions the chrome's resize call
        // would have used.
        return [8, 16];
    }
    setTrailCursor(json: string): void {
        this.inner.set_trail_cursor?.(json);
    }
    setCustomCursor(json: string): void {
        this.inner.set_custom_cursor?.(json);
    }
    setCursorlineOverlay(json: string): void {
        this.inner.set_cursorline_overlay?.(json);
    }
    setYankFlash(json: string): void {
        this.inner.set_yank_flash?.(json);
    }
    setContextMenu(payloadJson: string): void {
        this.inner.set_context_menu?.(payloadJson);
    }
    hideContextMenu(): void {
        this.inner.hide_context_menu?.();
    }
}

function isChromeRect(value: unknown): value is ChromeRect {
    if (!value || typeof value !== "object") return false;
    const rec = value as Record<string, unknown>;
    return ["x", "y", "w", "h"].every((key) => typeof rec[key] === "number");
}

function isChromeLayout(value: unknown): value is ChromeLayout {
    if (!value || typeof value !== "object") return false;
    const rec = value as Record<string, unknown>;
    return (
        isChromeRect(rec.buffer_tabs) &&
        isChromeRect(rec.status_line) &&
        isChromeRect(rec.terminal)
    );
}

/**
 * Try to load the real wasm bundle. Returns null if the bundle hasn't
 * been built (the import path 404s in dev mode); createTerminal decides
 * whether that is an explicit diagnostic fallback or a hard failure.
 */
async function loadRealWasm(): Promise<RealWasmModule | null> {
    try {
        // The wasm-pack output lives in src/wasm/ so vite resolves it as a
        // source module. The path is computed at runtime so dev can report a
        // clear ChromeBridge initialization error; diagnostic fallback is
        // still gated by VITE_NEOISM_ALLOW_TERMINAL_STUB.
        const wasmUrl = new URL(
            "../wasm/neoism_terminal_wasm.js",
            import.meta.url,
        ).href;
        const mod = (await import(/* @vite-ignore */ wasmUrl)) as RealWasmModule;
        await mod.default();
        wasmWorkspaceChromeActions = normalizeWorkspaceChromeActions(
            mod.workspace_chrome_actions?.(),
        );
        wasmWorkspaceChromeActionsForVisibility = mod.workspace_chrome_actions_for_visibility ?? null;
        wasmIslandChromeSpec = mod.island_chrome_spec ?? null;
        wasmIslandTabLabel = mod.island_tab_label ?? null;
        return mod;
    } catch (err) {
        if (terminalStubFallbackAllowed() && typeof console !== "undefined") {
            console.warn(
                "[neoism] real wasm bundle not found; using opt-in diagnostic stub. Build it with " +
                "`wasm-pack build --target web -d neoism-frontend/web/src/wasm neoism-frontend/wasm` " +
                "from the workspace root. (err: " +
                String(err) +
                ")",
            );
        }
        return null;
    }
}

export async function createTerminal(
    canvas: HTMLCanvasElement,
    cols: number,
    rows: number,
    workspaceRoot = "",
): Promise<TerminalAdapter> {
    const real = await loadRealWasm();
    const allowDiagnosticFallback = terminalStubFallbackAllowed();
    if (real) {
        // Prefer the chrome bridge: it owns the rendered terminal plus
        // neoism-ui panels on the same sugarloaf surface.
        let chromeBridgeError: unknown;
        if (real.ChromeBridge) {
            try {
                const scale = renderedScale();
                const klass = real.ChromeBridge as unknown as {
                    new: (
                        canvas: HTMLCanvasElement,
                        cols: number,
                        rows: number,
                        scale: number,
                        workspaceRoot: string,
                    ) => Promise<ChromeBridgeInstance>;
                };
                const chrome = await klass.new(canvas, cols, rows, scale, workspaceRoot);
                return new ChromeAdapter(chrome);
            } catch (err) {
                chromeBridgeError = err;
                if (allowDiagnosticFallback && typeof console !== "undefined") {
                    console.warn(
                        "[neoism] ChromeBridge init failed; using opt-in terminal-only " +
                        "rendered fallback. (err: " +
                        String(err) +
                        ")",
                    );
                }
            }
        } else {
            chromeBridgeError = "wasm module did not export ChromeBridge";
        }
        // RenderedTerminal is still Sugarloaf-backed, but it lacks shared
        // chrome. Keep it behind the same explicit fallback gate so normal
        // dev exercises ChromeBridge like desktop.
        if (allowDiagnosticFallback && real.RenderedTerminal) {
            try {
                const scale = renderedScale();
                // Static async constructor — NOT `new` (wasm-bindgen async ctors
                // export as static methods; `new` produces a JS shell with a
                // null Rust handle that crashes on every method call).
                const klass = real.RenderedTerminal as unknown as {
                    new: (
                        canvas: HTMLCanvasElement,
                        cols: number,
                        rows: number,
                        scale: number,
                    ) => Promise<RenderedTerminalInstance>;
                };
                const rendered = await klass.new(canvas, cols, rows, scale);
                return new RenderedAdapter(rendered);
            } catch (err) {
                if (typeof console !== "undefined") {
                    console.warn(
                        "[neoism] RenderedTerminal init failed; using opt-in data-only " +
                        "diagnostic fallback. (err: " +
                        String(err) +
                        ")",
                    );
                }
                // Fall through to data-only path.
            }
        }
        if (allowDiagnosticFallback) {
            return new RealAdapter(new real.Terminal(cols, rows));
        }
        throw formatInitError("ChromeBridge did not start", chromeBridgeError);
    }
    if (allowDiagnosticFallback) {
        return new StubAdapter(new WasmTerminalStub(cols, rows));
    }
    throw formatInitError("real wasm bundle was not loaded");
}
