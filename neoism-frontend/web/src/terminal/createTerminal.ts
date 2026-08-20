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
    bufferTabHitTest?(x: number, y: number): number;
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
    /** Shared PaneGrid pointer surface — divider drag, focus-by-click,
     *  drag-to-split previews — in window-space canvas coordinates.
     *  Bit flags: down 1=consumed 2=divider-drag 4=focus-changed;
     *  move 1=consumed 2=layout-changed; up 1=consumed 2=tree-changed. */
    paneGridPointerDown?(x: number, y: number): number;
    paneGridPointerMove?(x: number, y: number): number;
    paneGridPointerUp?(x: number, y: number): number;
    /** Begin the Rust-painted tear-out drop preview for a buffer-tab
     *  drag; `paneGridDragPreview` updates it, `paneGridCancelDrag`
     *  clears it. */
    paneGridBeginTabDrag?(): void;
    paneGridDragPreview?(x: number, y: number): boolean;
    paneGridCancelDrag?(): void;
    /** Drain PaneGridAction side effects queued by pointer interactions:
     *  `[{kind:"focus_pane"|"close_pane",external_id}|{kind:"open_pane",
     *  leaf_id,leaf_kind}|{kind:"relayout"}]`. */
    drainPaneGridActions?(): unknown;
    /** Current live pane-grid layout in the same result shape
     *  `applySessionLayoutPolicy` returns — re-pulled after pointer
     *  interactions mutate the Rust-owned tree. */
    paneGridLayoutResult?(): unknown;
    /** Push per-pane surface descriptors (`[{external_id, kind, path?,
     *  title?}]` JSON) so the chrome can render unfocused editor panes
     *  and label placeholders. */
    setPaneSurfaces?(json: string): void;
    /** Feed one split pane terminal's PTY stream (creates + seeds the
     *  pane-sized grid on first feed). */
    feedPaneTerminal?(externalId: number, bytes: Uint8Array): void;
    paneTerminalExists?(externalId: number): boolean;
    removePaneTerminal?(externalId: number): void;
    /** Retain only the pane terminals listed in `keepJson` (id array). */
    prunePaneTerminals?(keepJson: string): void;
    /** Replace one pane's local tab strip (`[{title, path?, kind?}]`
     *  JSON; empty drops the strip + its breadcrumbs). */
    setPaneTabs?(externalId: number, tabsJson: string, active: number): void;
    /** Drop pane strips whose panes went away (id array JSON). */
    retainPaneTabs?(keepJson: string): void;
    /** Drain queued per-pane strip interactions:
     *  `[{external_id, kind: "activate"|"close"|"new_tab", index}]`. */
    drainPaneTabIntents?(): unknown;
    /** Shared workspace-strip tab drag: arm at a window point (returns
     *  the dragged tab index or -1), advance, query the tear-out
     *  threshold, and release (`{kind: "none"|"reorder"|"tear_out",
     *  from?, to?, index?, release?}`). */
    bufferTabBeginDrag?(x: number, y: number): number;
    bufferTabUpdateDrag?(x: number, y: number): boolean;
    bufferTabDragTearArmed?(): boolean;
    bufferTabEndDrag?(): unknown;
    bufferTabCancelDrag?(): void;
    setActiveTab?(idx: number): void;
    setTabContent?(idx: number, text: string, path: string): void;
    setTerminalInput?(text: string): void;
    clearTerminalInput?(): void;
    terminalInput?(): string;
    terminalCommandComposerVisible?(): boolean;
    terminalShouldCaptureInput?(): boolean;
    terminalInputInsert?(text: string): void;
    /** Composer-owned paste: newline-preserving insert_paste (desktop
     *  Screen::paste parity) — never touches the PTY. Returns false
     *  when the wasm bundle predates the export so callers can fall
     *  back to the byte path. */
    terminalInputInsertPaste?(text: string): boolean;
    /** paste_policy-framed PTY bytes for a raw paste: bracketed
     *  sentinels when BRACKETED_PASTE is set, CR-normalised raw
     *  otherwise. Undefined on pre-export bundles. */
    terminalPastePayload?(text: string): Uint8Array | undefined;
    /** Star/unstar a command in the composer's favorites (feeds the
     *  Ctrl+F picker). True=added, false=removed, undefined=blank
     *  command or pre-export bundle. Desktop analogue: Cmd+F on a
     *  hovered block card. */
    terminalToggleFavoriteCommand?(command: string): boolean | undefined;
    terminalInputKey?(key: string): boolean;
    terminalSubmitPayload?(): Uint8Array;
    recordTerminalSubmit?(command: string): void;
    /** Wheel over the terminal grid (winit sign: positive = scroll
     *  up/left). Returns TERM_INPUT_* bit flags: 1 = handled,
     *  2 = PTY bytes queued (drain takeTerminalPointerBytes),
     *  4 = selection drag active. 0 = route to chrome. */
    terminalWheel?(
        x: number,
        y: number,
        deltaX: number,
        deltaY: number,
        shift: boolean,
    ): number;
    /** Pointer press on the terminal grid (button 0/1/2 = L/M/R).
     *  Same TERM_INPUT_* flag contract as terminalWheel. */
    terminalPointerDown?(
        x: number,
        y: number,
        button: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
        nowMs: number,
    ): number;
    terminalPointerMove?(
        x: number,
        y: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
    ): number;
    terminalPointerUp?(
        x: number,
        y: number,
        button: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
    ): number;
    /** 15ms selection-drag autoscroll tick (desktop
     *  selection_scroll_tick). True = redraw needed. */
    terminalDragScrollTick?(): boolean;
    /** Hover-probe the terminal grid for a clickable link (OSC 8 /
     *  URL / file token) and update the hover underline. Bits:
     *  1 = hover changed (redraw), 2 = link under pointer,
     *  4 = dir-listing requests queued (drain
     *  terminalDrainLinkDirRequests). */
    terminalHoverProbe?(x: number, y: number): number;
    /** Queued link-open intents from a link click / hint fire:
     *  `[{kind: "url"|"file"|"dir", target, line?}]`. */
    terminalDrainLinkOpens?(): unknown;
    /** Parent dirs the link existence probe wants listed through the
     *  daemon (answered via terminalSeedCompletionDir). */
    terminalDrainLinkDirRequests?(): unknown;
    /** Land a deferred file:line jump once the opened pane is live.
     *  False while no editor/markdown pane exists yet (retry). */
    terminalLinkGotoLine?(line: number): boolean;
    /** Enter hint mode (desktop Ctrl+Shift+O binding): label visible
     *  links, open on keystroke narrowing. False = no matches. */
    terminalHintStart?(): boolean;
    /** True while hint mode owns the keyboard. */
    terminalHintActive?(): boolean;
    /** Route one keydown into hint mode. Bits: 1 = consumed,
     *  2 = a match fired (drain terminalDrainLinkOpens). */
    terminalHintKey?(key: string): number;
    /** Drain queued PTY-bound mouse-report / CSI bytes. */
    takeTerminalPointerBytes?(): Uint8Array;
    /** Current selection text (undefined when empty). */
    terminalSelectedText?(): string | undefined;
    terminalHasSelection?(): boolean;
    terminalClearSelection?(): void;
    /** Shift+PageUp / Shift+PageDown scrollback paging; false on the
     *  alt screen (caller falls back to the PTY escape). */
    terminalScrollPage?(up: boolean): boolean;
    /** PTY-bound key: snap scrollback to the live tail + clear the
     *  selection (desktop SendToPty semantics). True = redraw. */
    terminalNotifyKeyInput?(): boolean;
    /** Desktop-parity PTY key encoder: DOM `KeyboardEvent` fields in,
     *  PTY byte sequence out. Reads the live terminal modes (DECCKM
     *  app-cursor, kitty keyboard protocol flags, alt screen, vi) and
     *  walks the exact desktop decision path (bindings Esc table →
     *  alt masking → kitty/CSI vs raw UTF-8). Empty array = the key is
     *  not PTY-bound in the current mode. Null = the wasm bundle
     *  predates the export; callers fall back to the legacy TS table. */
    encodeTerminalKey?(
        key: string,
        code: string,
        ctrl: boolean,
        alt: boolean,
        shift: boolean,
        meta: boolean,
        repeat: boolean,
    ): Uint8Array | null;
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
    /** Show the "Share with phone" QR sheet. Empty url => message-only. */
    /** Open the SHARED servers palette (desktop's server manager). */
    openServersPalette?(entriesJson: string): void;
    shareSheetShow?(url: string, hint?: string): void;
    shareSheetDismiss?(): boolean;
    shareSheetVisible?(): boolean;
    /** Point the notes sidebar at the host's linked vault
     *  (`WorkspaceSummary.linked_vault_dir`), or `null` when the
     *  workspace links none. Notes live only in vaults - this is NOT
     *  derivable from the workspace root. */
    setNotesVaultRoot?(vault: string | null): void;
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
    // ── Chrome helper pages (Settings / Extensions / NeoWorld / About) ──
    /** Open the full-screen Settings overlay. `configJson` is the daemon
     *  host's config.json as one JSON document; omit to open immediately
     *  and follow up with `setSettingsValues` once the fetch lands. */
    openSettingsPage?(configJson?: string | null): void;
    /** Refresh the settings overlay with a newer config snapshot. */
    setSettingsValues?(configJson: string): void;
    settingsPageActive?(): boolean;
    /** Drain queued settings actions as a JSON array string:
     *  `[{kind:"set",key,value}|{kind:"set_keybind",action,key,with}|
     *   {kind:"open_config_file"}|{kind:"run_action",action}]`.
     *  Null when nothing is pending. */
    drainSettingsActions?(): string | null;
    /** Open the About modal (version + commit). */
    openAboutModal?(): void;
    /** Seed the read-only Extensions catalog from the daemon's
     *  `ListExtensions` reply (JSON `ExtensionSummary[]`). */
    setExtensionsEntries?(entriesJson: string): void;
    /** Auto-focus the Extensions search box (page-open parity). */
    extensionsFocusSearch?(): void;
    /** Drain Extensions intents: `[{kind:"open_repository",url}]`. */
    drainExtensionsActions?(): string | null;
    /** Ensure the NeoWorld pane exists, seeding from a persisted
     *  StoredPet JSON blob (localStorage) when available. */
    neoworldEnsure?(storedJson?: string | null): void;
    /** Newest pet snapshot to persist (StoredPet JSON), or null. */
    drainNeoworldSnapshot?(): string | null;
    // ── Chrome-hosted UniversalModal (spec-driven channel) ──
    /** True while the chrome-hosted modal is up (it owns the keyboard
     *  via `chromeKeyboardCaptureActive`). */
    modalActive?(): boolean;
    /** Open desktop's "New File" prompt for `dir` (absolute path;
     *  echoed back verbatim in the drained action). */
    openFileTreeNewFileModal?(dir: string): void;
    /** Open desktop's "New Folder" prompt for `dir`. */
    openFileTreeNewFolderModal?(dir: string): void;
    /** Open desktop's "Rename" prompt for `path`, pre-filled with the
     *  current file name. */
    openFileTreeRenameModal?(path: string): void;
    /** Open desktop's destructive "Delete file/folder?" confirm.
     *  `isDir` may be omitted — the chrome resolves the kind from its
     *  tree entries. */
    openFileTreeDeleteModal?(path: string, isDir?: boolean | null): void;
    /** Open desktop's LSP rename form pre-filled with `word`
     *  (Enter submits, Esc cancels). */
    openLspRenameModal?(word: string): void;
    /** Open an arbitrary chrome-hosted modal from a JSON `ModalSpec`
     *  (see the wasm `open_modal_spec` docs for the shape). */
    openModalSpec?(specJson: string): void;
    /** Drain confirmed modal outcomes as a JSON array string:
     *  `[{kind:"file_tree_new_file",dir,name}|{kind:"file_tree_new_folder",dir,name}|
     *    {kind:"file_tree_rename",path,name}|{kind:"file_tree_delete",path}|
     *    {kind:"lsp_rename",name}|{kind:"generic",id,value}]`.
     *  Null when nothing was confirmed since the last drain. */
    drainModalActions?(): string | null;
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
    /** Full IDE theme catalog from the shared Rust source of truth
     *  (`all_ide_theme_names`): builtins + bundled NvChad Base46 set
     *  (+ runtime customs when registered). `dark` is the shared
     *  background-luma split; `accent` is `#rrggbb` for presence
     *  cursor colors. Empty when the served bundle predates the
     *  export — hosts fall back to the builtin four. */
    allIdeThemes?(): Array<{ name: string; dark: boolean; accent: string }>;
    /** Shared drag-to-split drop hit test
     *  (`session_layout::geometry::drop_zone_at` + DEFAULT_EDGE_FRAC).
     *  `panesJson` is the normalized pane list JSON; `(x, y)` is the
     *  pointer in the same unit space. Null = missed every pane. */
    paneDropTarget?(
        panesJson: string,
        x: number,
        y: number,
    ): {
        external_id: number;
        placement: string;
        rect: { x: number; y: number; w: number; h: number };
    } | null;
    /** Feed the wasm file tree's `path -> peers` presence index so
     *  tree rows light collaborator avatars (desktop parity:
     *  `rebuild_file_tree_presence_index`). Entries are
     *  `[{buffer_id, peers}]`; push only when presence changes. */
    setPresenceIndex?(entries: unknown): void;
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
    /** Desktop-breadth markdown key routing (shared dispatcher). */
    markdownKeyFull?(
        key: string,
        ctrl: boolean,
        shift: boolean,
        alt: boolean,
        meta: boolean,
    ): boolean;
    /** True when the served bundle exposes `markdown_key_full`. */
    markdownKeyFullSupported?(): boolean;
    /** True while a markdown `/`-search session owns the keyboard. */
    markdownSearchActive?(): boolean;
    /** Drag-move over the markdown pane (selection / block reorder). */
    markdownDragMove?(x: number, y: number): boolean;
    /** Pointer release for the markdown pane (drop / finish drag). */
    markdownMouseRelease?(): boolean;
    /** Right-click spelling menu for the word under the pointer. */
    markdownSpellingMenuAt?(x: number, y: number): boolean;
    /** Clipboard text queued by the last handled markdown event. */
    markdownDrainClipboardOut?(): string | null;
    /** Queued markdown activations (`markdown`/`editor`/`external`/
     *  `rename` intents), or null. */
    markdownDrainOpenIntents?(): unknown;
    /** Seed the markdown unnamed register from the browser clipboard. */
    markdownSeedClipboard?(text: string): void;
    /** Open a fetched non-markdown file into the right chrome-hosted
     *  editor pane (`.ipynb` → notebook, `.neodraw` → draw, else the
     *  native code pane). Returns the pane kind. */
    editorOpenFile?(tabIndex: number, path: string, text: string): string;
    /** Which hosted editor pane serves the ACTIVE tab: "code" /
     *  "notebook" / "draw", or null for terminal/markdown/agent tabs. */
    editorActiveKind?(): string | null;
    /** Drop every hosted editor pane (backing tab closed). */
    editorClosePanes?(): void;
    /** Route one `event.key` to the active editor pane (vim + standard
     *  editing for code, cell/vim surface for notebooks, tools for
     *  draw). True when consumed. */
    editorKey?(key: string, ctrl: boolean, shift: boolean, alt: boolean): boolean;
    /** Insert browser-pasted text into the active editor pane. */
    editorInsertPaste?(text: string): boolean;
    /** Pointer press in the editor surface (canvas CSS px). */
    editorPointerDown?(
        x: number,
        y: number,
        shift: boolean,
        ctrl: boolean,
        clickCount: number,
    ): boolean;
    /** Pointer move: drag-select / scrollbar drag / draw gestures. */
    editorPointerMove?(x: number, y: number): boolean;
    /** Pointer release for the editor surface. */
    editorPointerUp?(): boolean;
    /** Wheel over the editor surface (code/notebook scroll, draw
     *  pan; ctrl = zoom for draw). */
    editorScroll?(
        x: number,
        y: number,
        deltaX: number,
        deltaY: number,
        ctrl: boolean,
    ): boolean;
    /** Text the last handled editor key queued for the SYSTEM
     *  clipboard (vim yank, Ctrl+C/X). */
    editorDrainClipboardOut?(): string | null;
    /** Whether the active editor pane has unsaved changes. */
    editorDirty?(): boolean;
    /** The active code pane's caret for the presence plane. */
    editorCursor?(): { line: number; columnUtf16: number; insert?: boolean } | null;
    /** Bytes a host-side save should write for the active pane. */
    editorSavePayload?(): string | null;
    /** Record a successful host-side write of the payload. */
    editorMarkSaved?(payload: string): void;
    /** Queue a save: "crdt" (daemon single-writer via code_crdt_pump),
     *  "host" (write editorSavePayload through Files), or "none". */
    editorRequestSave?(): string;
    /** Queue a save with format-on-save: "format" (formatter fired;
     *  the save resumes via the `save_after_format` host action), else
     *  the same answers as editorRequestSave. */
    editorRequestSaveFormatted?(): string;
    /** Register the callback that ships one serialized LSP
     *  `EditorClientMessage` over the daemon editor envelope. */
    setEditorLspRequest?(cb: (envelopeJson: string) => void): void;
    /** Route one daemon `EditorReply` payload (JSON) into the code
     *  pane's LSP session. True when visible state changed. */
    editorLspReply?(json: string): boolean;
    /** Drain queued LSP host actions (open / rename_prompt /
     *  save_after_format) as a JSON array, or null when idle. */
    editorLspHostActions?(): string | null;
    /** Submit the rename prompt's answer. */
    editorLspRenameSubmit?(name: string): void;
    /** Code-pane co-editing pump — the code twin of crdtPump. */
    codeCrdtPump?(bufferId: string | null): string | null;
    /** Route one inbound CrdtServerMessage into the bound CODE pane. */
    editorCrdtApply?(json: string): boolean;
    /** Remote collaborator carets for the hosted CODE pane (same wire
     *  shape as setMarkdownRemoteCursors). */
    editorSetRemoteCursors?(peers: unknown): void;
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
    ): {
        handled: boolean;
        copy: string | null;
        link: string | null;
        selecting: boolean;
    } | null;
    /** Extend an in-progress timeline text selection (pointermove while
     *  the button is held, after `agentPointerDown` reported
     *  `selecting`). */
    agentSelectionDrag?(x: number, y: number): boolean;
    /** Finish the drag and return the selected text for the clipboard,
     *  or null when it was just a click. */
    agentSelectionEnd?(): string | null;
    agentHasActiveSelection?(): boolean;
    /** Position-aware wheel: picker / side panel / diff card / timeline. */
    /** Desktop-parity wheel: raw deltaY + DOM deltaMode, smoothed for
     *  notches by the shared scroll policy. */
    agentScrollWheelAt?(
        x: number,
        y: number,
        deltaY: number,
        deltaMode: number,
    ): boolean;
    /** Adopt daemon-computed tree-sitter spans for the code pane. */
    codeSetHighlightSpans?(
        path: string,
        revision: number,
        spansJson: string,
    ): boolean;
    codeBufferRevision?(): number;
    agentScrollAt?(x: number, y: number, deltaPixels: number): boolean;
    /** Axis-specific wheel for rendered Markdown code/table overflow. */
    agentScrollHorizontalAt?(x: number, y: number, deltaPixels: number): boolean;
    /** Direct pointer drag for the sticky code/table horizontal rail. */
    agentDragMarkdownHorizontalScrollbar?(x: number): boolean;
    agentEndMarkdownHorizontalScrollbarDrag?(): boolean;
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
    agentInsertPaste?(text: string): boolean;
    /** Attach a pasted clipboard image to the shared composer as an
     *  `[imageN]` token + chip (desktop's Ctrl+V-with-image flow). The
     *  image is sent with the next Enter, not immediately. Returns
     *  false when the pane rejected it (bad mime / over 20MB). */
    agentAttachClipboardImage?(
      filename: string,
      mime: string,
      bytes: Uint8Array,
    ): boolean;
    /** Attach a host-read file (drag-and-drop onto the agent pane) as a
     *  composer chip — the web analogue of desktop's DroppedFile →
     *  attach_path. Empty mime is sniffed from the filename. */
    agentAttachFile?(filename: string, mime: string, bytes: Uint8Array): boolean;
    /** Active `@`-mention query in the shared composer (text between
     *  `@` and the caret), or null when none is being typed. */
    agentFileMentionQuery?(): string | null;
    /** Feed the `@`-mention candidate list (JSON array of
     *  workspace-relative file paths); the pane fuzzy-ranks per
     *  keystroke, so re-feed only when the file list changes. */
    agentSetFileMentionCandidates?(json: string): boolean;
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
    | { kind: "workspace"; workspace_id: string }
    | { kind: "server"; action: string; id: string };

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
    markdown_key_full?(
        key: string,
        ctrl: boolean,
        shift: boolean,
        alt: boolean,
        meta: boolean,
    ): boolean;
    markdown_search_active?(): boolean;
    markdown_drag_move?(x: number, y: number): boolean;
    markdown_mouse_release?(): boolean;
    markdown_spelling_menu_at?(x: number, y: number): boolean;
    markdown_drain_clipboard_out?(): string | undefined;
    markdown_drain_open_intents?(): unknown;
    markdown_seed_clipboard?(text: string): void;
    markdown_in_insert_mode?(): boolean;
    crdt_pump?(bufferId: string | null): string | undefined;
    crdt_apply?(json: string): boolean;
    markdown_request_save?(): boolean;
    editor_open_file?(tabIndex: number, path: string, text: string): string;
    editor_active_kind?(): string | undefined;
    editor_close_panes?(): void;
    editor_key?(key: string, ctrl: boolean, shift: boolean, alt: boolean): boolean;
    editor_insert_paste?(text: string): boolean;
    editor_pointer_down?(
        x: number,
        y: number,
        shift: boolean,
        ctrl: boolean,
        clickCount: number,
    ): boolean;
    editor_pointer_move?(x: number, y: number): boolean;
    editor_pointer_up?(): boolean;
    editor_scroll?(
        x: number,
        y: number,
        deltaX: number,
        deltaY: number,
        ctrl: boolean,
    ): boolean;
    editor_drain_clipboard_out?(): string | undefined;
    editor_dirty?(): boolean;
    editor_cursor?(): Uint32Array | number[] | undefined;
    editor_save_payload?(): string | undefined;
    editor_mark_saved?(payload: string): void;
    editor_request_save?(): string;
    editor_request_save_formatted?(): string;
    set_editor_lsp_request?(cb: (envelopeJson: string) => void): void;
    editor_lsp_reply?(json: string): boolean;
    editor_lsp_host_actions?(): string | undefined;
    editor_lsp_rename_submit?(name: string): void;
    code_crdt_pump?(bufferId: string | null): string | undefined;
    editor_crdt_apply?(json: string): boolean;
    editor_set_remote_cursors?(peers: unknown): void;
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
    set_notes_vault_root?(vault: string | undefined): void;
    open_servers_palette?(entriesJson: string): void;
    share_sheet_show?(url: string, hint: string | undefined): void;
    share_sheet_dismiss?(): boolean;
    share_sheet_visible?(): boolean;
    /** True when the file tree currently owns chrome focus. */
    file_tree_focused?(): boolean;
    drain_buffer_tab_intents(): unknown;
    buffer_tab_hit_test?(x: number, y: number): number;
    drain_top_bar_action?(): string | undefined;
    open_settings_page?(configJson?: string | null): void;
    set_settings_values?(configJson: string): void;
    settings_page_active?(): boolean;
    drain_settings_actions?(): unknown;
    open_about_modal?(): void;
    set_extensions_entries?(entriesJson: string): void;
    extensions_focus_search?(): void;
    drain_extensions_actions?(): unknown;
    neoworld_ensure?(storedJson?: string | null): void;
    drain_neoworld_snapshot?(): string | undefined;
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
    /** Shared PaneGrid pointer surface (divider drag, focus-by-click,
     *  drag-to-split previews). Window-space canvas coordinates; bit
     *  flags per method doc in the wasm bridge. Optional — pre-pane-grid
     *  bundles fall back to keyboard-only resize. */
    pane_grid_pointer_down?(x: number, y: number): number;
    pane_grid_pointer_move?(x: number, y: number): number;
    pane_grid_pointer_up?(x: number, y: number): number;
    pane_grid_begin_tab_drag?(): void;
    pane_grid_drag_preview?(x: number, y: number): boolean;
    pane_grid_cancel_drag?(): void;
    drain_pane_grid_actions?(): unknown;
    pane_grid_layout_result?(): unknown;
    /** Per-pane terminal surfaces for split panes. */
    draw_pane_grid_host_surfaces?(): void;
    set_pane_surfaces?(json: string): void;
    feed_pane_terminal?(externalId: number, bytes: Uint8Array): void;
    pane_terminal_exists?(externalId: number): boolean;
    remove_pane_terminal?(externalId: number): void;
    prune_pane_terminals?(keepJson: string): void;
    /** Per-pane tab strips + breadcrumbs (desktop pane_tabs parity). */
    set_pane_tabs?(externalId: number, tabsJson: string, active: number): void;
    retain_pane_tabs?(keepJson: string): void;
    drain_pane_tab_intents?(): unknown;
    /** Shared workspace-strip tab drag pipeline. */
    buffer_tab_begin_drag?(x: number, y: number): number;
    buffer_tab_update_drag?(x: number, y: number): boolean;
    buffer_tab_drag_tear_armed?(): boolean;
    buffer_tab_end_drag?(): unknown;
    buffer_tab_cancel_drag?(): void;
    set_active_tab(idx: number): void;
    set_tab_content(idx: number, text: string, path: string): void;
    set_terminal_input(text: string): void;
    clear_terminal_input(): void;
    terminal_input(): string;
    terminal_command_composer_visible?(): boolean;
    terminal_should_capture_input?(): boolean;
    terminal_input_insert?(text: string): void;
    terminal_input_insert_paste?(text: string): void;
    terminal_paste_payload?(text: string): Uint8Array;
    terminal_toggle_favorite_command?(command: string): boolean | undefined;
    terminal_input_key?(key: string): boolean;
    terminal_submit_payload?(): Uint8Array;
    record_terminal_submit?(command: string): void;
    terminal_wheel?(
        x: number,
        y: number,
        deltaX: number,
        deltaY: number,
        shift: boolean,
    ): number;
    terminal_pointer_down?(
        x: number,
        y: number,
        button: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
        nowMs: number,
    ): number;
    terminal_pointer_move?(
        x: number,
        y: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
    ): number;
    terminal_pointer_up?(
        x: number,
        y: number,
        button: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
    ): number;
    terminal_drag_scroll_tick?(): boolean;
    terminal_hover_probe?(x: number, y: number): number;
    terminal_drain_link_opens?(): unknown;
    terminal_drain_link_dir_requests?(): unknown;
    terminal_link_goto_line?(line: number): boolean;
    terminal_hint_start?(): boolean;
    terminal_hint_active?(): boolean;
    terminal_hint_key?(key: string): number;
    take_terminal_pointer_bytes?(): Uint8Array;
    terminal_selected_text?(): string | undefined;
    terminal_has_selection?(): boolean;
    terminal_clear_selection?(): void;
    terminal_scroll_page?(up: boolean): boolean;
    terminal_notify_key_input?(): boolean;
    encode_terminal_key?(
        key: string,
        code: string,
        ctrl: boolean,
        alt: boolean,
        shift: boolean,
        meta: boolean,
        repeat: boolean,
    ): Uint8Array;
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
    modal_active?(): boolean;
    open_file_tree_new_file_modal?(dir: string): void;
    open_file_tree_new_folder_modal?(dir: string): void;
    open_file_tree_rename_modal?(path: string): void;
    open_file_tree_delete_modal?(path: string, isDir?: boolean | null): void;
    open_lsp_rename_modal?(word: string): void;
    open_modal_spec?(specJson: string): void;
    drain_modal_actions?(): unknown;
    animations_active?(): boolean;
    set_status_branch(branch: string | null): void;
    set_status_git_changes(added: number, deleted: number): void;
    set_ide_theme(name: string): void;
    all_ide_theme_names?(): unknown;
    pane_drop_target?(panesJson: string, x: number, y: number): unknown;
    set_presence_index?(entries: unknown): void;
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
    code_set_highlight_spans?(
        path: string,
        revision: number,
        spansJson: string,
    ): boolean;
    code_buffer_revision?(): number;
    agent_scroll_wheel_at?(
        x: number,
        y: number,
        deltaY: number,
        deltaMode: number,
    ): boolean;
    agent_selection_drag?(x: number, y: number): boolean;
    agent_selection_end?(): string | undefined;
    agent_has_active_selection?(): boolean;
    agent_scroll_horizontal_at?(x: number, y: number, deltaPixels: number): boolean;
    agent_drag_markdown_horizontal_scrollbar?(x: number): boolean;
    agent_end_markdown_horizontal_scrollbar_drag?(): boolean;
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
    agent_insert_paste?(text: string): boolean;
    agent_attach_clipboard_image?(
      filename: string,
      mime: string,
      bytes: Uint8Array,
    ): boolean;
    agent_attach_file?(filename: string, mime: string, bytes: Uint8Array): boolean;
    agent_file_mention_query?(): unknown;
    agent_set_file_mention_candidates?(json: string): boolean;
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
    set_minimap?(routeId: number, snapshotJson: string): void;
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

// ------------------------------------------------------------------
// Shared input-policy exports (wasm/src/rendered/input_policy.rs).
//
// IME composition, touch gestures, mobile soft-keyboard decisions and
// the remote-presence store all live in shared Rust; the wasm module
// exports them alongside `ChromeBridge`. The module-level holder below
// captures them once the bundle loads so the synchronous TS adapters
// (`services/imePolicy.ts`, `services/touchPolicy.ts`,
// `mobile/MobileKeyboard.ts`, `presence/RemotePresenceStore.ts`) can
// route their decisions through Rust without owning the async load.
// ------------------------------------------------------------------

/** One shared-policy touch classifier instance (`TouchGesturePolicy`). */
export interface WasmTouchGesturePolicyInstance {
    reset(): void;
    is_active(): boolean;
    start(id: number, x: number, y: number, timeMs: number, zone: string): unknown;
    move(
        id: number,
        x: number,
        y: number,
        timeMs: number,
        width: number,
        height: number,
    ): unknown;
    end(
        id: number,
        x: number,
        y: number,
        timeMs: number,
        width: number,
        height: number,
    ): unknown;
    tick_long_press(nowMs: number, width: number, height: number): unknown;
    free?(): void;
}

/** One shared outbound presence publisher (`PresencePublisherBridge`). */
export interface WasmPresencePublisherInstance {
    peer_id(): string;
    set_color(r: number, g: number, b: number): void;
    set_rainbow(rainbow: boolean): void;
    tick(active: unknown, nowMs: number): unknown;
    free?(): void;
}

/** One shared remote-presence store instance (`PresenceStoreBridge`). */
export interface WasmPresenceStoreInstance {
    set_local_peer_id(peerId: string): void;
    apply_server_message(message: unknown): boolean;
    cursors_for(bufferId: string): unknown;
    has_remote_cursors(bufferId: string): boolean;
    any_rainbow(): boolean;
    has_any_peers(): boolean;
    avatar_peers_by_buffer(): unknown;
    prune_stale(nowMs: number, ttlMs: number): boolean;
    clear(): boolean;
    free?(): void;
}

/** The input-policy slice of the loaded wasm module. Every member is
 *  optional so a stale served bundle (built before the exports landed)
 *  degrades gracefully — adapters fall back or warn per their own
 *  contract. */
export interface WasmInputPolicyModule {
    ime_commit_dispatch?: (
        text: string,
    ) => { text: string; useBracketedPaste: boolean } | null;
    ime_should_drop_keys_during_compose?: (hasPreedit: boolean) => boolean;
    ime_key_event_is_composing?: (isComposing: boolean, keyCode: number) => boolean;
    ime_assistant_blocks?: (assistantActive: boolean) => boolean;
    TouchGesturePolicy?: new () => WasmTouchGesturePolicyInstance;
    touch_should_suppress_swipe_back?: (zone: string) => boolean;
    mobile_keyboard_inset?: (
        innerHeight: number,
        viewportHeight: number,
        viewportOffsetTop: number,
    ) => { bottom: number; keyboardOpen: boolean } | null;
    mobile_input_attributes?: (
        context: string,
        toolbarVisible: boolean,
    ) => {
        autocapitalize: string;
        autocorrect: string;
        spellcheck: string;
        inputmode: string;
        enterkeyhint: string;
    } | null;
    mobile_named_key_bytes?: (key: string) => Uint8Array | undefined;
    mobile_ctrl_chord_byte?: (text: string) => number | undefined;
    PresenceStoreBridge?: new () => WasmPresenceStoreInstance;
    PresencePublisherBridge?: new (
        peerId: string,
        displayName: string,
        minIntervalMs?: number,
        heartbeatIntervalMs?: number,
    ) => WasmPresencePublisherInstance;
}

let wasmInputPolicyModule: WasmInputPolicyModule | null = null;
const wasmInputPolicyListeners: Array<(mod: WasmInputPolicyModule) => void> = [];

/** The loaded wasm module's input-policy exports, or `null` before the
 *  bundle finishes loading (and forever in stub-only diagnostic runs). */
export function wasmInputPolicy(): WasmInputPolicyModule | null {
    return wasmInputPolicyModule;
}

/** Run `listener` once the input-policy exports are available —
 *  immediately when the module already loaded. */
export function onWasmInputPolicyReady(
    listener: (mod: WasmInputPolicyModule) => void,
): void {
    if (wasmInputPolicyModule) {
        listener(wasmInputPolicyModule);
        return;
    }
    wasmInputPolicyListeners.push(listener);
}

/** Install the loaded module's exports. Called by `loadRealWasm` on
 *  the real bundle; tests inject a fake module through the same door. */
export function installWasmInputPolicy(mod: WasmInputPolicyModule): void {
    wasmInputPolicyModule = mod;
    const listeners = wasmInputPolicyListeners.splice(0);
    for (const listener of listeners) {
        try {
            listener(mod);
        } catch (err) {
            if (typeof console !== "undefined") {
                console.warn("[neoism] input-policy ready listener threw", err);
            }
        }
    }
}

interface RealWasmModule extends WasmInputPolicyModule {
    default(
        moduleOrPath?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module,
    ): Promise<unknown>;
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
        // Split pane surfaces (per-pane terminal grids) queue their
        // draws first so they join the same swapchain flip as the
        // chrome frame below.
        this.inner.draw_pane_grid_host_surfaces?.();
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
    /** Desktop-breadth markdown key routing (shared dispatcher):
     *  operators/visual mode/motions, undo/redo, tables, lists, title
     *  editing, `/` block menu, `[[` completion, `/` incsearch.
     *  Null-ish false when the bundle predates the export — the host
     *  falls back to `markdownKey`. */
    markdownKeyFull(
        key: string,
        ctrl: boolean,
        shift: boolean,
        alt: boolean,
        meta: boolean,
    ): boolean {
        return this.inner.markdown_key_full?.(key, ctrl, shift, alt, meta) ?? false;
    }
    markdownKeyFullSupported(): boolean {
        return typeof this.inner.markdown_key_full === "function";
    }
    /** True while a markdown `/`-search session owns the keyboard. */
    markdownSearchActive(): boolean {
        return this.inner.markdown_search_active?.() === true;
    }
    /** Drag-move over the markdown pane (selection extend / block
     *  reorder), desktop `handle_markdown_drag_move`. */
    markdownDragMove(x: number, y: number): boolean {
        return this.inner.markdown_drag_move?.(x, y) ?? false;
    }
    /** Pointer release for the markdown pane (drop reordered block,
     *  finish selection, open a queued block menu). */
    markdownMouseRelease(): boolean {
        return this.inner.markdown_mouse_release?.() ?? false;
    }
    /** Right-click spelling menu for the word under the pointer. */
    markdownSpellingMenuAt(x: number, y: number): boolean {
        return this.inner.markdown_spelling_menu_at?.(x, y) ?? false;
    }
    /** Text the last handled markdown key/press queued for the system
     *  clipboard (yanks, copy chip, contact-link yank). */
    markdownDrainClipboardOut(): string | null {
        return this.inner.markdown_drain_clipboard_out?.() ?? null;
    }
    /** Queued markdown activations: `[{ kind: "markdown" | "editor" |
     *  "external" | "rename", target, line? }]`, or null. */
    markdownDrainOpenIntents(): unknown {
        return this.inner.markdown_drain_open_intents?.() ?? null;
    }
    /** Seed the markdown unnamed register from the browser clipboard
     *  so vim `p` pastes real clipboard text. */
    markdownSeedClipboard(text: string): void {
        this.inner.markdown_seed_clipboard?.(text);
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
    editorOpenFile(tabIndex: number, path: string, text: string): string {
        return this.inner.editor_open_file?.(tabIndex, path, text) ?? "";
    }
    editorActiveKind(): string | null {
        return this.inner.editor_active_kind?.() ?? null;
    }
    editorClosePanes(): void {
        this.inner.editor_close_panes?.();
    }
    editorKey(key: string, ctrl: boolean, shift: boolean, alt: boolean): boolean {
        return this.inner.editor_key?.(key, ctrl, shift, alt) ?? false;
    }
    editorInsertPaste(text: string): boolean {
        return this.inner.editor_insert_paste?.(text) ?? false;
    }
    editorPointerDown(
        x: number,
        y: number,
        shift: boolean,
        ctrl: boolean,
        clickCount: number,
    ): boolean {
        return this.inner.editor_pointer_down?.(x, y, shift, ctrl, clickCount) ?? false;
    }
    editorPointerMove(x: number, y: number): boolean {
        return this.inner.editor_pointer_move?.(x, y) ?? false;
    }
    editorPointerUp(): boolean {
        return this.inner.editor_pointer_up?.() ?? false;
    }
    editorScroll(
        x: number,
        y: number,
        deltaX: number,
        deltaY: number,
        ctrl: boolean,
    ): boolean {
        return this.inner.editor_scroll?.(x, y, deltaX, deltaY, ctrl) ?? false;
    }
    editorDrainClipboardOut(): string | null {
        return this.inner.editor_drain_clipboard_out?.() ?? null;
    }
    editorDirty(): boolean {
        return this.inner.editor_dirty?.() === true;
    }
    editorCursor(): { line: number; columnUtf16: number; insert?: boolean } | null {
        const pair = this.inner.editor_cursor?.();
        if (!pair || pair.length < 2) return null;
        return {
            line: Number(pair[0]),
            columnUtf16: Number(pair[1]),
            insert: pair.length > 2 ? Number(pair[2]) === 1 : undefined,
        };
    }
    editorSavePayload(): string | null {
        return this.inner.editor_save_payload?.() ?? null;
    }
    editorMarkSaved(payload: string): void {
        this.inner.editor_mark_saved?.(payload);
    }
    editorRequestSave(): string {
        return this.inner.editor_request_save?.() ?? "none";
    }
    editorRequestSaveFormatted(): string {
        return (
            this.inner.editor_request_save_formatted?.() ??
            this.inner.editor_request_save?.() ??
            "none"
        );
    }
    setEditorLspRequest(cb: (envelopeJson: string) => void): void {
        this.inner.set_editor_lsp_request?.(cb);
    }
    editorLspReply(json: string): boolean {
        return this.inner.editor_lsp_reply?.(json) ?? false;
    }
    editorLspHostActions(): string | null {
        return this.inner.editor_lsp_host_actions?.() ?? null;
    }
    editorLspRenameSubmit(name: string): void {
        this.inner.editor_lsp_rename_submit?.(name);
    }
    /** Code-pane co-editing pump — the code twin of `crdtPump`. */
    codeCrdtPump(bufferId: string | null): string | null {
        return this.inner.code_crdt_pump?.(bufferId) ?? null;
    }
    /** Route one inbound CrdtServerMessage into the bound CODE pane. */
    editorSetRemoteCursors(peers: unknown): void {
        this.inner.editor_set_remote_cursors?.(peers);
    }
    editorCrdtApply(json: string): boolean {
        return this.inner.editor_crdt_apply?.(json) ?? false;
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
            } else if (
                kind === "server" &&
                typeof rec.action === "string" &&
                typeof rec.id === "string" &&
                rec.id.length > 0
            ) {
                out.push({ kind: "server", action: rec.action, id: rec.id });
            }
        }
        return out;
    }
    setBufferTabs(titlesJson: string, active: number) {
        this.inner.set_buffer_tabs(titlesJson, active);
    }
    bufferTabHitTest(x: number, y: number): number {
        return this.inner.buffer_tab_hit_test?.(x, y) ?? -1;
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
    paneGridPointerDown(x: number, y: number): number {
        return this.inner.pane_grid_pointer_down?.(x, y) ?? 0;
    }
    paneGridPointerMove(x: number, y: number): number {
        return this.inner.pane_grid_pointer_move?.(x, y) ?? 0;
    }
    paneGridPointerUp(x: number, y: number): number {
        return this.inner.pane_grid_pointer_up?.(x, y) ?? 0;
    }
    paneGridBeginTabDrag(): void {
        this.inner.pane_grid_begin_tab_drag?.();
    }
    paneGridDragPreview(x: number, y: number): boolean {
        return this.inner.pane_grid_drag_preview?.(x, y) === true;
    }
    paneGridCancelDrag(): void {
        this.inner.pane_grid_cancel_drag?.();
    }
    drainPaneGridActions(): unknown {
        return this.inner.drain_pane_grid_actions?.() ?? [];
    }
    paneGridLayoutResult(): unknown {
        return this.inner.pane_grid_layout_result?.();
    }
    setPaneSurfaces(json: string): void {
        this.inner.set_pane_surfaces?.(json);
    }
    feedPaneTerminal(externalId: number, bytes: Uint8Array): void {
        this.inner.feed_pane_terminal?.(externalId, bytes);
    }
    paneTerminalExists(externalId: number): boolean {
        return this.inner.pane_terminal_exists?.(externalId) === true;
    }
    removePaneTerminal(externalId: number): void {
        this.inner.remove_pane_terminal?.(externalId);
    }
    prunePaneTerminals(keepJson: string): void {
        this.inner.prune_pane_terminals?.(keepJson);
    }
    setPaneTabs(externalId: number, tabsJson: string, active: number): void {
        this.inner.set_pane_tabs?.(externalId, tabsJson, active);
    }
    retainPaneTabs(keepJson: string): void {
        this.inner.retain_pane_tabs?.(keepJson);
    }
    drainPaneTabIntents(): unknown {
        return this.inner.drain_pane_tab_intents?.() ?? [];
    }
    bufferTabBeginDrag(x: number, y: number): number {
        return this.inner.buffer_tab_begin_drag?.(x, y) ?? -1;
    }
    bufferTabUpdateDrag(x: number, y: number): boolean {
        return this.inner.buffer_tab_update_drag?.(x, y) === true;
    }
    bufferTabDragTearArmed(): boolean {
        return this.inner.buffer_tab_drag_tear_armed?.() === true;
    }
    bufferTabEndDrag(): unknown {
        return this.inner.buffer_tab_end_drag?.();
    }
    bufferTabCancelDrag(): void {
        this.inner.buffer_tab_cancel_drag?.();
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
    terminalInputInsertPaste(text: string): boolean {
        if (typeof this.inner.terminal_input_insert_paste !== "function") {
            return false;
        }
        this.inner.terminal_input_insert_paste(text);
        return true;
    }
    terminalPastePayload(text: string): Uint8Array | undefined {
        return this.inner.terminal_paste_payload?.(text);
    }
    terminalToggleFavoriteCommand(command: string): boolean | undefined {
        return this.inner.terminal_toggle_favorite_command?.(command);
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
    terminalWheel(
        x: number,
        y: number,
        deltaX: number,
        deltaY: number,
        shift: boolean,
    ): number {
        return this.inner.terminal_wheel?.(x, y, deltaX, deltaY, shift) ?? 0;
    }
    terminalPointerDown(
        x: number,
        y: number,
        button: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
        nowMs: number,
    ): number {
        return (
            this.inner.terminal_pointer_down?.(x, y, button, shift, ctrl, alt, nowMs) ??
            0
        );
    }
    terminalPointerMove(
        x: number,
        y: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
    ): number {
        return this.inner.terminal_pointer_move?.(x, y, shift, ctrl, alt) ?? 0;
    }
    terminalPointerUp(
        x: number,
        y: number,
        button: number,
        shift: boolean,
        ctrl: boolean,
        alt: boolean,
    ): number {
        return this.inner.terminal_pointer_up?.(x, y, button, shift, ctrl, alt) ?? 0;
    }
    terminalDragScrollTick(): boolean {
        return this.inner.terminal_drag_scroll_tick?.() === true;
    }
    /** Link hover probe (hover underline + dir-request queueing). */
    terminalHoverProbe(x: number, y: number): number {
        return this.inner.terminal_hover_probe?.(x, y) ?? 0;
    }
    /** Queued link-open intents (`[{kind, target, line?}]`). */
    terminalDrainLinkOpens(): unknown {
        return this.inner.terminal_drain_link_opens?.() ?? null;
    }
    /** Parent dirs the link existence probe wants listed. */
    terminalDrainLinkDirRequests(): unknown {
        return this.inner.terminal_drain_link_dir_requests?.() ?? null;
    }
    /** Deferred file:line jump; false until the pane is live. */
    terminalLinkGotoLine(line: number): boolean {
        return this.inner.terminal_link_goto_line?.(line) === true;
    }
    /** Enter terminal hint mode (desktop Ctrl+Shift+O). */
    terminalHintStart(): boolean {
        return this.inner.terminal_hint_start?.() === true;
    }
    terminalHintActive(): boolean {
        return this.inner.terminal_hint_active?.() === true;
    }
    /** Route one keydown into hint mode (1 = consumed, 2 = fired). */
    terminalHintKey(key: string): number {
        return this.inner.terminal_hint_key?.(key) ?? 0;
    }
    takeTerminalPointerBytes(): Uint8Array {
        return this.inner.take_terminal_pointer_bytes?.() ?? new Uint8Array();
    }
    terminalSelectedText(): string | undefined {
        return this.inner.terminal_selected_text?.() ?? undefined;
    }
    terminalHasSelection(): boolean {
        return this.inner.terminal_has_selection?.() === true;
    }
    terminalClearSelection() {
        this.inner.terminal_clear_selection?.();
    }
    terminalScrollPage(up: boolean): boolean {
        return this.inner.terminal_scroll_page?.(up) === true;
    }
    terminalNotifyKeyInput(): boolean {
        return this.inner.terminal_notify_key_input?.() === true;
    }
    encodeTerminalKey(
        key: string,
        code: string,
        ctrl: boolean,
        alt: boolean,
        shift: boolean,
        meta: boolean,
        repeat: boolean,
    ): Uint8Array | null {
        if (typeof this.inner.encode_terminal_key !== "function") {
            // Stale wasm bundle: signal the caller to use the legacy
            // TS fallback table.
            return null;
        }
        return this.inner.encode_terminal_key(
            key,
            code,
            ctrl,
            alt,
            shift,
            meta,
            repeat,
        );
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
    setNotesVaultRoot(vault: string | null): void {
        this.inner.set_notes_vault_root?.(vault ?? undefined);
    }
    openServersPalette(entriesJson: string): void {
        this.inner.open_servers_palette?.(entriesJson);
    }
    shareSheetShow(url: string, hint?: string): void {
        this.inner.share_sheet_show?.(url, hint);
    }
    shareSheetDismiss(): boolean {
        return this.inner.share_sheet_dismiss?.() === true;
    }
    shareSheetVisible(): boolean {
        return this.inner.share_sheet_visible?.() === true;
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
    openSettingsPage(configJson?: string | null) {
        this.inner.open_settings_page?.(configJson ?? undefined);
    }
    setSettingsValues(configJson: string) {
        this.inner.set_settings_values?.(configJson);
    }
    settingsPageActive(): boolean {
        return this.inner.settings_page_active?.() === true;
    }
    drainSettingsActions(): string | null {
        const raw = this.inner.drain_settings_actions?.();
        return typeof raw === "string" ? raw : null;
    }
    openAboutModal() {
        this.inner.open_about_modal?.();
    }
    setExtensionsEntries(entriesJson: string) {
        this.inner.set_extensions_entries?.(entriesJson);
    }
    extensionsFocusSearch() {
        this.inner.extensions_focus_search?.();
    }
    drainExtensionsActions(): string | null {
        const raw = this.inner.drain_extensions_actions?.();
        return typeof raw === "string" ? raw : null;
    }
    neoworldEnsure(storedJson?: string | null) {
        this.inner.neoworld_ensure?.(storedJson ?? undefined);
    }
    drainNeoworldSnapshot(): string | null {
        return this.inner.drain_neoworld_snapshot?.() ?? null;
    }
    modalActive(): boolean {
        return this.inner.modal_active?.() === true;
    }
    openFileTreeNewFileModal(dir: string) {
        this.inner.open_file_tree_new_file_modal?.(dir);
    }
    openFileTreeNewFolderModal(dir: string) {
        this.inner.open_file_tree_new_folder_modal?.(dir);
    }
    openFileTreeRenameModal(path: string) {
        this.inner.open_file_tree_rename_modal?.(path);
    }
    openFileTreeDeleteModal(path: string, isDir?: boolean | null) {
        this.inner.open_file_tree_delete_modal?.(path, isDir ?? null);
    }
    openLspRenameModal(word: string) {
        this.inner.open_lsp_rename_modal?.(word);
    }
    openModalSpec(specJson: string) {
        this.inner.open_modal_spec?.(specJson);
    }
    drainModalActions(): string | null {
        const raw = this.inner.drain_modal_actions?.();
        return typeof raw === "string" ? raw : null;
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
    allIdeThemes(): Array<{ name: string; dark: boolean; accent: string }> {
        const raw = this.inner.all_ide_theme_names?.();
        return Array.isArray(raw)
            ? (raw as Array<{ name: string; dark: boolean; accent: string }>)
            : [];
    }
    paneDropTarget(
        panesJson: string,
        x: number,
        y: number,
    ): {
        external_id: number;
        placement: string;
        rect: { x: number; y: number; w: number; h: number };
    } | null {
        const zone = this.inner.pane_drop_target?.(panesJson, x, y);
        if (!zone || typeof zone !== "object") return null;
        return zone as {
            external_id: number;
            placement: string;
            rect: { x: number; y: number; w: number; h: number };
        };
    }
    setPresenceIndex(entries: unknown): void {
        this.inner.set_presence_index?.(entries);
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
    ): {
        handled: boolean;
        copy: string | null;
        link: string | null;
        selecting: boolean;
    } | null {
        const raw = this.inner.agent_pointer_down?.(x, y);
        if (!raw || typeof raw !== "object") return null;
        const rec = raw as Record<string, unknown>;
        return {
            handled: rec.handled === true,
            copy: typeof rec.copy === "string" ? rec.copy : null,
            link: typeof rec.link === "string" ? rec.link : null,
            selecting: rec.selecting === true,
        };
    }
    agentSelectionDrag(x: number, y: number): boolean {
        return this.inner.agent_selection_drag?.(x, y) === true;
    }
    agentSelectionEnd(): string | null {
        const text = this.inner.agent_selection_end?.();
        return typeof text === "string" && text.length > 0 ? text : null;
    }
    agentHasActiveSelection(): boolean {
        return this.inner.agent_has_active_selection?.() === true;
    }
    codeSetHighlightSpans(
        path: string,
        revision: number,
        spansJson: string,
    ): boolean {
        return (
            this.inner.code_set_highlight_spans?.(path, revision, spansJson) === true
        );
    }
    codeBufferRevision(): number {
        return this.inner.code_buffer_revision?.() ?? 0;
    }
    agentScrollAt(x: number, y: number, deltaPixels: number): boolean {
        return this.inner.agent_scroll_at?.(x, y, deltaPixels) === true;
    }
    agentScrollWheelAt(
        x: number,
        y: number,
        deltaY: number,
        deltaMode: number,
    ): boolean {
        return this.inner.agent_scroll_wheel_at?.(x, y, deltaY, deltaMode) === true;
    }
    agentScrollHorizontalAt(x: number, y: number, deltaPixels: number): boolean {
        return this.inner.agent_scroll_horizontal_at?.(x, y, deltaPixels) === true;
    }
    agentDragMarkdownHorizontalScrollbar(x: number): boolean {
        return this.inner.agent_drag_markdown_horizontal_scrollbar?.(x) === true;
    }
    agentEndMarkdownHorizontalScrollbarDrag(): boolean {
        return this.inner.agent_end_markdown_horizontal_scrollbar_drag?.() === true;
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
    agentInsertPaste(text: string): boolean {
        return this.inner.agent_insert_paste?.(text) ?? false;
    }
    agentSendMessageWithAttachments(text: string, attachmentsJson: string): void {
        this.inner.agent_send_message_with_attachments?.(text, attachmentsJson);
    }
    agentAttachClipboardImage(
        filename: string,
        mime: string,
        bytes: Uint8Array,
    ): boolean {
        return (
            this.inner.agent_attach_clipboard_image?.(filename, mime, bytes) === true
        );
    }
    agentAttachFile(filename: string, mime: string, bytes: Uint8Array): boolean {
        return this.inner.agent_attach_file?.(filename, mime, bytes) === true;
    }
    agentFileMentionQuery(): string | null {
        const query = this.inner.agent_file_mention_query?.();
        return typeof query === "string" ? query : null;
    }
    agentSetFileMentionCandidates(json: string): boolean {
        return this.inner.agent_set_file_mention_candidates?.(json) === true;
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
        // Route 0 is the single-surface web pane; Chrome::draw falls
        // back to painting route 0 over the terminal rect when no
        // pane-grid external ids are bound.
        this.inner.set_minimap?.(0, snapshotJson);
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
        // wasm-bindgen's generated default loader resolves its `.wasm`
        // relative to the generated JS module. After Vite code-splits that
        // module into `dist/assets`, the raw relative lookup points at a file
        // Vite never emitted and the preview server returns index.html. A
        // literal source URL lets Vite copy/hash the binary and gives the
        // loader the exact production URL explicitly.
        const wasmBinaryUrl = new URL(
            "../wasm/neoism_terminal_wasm_bg.wasm",
            import.meta.url,
        );
        const mod = (await import(/* @vite-ignore */ wasmUrl)) as RealWasmModule;
        await mod.default(wasmBinaryUrl);
        wasmWorkspaceChromeActions = normalizeWorkspaceChromeActions(
            mod.workspace_chrome_actions?.(),
        );
        wasmWorkspaceChromeActionsForVisibility = mod.workspace_chrome_actions_for_visibility ?? null;
        wasmIslandChromeSpec = mod.island_chrome_spec ?? null;
        wasmIslandTabLabel = mod.island_tab_label ?? null;
        // Hand the shared input-policy exports (IME / touch / mobile
        // keyboard / presence store) to their synchronous TS adapters.
        installWasmInputPolicy(mod);
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
