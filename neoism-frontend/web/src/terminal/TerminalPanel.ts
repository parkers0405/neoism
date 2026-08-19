import { ProtocolClient } from "../workspace/ProtocolClient";
import type { PtyService } from "../services/PtyService";
import type { WorkplacePreferences } from "../services/WorkplaceService";
import { isMarkdownPath, renderMarkdownDocument } from "./MarkdownRenderer";
import { WasmTerminalStub, type TerminalSnapshot } from "./WasmTerminalStub";
import {
  createTerminal,
  sizeContractFor,
  type ChromeRect,
  type FileTreeContextTarget,
  type PaletteBufferTarget,
  type TerminalAdapter,
  type WorkspacesModalPayload,
} from "./createTerminal";
import { MarkdownPresenceOverlay } from "./MarkdownPresenceOverlay";
import {
  localPresenceIdentity,
  presenceBufferIdForPath,
} from "../presence/presence";
import {
  PresencePublisher,
  WORKSPACE_PRESENCE_BUFFER_ID,
  type ActivePresenceTarget,
} from "../presence/PresencePublisher";
import { RemotePresenceStore } from "../presence/RemotePresenceStore";
import { MobileKeyboard } from "../mobile/MobileKeyboard";
import {
  fromCompositionCommit,
  fromCompositionEnd,
  fromCompositionStart,
  fromCompositionUpdate,
  fromKeyboardEvent,
  fromKeyPressEvent,
  fromPointerDownEvent,
  fromPointerMoveEvent,
  fromPointerUpEvent,
  fromResizeEvent,
  fromTextEvent,
  fromWheelEvent,
  pointerLeaveEvent,
} from "../services/eventTranslator";
import {
  commitDispatch,
  keyEventIsImeComposing,
  shouldDropKeysDuringCompose,
} from "../services/imePolicy";
import {
  MAX_TAP_DISTANCE,
  TouchPolicy,
  type TouchAction as TouchPolicyAction,
  type TouchSample,
  type TouchZone,
} from "../services/touchPolicy";
import type {
  FilesServerMessage,
  GitServerMessage,
  AgentServerMessage,
  Attachment,
  DiffHunk as WireDiffHunk,
  ClipboardPayload,
  WorkspaceAction,
  WorkspaceTabSummary,
  WorkspaceServerMessage,
  DiagnosticsServerMessage,
  CursorOverlayServerMessage,
  CrdtServerMessage,
  CrdtClientMessage,
  EditorSurfaceSummary,
  EditorClientMessage,
} from "../workspace/types";
import type { ServiceRegistryBridge } from "../services/ServiceRegistry";
import {
  fetchConfig,
  fetchExtensions,
  loadStoredNeoworldPet,
  parseSettingsActions,
  persistKeybind,
  persistSetting,
  saveStoredNeoworldPet,
} from "../services/ConfigService";

const CELL_WIDTH = 8;
const CELL_HEIGHT = 16;
const MIN_COLS = 20;
const MIN_ROWS = 6;
const MAX_REPLAY_BYTES_PER_PTY = 2 * 1024 * 1024;
const MOBILE_SCROLL_TAP_SLOP = 10;
const TERMINAL_RESET_BYTES = new TextEncoder().encode("\x1bc\x1b[3J\x1b[H\x1b[2J");
// FALLBACK theme list only — used while the wasm bridge is still
// loading (or a stale bundle predates the export). The real catalog
// (~100 themes: builtins + the bundled NvChad Base46 set) comes from
// the bridge's `all_ide_theme_names` export, the same shared source
// of truth the desktop pickers read; see `ideThemeNames()`.
const WEB_IDE_THEMES = [
  "pastel_dark",
  "nvchad_one",
  "tokyo_night",
  "catppuccin_mocha",
] as const;
const WEB_SHADER_FILTERS = [
  { title: "None", detail: "Disable shaders", filter: null },
  { title: "Classic CRT TV", detail: "Browser CRT approximation", filter: "crt_curve" },
  { title: "New Pixie CRT", detail: "High contrast scanline filter", filter: "newpixiecrt" },
] as const;

// Tab-content ReadFile requests use a private id space starting above
// the wasm bridge's own counter range so the two never collide.
let nextFileReadRequestId = 0x4000_0000;

type BufferTabKind = "terminal" | "file" | "neoism-agent";

interface WebBufferTab {
  title: string;
  kind: BufferTabKind;
  path?: string;
  sessionId?: string;
  neoismAgentRouteId?: number;
}

interface PendingTerminalTabSpawn {
  title?: string;
  command?: string;
  /** Pane the freshly spawned shell should bind to (terminal split). */
  paneExternalId?: number;
}

interface WebPaneState {
  tabIndices: number[];
  activeTabIndex: number | null;
}

type BufferTabPolicyOperation =
  | "select_previous"
  | "select_next"
  | "select_index"
  | "move_previous"
  | "move_next"
  | "close_active"
  | "close_index"
  | "reorder";

interface BufferTabPolicyResult {
  active: number;
  remove_index?: number | null;
  move_from?: number | null;
  move_to?: number | null;
  changed: boolean;
}

type WebPaneSplitAxis = "horizontal" | "vertical";
type WebPaneResizeDirection = "up" | "down" | "left" | "right";
type WebPaneDropPlacement = "left" | "right" | "top" | "bottom" | "center";

interface WebPaneDropTarget {
  paneExternalId: number;
  placement: WebPaneDropPlacement;
  rect: { x: number; y: number; w: number; h: number };
}

interface WebBufferTabDrag {
  pointerId: number;
  tabIndex: number;
  startX: number;
  startY: number;
  active: boolean;
  target: WebPaneDropTarget | null;
}
type MobileChromeTouchTarget =
  | "buffer-tabs"
  | "file-tree"
  | "text-entry"
  | "other";

interface MobileBufferTabPan {
  id: number;
  start: TouchSample;
  last: TouchSample;
  panning: boolean;
  /** Trailing-window finger velocity samples for release momentum. */
  samples: Array<{ t: number; dx: number; dy: number }>;
  /** The touch-down stopped an in-flight glide; suppress the tap. */
  suppressTap: boolean;
}

interface MobileFileTreePan {
  id: number;
  start: TouchSample;
  last: TouchSample;
  scrolling: boolean;
  samples: Array<{ t: number; dx: number; dy: number }>;
  suppressTap: boolean;
}

interface WebPaneRect {
  external_id: number;
  leaf_id: number;
  kind: string;
  title?: string | null;
  focused: boolean;
  x: number;
  y: number;
  w: number;
  h: number;
}

interface WebSessionLayoutPolicyResult {
  state_json: string;
  focused_external_id?: number | null;
  active_external_ids: number[];
  panes: WebPaneRect[];
  changed: boolean;
}

function parseBufferTabPolicyResult(value: unknown): BufferTabPolicyResult | null {
  if (!value || typeof value !== "object") return null;
  const rec = value as Record<string, unknown>;
  const active = rec.active;
  if (typeof active !== "number" || !Number.isFinite(active)) return null;
  return {
    active,
    remove_index: typeof rec.remove_index === "number" ? rec.remove_index : null,
    move_from: typeof rec.move_from === "number" ? rec.move_from : null,
    move_to: typeof rec.move_to === "number" ? rec.move_to : null,
    changed: rec.changed === true,
  };
}

function parseSessionLayoutPolicyResult(value: unknown): WebSessionLayoutPolicyResult | null {
  if (!value || typeof value !== "object") return null;
  const rec = value as Record<string, unknown>;
  if (typeof rec.state_json !== "string") return null;
  const rawPanes = Array.isArray(rec.panes) ? rec.panes : [];
  const panes: WebPaneRect[] = [];
  for (const raw of rawPanes) {
    if (!raw || typeof raw !== "object") continue;
    const pane = raw as Record<string, unknown>;
    const externalId = pane.external_id;
    const leafId = pane.leaf_id;
    const x = pane.x;
    const y = pane.y;
    const w = pane.w;
    const h = pane.h;
    if (
      typeof externalId !== "number" ||
      typeof leafId !== "number" ||
      typeof x !== "number" ||
      typeof y !== "number" ||
      typeof w !== "number" ||
      typeof h !== "number"
    ) {
      continue;
    }
    panes.push({
      external_id: externalId,
      leaf_id: leafId,
      kind: typeof pane.kind === "string" ? pane.kind : "editor",
      title: typeof pane.title === "string" ? pane.title : null,
      focused: pane.focused === true,
      x,
      y,
      w,
      h,
    });
  }
  return {
    state_json: rec.state_json,
    focused_external_id:
      typeof rec.focused_external_id === "number" ? rec.focused_external_id : null,
    active_external_ids: Array.isArray(rec.active_external_ids)
      ? rec.active_external_ids.filter((id): id is number => typeof id === "number")
      : [],
    panes,
    changed: rec.changed === true,
  };
}

export interface TerminalPanelOptions {
  client: ProtocolClient;
  sessionId: string;
  mount: HTMLElement;
  /**
   * Optional formal PTY backend handle. When provided, `sendInput` /
   * `resize` go through it instead of the raw `ProtocolClient`. Lets
   * the panel be repointed at a different backend (a future native /
   * SharedArrayBuffer impl, in-page Web Worker, etc.) without
   * touching the panel internals.
   */
  pty?: PtyService;
  workspaceRoot?: string | null;
  /**
   * Fired once the wasm bridge has finished initialising. The host
   * binds protocol-level services (search, …) at this point because
   * they need the bridge to route replies back into the chrome's
   * pending-request slots.
   *
   * `bridge` is the same `TerminalAdapter` instance the panel
   * already uses internally; it is also a valid
   * `SearchBridge` since the bridge surface is a strict
   * superset of the search-service hook.
   */
  onBridgeReady?: (bridge: ServiceRegistryBridge) => void;
  onFontSizeChanged?: (fontSize: number) => void;
  onShowWorkplaces?: () => void;
  /**
   * Supply the host→workspace tree for the wasm Workspaces modal
   * (desktop's Ctrl+Shift+W picker rendered by the shared command
   * palette). Returning `null` (no tree yet / fallback adapter) makes
   * the panel fall back to `onShowWorkplaces`'s DOM overlay. The host
   * should also kick a `RequestHostWorkspaceTree` refresh inside this
   * getter so the modal stays current (mirrors the desktop's
   * `open_daemon_workspaces_picker` request-then-render pattern).
   */
  getWorkspacesModalPayload?: () => WorkspacesModalPayload | null;
  /** A workspace row was picked in the wasm Workspaces modal. The
   *  host switches the daemon workspace (same handler the legacy
   *  switcher overlay used). */
  onWorkspaceSelected?: (workspaceId: string) => void;
  onWorkspaceIslandIntent?: (intent: {
    kind: "activate" | "context_menu" | "open_workspaces";
    workspace_id?: string | null;
    x?: number | null;
    y?: number | null;
  }) => void;
  /** Alt+W / Ctrl+Shift+W — create a NEW workspace, mirroring
   *  desktop's Ctrl+Shift+W `create_tab` (a fresh top-level
   *  workspace). The host creates it on the connected daemon host
   *  and switches to it once the daemon confirms. */
  onCreateWorkspace?: () => void;
  /** The buffer-tab strip changed. The host publishes the snapshot as
   *  the active workspace's tab list in the daemon tree so other
   *  clients (desktop adopt) can rebuild this workspace — buffers,
   *  terminals, and all. */
  onBufferTabsChanged?: (
    tabs: Array<{
      title: string;
      kind: string;
      path: string | null;
      sessionId: string | null;
      active: boolean;
    }>,
  ) => void;
}

/**
 * Canvas-backed terminal panel.
 *
 * Owns a `<canvas>` element, watches its content rect with a
 * `ResizeObserver`, and forwards keystrokes to the daemon. Two render
 * paths can use the canvas:
 *
 *   1. Normal dev: `ChromeBridge` owns the canvas through Sugarloaf and
 *      draws the terminal plus shared neoism-ui chrome.
 *   2. Explicit diagnostics only: `VITE_NEOISM_ALLOW_TERMINAL_STUB=1`
 *      permits a canvas2d diagnostic surface when wasm/Sugarloaf is not
 *      available. That path is intentionally labeled as non-rendered.
 */
export class TerminalPanel {
  private readonly root: HTMLElement;
  private readonly canvas: HTMLCanvasElement;
  private readonly markdownLayer: HTMLDivElement;
  // Lazily acquired — calling getContext("2d") on the canvas locks it
  // to a 2D context for its lifetime, which makes sugarloaf's WebGL2
  // getContext return null. We must NOT touch this until we know wasm
  // has either failed or chosen the data-only path.
  private ctx: CanvasRenderingContext2D | null = null;
  private wasmInitResolved = false;
  private readonly observer: ResizeObserver;
  // devicePixelRatio watcher: `ResizeObserver` only fires when the CSS
  // rect changes, which covers browser zoom (layout reflows) but NOT a
  // window dragged onto a monitor with a different DPR — the CSS size
  // stays put while the backing density changes. A one-shot
  // `matchMedia('(resolution: <dpr>dppx)')` listener catches that; it
  // re-arms itself on every fire because the query is pinned to the
  // DPR it was created at.
  private dprMediaQuery: MediaQueryList | null = null;
  private readonly dprChangeHandler: () => void;
  private readonly mobileKeyboard: MobileKeyboard;
  // Stub renderer is the synchronous fast-path; if the real wasm bundle
  // loads we swap to a TerminalAdapter that wraps the engine (and, when
  // available, sugarloaf via RenderedTerminal).
  private stubTerminal: WasmTerminalStub;
  private wasmAdapter: TerminalAdapter | null = null;
  // Latest unsolicited git status (request_id 0 daemon pushes). Kept
  // so the values survive until the wasm adapter is ready — the
  // daemon only re-sends them when they change.
  private lastGitBranch: string | null | undefined = undefined;
  private lastGitChanges: { added: number; deleted: number } | null = null;
  private terminalInitError: string | null = null;
  private workspaceClipboardPayload: ClipboardPayload | null = null;
  // Correlation table for outstanding `MaterializeClipboardImage`
  // requests: `request_id -> originating pane id`. The daemon round-trip
  // is asynchronous, so the focused surface at reply time may not be the
  // one that initiated the paste (the user can switch panes, scroll the
  // page, or focus a sibling). When the reply arrives we look the
  // request id up here and dispatch `:edit <path>` against the
  // originating pane instead of `activeSurface()`. Entries are removed
  // as the corresponding reply lands; any orphans get cleaned up on
  // disconnect because the table is per-`TerminalPanel`.
  private pendingClipboardImages = new Map<string, number | null>();
  private nextClipboardRequestId = 1;
  private lastTrailCursorPos: { x: number; y: number } | null = null;
  private cols = 80;
  private rows = 24;
  // User-facing font zoom multiplier folded against on each Ctrl+= /
  // Ctrl+- press. Mirrors the bridge's `active_font_scale`; we keep a
  // local copy so we don't have to round-trip through wasm for every
  // keystroke. Ctrl+0 snaps back to 1.0. Clamped to [0.5, 3.0] — the
  // bridge clamps too, this just keeps `currentFontScale` honest.
  private currentFontScale = 1.0;
  private activeThemeName: string = "pastel_dark";
  private fallbackFontFamily =
    "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', 'Apple Color Emoji', 'Segoe UI Emoji', 'Noto Color Emoji', monospace";
  private activeShaderFilter: string | null = null;
  private rafHandle: number | null = null;
  private readonly keydownHandler: (event: KeyboardEvent) => void;
  private readonly documentKeydownHandler: (event: KeyboardEvent) => void;
  private readonly pointerMoveHandler: (event: PointerEvent) => void;
  private readonly pointerDownHandler: (event: PointerEvent) => void;
  private readonly pointerUpHandler: (event: PointerEvent) => void;
  private readonly pointerLeaveHandler: () => void;
  private readonly wheelHandler: (event: WheelEvent) => void;
  // IME composition tracking. `imeComposing` flips true between
  // `compositionstart` and `compositionend`; the keydown path uses it
  // (combined with `event.isComposing`) to drop keystrokes the IME
  // owns so the pty never sees the candidate-list navigation keys.
  private imeComposing = false;
  private readonly compositionStartHandler: (event: CompositionEvent) => void;
  private readonly compositionUpdateHandler: (event: CompositionEvent) => void;
  private readonly compositionEndHandler: (event: CompositionEvent) => void;
  private readonly pasteHandler: (event: ClipboardEvent) => void;
  private readonly contextMenuHandler: (event: MouseEvent) => void;
  /** Drag-and-drop a file onto the agent pane → composer attachment
   *  (the web analogue of desktop's `DroppedFile` → `attach_path`). */
  private readonly agentDragOverHandler: (event: DragEvent) => void;
  private readonly agentDropHandler: (event: DragEvent) => void;
  // Touch handlers — C3 polish. The classifier (`touchPolicy.ts`)
  // routes every decision through the SHARED RUST state machine
  // (`neoism-frontend/shared/src/touch_policy.rs`, via the wasm
  // `TouchGesturePolicy` export) so tap-vs-drag-vs-pinch-vs-pan and
  // the long-press timer ARE the desktop fork's behaviour, not a TS
  // mirror of it. Side effects are applied by `applyTouchAction`.
  // `touchLongPressTimer` polls `tickLongPress` while a finger is
  // held inside the tap radius.
  private readonly touchStartHandler: (event: TouchEvent) => void;
  private readonly touchMoveHandler: (event: TouchEvent) => void;
  private readonly touchEndHandler: (event: TouchEvent) => void;
  private readonly touchPolicy = new TouchPolicy();
  private touchLongPressTimer: ReturnType<typeof setInterval> | null = null;
  // Sticky decision: once a touch landed in the editor area, eat the
  // browser's swipe-from-edge back/forward gesture for the duration
  // of the gesture by calling `preventDefault()` on every touchmove.
  private touchSuppressSwipeBack = false;
  // Recent agent-timeline drag deltas (trailing ~120ms) so touch
  // release can launch a fling at the finger's velocity.
  private agentTouchScrollSamples: Array<{ t: number; dy: number }> | null =
    null;
  // A touch-down that stopped an in-flight glide must not also count
  // as a click when the finger lifts (iOS stop-scroll semantics).
  private agentTouchSuppressTap = false;
  // Set when an agent UI element (picker row / tool card / link)
  // consumed the current tap — handleTouchEnd must preventDefault so
  // compat mouse events don't steal focus from the soft keyboard.
  private agentTapConsumed = false;
  private mobileBufferTabPan: MobileBufferTabPan | null = null;
  private mobileFileTreePan: MobileFileTreePan | null = null;
  // DOM element for the active file-tree right-click menu (or null when
  // closed). Owned by `TerminalPanel` so we can dismiss + reposition on
  // re-open instead of layering overlays.
  private fileTreeMenuEl: HTMLDivElement | null = null;
  private fileTreeMenuDismiss: (() => void) | null = null;
  private readonly inputBytesHandler: (bytes: Uint8Array) => void;
  private readonly pendingServiceMappers = new Map<
    number,
    (payload: FilesServerMessage | GitServerMessage) => unknown
  >();
  // Wave 7F — multiplayer presence plane. Identity is stable per
  // browser profile (`chrome-<hex>@web`); the publisher coalesces the
  // local cursor to ≤~13Hz with TTL heartbeats and clears presence on
  // buffer switch/close; the store + overlay paint remote carets.
  private readonly presenceIdentity = localPresenceIdentity();
  private readonly crdtPeerId = this.presenceIdentity.peerId;
  private readonly presencePublisher = new PresencePublisher(
    this.presenceIdentity.peerId,
    this.presenceIdentity.displayName,
  );
  private readonly remotePresence = new RemotePresenceStore();
  private readonly markdownPresenceOverlay: MarkdownPresenceOverlay;
  private presenceTimer: ReturnType<typeof setInterval> | null = null;
  private readonly requestedPresenceBuffers = new Set<string>();
  private readonly markdownContentCache = new Map<string, string>();
  private readonly markdownReloadInFlight = new Set<string>();
  private markdownReloadCursor = 0;

  constructor(private readonly options: TerminalPanelOptions) {
    this.root = document.createElement("section");
    this.root.className = "terminal-panel";
    this.root.setAttribute("data-session-id", options.sessionId);
    this.registerTerminalSession(options.sessionId, false);

    this.canvas = document.createElement("canvas");
    this.canvas.className = "terminal-canvas";
    this.canvas.tabIndex = 0;
    this.root.appendChild(this.canvas);
    this.markdownLayer = document.createElement("div");
    this.markdownLayer.className = "web-markdown-layer";
    this.markdownLayer.tabIndex = 0;
    this.markdownLayer.hidden = true;
    this.root.appendChild(this.markdownLayer);
    this.markdownPresenceOverlay = new MarkdownPresenceOverlay(
      this.markdownLayer,
    );
    this.remotePresence.setLocalPeerId(this.crdtPeerId);
    // Coarse presence pump: heartbeats, markdown reading-position
    // publishes (scroll-driven), and client-side TTL pruning.
    this.presenceTimer = setInterval(() => {
      this.pumpPresence();
      // Safety-net CRDT flush: keystrokes pump synchronously from the
      // key handler; this catches mutations from other entry points
      // (paste, checkbox toggles, drags) within a frame or two.
      this.pumpCrdtOutbox();
      this.pumpCodeCrdt();
      this.pollOpenMarkdownTabs();
      if (this.remotePresence.pruneStale(Date.now(), 15_000)) {
        this.syncMarkdownPresenceOverlay();
        this.syncFileTreePresence();
        this.scheduleDraw();
      }
    }, 250);
    // (The old DOM pane-layout overlay + drag preview elements are
    // gone — pane chrome paints on the canvas via the shared PaneGrid.)

    this.stubTerminal = new WasmTerminalStub(this.cols, this.rows);
    // Try to upgrade to the real wasm engine asynchronously. While this
    // promise is pending, we DO NOT touch the canvas at all (no 2D
    // context, no width/height writes) so sugarloaf can later claim it
    // for WebGL2. Only when this resolves and we know the path do we
    // either let sugarloaf own the canvas (rendered) or grab a 2D
    // context ourselves for the stub overlay.
    void createTerminal(
      this.canvas,
      this.cols,
      this.rows,
      this.options.workspaceRoot ?? "",
    ).then((adapter) => {
      this.wasmInitResolved = true;
      this.terminalInitError = null;
      if (adapter.isReal()) {
        this.wasmAdapter = adapter;
        if (adapter.isChrome()) {
          this.installChromeCallbacks(adapter);
          this.ensureSessionLayoutState();
          this.options.client.listEditorSurfaces();
          // The chrome was constructed with the workspaceRoot captured
          // when the panel was built; any `setWorkspaceRoot` that
          // landed while the wasm module was still loading only
          // updated `options.workspaceRoot` (the adapter was null and
          // the optional-chained call no-op'd). Replay the current
          // value so the file tree roots at the corrected path —
          // without this, the tree can keep listing a stale absolute
          // daemon-side path the Files service rejects ("absolute
          // paths are not allowed") and its loading skeleton never
          // resolves. `set_workspace_root` is idempotent for an
          // unchanged root, so this is free in the common case.
          if (this.options.workspaceRoot) {
            adapter.setWorkspaceRoot?.(this.options.workspaceRoot);
          }
          adapter.refreshFileTree?.();
          // Align sugarloaf's clear color, the chrome panels, and the
          // terminal cell palette to one source so the web frontend
          // paints the same dark surface the desktop uses.
          this.setIdeTheme("pastel_dark");
        }
        if (adapter.isRendered()) {
          // sugarloaf path: initialize in CSS pixels with the SAME
          // effective scale `handleResize` derives — never a hardcoded
          // 1, which would rasterize the first frame at low density
          // and leave a blurry flash (or stick permanently if the
          // follow-up resize is a no-op because cols/rows match).
          const cssW = this.root.clientWidth;
          const cssH = this.root.clientHeight;
          adapter.setFontScale?.(this.currentFontScale);
          const scale = sizeContractFor(this.canvas, cssW, cssH).scale;
          adapter.resize(this.cols, this.rows, scale, cssW, cssH);
        }
      }
      // Now that we know which path is live, force a resize so the
      // canvas backing buffer is sized correctly (for the stub) or
      // the wgpu swapchain matches the canvas (for rendered).
      this.handleResize(this.root.clientWidth, this.root.clientHeight);
      this.syncBridgeStateAfterAdapterReady();
      requestAnimationFrame(() => {
        this.handleResize(this.root.clientWidth, this.root.clientHeight);
      });
      this.scheduleDraw();
      // Fire `onBridgeReady` so the host (App.ts) can install
      // protocol-level services that need bridge access (search). The
      // adapter is `SearchBridge`-shaped whether it's the chrome
      // adapter (real wasm) or the data-only adapter / stub — missing
      // optional methods are no-ops thanks to the `?` chain in
      // `SearchService.install()`.
      if (this.wasmAdapter) {
        try {
          this.options.onBridgeReady?.(
            this.wasmAdapter as unknown as ServiceRegistryBridge,
          );
        } catch (err) {
          if (typeof console !== "undefined") {
            console.warn("[neoism] onBridgeReady handler threw", err);
          }
        }
      }
    }).catch((err: unknown) => {
      const message = err instanceof Error ? err.message : String(err);
      this.terminalInitError = message;
      this.wasmInitResolved = true;
      if (typeof console !== "undefined") {
        console.error("[neoism] terminal bridge initialization failed", err);
      }
      this.handleResize(this.root.clientWidth, this.root.clientHeight);
      this.scheduleDraw();
    });

    this.inputBytesHandler = (bytes) => this.handleInputBytes(bytes);
    this.mobileKeyboard = new MobileKeyboard({
      mount: this.root,
      onBytes: this.inputBytesHandler,
      // When the soft keyboard opens, force a re-measure so the cell
      // grid contracts above the keyboard inset. We can't rely on
      // ResizeObserver alone: on iOS Safari the layout viewport keeps
      // its full height when the keyboard pops; only `visualViewport`
      // shrinks. Re-running `handleResize` with the trimmed height
      // reflows chrome panels + the terminal grid so the caret row
      // stays visible.
      onInsetsChanged: (insets) => {
        // Remember the inset: every other handleResize source (the
        // per-frame terminal-rect sync above all) must keep deducting
        // it, or the first relayout after the keyboard opens undoes
        // the push-up.
        this.keyboardInsetBottom = insets.keyboardOpen ? insets.bottom : 0;
        // iOS fires visualViewport resizes continuously while the
        // keyboard animates; reflowing the PTY for every frame of
        // that animation reads as the viewport thrashing. Trail the
        // final value instead.
        if (this.insetResizeTimer !== null) {
          window.clearTimeout(this.insetResizeTimer);
        }
        this.insetResizeTimer = window.setTimeout(() => {
          this.insetResizeTimer = null;
          const widthPx = this.root.clientWidth;
          const heightPx = Math.max(
            1,
            this.root.clientHeight - this.keyboardInsetBottom,
          );
          this.handleResize(widthPx, heightPx);
        }, 140);
      },
      scrollAnchor: this.markdownLayer,
    });

    this.keydownHandler = (event) => this.handleKeyDown(event);
    this.canvas.addEventListener("keydown", this.keydownHandler);
    this.markdownLayer.addEventListener("keydown", this.keydownHandler);
    this.documentKeydownHandler = (event) => {
      if (this.handleChromeShortcut(event) || this.routeKeyToChrome(event)) {
        event.preventDefault();
        event.stopPropagation();
        event.stopImmediatePropagation();
      }
    };
    document.addEventListener("keydown", this.documentKeydownHandler, true);
    this.pointerMoveHandler = (event) => this.handlePointerMove(event);
    this.pointerDownHandler = (event) => this.handlePointerDown(event);
    this.pointerUpHandler = (event) => this.handlePointerUp(event);
    this.pointerLeaveHandler = () => {
      this.hideCustomCursor();
      this.forwardChromeEvent(pointerLeaveEvent());
    };
    this.wheelHandler = (event) => this.handleWheel(event);
    this.pasteHandler = (event) => this.handlePaste(event);
    this.contextMenuHandler = (event) => this.handleContextMenu(event);
    this.agentDragOverHandler = (event) => this.handleAgentDragOver(event);
    this.agentDropHandler = (event) => {
      void this.handleAgentFileDrop(event);
    };
    this.touchStartHandler = (event) => this.handleTouchStart(event);
    this.touchMoveHandler = (event) => this.handleTouchMove(event);
    this.touchEndHandler = (event) => this.handleTouchEnd(event);
    this.compositionStartHandler = (event) => this.handleCompositionStart(event);
    this.compositionUpdateHandler = (event) =>
      this.handleCompositionUpdate(event);
    this.compositionEndHandler = (event) => this.handleCompositionEnd(event);
    this.canvas.addEventListener("pointermove", this.pointerMoveHandler);
    this.canvas.addEventListener("pointerdown", this.pointerDownHandler);
    this.canvas.addEventListener("pointerup", this.pointerUpHandler);
    this.canvas.addEventListener("pointerleave", this.pointerLeaveHandler);
    this.canvas.addEventListener("wheel", this.wheelHandler, { passive: false });
    this.canvas.addEventListener("paste", this.pasteHandler);
    this.canvas.addEventListener("contextmenu", this.contextMenuHandler);
    this.canvas.addEventListener("dragover", this.agentDragOverHandler);
    this.canvas.addEventListener("drop", this.agentDropHandler);
    this.markdownLayer.addEventListener("pointermove", this.pointerMoveHandler);
    this.markdownLayer.addEventListener("pointerdown", this.pointerDownHandler);
    this.markdownLayer.addEventListener("pointerup", this.pointerUpHandler);
    this.markdownLayer.addEventListener("pointerleave", this.pointerLeaveHandler);
    this.markdownLayer.addEventListener("wheel", this.wheelHandler, { passive: false });
    this.markdownLayer.addEventListener("paste", this.pasteHandler);
    this.markdownLayer.addEventListener("contextmenu", this.contextMenuHandler);
    // Touch listeners: passive:false on touchstart/touchmove so the
    // shared policy can decide to `preventDefault()` (pinch zoom,
    // swipe-back) before the browser commits to its default action.
    this.canvas.addEventListener("touchstart", this.touchStartHandler, {
      passive: false,
    });
    this.canvas.addEventListener("touchmove", this.touchMoveHandler, {
      passive: false,
    });
    this.canvas.addEventListener("touchend", this.touchEndHandler);
    this.canvas.addEventListener("touchcancel", this.touchEndHandler);
    this.markdownLayer.addEventListener("touchstart", this.touchStartHandler, {
      passive: false,
    });
    this.markdownLayer.addEventListener("touchmove", this.touchMoveHandler, {
      passive: false,
    });
    this.markdownLayer.addEventListener("touchend", this.touchEndHandler);
    this.markdownLayer.addEventListener("touchcancel", this.touchEndHandler);
    // IME composition: forward the browser composition lifecycle to
    // chrome (`Composition::{Start, Update, Commit, End}`) so the
    // shared decision table sees the same events the desktop fork
    // gets from winit. Without these, Japanese / Chinese input never
    // reaches the pty — the keydown path only fires for
    // single-byte keys.
    this.canvas.addEventListener("compositionstart", this.compositionStartHandler);
    this.canvas.addEventListener("compositionupdate", this.compositionUpdateHandler);
    this.canvas.addEventListener("compositionend", this.compositionEndHandler);
    this.markdownLayer.addEventListener("compositionstart", this.compositionStartHandler);
    this.markdownLayer.addEventListener("compositionupdate", this.compositionUpdateHandler);
    this.markdownLayer.addEventListener("compositionend", this.compositionEndHandler);

    this.observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const rect = entry.contentRect;
        this.handleResize(rect.width, rect.height);
      }
    });
    this.dprChangeHandler = () => {
      // DPR changed without a CSS-rect change (monitor swap / OS scale
      // flip). Re-run the full size contract, then re-arm the watcher
      // against the NEW ratio.
      this.handleResize(this.root.clientWidth, this.root.clientHeight);
      this.watchDevicePixelRatio();
    };
    this.watchDevicePixelRatio();

    options.mount.appendChild(this.root);
    this.observer.observe(this.root);
    this.handleResize(this.root.clientWidth, this.root.clientHeight);
    this.scheduleDraw();
    queueMicrotask(() => this.focus());
  }

  /** Feed bytes from a `PtyOutput` frame into the renderer. */
  ingest(bytes: Uint8Array): void {
    this.ingestPty(this.options.sessionId, bytes);
  }

  /** Feed bytes from one daemon PTY session into the owning web tab. */
  ingestPty(sessionId: string, bytes: Uint8Array): void {
    // Buffer EVERY session's stream (bounded per session by
    // MAX_REPLAY_BYTES_PER_PTY) — not just sessions we already have a
    // tab for. The daemon replays each live session's backlog right
    // after connect, before any workspace attach binds a tab to it;
    // dropping unknown sessions here threw that backlog away, so
    // adopting a desktop workspace from the modal opened a black
    // terminal until the remote shell next printed something.
    this.rememberPtyBytes(sessionId, bytes);
    if (!this.knowsPtySession(sessionId)) {
      return;
    }
    // Live multi-pane rendering: while the grid is split, every
    // terminal pane owns a per-pane wasm terminal — route this
    // session's bytes into it (the focused pane's session ALSO feeds
    // the main grid below so an un-split is instant).
    this.feedPaneTerminalBytes(sessionId, bytes);
    if (sessionId !== this.activePtySessionId()) {
      return;
    }
    // The daemon replays each session's backlog right after attach.
    // Like tab-switch replays, that backlog can contain capability
    // queries from programs that already exited — answering them now
    // just echoes garbage into the prompt. Suppress + drop the PTY
    // writes for the attach burst; live queries after it flow.
    const attachedAt = this.ptyAttachedAt.get(sessionId);
    const inAttachBurst =
      attachedAt !== undefined && performance.now() - attachedAt < 1500;
    this.feedVisiblePtyBytes(bytes, !inAttachBurst);
    if (inAttachBurst) {
      this.wasmAdapter?.takePtyWrites();
    }
  }

  /** Register a newly-created daemon PTY as a real terminal tab. */
  ptyCreated(sessionId: string): void {
    const pending = this.pendingTerminalTabSpawns.shift();
    const hasTerminal = this.bufferTabs.some(
      (tab) => tab.kind === "terminal" && !!tab.sessionId,
    );
    if (!pending && hasTerminal) {
      // PTY registry is daemon-global, so a second web/mobile client
      // broadcasts its PTY creation to this page too. Do not attach or
      // replay foreign shells here; otherwise one browser can fill this
      // panel's replay buffers with another browser's output and make
      // touch/click handling feel frozen while replay churns.
      return;
    }
    this.registerTerminalSession(sessionId, pending !== undefined, pending?.title);
    if (pending?.paneExternalId != null) {
      // Terminal split: bind the fresh shell's session to the pane
      // that asked for it so its bytes render into that pane.
      this.paneSessionIds.set(pending.paneExternalId, sessionId);
      this.syncPaneTerminals();
    }
    if (pending?.command) {
      this.options.pty?.sendInput(
        sessionId,
        new TextEncoder().encode(`${pending.command}\n`),
      );
    }
    this.replayBufferTabs();
    this.activatePtySession(sessionId);
  }

  /**
   * Replace a dead PTY binding. The persisted host workspace tree
   * survives daemon restarts but live PTYs do not, so an attach can
   * bind a tab to a session id the daemon answers with
   * `unknown session`. Drop that tab and spawn a fresh shell through
   * the pending-spawn queue so `ptyCreated` attaches the replacement
   * even when other (possibly equally stale) terminal tabs remain.
   * Returns false when no tab is bound to `sessionId`.
   */
  respawnDeadPtySession(sessionId: string): boolean {
    const index = this.bufferTabs.findIndex(
      (tab) => tab.kind === "terminal" && tab.sessionId === sessionId,
    );
    if (index < 0) {
      return false;
    }
    const title = this.bufferTabs[index]?.title;
    this.ptyClosed(sessionId);
    this.spawnTerminalTab(title ? { title } : {});
    return true;
  }

  applyWorkspaceLayoutSnapshot(layoutSnapshot: string | null | undefined): void {
    if (!layoutSnapshot) return;
    // The daemon broadcasts the authoritative pane tree as a
    // `PaneLayoutSnapshot` JSON blob (`{schema_version, root: {kind,
    // axis, ratios, children}}`). That is a different serde shape than
    // the local `SessionLayout` the policy path stores, so we lower it
    // through the shared `SessionLayout::from_pane_layout_snapshot`
    // mirror in the wasm bridge instead of feeding it to
    // `applySessionLayoutPolicy`. This makes the web render the exact
    // split intent — orientation, ratios, nesting, focus — the desktop
    // mirrors, rather than its own divergent local layout.
    const hydrated = this.mirrorPaneLayoutSnapshot(layoutSnapshot);
    if (!hydrated) {
      this.renderPaneLayoutOverlay();
    }
    this.syncBridgeStateAfterAdapterReady();
    this.scheduleDraw();
  }

  private mirrorPaneLayoutSnapshot(snapshotJson: string): boolean {
    const adapter = this.wasmAdapter;
    if (!adapter?.mirrorPaneLayoutSnapshot) return false;
    let result: WebSessionLayoutPolicyResult | null = null;
    try {
      result = parseSessionLayoutPolicyResult(
        adapter.mirrorPaneLayoutSnapshot(snapshotJson),
      );
    } catch {
      // Malformed/legacy snapshot blob — keep the current panes.
      return false;
    }
    if (!result) return false;
    this.sessionLayoutStateJson = result.state_json;
    this.paneLayoutPanes = result.panes;
    this.syncPaneRouteState(result.panes);
    this.assignActiveTabToFocusedEditorPane();
    this.nextWebPaneId = Math.max(
      this.nextWebPaneId,
      2,
      ...result.active_external_ids.map((id) => id + 1),
    );
    this.syncPaneTerminals();
    this.renderPaneLayoutOverlay();
    return true;
  }

  /** This screen's current strip, in the per-workspace-memory shape.
   *  The host saves it before leaving a workspace and restores it on
   *  return — each DEVICE remembers its own view of each workspace. */
  captureStripSnapshot(): Array<{
    title: string;
    kind: string;
    path: string | null;
    sessionId: string | null;
    active: boolean;
  }> {
    let terminalOrdinal = 0;
    return this.bufferTabs.map((tab, index) => ({
      title: this.stableTabTitle(
        tab,
        tab.kind === "terminal" ? ++terminalOrdinal : undefined,
      ),
      kind: tab.kind,
      path: tab.path ?? null,
      sessionId: tab.sessionId ?? null,
      active: index === this.activeTabIndex,
    }));
  }

  /** Restore a previously captured strip (returning to a workspace on
   *  THIS device). Terminal tabs re-attach to their sessions (replay
   *  buffers are global; dead sessions respawn via the stale-session
   *  recovery path); file tabs re-open by path. */
  restoreStripSnapshot(
    snapshot: Array<{
      title: string;
      kind: string;
      path: string | null;
      sessionId: string | null;
      active: boolean;
    }>,
    liveSessionIds?: Set<string>,
  ): void {
    this.bufferTabs = [];
    this.activeTabIndex = 0;
    let activeSession: string | null = null;
    let activePath: string | null = null;
    let firstTerminalSession: string | null = null;
    let droppedTerminal = false;
    let hadActiveTerminal = false;
    for (const tab of snapshot) {
      if (tab.kind === "terminal" && tab.sessionId) {
        // A remembered session can outlive its PTY (daemon restart):
        // attaching it produced a "Terminal" tab with no terminal in
        // it. When the caller knows the live session set, dead
        // sessions are dropped and replaced with a fresh shell below.
        if (liveSessionIds && !liveSessionIds.has(tab.sessionId)) {
          droppedTerminal = true;
          if (tab.active) hadActiveTerminal = true;
          continue;
        }
        this.attachTerminalTabInPlace(tab.sessionId, tab.title);
        firstTerminalSession ??= tab.sessionId;
        if (tab.active) activeSession = tab.sessionId;
      } else if (tab.kind === "file" && tab.path) {
        this.ensureFileTabForEditorSurface(tab.path, tab.title);
        if (tab.active) activePath = tab.path;
      }
    }
    const hasTerminalTab = this.bufferTabs.some(
      (tab) => tab.kind === "terminal",
    );
    if (this.bufferTabs.length === 0 || (droppedTerminal && !hasTerminalTab)) {
      // Either nothing survived, or every terminal in the memory was
      // dead — a workspace view always offers a live shell.
      this.spawnTerminalTab({});
      if (hadActiveTerminal) {
        // The dead session was the ACTIVE tab — keep focus on the
        // fresh shell rather than jumping to a file tab.
        activeSession = null;
        activePath = null;
      }
    }
    if (firstTerminalSession) {
      this.activatePtySession(firstTerminalSession);
    } else if (activeSession) {
      this.activatePtySession(activeSession);
    } else if (activePath) {
      this.activateFileTab(activePath);
    } else {
      this.replayBufferTabs();
    }
    this.scheduleDraw();
  }

  /**
   * Entering a workspace REPLACES the strip with that workspace's
   * tabs — a screen is "in" exactly one workspace, and tabs from the
   * previous one don't bleed over. Per-session replay buffers are
   * kept (they're keyed globally), so switching back re-attaches with
   * scrollback intact.
   */
  resetToWorkspaceTabs(
    tabs: WorkspaceTabSummary[],
    activeTabId: string | null | undefined,
    options: { terminalOnly?: boolean } = {},
  ): void {
    this.bufferTabs = [];
    this.activeTabIndex = 0;
    this.applyWorkspaceTabs(tabs, activeTabId, options);
    if (this.bufferTabs.length === 0) {
      // Empty workspace: land in a fresh shell rather than a dead
      // strip (mirrors the desktop adopt-empty behavior).
      this.spawnTerminalTab({});
      this.replayBufferTabs();
      this.scheduleDraw();
    }
  }

  setCommandPaletteWorkspaceVisibility(visibility: string): void {
    this.wasmAdapter?.setCommandPaletteWorkspaceVisibility?.(visibility);
  }

  applyWorkspaceTabs(
    tabs: WorkspaceTabSummary[],
    activeTabId: string | null | undefined,
    options: { terminalOnly?: boolean } = {},
  ): void {
    this.bufferTabs = [];
    this.activeTabIndex = 0;
    let activeSessionId: string | null = null;
    let activePath: string | null = null;
    let firstTerminalSession: string | null = null;
    const orderedTabs = this.workspaceTabsInDesktopOrder(tabs);
    for (const tab of orderedTabs) {
      if (tab.session_id && (tab.kind ?? "terminal") === "terminal") {
        this.attachTerminalTabInPlace(tab.session_id, tab.title || "Terminal 1");
        if (!firstTerminalSession) firstTerminalSession = tab.session_id;
      }
      if (!options.terminalOnly && this.isWorkspaceFileLikeTab(tab) && tab.cwd) {
        this.ensureFileTabForEditorSurface(tab.cwd, tab.title);
      }
      if (activeTabId && tab.id === activeTabId) {
        activeSessionId = tab.session_id ?? null;
        activePath = tab.cwd ?? null;
      }
    }
    // Land on a terminal when entering a workspace. If the recorded
    // active tab is itself a terminal, restore it; otherwise fall back to
    // the workspace's main shell rather than a recorded file tab. A file
    // tab as the entry surface used to cover the terminal (you had to
    // close it to reach the shell) AND triggered a file read that fails
    // with "absolute paths not allowed" when its path isn't under the
    // freshly-rooted workspace. Only open a file tab when there is no
    // terminal at all.
    if (firstTerminalSession) {
      this.activatePtySession(firstTerminalSession);
    } else if (activeSessionId) {
      this.activatePtySession(activeSessionId);
    } else if (activePath) {
      this.activateFileTab(activePath);
    } else {
      this.replayBufferTabs();
      this.scheduleDraw();
    }
  }

  private workspaceTabsInDesktopOrder(tabs: WorkspaceTabSummary[]): WorkspaceTabSummary[] {
    return [...tabs];
  }

  private isWorkspaceFileLikeTab(tab: WorkspaceTabSummary): boolean {
    return tab.kind === "editor" || tab.kind === "markdown" || tab.kind === "drawing" || !!tab.surface_id;
  }

  /**
   * Mark a daemon PTY closed. Returns true when at least one shell tab
   * remains alive, false when the whole panel should tear down.
   */
  ptyClosed(sessionId: string): boolean {
    const closingIndex = this.bufferTabs.findIndex(
      (tab) => tab.kind === "terminal" && tab.sessionId === sessionId,
    );
    this.ptyReplayBuffers.delete(sessionId);
    if (closingIndex >= 0) {
      this.bufferTabs.splice(closingIndex, 1);
      if (this.bufferTabs.length === 0) {
        return false;
      }
      if (this.activeTabIndex >= this.bufferTabs.length) {
        this.activeTabIndex = this.bufferTabs.length - 1;
      } else if (closingIndex < this.activeTabIndex) {
        this.activeTabIndex = Math.max(0, this.activeTabIndex - 1);
      }
      this.replayBufferTabs();
      const activeSession = this.activePtySessionId();
      if (activeSession) {
        this.activateCurrentTabContents(false);
      }
      this.scheduleDraw();
    }
    return this.bufferTabs.some((tab) => tab.kind === "terminal" && tab.sessionId);
  }

  private feedVisiblePtyBytes(bytes: Uint8Array, flushPtyWrites = true): void {
    // The stub keeps tracking byte counts / cursor for diagnostics
    // until sugarloaf is live; cheap and useful during dev.
    if (!this.wasmAdapter || !this.wasmAdapter.isRendered()) {
      this.stubTerminal.feed(bytes);
    }
    if (this.wasmAdapter) {
      this.wasmAdapter.feed(bytes);
      if (this.wasmAdapter.isRendered()) {
        // Paint the new state straight away — sugarloaf clears the
        // swapchain itself, no compositing with the stub.
        this.wasmAdapter.render();
      }
      // Forward any PTY response bytes (DSR, cursor pos, OSC 52) back
      // to the daemon.
      if (flushPtyWrites) {
        const ptyOut = this.wasmAdapter.takePtyWrites();
        if (ptyOut.length > 0) {
          this.sendPtyInput(ptyOut);
        }
      }
    }
    this.scheduleDraw();
  }

  focus(): void {
    this.focusSurface();
  }

  private focusSurface(): void {
    if (this.activeTabIsMarkdown()) {
      this.markdownLayer.focus({ preventScroll: true });
      return;
    }
    this.canvas.focus({ preventScroll: true });
  }

  private requestSoftKeyboard(): void {
    if (this.isMobileViewport()) {
      // Buffer-editing surfaces insert newlines — the iOS return key
      // should read "return", not "send".
      const surface = this.activeSurface();
      this.mobileKeyboard.setContext(
        surface === "editor" || surface === "markdown" ? "editor" : "code",
      );
      this.mobileKeyboard.focus();
    } else {
      this.focusSurface();
    }
  }

  private dismissSoftKeyboard(): void {
    if (this.isMobileViewport()) {
      this.mobileKeyboard.blur();
    }
  }

  dispose(): void {
    // Leave the presence plane cleanly: a final `tick(null)` emits the
    // ClearPresence for whatever buffer we were last in.
    if (this.presenceTimer !== null) {
      clearInterval(this.presenceTimer);
      this.presenceTimer = null;
    }
    for (const message of this.presencePublisher.tick(null, Date.now())) {
      this.options.client.sendCrdt(message);
    }
    this.observer.disconnect();
    this.dprMediaQuery?.removeEventListener("change", this.dprChangeHandler);
    this.dprMediaQuery = null;
    this.canvas.removeEventListener("keydown", this.keydownHandler);
    this.markdownLayer.removeEventListener("keydown", this.keydownHandler);
    document.removeEventListener("keydown", this.documentKeydownHandler, true);
    this.canvas.removeEventListener("pointermove", this.pointerMoveHandler);
    this.canvas.removeEventListener("pointerdown", this.pointerDownHandler);
    this.canvas.removeEventListener("pointerup", this.pointerUpHandler);
    this.canvas.removeEventListener("pointerleave", this.pointerLeaveHandler);
    this.canvas.removeEventListener("wheel", this.wheelHandler);
    this.canvas.removeEventListener("paste", this.pasteHandler);
    this.canvas.removeEventListener("contextmenu", this.contextMenuHandler);
    this.canvas.removeEventListener("dragover", this.agentDragOverHandler);
    this.canvas.removeEventListener("drop", this.agentDropHandler);
    this.markdownLayer.removeEventListener("pointermove", this.pointerMoveHandler);
    this.markdownLayer.removeEventListener("pointerdown", this.pointerDownHandler);
    this.markdownLayer.removeEventListener("pointerup", this.pointerUpHandler);
    this.markdownLayer.removeEventListener("pointerleave", this.pointerLeaveHandler);
    this.markdownLayer.removeEventListener("wheel", this.wheelHandler);
    this.markdownLayer.removeEventListener("paste", this.pasteHandler);
    this.markdownLayer.removeEventListener("contextmenu", this.contextMenuHandler);
    this.canvas.removeEventListener("touchstart", this.touchStartHandler);
    this.canvas.removeEventListener("touchmove", this.touchMoveHandler);
    this.canvas.removeEventListener("touchend", this.touchEndHandler);
    this.canvas.removeEventListener("touchcancel", this.touchEndHandler);
    this.markdownLayer.removeEventListener("touchstart", this.touchStartHandler);
    this.markdownLayer.removeEventListener("touchmove", this.touchMoveHandler);
    this.markdownLayer.removeEventListener("touchend", this.touchEndHandler);
    this.markdownLayer.removeEventListener("touchcancel", this.touchEndHandler);
    this.stopTouchLongPressTimer();
    this.canvas.removeEventListener(
      "compositionstart",
      this.compositionStartHandler,
    );
    this.markdownLayer.removeEventListener(
      "compositionstart",
      this.compositionStartHandler,
    );
    this.canvas.removeEventListener(
      "compositionupdate",
      this.compositionUpdateHandler,
    );
    this.markdownLayer.removeEventListener(
      "compositionupdate",
      this.compositionUpdateHandler,
    );
    this.canvas.removeEventListener(
      "compositionend",
      this.compositionEndHandler,
    );
    this.markdownLayer.removeEventListener(
      "compositionend",
      this.compositionEndHandler,
    );
    this.dismissFileTreeMenu();
    this.mobileKeyboard.dispose();
    if (this.rafHandle !== null) {
      cancelAnimationFrame(this.rafHandle);
      this.rafHandle = null;
    }
    this.root.remove();
  }

  // ---------------------------------------------------------------

  /// Sink for daemon `EditorReply` envelopes. The embedded-nvim grid
  /// path is gone; what remains on this channel is the native code
  /// pane's LSP plane — diagnostics pushes, status snapshots, and
  /// seq-tokened hover/completion/query results — routed into the
  /// wasm session layer (`editor_lsp_reply`).
  editorReply(payload: unknown): void {
    const adapter = this.wasmAdapter as {
      editorLspReply?: (json: string) => boolean;
    };
    if (!adapter?.editorLspReply) return;
    let changed = false;
    try {
      changed = adapter.editorLspReply(JSON.stringify(payload));
    } catch (err) {
      console.warn("[lsp] editor reply routing failed", err);
    }
    this.processEditorLspHostActions();
    if (changed) this.scheduleDraw();
  }

  /// Drain + execute host actions queued by the wasm LSP session
  /// (cross-file definition jumps, the rename prompt, finishing a
  /// deferred format-on-save).
  private processEditorLspHostActions(): void {
    const adapter = this.wasmAdapter as {
      editorLspHostActions?: () => string | null;
      editorLspRenameSubmit?: (name: string) => void;
    };
    const raw = adapter?.editorLspHostActions?.();
    if (!raw) return;
    let actions: Array<Record<string, unknown>> = [];
    try {
      actions = JSON.parse(raw) as Array<Record<string, unknown>>;
    } catch {
      return;
    }
    for (const action of actions) {
      switch (action.kind) {
        case "open": {
          const path = typeof action.path === "string" ? action.path : null;
          if (path) this.openActivatedPaths([path]);
          break;
        }
        case "rename_prompt": {
          const word = typeof action.word === "string" ? action.word : "";
          // Desktop's rename modal (`open_code_rename_prompt`): the
          // `code_rename_to` form pre-filled with the symbol, Enter
          // submits, Esc cancels. The confirmed name drains as
          // `{kind:"lsp_rename"}` and lands in
          // `handleModalHostActions` → `editorLspRenameSubmit`.
          const modalAdapter = this.wasmAdapter;
          if (
            this.modalChannelAvailable() &&
            typeof modalAdapter?.openLspRenameModal === "function"
          ) {
            modalAdapter.openLspRenameModal(word);
            this.focus();
            this.scheduleDraw();
            this.pumpModalOutcomes();
            break;
          }
          // Fallback for bundles predating the modal exports.
          const name = window.prompt(`Rename \`${word}\` to:`, word);
          if (name && name.trim().length > 0) {
            adapter.editorLspRenameSubmit?.(name.trim());
          }
          break;
        }
        case "save_after_format": {
          // Format edits landed (or formatting failed) — complete the
          // save WITHOUT re-queueing the formatter.
          this.saveActiveEditorPane(true);
          break;
        }
        default:
          break;
      }
    }
    this.scheduleDraw();
  }

  private crdtInboundLogAt = new Map<string, number>();
  crdtReply(payload: CrdtServerMessage): void {
    // Wave 8D: document plane first — snapshots seed the wasm pane's
    // doc binding, syncs splice remote keystrokes into the visible
    // text (echo-guarded in wasm), `Saved` clears the doc dirty bit.
    const payloadJson = JSON.stringify(payload);
    let textChanged = this.wasmAdapter?.crdtApply?.(payloadJson) === true;
    // Code-pane document plane: same message stream, code binding.
    const editorAdapter = this.wasmAdapter as {
      editorCrdtApply?: (json: string) => boolean;
    };
    if (editorAdapter?.editorCrdtApply?.(payloadJson) === true) {
      textChanged = true;
    }
    this.pumpCodeCrdt();
    // Visible diagnostics (info level — debug is hidden by default):
    // one line per second per buffer saying what arrived and whether
    // the pane spliced it. This is the desktop→web display question
    // answered in the user's own console.
    if ("Sync" in payload) {
      const envelope = payload.Sync.envelope;
      const now = Date.now();
      const last = this.crdtInboundLogAt.get(envelope.buffer_id) ?? 0;
      if (now - last > 1000) {
        this.crdtInboundLogAt.set(envelope.buffer_id, now);
        console.info(
          `[crdt] in Sync buf=${envelope.buffer_id.split("/").pop()} origin=${envelope.origin_client_id} spliced=${textChanged}`,
        );
      }
    } else if ("Snapshot" in payload || "SnapshotFallback" in payload) {
      console.info(`[crdt] in Snapshot seeded=${textChanged}`);
    }
    // The apply may queue follow-ups (flushed pending edits, drift
    // recovery snapshot requests) — ship them now.
    this.pumpCrdtOutbox();
    const changed = this.remotePresence.applyServerMessage(payload);
    if (changed) {
      this.syncMarkdownPresenceOverlay();
      this.syncFileTreePresence();
    }
    if (textChanged) {
      this.pumpMarkdownAnimation();
    }
    this.scheduleDraw();
  }

  agentReply(payload: AgentServerMessage): void {
    try {
      // The wasm bridge's `agent_event` handler mirrors `Notice`
      // events into the chrome's global toast stack
      // (`mirror_agent_event_to_bridge` -> `chrome.notifications`),
      // so we don't double-push from here. Plain forward and let the
      // bridge fan it out.
      this.wasmAdapter?.agentEvent?.(JSON.stringify(payload));
      this.scheduleDraw();
    } catch (err) {
      console.warn("[agent] failed to forward agent frame", err);
    }
  }

  /// Forward a daemon-pushed `DiagnosticsServerMessage` to the bridge.
  /// The bridge translates each variant into the matching
  /// `Chrome::set_diagnostics(...)` / `status_line` mutation.
  diagnosticsReply(payload: DiagnosticsServerMessage): void {
    try {
      this.wasmAdapter?.diagnosticsEvent?.(JSON.stringify(payload));
      this.scheduleDraw();
    } catch (err) {
      console.warn("[diagnostics] failed to forward frame", err);
    }
  }

  /// Forward a daemon-pushed `WorkspaceServerMessage` to the bridge.
  /// The bridge updates its workspace registry and refreshes any
  /// workspace-bound panels.
  workspaceReply(payload: WorkspaceServerMessage): void {
    try {
      if ("ClipboardPayload" in payload) {
        this.ingestWorkspaceClipboardPayload(payload.ClipboardPayload.payload);
      } else if ("ClipboardImageMaterialized" in payload) {
        this.ingestClipboardImageMaterialized(
          payload.ClipboardImageMaterialized,
        );
      } else if ("WorkspaceActionCompleted" in payload) {
        this.ingestWorkspaceActionCompleted(payload.WorkspaceActionCompleted);
      } else if ("EditorSurfaceList" in payload) {
        this.ingestEditorSurfaceList(payload.EditorSurfaceList.surfaces);
      } else if ("EditorSurfaceChanged" in payload) {
        this.ingestEditorSurfaceChanged(payload.EditorSurfaceChanged.surface);
      } else if ("EditorSurfaceClosed" in payload) {
        this.ingestEditorSurfaceClosed(payload.EditorSurfaceClosed.surface_id);
      } else if ("SessionList" in payload) {
        this.ingestWorkspaceSessionList(payload.SessionList.sessions);
      } else if ("SessionCreated" in payload) {
        // Daemon-driven session creation (e.g., neoism-agent on a
        // paired phone tells the laptop "open a session"). Mirror it
        // into the chrome so the user sees the new tab without having
        // to re-list manually.
        this.ingestRemoteSessionCreated(payload.SessionCreated.session.id);
      } else if ("SessionClosed" in payload) {
        this.ingestRemoteSessionClosed(payload.SessionClosed.session_id);
      } else if ("SessionChanged" in payload) {
        // Daemon picked a different active session (likely via
        // `SwitchSession` from another client). Activate the matching
        // local tab if we have one.
        const id = payload.SessionChanged.session_id;
        if (typeof id === "string") {
          this.workspaceSessionId = id;
          this.ingestRemoteSessionFocus(id);
        }
      } else if ("PaneLayoutChanged" in payload) {
        // The daemon broadcasts the authoritative pane tree whenever a
        // `PaneLayoutOp` lands (here or on a paired surface). Mirror the
        // snapshot so this client converges on the exact split intent —
        // orientation, ratios, nesting, focus — the desktop renders.
        const snapshot = payload.PaneLayoutChanged.new_layout_snapshot;
        if (snapshot) {
          this.applyWorkspaceLayoutSnapshot(snapshot);
        }
      }
      this.wasmAdapter?.workspaceEvent?.(JSON.stringify(payload));
      this.scheduleDraw();
    } catch (err) {
      console.warn("[workspace] failed to forward frame", err);
    }
  }

  /// React to a daemon-pushed `SessionCreated` for a session that
  /// originated elsewhere (typically neoism-agent on a paired device).
  /// The local UI doesn't own a PTY for the new session yet — those
  /// frames will flow through `PtyService.ingestCreated` once the
  /// daemon emits `PtyCreated` — so this hook only refreshes the
  /// editor-surface registry so any newly bound panes appear.
  private ingestRemoteSessionCreated(_sessionId: string): void {
    this.workspaceSessionId = _sessionId;
    this.options.client.listEditorSurfaces();
  }

  /// React to a daemon-pushed `SessionClosed`: drop any tabs/surfaces
  /// referencing it. The matching `PtyClosed` arrives separately for
  /// the terminal half; this only handles the workspace-level cleanup.
  private ingestRemoteSessionClosed(sessionId: string): void {
    if (this.workspaceSessionId === sessionId) {
      this.workspaceSessionId = null;
    }
    for (const [surfaceId, surface] of this.editorSurfaceBindings) {
      if (surface.session_id === sessionId) {
        this.ingestEditorSurfaceClosed(surfaceId);
      }
    }
  }

  /// React to a daemon-pushed `SessionChanged`. If a local pane is
  /// already bound to the new session, focus it.
  private ingestRemoteSessionFocus(sessionId: string): void {
    this.workspaceSessionId = sessionId;
    for (const [surfaceId, surface] of this.editorSurfaceBindings) {
      if (surface.session_id !== sessionId) continue;
      const externalId = this.externalIdFromEditorSurface(surfaceId);
      if (externalId !== null) {
        this.activatePaneExternalId(externalId, true);
        return;
      }
    }
  }

  private ingestWorkspaceSessionList(
    sessions: Array<{ id: string; last_active?: number }>,
  ): void {
    if (
      this.workspaceSessionId &&
      sessions.some((session) => session.id === this.workspaceSessionId)
    ) {
      return;
    }
    const newest = [...sessions].sort(
      (a, b) => (b.last_active ?? 0) - (a.last_active ?? 0),
    )[0];
    this.workspaceSessionId = newest?.id ?? null;
  }

  /// Forward a daemon-pushed `CursorOverlayServerMessage` to the
  /// bridge. The daemon ships cell-grid coordinates because it has no
  /// notion of physical-pixel cell metrics (those depend on the
  /// client's font + DPR); we translate here via the bridge's
  /// `cellMetrics()` accessor before invoking the matching setter.
  cursorOverlayReply(payload: CursorOverlayServerMessage): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    try {
      if ("TrailCursor" in payload) {
        if (this.activeSurface() === "editor") {
          this.lastTrailCursorPos = null;
          return;
        }
        const { col, row, shape, no_jump, reset, snap } = payload.TrailCursor;
        const terminal = adapter.chromeLayout?.()?.terminal;
        const { cols, rows } = this.activeEditorGridSize();
        const cellW = terminal
          ? Math.max(1, terminal.w / cols)
          : (adapter.cellMetrics?.()[0] ?? CELL_WIDTH);
        const cellH = terminal
          ? Math.max(1, terminal.h / rows)
          : (adapter.cellMetrics?.()[1] ?? CELL_HEIGHT);
        const x = (terminal?.x ?? 0) + col * cellW;
        const y = (terminal?.y ?? 0) + row * cellH;
        const last = this.lastTrailCursorPos;
        const jumpCells = last
          ? Math.hypot((x - last.x) / cellW, (y - last.y) / cellH)
          : Infinity;
        this.lastTrailCursorPos = { x, y };
        const shapeLower = shape ? shape.toLowerCase() : "block";
        adapter.setTrailCursor?.(
          JSON.stringify({
            x,
            y,
            cell_w: cellW,
            cell_h: cellH,
            shape: shapeLower,
            no_jump: !!no_jump,
            reset: !!reset,
            snap: !!snap || jumpCells > 12,
          }),
        );
      } else if ("CustomCursor" in payload) {
        this.hideCustomCursor();
      } else if ("CursorlineOverlay" in payload) {
        if (this.activeSurface() === "editor") {
          return;
        }
        const { rich_text_id, target_row, snap, forget } =
          payload.CursorlineOverlay;
        const terminal = adapter.chromeLayout?.()?.terminal;
        const { rows } = this.activeEditorGridSize();
        const cellH = terminal
          ? Math.max(1, terminal.h / rows)
          : (adapter.cellMetrics?.()[1] ?? CELL_HEIGHT);
        adapter.setCursorlineOverlay?.(
          JSON.stringify({
            rich_text_id,
            target_y: (terminal?.y ?? 0) + target_row * cellH,
            snap: !!snap,
            forget: !!forget,
          }),
        );
      } else if ("YankFlash" in payload) {
        const { regions } = payload.YankFlash;
        adapter.setYankFlash?.(JSON.stringify({ regions }));
      }
      this.scheduleDraw();
    } catch (err) {
      console.warn("[cursor-overlay] failed to forward frame", err);
    }
  }

  serviceReply(
    requestId: number,
    payload: FilesServerMessage | GitServerMessage,
  ): void {
    // Unsolicited daemon push: the daemon tags poll-loop status frames
    // with the reserved `request_id = 0` so the status line stays live
    // with the workspace. Route them straight into the chrome and skip
    // the wasm service-reply path.
    if (requestId === 0 && payload && typeof payload === "object") {
      // The daemon only re-pushes these on CHANGE, so a frame that
      // lands before the wasm adapter is up would otherwise be lost
      // forever (the bar then shows the wasm first-paint seed, e.g.
      // branch "main", with no +/- counts). Remember the latest values
      // and replay them in `syncBridgeStateAfterAdapterReady`.
      if ("Branch" in payload) {
        const name = (payload as { Branch: { name: string | null } }).Branch.name;
        this.lastGitBranch = name;
        this.wasmAdapter?.setStatusBranch?.(name);
        this.scheduleDraw();
        return;
      }
      if ("Changes" in payload) {
        const { added, deleted } = (
          payload as { Changes: { added: number; deleted: number } }
        ).Changes;
        this.lastGitChanges = { added, deleted };
        this.wasmAdapter?.setStatusGitChanges?.(added, deleted);
        this.scheduleDraw();
        return;
      }
    }
    const mapper = this.pendingServiceMappers.get(requestId);
    if (mapper) {
      this.pendingServiceMappers.delete(requestId);
    }
    this.wasmAdapter?.serviceReply?.(requestId, mapper ? mapper(payload) : payload);
    this.scheduleDraw();
  }

  private installChromeCallbacks(adapter: TerminalAdapter): void {
    adapter.setChromeCallbacks?.({
      listDir: (requestId, path) => {
        const daemonPath = this.toDaemonWorkspacePath(path);
        this.pendingServiceMappers.set(requestId, (payload) => {
          if ("DirListing" in payload) {
            return payload.DirListing.entries;
          }
          if ("Error" in payload) {
            this.pushInAppNotification(
              "File tree failed",
              payload.Error.message,
              "error",
            );
          }
          return [];
        });
        this.options.client.sendFiles(
          requestId,
          { ListDir: { path: daemonPath } },
          this.options.workspaceRoot ?? null,
        );
      },
      readFile: (requestId, path) => {
        const daemonPath = this.toDaemonWorkspacePath(path);
        this.pendingServiceMappers.set(requestId, (payload) => {
          if ("FileContent" in payload) {
            return payload.FileContent.bytes;
          }
          if ("Error" in payload) {
            this.pushInAppNotification(
              "File read failed",
              payload.Error.message,
              "error",
            );
          }
          return [];
        });
        this.options.client.sendFiles(
          requestId,
          { ReadFile: { path: daemonPath } },
          this.options.workspaceRoot ?? null,
        );
      },
      writeFile: (requestId, path, bytes) => {
        const daemonPath = this.toDaemonWorkspacePath(path);
        this.pendingServiceMappers.set(requestId, (payload) =>
          "FileWritten" in payload ? payload.FileWritten.bytes_written : null,
        );
        this.options.client.sendFiles(
          requestId,
          { WriteFile: { path: daemonPath, bytes: Array.from(bytes) } },
          this.options.workspaceRoot ?? null,
        );
      },
      stat: (requestId, path) => {
        const daemonPath = this.toDaemonWorkspacePath(path);
        this.pendingServiceMappers.set(requestId, (payload) =>
          "Stat" in payload ? payload.Stat.entry : null,
        );
        this.options.client.sendFiles(
          requestId,
          { Stat: { path: daemonPath } },
          this.options.workspaceRoot ?? null,
        );
      },
      clipboardRead: (requestId) => {
        void this.readClipboard().then((text) => {
          adapter.setClipboardValue?.(text);
          adapter.serviceReply?.(requestId, text);
          this.scheduleDraw();
        });
      },
      clipboardWrite: (text) => {
        void this.writeClipboard(text);
      },
      notify: (title, body, level) => {
        // OS-notification request from shared chrome
        // (`NotificationService::notify`). The helper handles lazy
        // permission negotiation and falls back to the in-app toast
        // stack when the browser denies or the API is missing.
        void this.deliverNotification(title, body, level, adapter);
      },
      commandRun: (requestId, command) => {
        if (!this.runChromeCommand(command, adapter)) {
          this.handleInputBytes(new TextEncoder().encode(`${command}\r`));
        }
        adapter.serviceReply?.(requestId, { ok: true });
        this.scheduleDraw();
      },
      gitStatus: (requestId, _repo) => {
        this.pendingServiceMappers.set(requestId, (payload) => {
          if ("Status" in payload) {
            return { branch: null, dirty: payload.Status.entries.length > 0 };
          }
          return { branch: null, dirty: false };
        });
        this.options.client.sendGit(requestId, "Status");
      },
      gitDiff: (requestId, _repo, path) => {
        this.pendingServiceMappers.set(requestId, (payload) =>
          "Diff" in payload ? diffFilesFromWire(payload.Diff.hunks) : [],
        );
        this.options.client.sendGit(requestId, { Diff: { path } });
      },
    });
    // Install the PTY outbox so the wasm bridge can push DSR / OSC /
    // clipboard responses straight to the daemon without the host
    // having to poll `takePtyWrites()` after every feed. The poll
    // path in `ingest()` stays — it's a no-op once the outbox has
    // drained, and keeps older wasm bundles that lack `set_pty_outbox`
    // (the optional chain in `setPtyOutbox` no-ops there) working.
    adapter.setPtyOutbox?.((bytesB64) => {
      const bytes = base64ToBytes(bytesB64);
      this.sendPtyInput(bytes);
    });
    adapter.setAgentSend?.((requestId, envelopeJson) => {
      let message: unknown;
      try {
        message = JSON.parse(envelopeJson);
      } catch (err) {
        console.warn("[agent] failed to parse outbound envelope", err);
        return;
      }
      this.options.client.sendRaw(
        JSON.stringify({
          Agent: {
            request_id: requestId,
            message,
          },
        }),
      );
    });
    // Code-pane LSP requests: the wasm session layer serializes one
    // `EditorClientMessage` per request (OpenBuffer sync / LspQueryAt /
    // ApplyLspCodeActionAt); ship it over the daemon editor envelope.
    (adapter as {
      setEditorLspRequest?: (cb: (envelopeJson: string) => void) => void;
    }).setEditorLspRequest?.((envelopeJson) => {
      try {
        const message = JSON.parse(envelopeJson) as EditorClientMessage;
        this.sendEditorMessage(message);
      } catch (err) {
        console.warn("[lsp] failed to parse outbound editor envelope", err);
      }
    });
  }

  private toDaemonWorkspacePath(path: string | null | undefined): string {
    const input = (path ?? "").trim();
    if (input.length === 0 || input === ".") return "";
    const root = (this.options.workspaceRoot ?? "").replace(/\/+$/, "");
    const normalized = input.replace(/\\/g, "/");
    if (root.length > 0) {
      const normalizedRoot = root.replace(/\\/g, "/");
      if (normalized === normalizedRoot) return "";
      if (normalized.startsWith(`${normalizedRoot}/`)) {
        return normalized.slice(normalizedRoot.length + 1);
      }
    }
    return normalized.replace(/^\.\//, "");
  }

  private async readClipboard(): Promise<string | null> {
    this.options.client.sendWorkspace("LoadClipboard");
    if (!navigator.clipboard) {
      return this.workspaceClipboardPayload?.text ?? null;
    }
    try {
      const text = await navigator.clipboard.readText();
      if (text.length > 0) {
        const payload = this.textClipboardPayload(text);
        this.workspaceClipboardPayload = payload;
        this.options.client.sendWorkspace({ StoreClipboard: { payload } });
        return text;
      }
      return this.workspaceClipboardPayload?.text ?? text;
    } catch {
      return this.workspaceClipboardPayload?.text ?? null;
    }
  }

  private async writeClipboard(text: string): Promise<void> {
    const payload = this.textClipboardPayload(text);
    this.workspaceClipboardPayload = payload;
    this.options.client.sendWorkspace({ StoreClipboard: { payload } });
    if (!navigator.clipboard) return;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Best-effort clipboard write.
    }
  }

  private textClipboardPayload(text: string): ClipboardPayload {
    return {
      mime_type: "text/plain",
      text,
      bytes: Array.from(new TextEncoder().encode(text)),
      filename: null,
    };
  }

  private ingestWorkspaceClipboardPayload(payload: ClipboardPayload | null): void {
    this.workspaceClipboardPayload = payload;
    this.wasmAdapter?.setClipboardValue?.(payload?.text ?? null);
  }

  private ingestWorkspaceActionCompleted(event: {
    action: WorkspaceAction;
    path: string | null;
    message: string;
  }): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    try {
      adapter.pushNotification?.(
        JSON.stringify({
          title: "Workspace",
          message: event.message,
          severity: "info",
        }),
      );
    } catch {
      // Optional bridge surface.
    }
    // Only note-creation hands back a file worth opening —
    // Init/Reindex report the workspace ROOT, and reading a directory
    // just produces an error toast.
    if (event.path && event.action === "CreateNeoismNote") {
      this.requestFileContent(event.path, this.activeTabIndex);
    }
    // Any workspace action that can change the notes tree (create a
    // note, init the workspace, reindex) must re-list so the sidebar
    // surfaces the full, current set — desktop re-walks the vault after
    // each of these. The web only queued a listing on first open, so a
    // freshly-created note never appeared until the panel was toggled.
    if (event.action === "CreateNeoismNote") {
      void this.refreshNotesSidebarEntries();
    }
  }

  private ingestEditorSurfaceList(surfaces: EditorSurfaceSummary[]): void {
    this.editorSurfaceBindings.clear();
    for (const surface of surfaces) {
      this.ingestEditorSurfaceChanged(surface, false);
    }
    this.replayBufferTabs();
    this.renderPaneLayoutOverlay();
  }

  private ingestEditorSurfaceChanged(
    surface: EditorSurfaceSummary,
    replay = true,
  ): void {
    if (!this.workspaceSessionId) {
      this.workspaceSessionId = surface.session_id;
    }
    if (this.workspaceSessionId && surface.session_id !== this.workspaceSessionId) {
      return;
    }
    this.editorSurfaceBindings.set(surface.surface_id, surface);
    const externalId = this.externalIdFromEditorSurface(surface.surface_id);
    if (externalId === null) {
      return;
    }
    // Phone-control parity: if neoism-agent (or any other client)
    // binds an editor surface for an external_id we don't have a pane
    // for yet, materialise one via the session-layout policy so the
    // remote pane shows up in the local chrome instead of being
    // silently dropped.
    if (!this.paneLayoutPanes.some((pane) => pane.external_id === externalId)) {
      const title =
        surface.path && surface.path.length > 0
          ? surface.path.split(/[\\/]/).pop() ?? `Editor ${externalId}`
          : `Editor ${externalId}`;
      const result = this.applySessionLayoutPolicy(
        "ensure_external",
        "horizontal",
        title,
        externalId,
      );
      if (!result || !result.panes.some((p) => p.external_id === externalId)) {
        return;
      }
      this.nextWebPaneId = Math.max(this.nextWebPaneId, externalId + 1);
    }

    const state =
      this.paneTabState.get(externalId) ??
      { tabIndices: [], activeTabIndex: null };
    const tabIndex = surface.path
      ? this.ensureFileTabForEditorSurface(surface.path)
      : null;
    if (tabIndex === null) {
      state.activeTabIndex = null;
      state.tabIndices = [];
    } else {
      if (!state.tabIndices.includes(tabIndex)) {
        state.tabIndices.push(tabIndex);
      }
      state.activeTabIndex = tabIndex;
      if (this.activePaneExternalId() === externalId) {
        this.activeTabIndex = tabIndex;
        this.wasmAdapter?.setActiveTab?.(tabIndex);
      }
    }
    this.paneTabState.set(externalId, state);
    this.prunePaneTabIndices();
    if (replay) {
      this.replayBufferTabs();
      this.renderPaneLayoutOverlay();
    }
  }

  private ingestEditorSurfaceClosed(surfaceId: string): void {
    this.editorSurfaceBindings.delete(surfaceId);
    const externalId = this.externalIdFromEditorSurface(surfaceId);
    if (externalId === null) {
      return;
    }
    const state = this.paneTabState.get(externalId);
    if (state) {
      state.activeTabIndex = null;
      state.tabIndices = [];
      this.paneTabState.set(externalId, state);
    }
    this.renderPaneLayoutOverlay();
  }

  private externalIdFromEditorSurface(surfaceId: string): number | null {
    const externalId = Number(surfaceId);
    return Number.isInteger(externalId) && externalId > 0 ? externalId : null;
  }

  private ensureFileTabForEditorSurface(path: string, title?: string | null): number {
    const existing = this.bufferTabs.findIndex((tab) => tab.path === path);
    if (existing >= 0) {
      return existing;
    }
    const fileName = title?.trim() || path.split(/[\\/]/).pop() || path;
    this.bufferTabs.push({ title: fileName, kind: "file", path });
    const tabIndex = this.bufferTabs.length - 1;
    this.requestFileContent(path, tabIndex);
    return tabIndex;
  }

  private activateFileTab(path: string): void {
    const index = this.bufferTabs.findIndex((tab) => tab.path === path);
    if (index < 0) return;
    this.activeTabIndex = index;
    this.openFileTabContent(path);
    this.replayBufferTabs();
    this.scheduleDraw();
  }

  /**
   * Deliver an OS-notification request from shared chrome. Routes
   * through the browser's `Notification` API after lazily requesting
   * permission on first use, and falls back to the in-app toast stack
   * (`pushNotification`) when permission is denied, the API is
   * missing (Safari iOS, insecure context, etc.), or construction
   * throws.
   *
   * The `level` parameter is one of `"info" | "warn" | "error"`
   * (matching the Rust `NotificationLevel` discriminator); it isn't
   * surfaced to the platform `Notification` directly (the spec has
   * no urgency field), but it IS mirrored into the in-app fallback
   * toast so the user still sees the severity coloring.
   */
  private async deliverNotification(
    title: string,
    body: string,
    level: string,
    adapter: TerminalAdapter,
  ): Promise<void> {
    const severity =
      level === "warn" || level === "error" ? level : "info";

    const fallbackToast = () => {
      // Mirror the shape `ChromeBridge::push_notification` expects:
      // `{ title, message, severity }`. The bridge stitches `title`
      // and `message` into the in-app toast body and picks the
      // matching `NotificationLevel`.
      try {
        adapter.pushNotification?.(
          JSON.stringify({ title, message: body, severity }),
        );
      } catch {
        // Best-effort fallback; bridge may not expose
        // pushNotification on pre-W3 builds.
      }
      this.scheduleDraw();
    };

    if (typeof Notification === "undefined") {
      fallbackToast();
      return;
    }
    let permission: NotificationPermission;
    try {
      permission = await this.ensureNotificationPermission();
    } catch {
      fallbackToast();
      return;
    }
    if (permission !== "granted") {
      fallbackToast();
      return;
    }
    try {
      new Notification(title || "Neoism", { body });
    } catch {
      fallbackToast();
    }
  }

  /**
   * Lazily request `Notification` permission on first use. Cached on
   * the panel so we only prompt once per session — subsequent calls
   * return the cached decision. `Notification.permission` is the
   * authoritative starting point; "default" means we haven't asked,
   * "granted" / "denied" are sticky.
   */
  private notificationPermission: NotificationPermission | null = null;
  private async ensureNotificationPermission(): Promise<NotificationPermission> {
    if (this.notificationPermission !== null) {
      return this.notificationPermission;
    }
    const current = Notification.permission;
    if (current === "granted" || current === "denied") {
      this.notificationPermission = current;
      return current;
    }
    // "default" — ask once. `requestPermission` returns either a
    // Promise or invokes a callback on older browsers; we treat both
    // shapes uniformly.
    let result: NotificationPermission;
    try {
      result = await Notification.requestPermission();
    } catch {
      result = "denied";
    }
    this.notificationPermission = result;
    return result;
  }

  private runChromeCommand(command: string, adapter: TerminalAdapter): boolean {
    switch (command) {
      case "open-composer":
        adapter.showCommandComposer?.();
        return true;
      case "show-git-diff":
        adapter.showGitDiff?.();
        return true;
      case "refresh-file-tree":
        adapter.refreshFileTree?.();
        return true;
      default:
        return false;
    }
  }

  private forwardChromeEvent(event: unknown): void {
    try {
      this.wasmAdapter?.handleUiEvent?.(event);
    } catch (err) {
      if (typeof console !== "undefined") {
        console.warn("[neoism] chrome event failed", err);
      }
    }
    this.scheduleDraw();
  }

  private isMobileViewport(): boolean {
    return window.matchMedia("(max-width: 600px)").matches;
  }

  private isRendered(): boolean {
    return this.wasmAdapter?.isRendered() === true;
  }

  applyWorkplacePreferences(prefs: WorkplacePreferences): void {
    // Accept any stored name: the full catalog lives in the wasm
    // bridge (which may still be loading when prefs arrive), and the
    // bridge's `IdeTheme::by_name` falls back to pastel_dark for
    // unknown names — same forgiving path the desktop config takes.
    if (typeof prefs.theme === "string" && prefs.theme.length > 0) {
      this.setIdeTheme(prefs.theme);
    }
    if (typeof prefs.font_size === "number" && Number.isFinite(prefs.font_size)) {
      this.applyFontScale(prefs.font_size / 14.0, false);
    }
  }

  /** Accent (cursor) color per theme — mirrors the Rust
   *  `IdeTheme::by_name` accents so presence broadcasts the color this
   *  user's cursor actually has. */
  private static readonly THEME_ACCENTS: Record<
    string,
    { r: number; g: number; b: number }
  > = {
    pastel_dark: { r: 0xe8, g: 0xe8, b: 0xe8 },
    nvchad_one: { r: 0x61, g: 0xaf, b: 0xef },
    tokyo_night: { r: 0x7a, g: 0xa2, b: 0xf7 },
    catppuccin_mocha: { r: 0xcb, g: 0xa6, b: 0xf7 },
  };

  /** User cursor overrides (mirrors desktop's `[neoism] cursor-color`
   *  / `cursor-style` config keys): a `#RRGGBB` color that beats the
   *  theme accent, and the `"rainbow"` preset that ignores color. */
  private cursorStyleConfig(): { colorHex: string | null; style: string } {
    let colorHex: string | null = null;
    let style = "solid";
    try {
      colorHex = window.localStorage.getItem("neoism.cursor-color");
      style = window.localStorage.getItem("neoism.cursor-style") ?? "solid";
    } catch {
      // Storage unavailable (private mode) — theme defaults apply.
    }
    return { colorHex, style };
  }

  private applyPresenceThemeColor(theme: string): void {
    const { colorHex, style } = this.cursorStyleConfig();
    // Keep the wasm chrome's local cursor in sync with the same config.
    (
      this.wasmAdapter as {
        setCursorStyle?: (colorHex: string | null, style: string) => void;
      }
    )?.setCursorStyle?.(colorHex, style);
    this.presencePublisher.setRainbow(style === "rainbow");
    const parsed = colorHex && /^#?[0-9a-fA-F]{6}$/.test(colorHex.trim())
      ? parseInt(colorHex.trim().replace(/^#/, ""), 16)
      : null;
    if (parsed !== null) {
      this.presencePublisher.setColor({
        r: (parsed >> 16) & 0xff,
        g: (parsed >> 8) & 0xff,
        b: parsed & 0xff,
      });
      return;
    }
    // Prefer the wasm catalog's accent (covers all ~100 themes); the
    // static table only backs the builtin four for pre-wasm frames.
    const catalogAccent = this.wasmAdapter
      ?.allIdeThemes?.()
      .find((entry) => entry.name === theme)?.accent;
    const catalogParsed =
      catalogAccent && /^#?[0-9a-fA-F]{6}$/.test(catalogAccent.trim())
        ? parseInt(catalogAccent.trim().replace(/^#/, ""), 16)
        : null;
    if (catalogParsed !== null) {
      this.presencePublisher.setColor({
        r: (catalogParsed >> 16) & 0xff,
        g: (catalogParsed >> 8) & 0xff,
        b: catalogParsed & 0xff,
      });
      return;
    }
    const accent = TerminalPanel.THEME_ACCENTS[theme];
    if (accent) this.presencePublisher.setColor(accent);
  }

  private applyFontScale(scale: number, persist = true): void {
    this.currentFontScale = Math.max(0.5, Math.min(3.0, scale));
    this.wasmAdapter?.setFontScale?.(this.currentFontScale);
    this.handleResize(this.root.clientWidth, this.root.clientHeight);
    requestAnimationFrame(() => {
      this.handleResize(this.root.clientWidth, this.root.clientHeight);
    });
    if (persist) {
      this.options.onFontSizeChanged?.(this.currentFontScale * 14.0);
    }
  }

  private hideCustomCursor(): void {
    this.wasmAdapter?.setCustomCursor?.(
      JSON.stringify({ x: 0, y: 0, visible: false }),
    );
  }

  /** Markdown Space-leader: Space then x closes the tab. Only in
   *  normal mode — insert-mode spaces are text. */
  private markdownLeaderPendingAt: number | null = null;
  private handleMarkdownLeaderShortcut(event: KeyboardEvent): boolean {
    if (!this.activeTabIsMarkdown() || !this.useWasmMarkdown()) {
      this.markdownLeaderPendingAt = null;
      return false;
    }
    if (event.altKey || event.ctrlKey || event.metaKey) {
      this.markdownLeaderPendingAt = null;
      return false;
    }
    const adapter = this.wasmAdapter as {
      markdownInInsertMode?: () => boolean;
      markdownSearchActive?: () => boolean;
    };
    if (adapter?.markdownInInsertMode?.() === true) {
      this.markdownLeaderPendingAt = null;
      return false;
    }
    // A live `/`-search session owns the keyboard — spaces belong to
    // the query (multi-word searches), not the leader chord.
    if (adapter?.markdownSearchActive?.() === true) {
      this.markdownLeaderPendingAt = null;
      return false;
    }
    const now = performance.now();
    if (
      this.markdownLeaderPendingAt !== null &&
      now - this.markdownLeaderPendingAt > 900
    ) {
      this.markdownLeaderPendingAt = null;
    }
    if (this.markdownLeaderPendingAt !== null) {
      this.markdownLeaderPendingAt = null;
      if (!event.shiftKey && matchesKey(event, "KeyX", "x")) {
        this.closeActiveBufferTab();
        return true;
      }
      // Not the close chord — fall through to normal markdown routing.
      return false;
    }
    if (!event.shiftKey && event.code === "Space") {
      this.markdownLeaderPendingAt = now;
      return true;
    }
    return false;
  }

  /** (Re-)arm the one-shot media query that fires when
   *  `window.devicePixelRatio` changes. See `dprMediaQuery` docs. */
  private watchDevicePixelRatio(): void {
    if (
      typeof window === "undefined" ||
      typeof window.matchMedia !== "function"
    ) {
      return;
    }
    this.dprMediaQuery?.removeEventListener("change", this.dprChangeHandler);
    const dpr = window.devicePixelRatio || 1;
    this.dprMediaQuery = window.matchMedia(`(resolution: ${dpr}dppx)`);
    this.dprMediaQuery.addEventListener("change", this.dprChangeHandler);
  }

  private handleResize(widthPx: number, heightPx: number): void {
    // One contract for every size source: canvas style = CSS rect,
    // chrome layout = CSS pixels, render scale = devicePixelRatio
    // clamped by the GPU texture cap (`sizeContractFor`), backing
    // store / swapchain = CSS x scale. Using RAW devicePixelRatio here
    // while sugarloaf clamps the swapchain to its texture limit is the
    // blurry-overflow bug: chrome would lay out bigger than the
    // surface it paints into.
    const contract = sizeContractFor(this.canvas, widthPx, heightPx);
    const width = contract.cssWidth;
    const height = contract.cssHeight;
    const dpr = contract.scale;

    // CSS dimensions describe layout — the canvas always occupies the
    // CSS-pixel rect the panel measured.
    this.canvas.style.width = `${width}px`;
    this.canvas.style.height = `${height}px`;
    this.syncMarkdownLayerBounds();

    // Only mutate canvas backing buffer when we're committed to the 2D
    // stub path. If wasm init hasn't resolved yet, leave the canvas
    // untouched so sugarloaf can still claim WebGL2. If sugarloaf is
    // live, the wgpu surface owns the buffer — RenderedTerminal::resize
    // drives the swapchain.
    if (this.wasmInitResolved && !this.isRendered()) {
      this.canvas.width = contract.physicalWidth;
      this.canvas.height = contract.physicalHeight;
      const ctx = this.ensureCtx();
      if (ctx) {
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      }
    }

    // ChromeBridge::resize expects `width_px`/`height_px` in CSS-pixel
    // (logical layout) units — chrome layout math operates in CSS
    // pixels, and the Rust side internally multiplies by `scale` when
    // resizing the sugarloaf swapchain. So pass CSS dims here, not
    // physical: the DPR argument is what scales glyph rasterization up
    // to the physical backing store.
    if (this.wasmAdapter?.isChrome()) {
      this.wasmAdapter.resize(this.cols, this.rows, dpr, width, height);
    }
    const chromeTerminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    const terminalWidth = chromeTerminal?.w ?? width;
    const terminalHeight = chromeTerminal?.h ?? height;
    const scaledCellWidth = CELL_WIDTH * this.currentFontScale;
    const scaledCellHeight = CELL_HEIGHT * this.currentFontScale;
    const cols = Math.max(MIN_COLS, Math.floor(terminalWidth / scaledCellWidth));
    const rows = Math.max(MIN_ROWS, Math.floor(terminalHeight / scaledCellHeight));
    if (cols !== this.cols || rows !== this.rows) {
      this.cols = cols;
      this.rows = rows;
      this.stubTerminal.resize(cols, rows);
      this.wasmAdapter?.resize(cols, rows, dpr, width, height);
      this.resizePty(cols, rows);
      this.sendEditorResize(cols, rows);
    } else if (this.isRendered()) {
      // Dimensions in cells didn't change but the CSS size or DPR may
      // have shifted — push a resize so sugarloaf rescales.
      this.wasmAdapter?.resize(cols, rows, dpr, width, height);
      this.sendEditorResize(cols, rows);
    }
    if (this.wasmAdapter?.isChrome()) {
      this.forwardChromeEvent(fromResizeEvent({ w: width, h: height, scale: dpr }));
    }
    this.scheduleDraw();
  }

  /// Open buffer-tab bookkeeping. Index 0 is the always-present
  /// Terminal tab; user-opened files append onto this list.
  private bufferTabs: WebBufferTab[] = [
    { title: "Terminal 1", kind: "terminal" },
  ];
  private activeTabIndex = 0;
  private pendingTerminalTabSpawns: PendingTerminalTabSpawn[] = [];
  private lastBufferTabsFingerprint = "";
  private readonly ptyReplayBuffers = new Map<string, Uint8Array>();
  private readonly neoismAgentRouteId = 1;
  private markdownLayerTabIndex: number | null = null;
  private sessionLayoutStateJson: string | null = null;
  private paneLayoutPanes: WebPaneRect[] = [];
  private readonly paneTabState = new Map<number, WebPaneState>();
  /** Pane external id → PTY session id for TERMINAL panes. Focused
   *  terminal panes ride the main grid; the map keeps every split
   *  terminal pane's session known so its bytes route into the
   *  per-pane wasm terminal that renders it live. */
  private readonly paneSessionIds = new Map<number, string>();
  private nextWebPaneId = 2;
  private bufferTabDrag: WebBufferTabDrag | null = null;
  private agentInput = "";
  private agentLastAttachAt = 0;
  /** Cached `@`-mention candidate list (workspace-relative file paths)
   *  last fed into the shared pane, plus the fetch timestamp. The pane
   *  fuzzy-ranks per keystroke locally, so the list only needs a
   *  periodic refresh. */
  private agentMentionFileCache: { paths: string[]; at: number } | null = null;
  private agentMentionFetchInFlight = false;
  private terminalInput = "";
  private editorSessionStarted = false;
  /// Delayed-frame timer that matures the code pane's mouse-rest LSP
  /// hover after the pointer stops moving (draw-on-demand otherwise
  /// never ticks the ~400ms candidate).
  private editorLspHoverTimer: ReturnType<typeof setTimeout> | null = null;
  /// Fallback timer completing a format-on-save when the formatter's
  /// reply is lost (old daemon / dropped frame).
  private editorLspFormatFallback: ReturnType<typeof setTimeout> | null = null;
  private editorGridCols = 0;
  private editorGridRows = 0;
  private workspaceSessionId: string | null = null;
  private readonly editorSurfaceBindings = new Map<string, EditorSurfaceSummary>();
  private readonly editorResizeBySurface = new Map<string, { width: number; height: number }>();

  private drainChromeIntents(): void {
    this.drainTopBarActions();
    this.drainChromePageIntents();
    this.drainAgentTabOpens();
    this.drainFileTreeOpens();
    this.drainSidePanelOpens();
    this.drainBufferTabClicks();
    this.drainFinderOpenIntents();
    this.drainPaletteIntents();
    this.pumpSidePanelRefreshes();
    this.pumpCompletionDirRequests();
  }

  private drainTopBarActions(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.drainTopBarAction) return;
    let acted = false;
    for (let i = 0; i < 8; i++) {
      const action = adapter.drainTopBarAction();
      if (!action) break;
      acted = true;
      switch (action) {
        case "open_agent":
          this.openNeoismAgentTab();
          break;
        case "open_servers":
          // Web connection ownership lives above TerminalPanel; use the
          // existing host-provided workplace/server surface there.
          this.options.onShowWorkplaces?.();
          break;
        case "open_workspaces":
          window.setTimeout(() => this.openWorkspacesModal(), 80);
          break;
        case "start_web_server":
          window.open(window.location.origin, "_blank", "noopener,noreferrer");
          break;
        case "open_settings":
          this.openWebSettingsPage();
          break;
        case "open_extensions":
          this.openChromePageTab("chrome-extensions", "Extensions");
          this.wasmAdapter?.extensionsFocusSearch?.();
          this.refreshWebExtensions();
          break;
        case "open_neoworld":
          // Seed the pane from the persisted pet (localStorage — the
          // browser twin of desktop's sqlite NeoWorldStore) BEFORE the
          // tab activates so the sim resumes instead of resetting.
          this.wasmAdapter?.neoworldEnsure?.(loadStoredNeoworldPet());
          this.openChromePageTab("chrome-neoworld", "NeoWorld");
          break;
        case "open_about":
          this.wasmAdapter?.openAboutModal?.();
          break;
        case "open_themes": {
          // Chrome normally consumes OpenThemes internally (palette
          // themes mode); this is the defensive host-side route for
          // bundles that surface it instead.
          const themes = this.wasmAdapter?.allIdeThemes?.() ?? [];
          if (themes.length > 0) {
            this.wasmAdapter?.enterPaletteThemesMode?.(
              JSON.stringify(themes.map((theme) => theme.name)),
            );
          }
          break;
        }
        case "open_search": {
          const adapter = this.wasmAdapter;
          (adapter?.showFinderGrep ?? adapter?.showFinder)?.call(adapter);
          break;
        }
      }
    }
    if (acted) this.scheduleDraw();
  }

  /** Open the full-screen Settings overlay immediately, then refresh
   *  it with the daemon host's config.json once the fetch lands. */
  private openWebSettingsPage(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.openSettingsPage) return;
    adapter.openSettingsPage(null);
    this.scheduleDraw();
    void fetchConfig(this.options.client).then((value) => {
      if (value == null) return;
      try {
        adapter.setSettingsValues?.(JSON.stringify(value));
      } catch {
        // Serialization hiccup — overlay keeps last-known values.
      }
      this.scheduleDraw();
    });
  }

  /** Append (or re-activate) a chrome helper-page tab — the web twin
   *  of desktop's `open_chrome_page(ChromePageKind::…)` singleton
   *  tabs. The kind string maps to a `ChromePageRef` inside the wasm
   *  `setBufferTabs` replay. */
  private openChromePageTab(
    kind: "chrome-extensions" | "chrome-neoworld",
    title: string,
  ): void {
    const existing = this.bufferTabs.findIndex(
      (tab) => (tab.kind as string) === kind,
    );
    if (existing >= 0) {
      this.activeTabIndex = existing;
    } else {
      this.bufferTabs.push({
        title,
        kind: kind as unknown as WebBufferTab["kind"],
      });
      this.activeTabIndex = this.bufferTabs.length - 1;
    }
    this.assignActiveTabToFocusedEditorPane();
    this.replayBufferTabs();
    this.scheduleDraw();
  }

  /** Fetch the daemon host's read-only extensions inventory and seed
   *  the shared Extensions page. Statuses reflect the DAEMON machine;
   *  install/uninstall stay desktop-host actions. */
  private refreshWebExtensions(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.setExtensionsEntries) return;
    void fetchExtensions(this.options.client).then((entries) => {
      try {
        adapter.setExtensionsEntries?.(JSON.stringify(entries));
      } catch {
        // Malformed reply — the page keeps its empty state.
      }
      this.scheduleDraw();
    });
  }

  /** Per-frame drain for the chrome-page hosts: persist settings
   *  writes through the daemon config plane, open repository links,
   *  and store NeoWorld pet snapshots. */
  private drainChromePageIntents(): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    const actions = parseSettingsActions(
      adapter.drainSettingsActions?.() ?? null,
    );
    for (const action of actions) {
      if (action.kind === "set") {
        void persistSetting(this.options.client, action.key, action.value);
      } else if (action.kind === "set_keybind") {
        void persistKeybind(
          this.options.client,
          action.action,
          action.key,
          action.with,
        );
      }
      // "open_config_file" / "run_action" are fully handled inside
      // the wasm bridge (toast / agent model picker).
    }
    if (actions.length > 0) this.scheduleDraw();
    const extRaw = adapter.drainExtensionsActions?.() ?? null;
    if (extRaw) {
      try {
        const rows: unknown = JSON.parse(extRaw);
        if (Array.isArray(rows)) {
          for (const row of rows) {
            const rec = row as { kind?: unknown; url?: unknown };
            if (
              rec?.kind === "open_repository" &&
              typeof rec.url === "string"
            ) {
              window.open(rec.url, "_blank", "noopener,noreferrer");
            }
          }
        }
      } catch {
        // Malformed drain — ignore.
      }
    }
    const snapshot = adapter.drainNeoworldSnapshot?.();
    if (snapshot) {
      saveStoredNeoworldPet(snapshot);
    }
  }

  /** Tab completion on web has no filesystem — feed it daemon dir
   *  listings on demand. The first Tab in an unseeded directory
   *  queues the request; the listing lands within a frame or two and
   *  the next Tab completes. */
  private pumpCompletionDirRequests(): void {
    const raw = this.wasmAdapter?.drainCompletionDirRequests?.();
    if (!raw || !Array.isArray(raw) || raw.length === 0) return;
    for (const dir of raw) {
      if (typeof dir !== "string" || dir.length === 0) continue;
      void this.seedCompletionDir(dir);
    }
  }

  private async seedCompletionDir(dir: string): Promise<void> {
    const root = this.options.workspaceRoot?.replace(/\/+$/, "");
    if (!root) return;
    // The daemon's Files surface is workspace-relative; skip dirs
    // outside the root (completion falls back to builtins there).
    let rel: string;
    if (dir === root) {
      rel = ".";
    } else if (dir.startsWith(`${root}/`)) {
      rel = dir.slice(root.length + 1);
    } else {
      return;
    }
    try {
      const reply = await this.options.client.requestFiles(
        { ListDir: { path: rel } },
        this.options.workspaceRoot ?? null,
      );
      if (!reply || typeof reply !== "object" || !("DirListing" in reply)) {
        return;
      }
      const entries = (reply as {
        DirListing: { entries: Array<{ name: string; is_dir: boolean }> };
      }).DirListing.entries.map(
        (entry) => [entry.name, entry.is_dir] as [string, boolean],
      );
      this.wasmAdapter?.terminalSeedCompletionDir?.(dir, JSON.stringify(entries));
    } catch {
      // Daemon hiccup — the next Tab re-queues the request.
    }
  }

  /** Desktop's composer recalls ~/.zsh_history on ArrowUp; fetch the
   *  same entries through the daemon and seed the shared input. */
  private async seedShellHistory(): Promise<void> {
    const adapter = this.wasmAdapter;
    if (!adapter?.terminalSeedHistory) return;
    try {
      const reply = await this.options.client.requestFiles(
        { ReadShellHistory: { max_entries: 500 } },
        this.options.workspaceRoot ?? null,
      );
      if (!reply || typeof reply !== "object" || !("ShellHistory" in reply)) {
        return;
      }
      const entries = (reply as { ShellHistory: { entries: string[] } })
        .ShellHistory.entries;
      if (entries.length > 0) {
        adapter.terminalSeedHistory(JSON.stringify(entries));
      }
    } catch {
      // Old daemon without ReadShellHistory — session-local history
      // still works.
    }
  }

  /** Alt+G entry point: toggle the shared rich git side panel and, on
   *  open, fetch its data from the daemon. */
  private toggleGitSidePanel(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.toggleGitDiffPanel) {
      adapter?.toggleGitDiff?.();
      return;
    }
    adapter.toggleGitDiffPanel();
    // The refresh intent is queued chrome-side; pump it now so the
    // fetch starts this frame instead of after the next draw.
    this.pumpSidePanelRefreshes();
  }

  /** Answer the shared panels' "I just opened, fetch my data" flags. */
  private pumpSidePanelRefreshes(): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    if (adapter.takeGitPanelRefresh?.()) {
      void this.refreshGitSidePanelData();
    }
    if (adapter.takeNotesRefresh?.()) {
      void this.refreshNotesSidebarEntries();
    }
  }

  /** Note rows / git-panel rows the user activated — same open
   *  pipeline as file-tree picks. */
  private drainSidePanelOpens(): void {
    const opens = this.wasmAdapter?.drainPanelOpenPaths?.();
    if (!opens || !Array.isArray(opens) || opens.length === 0) return;
    this.openActivatedPaths(opens.filter(
      (raw): raw is string => typeof raw === "string" && raw.length > 0,
    ));
  }

  /** Fetch `git status` + whole-repo diff from the daemon and push
   *  them into the shared rich git panel. The panel wants the desktop
   *  `collect_files` shape: repo-relative paths, per-file add/del
   *  counts, and raw patch text per file for the diff card. */
  private async refreshGitSidePanelData(): Promise<void> {
    const adapter = this.wasmAdapter;
    if (!adapter?.gitPanelSetFiles) return;
    try {
      const [status, diff] = await Promise.all([
        this.options.client.requestGit("Status"),
        this.options.client.requestGit({ Diff: { path: null } }),
      ]);
      const statusByPath = new Map<string, string>();
      if (typeof status === "object" && status && "Status" in status) {
        for (const entry of (status as {
          Status: { entries: Array<{ path: string; status: string }> };
        }).Status.entries) {
          statusByPath.set(entry.path, entry.status);
        }
      }
      const patchByPath = new Map<string, string>();
      const countsByPath = new Map<string, { add: number; del: number }>();
      if (typeof diff === "object" && diff && "Diff" in diff) {
        for (const hunk of (diff as {
          Diff: { hunks: Array<{ path: string; patch: string }> };
        }).Diff.hunks) {
          patchByPath.set(
            hunk.path,
            (patchByPath.get(hunk.path) ?? "") + hunk.patch,
          );
          const counts = countsByPath.get(hunk.path) ?? { add: 0, del: 0 };
          for (const line of hunk.patch.split("\n")) {
            if (line.startsWith("+") && !line.startsWith("+++")) counts.add += 1;
            else if (line.startsWith("-") && !line.startsWith("---")) counts.del += 1;
          }
          countsByPath.set(hunk.path, counts);
        }
      }
      const paths = new Set<string>([
        ...statusByPath.keys(),
        ...countsByPath.keys(),
      ]);
      const files = [...paths].sort().map((path) => {
        const counts = countsByPath.get(path);
        return {
          path,
          status: statusByPath.get(path) ?? "Modified",
          additions: counts?.add ?? 0,
          deletions: counts?.del ?? 0,
        };
      });
      adapter.gitPanelSetFiles(JSON.stringify(files));
      for (const [path, patch] of patchByPath) {
        adapter.gitPanelSetDiff?.(path, patch);
      }
      this.scheduleDraw();
    } catch (err) {
      this.wasmAdapter?.gitPanelSetError?.(
        err instanceof Error ? err.message : String(err),
      );
      this.scheduleDraw();
    }
  }

  /** Recursively list the active workspace's Neoism notes dir through the
   *  daemon and push the tree into the shared notes sidebar. Desktop resolves
   *  this from the active workspace root; web follows the daemon-owned root
   *  that desktop publishes for the main terminal. */
  private async refreshNotesSidebarEntries(): Promise<void> {
    const adapter = this.wasmAdapter;
    if (!adapter?.notesSetEntries) return;
    const root = this.options.workspaceRoot;
    if (!root) return;
    const entries: Array<{ path: string; is_dir: boolean }> = [];
    const listDir = async (dir: string, depth: number): Promise<boolean> => {
      if (depth > 6 || entries.length > 800) return true;
      let reply: unknown;
      try {
        reply = await this.options.client.requestFiles(
          { ListDir: { path: dir } },
          this.options.workspaceRoot ?? null,
        );
      } catch {
        return false;
      }
      if (!reply || typeof reply !== "object" || !("DirListing" in reply)) {
        return false;
      }
      const listing = (reply as {
        DirListing: { entries: Array<{ name: string; is_dir: boolean }> };
      }).DirListing.entries;
      for (const entry of listing) {
        if (entry.name.startsWith(".")) continue;
        const path = `${dir}/${entry.name}`;
        entries.push({ path, is_dir: entry.is_dir });
        if (entry.is_dir) {
          await listDir(path, depth + 1);
        }
      }
      return true;
    };
    let ok = await listDir("notes", 0);
    if (!ok) {
      // No notes dir yet. The legacy per-project scaffold action is
      // gone (Vaults are the only notes model); one delayed retry
      // covers a vault that is still being created server-side.
      await new Promise((resolve) => setTimeout(resolve, 350));
      ok = await listDir("notes", 0);
    }
    adapter.notesSetEntries(JSON.stringify(entries));
    this.scheduleDraw();
  }

  private syncBridgeStateAfterAdapterReady(): void {
    this.replayBufferTabs();
    this.activateCurrentTabContents(true);
    if (this.lastGitBranch !== undefined) {
      this.wasmAdapter?.setStatusBranch?.(this.lastGitBranch);
    }
    if (this.lastGitChanges) {
      this.wasmAdapter?.setStatusGitChanges?.(
        this.lastGitChanges.added,
        this.lastGitChanges.deleted,
      );
    }
    // Composer parity with desktop: ArrowUp shell history + a warm
    // Tab-completion listing for the workspace root.
    void this.seedShellHistory();
    if (this.options.workspaceRoot) {
      void this.seedCompletionDir(
        this.options.workspaceRoot.replace(/\/+$/, ""),
      );
    }
  }

  /// Apply finder Enter / click picks the wasm bridge queued via
  /// `pick_finder_selection`. Each intent becomes a buffer-tab append
  /// (same path the file-tree open intents take). The grep/git line
  /// jump was an embedded-nvim SendKeys hop; the native CodePane will
  /// reintroduce line targeting.
  private drainFinderOpenIntents(): void {
    const intents = this.wasmAdapter?.drainFinderOpenIntents?.();
    if (!intents || intents.length === 0) return;
    let changed = false;
    // Line-carrying hits (grep / git-changes / Project Problems rows,
    // `intent.line` 1-based): the wasm bridge armed its deferred
    // cross-file cursor target when it queued the intent, so the
    // caret lands on the hit line automatically once the fetched file
    // routes back through `editorOpenFile` (requestFileContent below)
    // — same mechanism as LSP go-to-definition. No extra hop here.
    for (const intent of intents) {
      const fileName = intent.path.split(/[\\/]/).pop() ?? intent.path;
      const existing = this.bufferTabs.findIndex((t) => t.path === intent.path);
      if (existing >= 0) {
        this.activeTabIndex = existing;
        this.requestFileContent(intent.path, this.activeTabIndex);
        this.openFileTabContent(intent.path);
      } else {
        this.bufferTabs.push({
          title: fileName,
          kind: "file",
          path: intent.path,
        });
        this.activeTabIndex = this.bufferTabs.length - 1;
        this.requestFileContent(intent.path, this.activeTabIndex);
        this.openFileTabContent(intent.path);
      }
      changed = true;
      this.assignActiveTabToFocusedEditorPane();
    }
    if (changed) {
      this.replayBufferTabs();
      this.activateCurrentTabContents();
    }
    this.scheduleDraw();
  }

  /// Dispatch command-palette Enter / click picks the wasm bridge
  /// queued via `pick_palette_action`. The bridge serializes the pick
  /// as a discriminated union; this method maps each `kind` onto the
  /// existing host-side handler (toggle panels, run ex commands, etc.).
  /// Buffer, font, search, and ex-command picks carry enough payload
  /// to execute directly on the web side.
  private drainPaletteIntents(): void {
    const intents = this.wasmAdapter?.drainPaletteIntents?.();
    if (!intents || intents.length === 0) return;
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    for (const intent of intents) {
      switch (intent.kind) {
        case "action":
          this.dispatchPaletteAction(intent.action);
          break;
        case "ex_command":
          if (intent.command.length > 0) {
            this.dispatchPaletteExCommand(intent.command, adapter);
          }
          break;
        case "search":
          // Buffer search commits were embedded-nvim lua calls; the
          // native CodePane will own search-commit routing.
          break;
        case "font":
          this.handlePaletteFontPick(intent.family, adapter);
          break;
        case "theme":
          this.handlePaletteThemePick(intent.name, adapter);
          break;
        case "shader":
          this.handlePaletteShaderPick(intent.title, intent.filter, adapter);
          break;
        case "buffer":
          this.activatePaletteBuffer(intent.target);
          break;
        case "workspace":
          this.options.onWorkspaceSelected?.(intent.workspace_id);
          break;
      }
    }
    this.scheduleDraw();
  }

  /**
   * Open the desktop-parity Workspaces modal: the shared command
   * palette's grouped host→workspace tree, rendered on canvas by the
   * wasm chrome. Mirrors desktop's `open_daemon_workspaces_picker`.
   * Falls back to the legacy DOM workplace-switcher overlay when the
   * adapter doesn't expose the mode (stub / data-only adapters, stale
   * wasm pkg) or the host has no tree data yet.
   */
  private openWorkspacesModal(): void {
    const adapter = this.wasmAdapter;
    const payload = this.options.getWorkspacesModalPayload?.() ?? null;
    // Always prefer the real modal when the bridge export exists — even
    // with a sparse/empty tree (it fills in as the daemon publishes;
    // see refreshWorkspacesModal). The old DOM overlay is only the
    // no-wasm / no-workspace-service fallback.
    if (
      adapter &&
      payload &&
      adapter.openWorkspacesPalette?.(JSON.stringify(payload))
    ) {
      this.scheduleDraw();
      return;
    }
    this.options.onShowWorkplaces?.();
  }

  /**
   * Public entry to the Workspaces modal — the web's "entry page".
   * App opens it once on boot so the first thing the operator does is
   * pick a running workspace (or Alt+W a new one) instead of landing
   * in whatever session the daemon replayed first.
   */
  showWorkspacesModal(): void {
    this.openWorkspacesModal();
  }

  /**
   * Live-refresh the Workspaces modal if it is currently open. The
   * host calls this whenever a daemon `HostWorkspaceTree` push lands,
   * so a modal opened before the (async) tree arrived fills in
   * instead of sitting stale. Preserves the user's query/selection.
   */
  refreshWorkspacesModal(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.workspacesPaletteOpen?.()) return;
    const payload = this.options.getWorkspacesModalPayload?.();
    if (!payload) return;
    adapter.refreshWorkspacesPalette?.(JSON.stringify(payload));
    this.scheduleDraw();
  }

  setWorkspaceRoot(workspaceRoot: string | null): void {
    if (!workspaceRoot || workspaceRoot.length === 0) return;
    const changed = this.options.workspaceRoot !== workspaceRoot;
    this.options.workspaceRoot = workspaceRoot;
    this.wasmAdapter?.setWorkspaceRoot?.(workspaceRoot);
    this.wasmAdapter?.refreshFileTree?.();
    if (changed) void this.refreshNotesSidebarEntries();
    this.scheduleDraw();
  }

  /**
   * Open a fresh terminal tab — public entry for the host's
   * create-workspace flow (Alt+W), so a brand-new (tab-less) daemon
   * workspace lands somewhere usable. Same path as the palette's
   * `TabCreate` action.
   */
  openFreshTerminalTab(): void {
    this.openTerminalTabPlaceholder();
    this.scheduleDraw();
  }

  private dispatchPaletteExCommand(command: string, adapter: TerminalAdapter): void {
    const trimmed = command.trim();
    if (trimmed.length === 0) return;
    const normalized = trimmed.toLowerCase();
    if (normalized === "themepicker" || normalized === "theme picker") {
      adapter.enterPaletteThemesMode?.(JSON.stringify(this.ideThemeNames()));
      return;
    }
    if (normalized === "shaderpicker" || normalized === "shader picker") {
      adapter.enterPaletteShadersMode?.(JSON.stringify(WEB_SHADER_FILTERS));
      return;
    }
    if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
      if (normalized === "w" || normalized === "write") {
        this.saveActiveMarkdown();
        return;
      }
      if (normalized === "q" || normalized === "quit") {
        this.closeCurrentSplitOrTab();
        return;
      }
    }
    if (this.activeEditorPaneKind() !== null) {
      // Native editor panes: `:w` / `:q` / `:wq` — the vim ex surface.
      if (normalized === "w" || normalized === "write") {
        this.saveActiveEditorPane();
        return;
      }
      if (normalized === "q" || normalized === "quit") {
        this.closeCurrentSplitOrTab();
        return;
      }
      if (normalized === "wq" || normalized === "x") {
        this.saveActiveEditorPane();
        this.closeCurrentSplitOrTab();
        return;
      }
    }
    // Unintercepted ex commands used to forward to the embedded nvim.
    // That backend is gone; surface the miss instead of silently
    // dropping the input. The native CodePane will own `:` routing.
    this.notifyPaletteUnavailable(
      `Ex command ":${trimmed}" is not available in the web frontend.`,
      adapter,
    );
  }

  /// Map a stable PaletteAction variant name (as serialized by the
  /// wasm bridge) onto the matching host-side handler. Mirrors the
  /// `Screen::execute_palette_action` arm-by-arm on desktop. Shared
  /// Rust owns the command list and palette UI; this host layer owns
  /// transport-specific effects such as spawning daemon PTYs or
  /// opening browser windows.
  ///
  /// Not every action lands here: the wasm bridge executes chrome-side
  /// arms itself at drain time (GoToLine, ToggleWordWrap, Search*/
  /// ReplaceInFile over a code pane, ProjectProblems, OpenMashupPacks,
  /// OpenNeoismNotes, notebook cell ops — see `palettes_finder.rs`'s
  /// `execute_palette_action_chrome_side`), and commands the web host
  /// cannot execute are filtered out of the listing entirely by the
  /// shared `PaletteHostCapabilities::web()` visibility hook. The
  /// default arm's toast is a last-resort safety net, not a routing
  /// strategy.
  private dispatchPaletteAction(action: string): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    switch (action) {
      case "SearchForward":
      case "SearchBackward":
        // Reaches TS only when no code pane owns focus (the bridge
        // handles the in-buffer `/` search chrome-side). The web has
        // no terminal scrollback search yet, so fall back to the
        // workspace grep finder.
        (adapter.showFinderGrep ?? adapter.showFinder)?.call(adapter);
        break;
      case "ShowServers":
        // Web connection ownership lives above TerminalPanel — route
        // to the host's workplace/server switcher, same as the
        // top-bar's open_servers action.
        this.options.onShowWorkplaces?.();
        break;
      case "ToggleInlayHints":
        // Kept listed on web deliberately (discoverability); honest
        // notice until the web LSP surface grows inlay hints.
        this.notifyPaletteUnavailable(
          "Inlay hints haven't landed in the web LSP surface yet.",
          adapter,
        );
        break;
      case "ToggleGitDiffPanel":
        this.toggleGitSidePanel();
        break;
      case "ConfigEditor":
        // Same route as the top-bar hamburger's `open_settings`
        // action: open the shared Settings overlay, then seed it
        // with the daemon host's config.json.
        this.openWebSettingsPage();
        break;
      case "CreateNeoismNote":
        this.options.client.sendWorkspace({
          RunWorkspaceAction: { action },
        });
        break;
      case "SearchFiles":
        (adapter.showFinderFiles ?? adapter.showFinder)?.call(adapter);
        break;
      case "SearchWords":
        (adapter.showFinderGrep ?? adapter.showFinder)?.call(adapter);
        break;
      case "SearchGitChanges":
        (adapter.showFinderGitChanges ?? adapter.showFinder)?.call(adapter);
        break;
      case "OpenNeoismAgent":
        // The bridge has its own queue path for this action; we skip
        // it inside `pick_palette_action`, so this case is defensive.
        this.openNeoismAgentTab();
        break;
      case "RunClaude":
        this.openTerminalTabWithCommand("Claude", "claude");
        break;
      case "RunCodex":
        this.openTerminalTabWithCommand("Codex", "codex");
        break;
      case "RunOpenCode":
        this.openTerminalTabWithCommand("OpenCode", "opencode");
        break;
      case "IncreaseFontSize": {
        const next = Math.min(3.0, this.currentFontScale * 1.1);
        this.applyFontScale(next);
        break;
      }
      case "DecreaseFontSize": {
        const next = Math.max(0.5, this.currentFontScale / 1.1);
        this.applyFontScale(next);
        break;
      }
      case "ResetFontSize":
        this.applyFontScale(1.0);
        break;
      case "ListFonts":
        adapter.enterPaletteFontsMode?.(
          JSON.stringify(["Geist Mono", "Symbols Nerd Font Mono"]),
        );
        break;
      case "ListBuffers":
        adapter.enterPaletteBuffersMode?.(
          JSON.stringify(
            this.bufferTabs.map((tab, tab_index) => ({
              title: tab.title,
              detail:
                tab.path ??
                (tab.kind === "neoism-agent" ? "Neoism" : "Terminal"),
              tab_index,
            })),
          ),
        );
        break;
      case "ShowWorkplaces":
        this.openWorkspacesModal();
        break;
      case "CreateWorkspace":
        this.options.onCreateWorkspace?.();
        break;
      case "TabCreate":
        this.openTerminalTabPlaceholder();
        break;
      case "SplitRight":
        this.splitEditorPane("horizontal");
        break;
      case "SplitDown":
        this.splitEditorPane("vertical");
        break;
      case "SelectNextSplit":
        this.focusEditorPane(false);
        break;
      case "SelectPrevSplit":
        this.focusEditorPane(true);
        break;
      case "TabClose":
        this.closeActiveBufferTab();
        break;
      case "CloseCurrentSplitOrTab":
        this.closeCurrentSplitOrTab();
        break;
      case "TabCloseUnfocused":
        this.closeUnfocusedBufferTabs();
        break;
      case "SelectNextTab":
        this.selectRelativeTab(1);
        break;
      case "SelectPrevTab":
        this.selectRelativeTab(-1);
        break;
      case "Copy":
        void this.writeClipboard(this.terminalInput || this.agentInput);
        break;
      case "Paste":
        void this.readClipboard().then((text) => {
          if (text) this.pasteTextToActiveSurface(text);
        });
        break;
      case "SaveDocument":
        if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
          this.saveActiveMarkdown();
        } else if (this.activeEditorPaneKind() !== null) {
          this.saveActiveEditorPane();
        } else {
          this.notifyPaletteUnavailable(
            "Save is only available for document tabs on the web today.",
            adapter,
          );
        }
        break;
      case "WindowCreateNew":
        window.open(window.location.href, "_blank", "noopener");
        break;
      case "ToggleViMode":
        adapter.toggleViMode?.();
        break;
      case "ToggleAppearanceTheme":
        this.cycleIdeTheme(1);
        break;
      case "OpenThemePicker":
        adapter.enterPaletteThemesMode?.(JSON.stringify(this.ideThemeNames()));
        break;
      case "OpenShaders":
        adapter.enterPaletteShadersMode?.(JSON.stringify(WEB_SHADER_FILTERS));
        break;
      case "ClearHistory":
        this.setTerminalInput("");
        this.sendPtyInput(new TextEncoder().encode("\x1b[3J\x1b[H\x1b[2J"));
        break;
      case "ToggleFullscreen":
        if (document.fullscreenElement) {
          void document.exitFullscreen?.().catch(() => {});
        } else {
          void document.documentElement.requestFullscreen?.().catch(() => {});
        }
        break;
      case "Quit":
        if (this.activeTabIsMarkdown() || this.activeSurface() === "editor") {
          this.closeCurrentSplitOrTab();
        } else if (this.options.pty) {
          this.options.pty.close(this.activePtySessionId() ?? this.options.sessionId);
        } else {
          this.options.client.closePty(this.activePtySessionId() ?? this.options.sessionId);
        }
        break;
      default:
        this.notifyPaletteUnavailable(
          `Palette action "${action}" is not available in the web frontend.`,
          adapter,
        );
        break;
    }
  }

  private handlePaletteFontPick(family: string, adapter: TerminalAdapter): void {
    this.fallbackFontFamily = `'${family.replace(/'/g, "\\'")}', ${this.fallbackFontFamily}`;
    adapter.pushNotification?.(
      JSON.stringify({
        title: "Command palette",
        message: `Font family set to ${family}.`,
        severity: "info",
      }),
    );
  }

  private handlePaletteThemePick(name: string, adapter: TerminalAdapter): void {
    if (!this.ideThemeNames().includes(name)) {
      this.notifyPaletteUnavailable(`Unknown IDE theme "${name}".`, adapter);
      return;
    }
    this.setIdeTheme(name);
    adapter.pushNotification?.(
      JSON.stringify({
        title: "Theme picker",
        message: `Theme set to ${name.replace(/_/g, " ")}.`,
        severity: "info",
      }),
    );
  }

  private handlePaletteShaderPick(
    title: string,
    filter: string | null,
    adapter: TerminalAdapter,
  ): void {
    const known = WEB_SHADER_FILTERS.some((entry) => entry.filter === filter);
    if (!known) {
      this.notifyPaletteUnavailable(`Unknown shader filter "${filter ?? "none"}".`, adapter);
      return;
    }
    this.activeShaderFilter = filter;
    this.applyWebShaderFilter();
    adapter.pushNotification?.(
      JSON.stringify({
        title: "Shader picker",
        message: filter ? `Shader set to ${title}.` : "Shader filter disabled.",
        severity: "info",
      }),
    );
  }

  private applyWebShaderFilter(): void {
    const filter = this.activeShaderFilter;
    this.canvas.classList.toggle("terminal-shader-crt", filter === "crt_curve");
    this.canvas.classList.toggle("terminal-shader-newpixiecrt", filter === "newpixiecrt");
  }

  private notifyPaletteUnavailable(message: string, adapter: TerminalAdapter): void {
    console.info(`[neoism] ${message}`);
    try {
      adapter.pushNotification?.(
        JSON.stringify({
          title: "Command palette",
          message,
          severity: "info",
        }),
      );
    } catch {
      // Optional bridge surface.
    }
  }

  private pushInAppNotification(
    title: string,
    message: string,
    severity: "info" | "warn" | "error" = "info",
  ): void {
    try {
      this.wasmAdapter?.pushNotification?.(
        JSON.stringify({ title, message, severity }),
      );
    } catch {
      // Optional bridge surface.
    }
    this.scheduleDraw();
  }

  private activatePaletteBuffer(target: PaletteBufferTarget): void {
    const tabIndex = target.tab_index;
    if (tabIndex < 0 || tabIndex >= this.bufferTabs.length) return;
    this.activeTabIndex = tabIndex;
    this.wasmAdapter?.setActiveTab?.(this.activeTabIndex);
    this.assignActiveTabToFocusedEditorPane();
    this.replayBufferTabs();
    this.activateCurrentTabContents(false);
    this.scheduleDraw();
  }

  private openTerminalTabPlaceholder(): void {
    this.spawnTerminalTab({});
  }

  private openTerminalTabWithCommand(title: string, command: string): void {
    this.spawnTerminalTab({ title, command });
  }

  private spawnTerminalTab(pending: PendingTerminalTabSpawn): void {
    if (!this.options.pty) {
      this.activateFirstTerminalTab();
      if (pending.command) {
        this.handleInputBytes(new TextEncoder().encode(`${pending.command}\n`));
      }
      return;
    }
    this.pendingTerminalTabSpawns.push(pending);
    // New shells open IN the workspace directory (the anchor). The user
    // can `cd` anywhere afterward — that stays local to this shell.
    this.options.pty.spawn({
      cwd: this.options.workspaceRoot ?? null,
      cols: this.cols,
      rows: this.rows,
    });
  }

  private activateFirstTerminalTab(): void {
    const index = this.bufferTabs.findIndex((tab) => tab.kind === "terminal");
    if (index < 0) return;
    this.activeTabIndex = index;
    this.wasmAdapter?.setActiveTab?.(index);
    this.replayBufferTabs();
    this.activateCurrentTabContents(false);
    this.focus();
  }

  private closeActiveBufferTab(): void {
    this.applyBufferTabPolicy("close_active");
  }

  private closeCurrentSplitOrTab(): void {
    this.closeEditorPaneOrTab();
  }

  private closeUnfocusedBufferTabs(): void {
    const oldTabs = this.bufferTabs;
    const fallbackTerminal = oldTabs.find((tab) => tab.kind === "terminal") ?? {
      title: "Terminal 1",
      kind: "terminal" as const,
      sessionId: this.options.sessionId,
    };
    const active = oldTabs[this.activeTabIndex] ?? fallbackTerminal;
    for (let i = 0; i < oldTabs.length; i += 1) {
      const tab = oldTabs[i];
      if (i === this.activeTabIndex) continue;
      if (tab.kind === "terminal" && tab.sessionId) {
        this.options.pty?.close(tab.sessionId);
        this.ptyReplayBuffers.delete(tab.sessionId);
      }
    }
    if (active.kind === "terminal") {
      this.bufferTabs = [active];
      this.activeTabIndex = 0;
    } else {
      this.bufferTabs = [fallbackTerminal, active];
      this.activeTabIndex = 1;
    }
    const sessionId = this.activePtySessionId();
    this.replayBufferTabs();
    if (sessionId) {
      this.activateCurrentTabContents(false);
    }
    this.scheduleDraw();
  }

  private selectRelativeTab(delta: number): void {
    this.applyBufferTabPolicy(delta < 0 ? "select_previous" : "select_next");
  }

  private selectIndexedTab(index: number): void {
    this.applyBufferTabPolicy("select_index", index);
  }

  private moveActiveBufferTab(delta: -1 | 1): void {
    this.applyBufferTabPolicy(delta < 0 ? "move_previous" : "move_next");
  }

  private activePaneExternalId(): number | null {
    return this.paneLayoutPanes.find((pane) => pane.focused)?.external_id ?? null;
  }

  private editorSurfaceId(externalId: number): string {
    return String(externalId);
  }

  private focusedEditorSurfaceId(): string | null {
    const externalId = this.activePaneExternalId();
    return externalId === null ? null : this.editorSurfaceId(externalId);
  }

  private activeEditorGridSize(): { cols: number; rows: number } {
    return {
      cols: Math.max(1, Math.trunc(this.editorGridCols || this.cols)),
      rows: Math.max(1, Math.trunc(this.editorGridRows || this.rows)),
    };
  }

  private sendEditorMessage(message: EditorClientMessage): void {
    this.options.client.sendEditor(message, this.options.workspaceRoot ?? null);
  }

  /** `bufferId` must already be the canonical `file://<abs>` form. */
  private requestPresenceSnapshot(bufferId: string): void {
    if (!bufferId || this.requestedPresenceBuffers.has(bufferId)) return;
    this.requestedPresenceBuffers.add(bufferId);
    this.options.client.sendCrdt({
      RequestPresenceSnapshot: {
        buffer_id: bufferId,
        exclude_peer_id: this.crdtPeerId,
      },
    });
  }

  /**
   * Wave 8D outbound co-editing pump: let the wasm pane bind the
   * active markdown doc (OpenBuffer on first sight), fold any pane
   * mutations into its replica, and ship whatever client messages it
   * queued. Cheap when idle — one wasm call returning null.
   */
  private crdtStaleBundleWarned = false;
  private pumpCrdtOutbox(): void {
    const bufferId = this.activeMarkdownBufferId();
    // Tripwire: a markdown tab is live on the rendered chrome but the
    // served wasm predates the co-editing exports. Without this the
    // failure mode is "cursors sync, text doesn't" with zero signal.
    if (
      bufferId &&
      !this.crdtStaleBundleWarned &&
      this.useWasmMarkdown() &&
      this.wasmAdapter?.crdtSupported?.() === false
    ) {
      this.crdtStaleBundleWarned = true;
      console.warn(
        "[crdt] wasm bundle predates live co-editing (no crdt_pump export) — hard-refresh to load the new bundle",
      );
      this.pushInAppNotification(
        "Live editing inactive",
        "This tab loaded an older app bundle. Hard-refresh (Ctrl+Shift+R) to enable live co-editing.",
        "error",
      );
      return;
    }
    const json = this.wasmAdapter?.crdtPump?.(bufferId);
    if (!json) return;
    try {
      const messages = JSON.parse(json) as CrdtClientMessage[];
      console.info(`[crdt] out ${messages.length} message(s)`);
      for (const message of messages) {
        this.options.client.sendCrdt(message);
      }
    } catch (err) {
      console.warn("[crdt] failed to ship outbound batch", err);
    }
  }

  /** Daemon-owned save for the active markdown tab (Ctrl+S): flush
   *  pending edits into the shared doc, then ask the daemon (single
   *  writer) to flush the CONVERGED doc to disk. */
  private saveActiveMarkdown(): void {
    // Bind/flush first so the doc includes everything just typed.
    this.pumpCrdtOutbox();
    if (this.wasmAdapter?.markdownRequestSave?.() === true) {
      this.pumpCrdtOutbox();
    } else {
      this.pushInAppNotification(
        "Not saved",
        "This document isn't connected to the workspace daemon yet.",
        "error",
      );
    }
  }

  /** Presence buffer id for the active CODE pane tab (feeds the code
   *  co-editing pump). Null while any other surface is active. */
  private activeCodeBufferId(): string | null {
    if (this.activeEditorPaneKind() !== "code") return null;
    const tab = this.bufferTabs[this.activeTabIndex];
    if (tab?.kind !== "file" || !tab.path) return null;
    return presenceBufferIdForPath(tab.path, this.options.workspaceRoot);
  }

  /** Code-pane co-editing pump — the code twin of `pumpCrdtOutbox`.
   *  Binds the active code pane's doc (OpenBuffer on first sight),
   *  flushes pane mutations, ships queued client messages. */
  private pumpCodeCrdt(): void {
    const adapter = this.wasmAdapter as {
      codeCrdtPump?: (bufferId: string | null) => string | null;
    };
    if (!adapter?.codeCrdtPump) return;
    const json = adapter.codeCrdtPump(this.activeCodeBufferId());
    if (!json) return;
    try {
      const messages = JSON.parse(json) as CrdtClientMessage[];
      for (const message of messages) {
        this.options.client.sendCrdt(message);
      }
    } catch (err) {
      console.warn("[crdt] failed to ship code outbound batch", err);
    }
  }

  /** Save the active chrome-hosted editor pane. Code panes prefer the
   *  daemon-owned single-writer save (CRDT `SaveBuffer`, markdown
   *  parity); unbound code panes and notebook/draw panes fall back to
   *  a direct daemon `WriteFile` of the pane's serialized payload. */
  private saveActiveEditorPane(skipFormat = false): void {
    const adapter = this.wasmAdapter as {
      editorRequestSave?: () => string;
      editorRequestSaveFormatted?: () => string;
      editorSavePayload?: () => string | null;
      editorMarkSaved?: (payload: string) => void;
    };
    if (!adapter?.editorRequestSave) return;
    // Bind/flush first so a bound doc includes everything just typed.
    this.pumpCodeCrdt();
    // Format-on-save (code panes with a live LSP backend): the wasm
    // side fires the formatter and answers "format"; the save resumes
    // through the `save_after_format` host action with skipFormat.
    if (this.editorLspFormatFallback !== null) {
      clearTimeout(this.editorLspFormatFallback);
      this.editorLspFormatFallback = null;
    }
    const mode = skipFormat
      ? adapter.editorRequestSave()
      : (adapter.editorRequestSaveFormatted?.() ?? adapter.editorRequestSave());
    if (mode === "format") {
      // Safety net: if the formatter's reply never lands (old daemon,
      // dropped frame), finish the save unformatted instead of
      // hanging the user's Ctrl+S.
      this.editorLspFormatFallback = setTimeout(() => {
        this.editorLspFormatFallback = null;
        this.saveActiveEditorPane(true);
      }, 3000);
      return;
    }
    if (mode === "crdt") {
      // The SaveBuffer message is queued in the wasm outbound — ship it.
      this.pumpCodeCrdt();
      return;
    }
    if (mode !== "host") return;
    const tab = this.bufferTabs[this.activeTabIndex];
    const path = tab?.kind === "file" ? tab.path : null;
    const payload = adapter.editorSavePayload?.();
    if (!path || payload == null) {
      this.pushInAppNotification(
        "Not saved",
        "The editor pane has no writable document.",
        "error",
      );
      return;
    }
    const requestId = nextFileReadRequestId++;
    this.pendingServiceMappers.set(requestId, (reply) => {
      if ("FileWritten" in reply) {
        adapter.editorMarkSaved?.(payload);
        this.pushInAppNotification("Saved", `Wrote ${path}`, "info");
        this.scheduleDraw();
        return reply.FileWritten.bytes_written;
      }
      if ("Error" in reply) {
        this.pushInAppNotification("Save failed", reply.Error.message, "error");
      }
      return null;
    });
    this.options.client.sendFiles(
      requestId,
      {
        WriteFile: {
          path: this.toDaemonWorkspacePath(path),
          bytes: Array.from(new TextEncoder().encode(payload)),
        },
      },
      this.options.workspaceRoot ?? null,
    );
  }

  /**
   * Outbound presence pump. Computes the buffer + cursor the local
   * user is "in" and lets the coalescing publisher decide whether
   * anything goes on the wire (≤~13Hz on movement, 4s heartbeats,
   * `ClearPresence` on buffer switch / focus loss).
   */
  private pumpPresence(): void {
    const messages = this.presencePublisher.tick(
      this.currentPresenceTarget(),
      Date.now(),
    );
    for (const message of messages) {
      this.options.client.sendCrdt(message);
    }
  }

  /**
   * Where is the local user? Markdown tabs publish their caret (or
   * reading position on the legacy DOM viewer). Other tabs publish
   * workspace membership without claiming a file cursor. This panel is
   * workspace-scoped, so dispose() remains the true leave operation.
   */
  private currentPresenceTarget(): ActivePresenceTarget | null {
    const markdownBufferId = this.activeMarkdownBufferId();
    if (markdownBufferId) {
      // The wasm markdown pane has a REAL caret now (Live Preview is
      // editable on web). Publish it; the top-visible-line fallback is
      // a relic of the read-only DOM viewer and painted this client's
      // caret in the wrong place on every other screen.
      const cursor = this.useWasmMarkdown()
        ? this.wasmAdapter?.markdownCursor?.()
        : null;
      return {
        bufferId: markdownBufferId,
        cursor: cursor
          ? { line: cursor.line, column: cursor.columnUtf16, offset: null }
          : { line: this.topVisibleMarkdownLine(), column: 0, offset: null },
        selection: null,
        insert: cursor?.insert ?? false,
      };
    }
    // Native code pane tabs publish the pane's REAL caret (same wire
    // shape as markdown), so peers see this client where it types.
    const codeBufferId = this.activeCodeBufferId();
    if (codeBufferId) {
      const cursor = (this.wasmAdapter as {
        editorCursor?: () => {
          line: number;
          columnUtf16: number;
          insert?: boolean;
        } | null;
      })?.editorCursor?.();
      if (cursor) {
        return {
          bufferId: codeBufferId,
          cursor: { line: cursor.line, column: cursor.columnUtf16, offset: null },
          selection: null,
          insert: cursor.insert ?? false,
        };
      }
    }
    // Other editor-like file tabs (notebook/draw) retain workspace
    // membership like terminal/agent tabs.
    return {
      bufferId: WORKSPACE_PRESENCE_BUFFER_ID,
      cursor: { line: 0, column: 0, offset: null },
      selection: null,
      insert: false,
    };
  }

  private activeMarkdownBufferId(): string | null {
    if (!this.activeTabIsMarkdown()) return null;
    const tab = this.bufferTabs[this.activeTabIndex];
    if (tab?.kind !== "file" || !tab.path) return null;
    return presenceBufferIdForPath(tab.path, this.options.workspaceRoot);
  }

  /** First source line whose rendered block is visible at the current
   * markdown scroll position — the web reader's "cursor". */
  private topVisibleMarkdownLine(): number {
    const scrollTop = this.markdownLayer.scrollTop;
    const blocks =
      this.markdownLayer.querySelectorAll<HTMLElement>("[data-md-line]");
    for (const el of blocks) {
      if (el.offsetTop + el.offsetHeight > scrollTop) {
        const line = Number(el.dataset.mdLine);
        return Number.isFinite(line) ? Math.max(0, line) : 0;
      }
    }
    return 0;
  }

  /** Feed collaborator carets into the chrome-hosted CODE pane so the
   *  shared renderer draws them (colored bar + name flag), the code
   *  twin of the markdown remote-caret push. */
  private syncCodePanePresence(): void {
    const adapter = this.wasmAdapter as {
      editorSetRemoteCursors?: (peers: unknown) => void;
    };
    if (!adapter?.editorSetRemoteCursors) return;
    const bufferId = this.activeCodeBufferId();
    if (!bufferId) {
      adapter.editorSetRemoteCursors([]);
      return;
    }
    const peers = this.remotePresence.cursorsFor(bufferId).map((p) => ({
      name: p.display_name,
      color: [p.color.r, p.color.g, p.color.b],
      rainbow: p.rainbow ?? false,
      insert: p.insert ?? true,
      line: p.cursor.line,
      col_utf16: p.cursor.column,
    }));
    adapter.editorSetRemoteCursors(peers);
  }

  /** Repaint the markdown DOM overlay from the presence store. */
  private syncMarkdownPresenceOverlay(): void {
    this.syncCodePanePresence();
    const bufferId = this.activeMarkdownBufferId();
    if (bufferId && this.useWasmMarkdown() && this.activeTabIsMarkdown()) {
      // Real-renderer path: feed peers into the wasm pane so the shared
      // renderer draws exact carets + roster (same as desktop).
      this.markdownPresenceOverlay.clear();
      const peers = this.remotePresence.cursorsFor(bufferId).map((p) => ({
        name: p.display_name,
        color: [p.color.r, p.color.g, p.color.b],
        rainbow: p.rainbow ?? false,
        line: p.cursor.line,
        col_utf16: p.cursor.column,
      }));
      (this.wasmAdapter as { setMarkdownRemoteCursors?: (peers: unknown) => void })
        ?.setMarkdownRemoteCursors?.(peers);
      return;
    }
    if (!bufferId || this.markdownLayer.hidden) {
      this.markdownPresenceOverlay.clear();
      return;
    }
    this.markdownPresenceOverlay.sync(this.remotePresence.cursorsFor(bufferId));
  }

  /** Feed the wasm file tree's `path -> peers` presence index so tree
   *  rows light collaborator avatars (desktop parity:
   *  `Screen::rebuild_file_tree_presence_index`). EVENT-DRIVEN:
   *  called from the same presence-store updates that repaint the
   *  markdown carets — never per frame. */
  private syncFileTreePresence(): void {
    this.wasmAdapter?.setPresenceIndex?.(
      this.remotePresence.avatarPeersByBuffer(),
    );
  }

  private bindEditorSurface(externalId: number, path: string | null): void {
    const sessionId = this.workspaceSessionId ?? this.options.sessionId;
    this.options.client.bindEditorSurface(
      this.editorSurfaceId(externalId),
      sessionId,
      path,
    );
  }

  private closeEditorSurface(externalId: number): void {
    const surfaceId = this.editorSurfaceId(externalId);
    this.editorResizeBySurface.delete(surfaceId);
    this.options.client.closeEditorSurface(surfaceId);
  }

  private bindEditorSurfaceForTab(externalId: number, tabIndex: number): void {
    const tab = this.bufferTabs[tabIndex];
    if (!this.isEditorLikeTab(tab)) return;
    this.bindEditorSurface(externalId, tab?.kind === "file" ? tab.path ?? null : null);
  }

  private isEditorLikeTab(tab: WebBufferTab | undefined): boolean {
    return (
      (tab?.kind === "file" && !isMarkdownPath(tab.path)) ||
      tab?.kind === "neoism-agent"
    );
  }

  private activeTabIsMarkdown(): boolean {
    const tab = this.bufferTabs[this.activeTabIndex];
    return tab?.kind === "file" && isMarkdownPath(tab.path);
  }

  /** Which chrome-hosted editor pane serves the active tab: "code" /
   *  "notebook" / "draw", or null for terminal/markdown/agent tabs
   *  (and for stale bundles without the editor-pane exports). Gated on
   *  the rendered wasm chrome, same as the markdown pane path. */
  private activeEditorPaneKind(): string | null {
    if (!this.useWasmMarkdown()) return null;
    const tab = this.bufferTabs[this.activeTabIndex];
    if (tab?.kind !== "file" || !tab.path || isMarkdownPath(tab.path)) return null;
    const adapter = this.wasmAdapter as {
      editorActiveKind?: () => string | null;
    };
    return adapter?.editorActiveKind?.() ?? null;
  }

  private activeSurface(): "terminal" | "editor" | "agent" | "markdown" {
    if (this.activeTabIsMarkdown()) return "markdown";
    const raw = this.wasmAdapter?.activeSurface?.();
    return raw === "agent" || raw === "editor" ? raw : "terminal";
  }

  private syncMarkdownLayerBounds(): void {
    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal) {
      this.markdownLayer.style.left = "";
      this.markdownLayer.style.top = "";
      this.markdownLayer.style.width = "";
      this.markdownLayer.style.height = "";
      return;
    }
    this.markdownLayer.style.left = `${Math.max(0, terminal.x)}px`;
    this.markdownLayer.style.top = `${Math.max(0, terminal.y)}px`;
    this.markdownLayer.style.width = `${Math.max(0, terminal.w)}px`;
    this.markdownLayer.style.height = `${Math.max(0, terminal.h)}px`;
  }

  private setMarkdownLayerVisible(visible: boolean): void {
    if (visible && this.useWasmMarkdown()) {
      // Real-renderer path: the wasm chrome draws the markdown pane on
      // the canvas — showing the (possibly empty) DOM layer over it
      // blacks out the document. Every show-path funnels through here,
      // so gate it once: the DOM article only appears on the fallback
      // (stub / non-rendered) adapters.
      visible = false;
    }
    this.markdownLayer.hidden = !visible;
    this.canvas.setAttribute("aria-hidden", visible ? "true" : "false");
    if (visible) {
      this.syncMarkdownLayerBounds();
      const bufferId = this.activeMarkdownBufferId();
      if (bufferId) {
        this.requestPresenceSnapshot(bufferId);
      }
    }
    // Visibility flips are buffer enters/exits for the presence
    // plane: publish the reading position (or clear it) right away
    // and repaint remote carets for the now-active buffer.
    this.syncMarkdownPresenceOverlay();
    this.pumpPresence();
  }

  private clearMarkdownLayer(): void {
    this.markdownLayer.replaceChildren();
    this.markdownLayerTabIndex = null;
    this.setMarkdownLayerVisible(false);
  }

  /** Keep frames flowing while the markdown pane's eased scroll (wheel
   *  inertia, follow-cursor) settles — the web only draws on events, so
   *  without this the scroll target is set but never animates to. */
  private markdownAnimationPumping = false;
  private pumpMarkdownAnimation(): void {
    if (this.markdownAnimationPumping) return;
    const adapter = this.wasmAdapter as { markdownTick?: () => boolean };
    if (!adapter?.markdownTick) return;
    this.markdownAnimationPumping = true;
    const step = () => {
      if (
        this.activeTabIsMarkdown() &&
        this.useWasmMarkdown() &&
        adapter.markdownTick!()
      ) {
        this.scheduleDraw();
        requestAnimationFrame(step);
      } else {
        this.markdownAnimationPumping = false;
        this.scheduleDraw();
      }
    };
    requestAnimationFrame(step);
  }

  /** True when the wasm chrome is live and rendering — .md tabs then use
   *  the REAL shared markdown pane (Live Preview, remote carets, roster)
   *  drawn on the canvas, and the DOM article overlay stays hidden. The
   *  DOM path remains the fallback for the stub/non-rendered adapters. */
  private useWasmMarkdown(): boolean {
    return !!(
      this.wasmAdapter?.isChrome?.() &&
      this.wasmAdapter?.isRendered?.()
    );
  }

  private renderMarkdownLayer(tabIdx: number, source: string): void {
    this.markdownLayerTabIndex = tabIdx;
    if (this.useWasmMarkdown()) {
      // Content already reached the wasm pane via the tab-content push
      // (set_markdown_content); never double-render the DOM article.
      this.markdownLayer.replaceChildren();
      this.setMarkdownLayerVisible(false);
      this.syncMarkdownPresenceOverlay();
      return;
    }
    this.markdownLayer.replaceChildren(renderMarkdownDocument(source));
    this.setMarkdownLayerVisible(
      this.activeTabIndex === tabIdx && this.activeTabIsMarkdown(),
    );
  }

  private syncActiveMarkdownLayer(): void {
    if (!this.activeTabIsMarkdown()) {
      this.setMarkdownLayerVisible(false);
      return;
    }
    if (this.markdownLayerTabIndex === this.activeTabIndex) {
      this.setMarkdownLayerVisible(true);
    } else {
      this.markdownLayer.textContent = "Loading markdown…";
      this.setMarkdownLayerVisible(true);
    }
  }

  private showMarkdownTab(): void {
    this.syncActiveMarkdownLayer();
  }

  private openFileTabContent(path: string): void {
    if (isMarkdownPath(path)) {
      this.showMarkdownTab();
    } else {
      this.sendEditorOpenBuffer(path);
    }
  }

  private syncPaneRouteState(panes: WebPaneRect[]): void {
    const live = new Set<number>();
    for (const pane of panes) {
      live.add(pane.external_id);
      if (!this.paneTabState.has(pane.external_id)) {
        this.paneTabState.set(pane.external_id, {
          tabIndices: [],
          activeTabIndex: null,
        });
      }
    }
    for (const externalId of [...this.paneTabState.keys()]) {
      if (!live.has(externalId)) {
        this.paneTabState.delete(externalId);
      }
    }
  }

  private prunePaneTabIndices(): void {
    for (const state of this.paneTabState.values()) {
      state.tabIndices = state.tabIndices.filter(
        (idx) =>
          idx >= 0 &&
          idx < this.bufferTabs.length &&
          this.isEditorLikeTab(this.bufferTabs[idx]),
      );
      if (
        state.activeTabIndex === null ||
        state.activeTabIndex < 0 ||
        state.activeTabIndex >= this.bufferTabs.length ||
        !state.tabIndices.includes(state.activeTabIndex)
      ) {
        state.activeTabIndex = state.tabIndices[0] ?? null;
      }
    }
  }

  private removeTabFromPaneState(removed: number): void {
    for (const state of this.paneTabState.values()) {
      state.tabIndices = state.tabIndices
        .filter((idx) => idx !== removed)
        .map((idx) => (idx > removed ? idx - 1 : idx));
      if (state.activeTabIndex === removed) {
        state.activeTabIndex = state.tabIndices[0] ?? null;
      } else if (state.activeTabIndex !== null && state.activeTabIndex > removed) {
        state.activeTabIndex -= 1;
      }
    }
    this.prunePaneTabIndices();
  }

  private moveTabInPaneState(from: number, to: number): void {
    const rebase = (idx: number): number => {
      if (idx === from) return to;
      if (from < to && idx > from && idx <= to) return idx - 1;
      if (from > to && idx >= to && idx < from) return idx + 1;
      return idx;
    };
    for (const state of this.paneTabState.values()) {
      state.tabIndices = state.tabIndices.map(rebase);
      if (state.activeTabIndex !== null) {
        state.activeTabIndex = rebase(state.activeTabIndex);
      }
    }
    this.prunePaneTabIndices();
  }

  private assignTabToPane(externalId: number, tabIndex: number): void {
    if (!this.isEditorLikeTab(this.bufferTabs[tabIndex])) return;
    const state =
      this.paneTabState.get(externalId) ??
      { tabIndices: [], activeTabIndex: null };
    if (!state.tabIndices.includes(tabIndex)) {
      state.tabIndices.push(tabIndex);
    }
    state.activeTabIndex = tabIndex;
    this.paneTabState.set(externalId, state);
  }

  private assignActiveTabToFocusedEditorPane(): void {
    const externalId = this.activePaneExternalId();
    if (externalId === null) return;
    this.assignTabToPane(externalId, this.activeTabIndex);
    this.bindEditorSurfaceForTab(externalId, this.activeTabIndex);
  }

  private activatePaneExternalId(externalId: number, openEditorBuffer: boolean): void {
    const state = this.paneTabState.get(externalId);
    const tabIndex = state?.activeTabIndex;
    const editorTabBound =
      typeof tabIndex === "number" &&
      tabIndex >= 0 &&
      tabIndex < this.bufferTabs.length &&
      this.isEditorLikeTab(this.bufferTabs[tabIndex]);
    // Terminal pane: its bound session takes over the main grid (tab
    // switch + replay + PTY viewport), while the pane terminal keeps
    // rendering it in place.
    const paneSession = this.paneSessionIds.get(externalId);
    if (!editorTabBound && paneSession) {
      if (paneSession !== this.activePtySessionId()) {
        this.activatePtySession(paneSession);
      }
      this.syncPaneTerminals();
      return;
    }
    if (editorTabBound && typeof tabIndex === "number") {
      // A pane hosting an editor tab must not keep a stale terminal
      // binding — otherwise its pane terminal would paint under/over
      // the editor surface.
      if (this.paneSessionIds.delete(externalId)) {
        this.wasmAdapter?.removePaneTerminal?.(externalId);
      }
      this.activeTabIndex = tabIndex;
      this.wasmAdapter?.setActiveTab?.(tabIndex);
      const tab = this.bufferTabs[tabIndex];
      if (openEditorBuffer && tab?.kind === "file" && tab.path) {
        this.openFileTabContent(tab.path);
      } else {
        this.bindEditorSurfaceForTab(externalId, tabIndex);
      }
      this.replayBufferTabs();
      return;
    }

    const fallback = this.bufferTabs.findIndex((tab) => this.isEditorLikeTab(tab));
    if (fallback >= 0) {
      this.assignTabToPane(externalId, fallback);
      this.activeTabIndex = fallback;
      this.wasmAdapter?.setActiveTab?.(fallback);
      const tab = this.bufferTabs[fallback];
      if (openEditorBuffer && tab?.kind === "file" && tab.path) {
        this.openFileTabContent(tab.path);
      } else {
        this.bindEditorSurfaceForTab(externalId, fallback);
      }
      this.replayBufferTabs();
    }
  }

  private ensureSessionLayoutState(): void {
    if (this.sessionLayoutStateJson || !this.wasmAdapter?.applySessionLayoutPolicy) {
      return;
    }
    const result = parseSessionLayoutPolicyResult(
      this.wasmAdapter.applySessionLayoutPolicy(null, "init_editor", null, "Editor 1", 1),
    );
    if (!result) return;
    this.sessionLayoutStateJson = result.state_json;
    this.paneLayoutPanes = result.panes;
    this.syncPaneRouteState(result.panes);
    this.assignActiveTabToFocusedEditorPane();
    this.nextWebPaneId = Math.max(2, ...result.active_external_ids.map((id) => id + 1));
    this.renderPaneLayoutOverlay();
  }

  private applySessionLayoutPolicy(
    operation: string,
    axis?: WebPaneSplitAxis | WebPaneResizeDirection | null,
    title?: string | null,
    externalId?: number | null,
  ): WebSessionLayoutPolicyResult | null {
    const adapter = this.wasmAdapter;
    if (!adapter?.applySessionLayoutPolicy) return null;
    this.ensureSessionLayoutState();
    // Capture the outgoing focus BEFORE the op so a terminal pane
    // losing focus keeps its session bound (and keeps rendering live).
    const prevFocusedPane =
      this.paneLayoutPanes.find((pane) => pane.focused)?.external_id ?? null;
    const prevSession = this.activePtySessionId();
    const result = parseSessionLayoutPolicyResult(
      adapter.applySessionLayoutPolicy(
        this.sessionLayoutStateJson,
        operation,
        axis ?? null,
        title ?? null,
        externalId ?? null,
      ),
    );
    if (!result) return null;
    this.sessionLayoutStateJson = result.state_json;
    this.paneLayoutPanes = result.panes;
    this.syncPaneRouteState(result.panes);
    if (
      prevFocusedPane !== null &&
      prevSession &&
      result.focused_external_id !== prevFocusedPane &&
      result.panes.some((pane) => pane.external_id === prevFocusedPane)
    ) {
      this.paneSessionIds.set(prevFocusedPane, prevSession);
    }
    this.nextWebPaneId = Math.max(
      this.nextWebPaneId,
      1,
      ...result.active_external_ids.map((id) => id + 1),
    );
    this.syncPaneTerminals();
    this.renderPaneLayoutOverlay();
    return result;
  }

  private splitEditorPane(axis: WebPaneSplitAxis): void {
    const surface = this.activeSurface();
    if (surface === "terminal") {
      this.splitTerminalPane(axis);
      return;
    }
    if (surface !== "editor") {
      return;
    }
    const paneId = this.nextWebPaneId++;
    const tabToCarry = this.isEditorLikeTab(this.bufferTabs[this.activeTabIndex])
      ? this.activeTabIndex
      : null;
    const result = this.applySessionLayoutPolicy(
      "split",
      axis,
      `Editor ${paneId}`,
      paneId,
    );
    if (!result) {
      this.nextWebPaneId -= 1;
    } else if (tabToCarry !== null) {
      this.assignTabToPane(paneId, tabToCarry);
      this.activatePaneExternalId(paneId, false);
      this.bindEditorSurfaceForTab(paneId, tabToCarry);
    }
  }

  /** Terminal split (desktop parity): the focused pane keeps the
   *  current shell — bound through `paneSessionIds` so it renders
   *  live in its pane — and the new (focused) pane spawns a fresh
   *  shell that attaches through the pending-spawn queue. */
  private splitTerminalPane(axis: WebPaneSplitAxis): void {
    this.ensureSessionLayoutState();
    const currentSession = this.activePtySessionId();
    const focusedPane =
      this.paneLayoutPanes.find((pane) => pane.focused)?.external_id ?? null;
    const paneId = this.nextWebPaneId++;
    const result = this.applySessionLayoutPolicy(
      "split_terminal",
      axis,
      "Terminal",
      paneId,
    );
    if (!result) {
      this.nextWebPaneId -= 1;
      return;
    }
    if (currentSession && focusedPane !== null) {
      this.paneSessionIds.set(focusedPane, currentSession);
    }
    this.syncPaneTerminals();
    this.spawnTerminalTab({ paneExternalId: paneId });
  }

  private focusEditorPane(previous: boolean): void {
    const surface = this.activeSurface();
    if (surface !== "editor" && surface !== "terminal") {
      return;
    }
    const result = this.applySessionLayoutPolicy(previous ? "focus_prev" : "focus_next");
    if (typeof result?.focused_external_id === "number") {
      this.activatePaneExternalId(result.focused_external_id, true);
    }
  }

  private closeEditorPaneOrTab(): void {
    const surface = this.activeSurface();
    if (surface === "terminal" && this.paneLayoutPanes.length > 1) {
      // Close the focused TERMINAL pane (its tab + session survive in
      // the strip; only the split cell collapses).
      const closingPane =
        this.paneLayoutPanes.find((pane) => pane.focused)?.external_id ?? null;
      const result = this.applySessionLayoutPolicy("close_focused");
      if (result) {
        if (closingPane !== null) {
          this.paneSessionIds.delete(closingPane);
          this.wasmAdapter?.removePaneTerminal?.(closingPane);
        }
        if (typeof result.focused_external_id === "number") {
          this.activatePaneExternalId(result.focused_external_id, true);
        }
      }
      return;
    }
    if (surface !== "editor") {
      this.closeActiveBufferTab();
      return;
    }
    const before = this.paneLayoutPanes.length;
    const closingExternalId = this.activePaneExternalId();
    const result = before > 1 ? this.applySessionLayoutPolicy("close_focused") : null;
    if (result || before > 1) {
      if (closingExternalId !== null) {
        this.closeEditorSurface(closingExternalId);
      }
      if (typeof result?.focused_external_id === "number") {
        this.activatePaneExternalId(result.focused_external_id, true);
      }
      return;
    }
    this.closeActiveBufferTab();
  }

  private renderPaneLayoutOverlay(): void {
    // The decorative DOM pane-chip overlay is gone: the shared Rust
    // PaneGrid paints dividers, the focused-pane outline, and the
    // drag-to-split preview directly on the canvas (Chrome::draw), and
    // pane surfaces render per-rect through the wasm frame path. This
    // stub survives only so the many historical call sites keep
    // triggering a repaint of the canvas-side pane chrome.
    this.scheduleDraw();
  }

  private focusEditorPaneByExternalId(externalId: number): void {
    const result = this.applySessionLayoutPolicy("focus_external", null, null, externalId);
    if (typeof result?.focused_external_id === "number") {
      this.activatePaneExternalId(result.focused_external_id, true);
    }
    this.focus();
    this.scheduleDraw();
  }

  private moveEditorDivider(direction: WebPaneResizeDirection): void {
    const surface = this.activeSurface();
    if (surface === "editor" || surface === "terminal") {
      this.applySessionLayoutPolicy("resize", direction);
    }
  }

  /** Theme names for the pickers/cycling: the wasm bridge's full
   *  shared catalog (builtins + bundled NvChad set, same list the
   *  desktop offers) when loaded, else the builtin fallback four. */
  private ideThemeNames(): readonly string[] {
    const catalog = this.wasmAdapter?.allIdeThemes?.() ?? [];
    return catalog.length > 0 ? catalog.map((entry) => entry.name) : WEB_IDE_THEMES;
  }

  private setIdeTheme(name: string): void {
    this.activeThemeName = name;
    this.wasmAdapter?.setIdeTheme?.(name);
    // Presence broadcasts MY cursor color — switching themes updates
    // what peers see within a heartbeat.
    this.applyPresenceThemeColor(name);
  }

  private cycleIdeTheme(delta: number): void {
    const names = this.ideThemeNames();
    const current = names.indexOf(this.activeThemeName);
    const next = (current + delta + names.length) % names.length;
    this.setIdeTheme(names[next]);
  }

  private drainAgentTabOpens(): void {
    const count = this.wasmAdapter?.drainAgentTabOpens?.() ?? 0;
    for (let i = 0; i < count; i += 1) {
      this.openNeoismAgentTab();
    }
  }

  private replayBufferTabs(): void {
    let terminalOrdinal = 0;
    const tabPayload = this.bufferTabs.map((t) => ({
      title: this.stableTabTitle(
        t,
        t.kind === "terminal" ? ++terminalOrdinal : undefined,
      ),
      path: t.path ?? null,
      kind: t.kind,
      session_id: t.sessionId ?? null,
      neoism_agent_route_id: t.neoismAgentRouteId ?? null,
    }));
    this.wasmAdapter?.setBufferTabs?.(
      JSON.stringify(tabPayload),
      this.activeTabIndex,
    );
    this.wasmAdapter?.setActiveTab?.(this.activeTabIndex);
    this.syncActiveBreadcrumbs();
    // Per-pane strips mirror slices of this list — keep them (and the
    // pane surface descriptors) fresh on every replay.
    this.syncPaneTerminals();
    this.renderPaneLayoutOverlay();
    this.notifyBufferTabsChanged();
  }

  /** Tell the host the tab strip changed (deduped) so it can publish
   *  this workspace's tabs into the daemon tree — that's what lets a
   *  desktop adopt this workspace WITH its buffers and sessions. */
  private notifyBufferTabsChanged(): void {
    if (!this.options.onBufferTabsChanged) return;
    let terminalOrdinal = 0;
    const snapshot = this.bufferTabs.map((tab, index) => ({
      title: this.stableTabTitle(
        tab,
        tab.kind === "terminal" ? ++terminalOrdinal : undefined,
      ),
      kind: tab.kind,
      path: tab.path ?? null,
      sessionId: tab.sessionId ?? null,
      active: index === this.activeTabIndex,
    }));
    const fingerprint = JSON.stringify(snapshot);
    if (fingerprint === this.lastBufferTabsFingerprint) return;
    this.lastBufferTabsFingerprint = fingerprint;
    this.options.onBufferTabsChanged(snapshot);
  }

  setWorkspaceIslandTabs(payloadJson: string): void {
    this.wasmAdapter?.setWorkspaceIslandTabs?.(payloadJson);
    this.scheduleDraw();
  }

  private drainWorkspaceIslandIntents(): void {
    const raw = this.wasmAdapter?.drainWorkspaceIslandIntents?.();
    if (!Array.isArray(raw)) return;
    for (const item of raw) {
      if (!item || typeof item !== "object") continue;
      const rec = item as Record<string, unknown>;
      const kind = rec.kind;
      if (kind !== "activate" && kind !== "context_menu" && kind !== "open_workspaces") {
        continue;
      }
      this.options.onWorkspaceIslandIntent?.({
        kind,
        workspace_id: typeof rec.workspace_id === "string" ? rec.workspace_id : null,
        x: typeof rec.x === "number" ? rec.x : null,
        y: typeof rec.y === "number" ? rec.y : null,
      });
    }
  }

  private syncActiveBreadcrumbs(): void {
    const tab = this.bufferTabs[this.activeTabIndex];
    if (tab?.kind === "file" && tab.path) {
      this.syncBreadcrumbsForPath(tab.path);
    } else {
      this.wasmAdapter?.setBreadcrumbs?.("[]");
    }
  }

  private syncBreadcrumbsForPath(path: string): void {
    const root = this.wasmAdapter?.fileTreeWorkspaceRoot?.() ?? "";
    let displayPath = path;
    if (
      root &&
      (path === root || path.startsWith(`${root}/`) || path.startsWith(`${root}\\`))
    ) {
      displayPath = path.slice(root.length).replace(/^[\\/]+/, "");
    }
    const parts = displayPath.split(/[\\/]+/).filter(Boolean);
    const fallback = path.split(/[\\/]+/).filter(Boolean).pop();
    const labels = parts.length > 0 ? parts : fallback ? [fallback] : [];
    this.wasmAdapter?.setBreadcrumbs?.(
      JSON.stringify(labels.map((label) => ({ label, path: null }))),
    );
  }

  private openNeoismAgentTab(): void {
    const existing = this.bufferTabs.findIndex((t) => t.kind === "neoism-agent");
    if (existing >= 0) {
      this.activeTabIndex = existing;
      // Explicitly re-invoking "Neoism Agent" means the user wants a
      // FRESH chat, not a teleport back to the old conversation. The
      // previous session stays reachable via /sessions.
      if (this.wasmAdapter?.agentHasConversation?.()) {
        const directory = this.wasmAdapter.fileTreeWorkspaceRoot?.() ?? null;
        this.wasmAdapter.agentNewThread?.(directory);
      }
    } else {
      this.bufferTabs.push({
        title: "Neoism",
        kind: "neoism-agent",
        neoismAgentRouteId: this.neoismAgentRouteId,
      });
      this.activeTabIndex = this.bufferTabs.length - 1;
    }
    this.assignActiveTabToFocusedEditorPane();
    this.replayBufferTabs();
    this.wasmAdapter?.agentSetInput?.(this.agentInput);
    this.ensureNeoismAgentAttached();
    this.scheduleDraw();
  }

  private ensureNeoismAgentAttached(): void {
    // Re-attach on every agent-tab open (debounced) instead of once per
    // panel lifetime: the first attach can race the embedded
    // agent-server's boot, and a once-guard left the pane stuck with
    // "server default" chips and empty catalogs until a full reload.
    const now = Date.now();
    if (now - this.agentLastAttachAt < 2000) return;
    const adapter = this.wasmAdapter;
    if (!adapter?.agentAttach) return;
    this.agentLastAttachAt = now;
    const directory = adapter.fileTreeWorkspaceRoot?.() ?? null;
    adapter.agentAttach(directory);
  }

  private drainFileTreeOpens(): void {
    const opens = this.wasmAdapter?.drainFileTreeOpens?.();
    if (!opens || !Array.isArray(opens) || opens.length === 0) return;
    this.openActivatedPaths(opens.filter(
      (raw): raw is string => typeof raw === "string" && raw.length > 0,
    ));
  }

  /** Open daemon paths as buffer tabs — the shared pipeline behind
   *  file-tree, git-panel, and notes-sidebar activations. */
  private openActivatedPaths(opens: string[]): void {
    if (opens.length === 0) return;
    let changed = false;
    for (const raw of opens) {
      const fileName = raw.split(/[\\/]/).pop() ?? raw;
      const existing = this.bufferTabs.findIndex((t) => t.path === raw);
      if (existing >= 0) {
        this.activeTabIndex = existing;
        this.requestFileContent(raw, this.activeTabIndex);
        this.openFileTabContent(raw);
      } else {
        this.bufferTabs.push({ title: fileName, kind: "file", path: raw });
        this.activeTabIndex = this.bufferTabs.length - 1;
        // Kick off a daemon read so the file-viewer pane has content
        // to render when the user lands on the new tab. The reply
        // routes through `pendingServiceMappers` set below.
        this.requestFileContent(raw, this.activeTabIndex);
        // Also bind the editor surface + ship the OpenBuffer envelope
        // (fire-and-forget; the future native CodePane will consume
        // the replies).
        this.openFileTabContent(raw);
      }
      this.assignActiveTabToFocusedEditorPane();
      changed = true;
    }
    if (changed) {
      if (this.isMobileViewport()) {
        this.wasmAdapter?.hideFileTree?.();
      }
      this.replayBufferTabs();
      this.activateCurrentTabContents();
    }
  }

  /// File-tree CRUD context menu (task #68).
  ///
  /// Right-clicking inside the file-tree panel pops a small DOM menu
  /// with Rename / New File / New Folder / Delete entries. The wasm
  /// bridge's `file_tree_context_target` does the hit test and tells
  /// us which row the user clicked (and its parent directory for the
  /// "New ..." targets). Selection is also nudged onto the hit row so
  /// the F2 / Delete keyboard shortcuts act on the same entry.
  ///
  /// Right-clicks outside the file-tree fall through to the browser's
  /// native menu (we only call `preventDefault` when the hit landed in
  /// the panel) so terminal selections / link menus still work.
  private handleContextMenu(event: MouseEvent): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    const { x, y } = this.canvasLogicalPoint(event);
    if (adapter.workspaceIslandContextClick?.(x, y)) {
      event.preventDefault();
      this.drainWorkspaceIslandIntents();
      this.scheduleDraw();
      return;
    }
    if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
      // Right-click on the markdown surface: spelling menu for the
      // word under the pointer (desktop markdown context spelling).
      if (adapter.markdownSpellingMenuAt?.(x, y)) {
        event.preventDefault();
        this.drainMarkdownPointerEffects();
        this.scheduleDraw();
        return;
      }
    }
    const layout = adapter.chromeLayout?.();
    const treeRect = layout?.file_tree ?? null;
    if (!treeRect || !pointInRect({ x, y }, treeRect)) return;
    event.preventDefault();
    // Pull the target row from the wasm bridge. `null` means we
    // clicked inside the panel but past the last row — fall back to
    // the workspace root so "New File / New Folder" still works.
    const target = adapter.fileTreeContextTarget?.(x, y) ?? null;
    const workspaceRoot = adapter.fileTreeWorkspaceRoot?.() ?? "";
    const parentDir = target?.parent_dir ?? workspaceRoot;
    if (!parentDir) return;
    this.openFileTreeMenu(event.clientX, event.clientY, {
      target,
      parentDir,
    });
  }

  private dismissFileTreeMenu(): void {
    if (this.fileTreeMenuDismiss) {
      this.fileTreeMenuDismiss();
      this.fileTreeMenuDismiss = null;
    }
    if (this.fileTreeMenuEl) {
      this.fileTreeMenuEl.remove();
      this.fileTreeMenuEl = null;
    }
  }

  private openFileTreeMenu(
    clientX: number,
    clientY: number,
    ctx: {
      target: FileTreeContextTarget | null;
      parentDir: string;
    },
  ): void {
    this.dismissFileTreeMenu();
    const menu = document.createElement("div");
    menu.className = "file-tree-context-menu";
    Object.assign(menu.style, {
      position: "fixed",
      left: `${clientX}px`,
      top: `${clientY}px`,
      zIndex: "10000",
      minWidth: "160px",
      padding: "4px 0",
      background: "#1f2228",
      color: "#e6e6e6",
      border: "1px solid #3a3f47",
      borderRadius: "6px",
      boxShadow: "0 8px 24px rgba(0, 0, 0, 0.45)",
      fontFamily: "system-ui, -apple-system, sans-serif",
      fontSize: "12.5px",
      userSelect: "none",
    });

    const hasTarget = !!ctx.target?.path;
    const items: Array<{ label: string; enabled: boolean; run: () => void }> = [
      {
        label: "New File",
        enabled: true,
        run: () => void this.promptCreateFile(ctx.parentDir),
      },
      {
        label: "New Folder",
        enabled: true,
        run: () => void this.promptCreateDir(ctx.parentDir),
      },
      {
        label: "Rename",
        enabled: hasTarget,
        run: () => {
          if (ctx.target?.path) void this.promptRename(ctx.target.path);
        },
      },
      {
        label: "Delete",
        enabled: hasTarget,
        run: () => {
          if (ctx.target?.path) {
            void this.confirmDelete(ctx.target.path, ctx.target.is_dir);
          }
        },
      },
    ];

    for (const item of items) {
      const row = document.createElement("div");
      row.textContent = item.label;
      Object.assign(row.style, {
        padding: "6px 14px",
        cursor: item.enabled ? "pointer" : "default",
        color: item.enabled ? "#e6e6e6" : "#6b7079",
      });
      if (item.enabled) {
        row.addEventListener("mouseenter", () => {
          row.style.background = "#2d323b";
        });
        row.addEventListener("mouseleave", () => {
          row.style.background = "";
        });
        row.addEventListener("click", () => {
          this.dismissFileTreeMenu();
          item.run();
        });
      }
      menu.appendChild(row);
    }

    document.body.appendChild(menu);
    this.fileTreeMenuEl = menu;

    // Clamp inside the viewport so the menu isn't clipped off-screen.
    queueMicrotask(() => {
      const rect = menu.getBoundingClientRect();
      const overflowX = rect.right - window.innerWidth;
      const overflowY = rect.bottom - window.innerHeight;
      if (overflowX > 0) {
        menu.style.left = `${Math.max(0, clientX - overflowX - 4)}px`;
      }
      if (overflowY > 0) {
        menu.style.top = `${Math.max(0, clientY - overflowY - 4)}px`;
      }
    });

    // Dismiss on outside click / Escape / window blur. Using
    // `capture: true` on pointerdown so we win the race against any
    // other click handler (the canvas's own pointerdown that focuses
    // the terminal, the chrome forwarder, etc.).
    const onPointerDown = (e: PointerEvent | MouseEvent) => {
      if (!menu.contains(e.target as Node)) {
        this.dismissFileTreeMenu();
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.dismissFileTreeMenu();
      }
    };
    const onBlur = () => this.dismissFileTreeMenu();
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", onBlur, { once: true });
    this.fileTreeMenuDismiss = () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", onBlur);
    };
  }

  // ── File-tree modal flows ──────────────────────────────────────
  //
  // Create/rename/delete run through the chrome-hosted shared
  // `UniversalModal` — the exact ModalSpecs desktop opens in
  // `bridges/file_tree/create_rename.rs` / `path_ops.rs` (same
  // titles, placeholders, validation messages, `d`/Enter/Esc keys).
  // Confirmed outcomes drain out of the wasm bridge and land in
  // `performCreateFile` etc., which drive the daemon Files ops the
  // old `window.prompt` flow used. The prompt/confirm bodies remain
  // only as a fallback for bundles predating the modal exports.

  /** True when the wasm bundle carries the spec-driven modal channel. */
  private modalChannelAvailable(): boolean {
    const adapter = this.wasmAdapter;
    return (
      typeof adapter?.drainModalActions === "function" &&
      typeof adapter?.modalActive === "function"
    );
  }

  /** Single-flight rAF pump: while the chrome modal is up, drain
   *  confirmed outcomes each frame and dispatch them; when the modal
   *  closes with nothing drained the flow was cancelled and the loop
   *  simply ends. Survives the confirm-then-close ordering because
   *  the wasm queue outlives the modal's close. */
  private modalPumpActive = false;
  private pumpModalOutcomes(): void {
    if (this.modalPumpActive) return;
    this.modalPumpActive = true;
    const poll = () => {
      const adapter = this.wasmAdapter;
      if (!adapter) {
        this.modalPumpActive = false;
        return;
      }
      const raw = adapter.drainModalActions?.();
      if (raw) {
        let actions: Array<Record<string, unknown>> = [];
        try {
          actions = JSON.parse(raw) as Array<Record<string, unknown>>;
        } catch {
          actions = [];
        }
        if (actions.length > 0) this.handleModalHostActions(actions);
      }
      if (adapter.modalActive?.() === true) {
        requestAnimationFrame(poll);
      } else {
        this.modalPumpActive = false;
        this.scheduleDraw();
      }
    };
    requestAnimationFrame(poll);
  }

  /** Execute drained modal outcomes — the web half of desktop's
   *  `execute_modal_action` file-tree/LSP arms. */
  private handleModalHostActions(actions: Array<Record<string, unknown>>): void {
    for (const action of actions) {
      const str = (key: string): string =>
        typeof action[key] === "string" ? (action[key] as string) : "";
      switch (action.kind) {
        case "file_tree_new_file": {
          const dir = str("dir");
          const name = str("name");
          if (dir && name) void this.performCreateFile(dir, name);
          break;
        }
        case "file_tree_new_folder": {
          const dir = str("dir");
          const name = str("name");
          if (dir && name) void this.performCreateDir(dir, name);
          break;
        }
        case "file_tree_rename": {
          const path = str("path");
          const name = str("name");
          if (path && name) void this.performRename(path, name);
          break;
        }
        case "file_tree_delete": {
          const path = str("path");
          if (path) void this.performDelete(path);
          break;
        }
        case "lsp_rename": {
          const name = str("name");
          if (name) {
            const adapter = this.wasmAdapter as {
              editorLspRenameSubmit?: (name: string) => void;
            } | null;
            adapter?.editorLspRenameSubmit?.(name);
          }
          break;
        }
        default:
          // "generic" spec outcomes have no consumers on this panel
          // yet (vault/workspace flows adopt them as their wire
          // paths land).
          break;
      }
    }
    this.scheduleDraw();
  }

  private async promptCreateFile(parentDir: string): Promise<void> {
    if (this.modalChannelAvailable()) {
      this.wasmAdapter?.openFileTreeNewFileModal?.(parentDir);
      this.focus();
      this.scheduleDraw();
      this.pumpModalOutcomes();
      return;
    }
    const name = window.prompt(`New file in ${parentDir}`, "untitled.txt");
    if (name === null) return;
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    await this.performCreateFile(parentDir, trimmed);
  }

  private async performCreateFile(parentDir: string, name: string): Promise<void> {
    try {
      const reply = await this.options.client.requestFiles(
        {
          CreateFile: {
            dir: this.toDaemonWorkspacePath(parentDir),
            name,
          },
        },
        this.options.workspaceRoot ?? null,
      );
      if ("Error" in reply) {
        this.reportFsError(`create file: ${reply.Error.message}`);
        return;
      }
      this.refreshFileTreeAfterMutation();
    } catch (err) {
      this.reportFsError(`create file: ${String(err)}`);
    }
  }

  private async promptCreateDir(parentDir: string): Promise<void> {
    if (this.modalChannelAvailable()) {
      this.wasmAdapter?.openFileTreeNewFolderModal?.(parentDir);
      this.focus();
      this.scheduleDraw();
      this.pumpModalOutcomes();
      return;
    }
    const name = window.prompt(`New folder in ${parentDir}`, "untitled");
    if (name === null) return;
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    await this.performCreateDir(parentDir, trimmed);
  }

  private async performCreateDir(parentDir: string, name: string): Promise<void> {
    try {
      const reply = await this.options.client.requestFiles(
        {
          CreateDir: {
            dir: this.toDaemonWorkspacePath(parentDir),
            name,
          },
        },
        this.options.workspaceRoot ?? null,
      );
      if ("Error" in reply) {
        this.reportFsError(`create folder: ${reply.Error.message}`);
        return;
      }
      this.refreshFileTreeAfterMutation();
    } catch (err) {
      this.reportFsError(`create folder: ${String(err)}`);
    }
  }

  private async promptRename(fromPath: string): Promise<void> {
    if (this.modalChannelAvailable()) {
      this.wasmAdapter?.openFileTreeRenameModal?.(fromPath);
      this.focus();
      this.scheduleDraw();
      this.pumpModalOutcomes();
      return;
    }
    const oldName = fromPath.split(/[\\/]/).pop() ?? fromPath;
    const next = window.prompt(`Rename ${oldName}`, oldName);
    if (next === null) return;
    const trimmed = next.trim();
    if (trimmed.length === 0 || trimmed === oldName) return;
    await this.performRename(fromPath, trimmed);
  }

  private async performRename(fromPath: string, newName: string): Promise<void> {
    const oldName = fromPath.split(/[\\/]/).pop() ?? fromPath;
    const parent = fromPath.slice(0, fromPath.length - oldName.length);
    const toPath = `${parent}${newName}`;
    if (toPath === fromPath) return;
    try {
      const reply = await this.options.client.requestFiles(
        {
          Rename: {
            from: this.toDaemonWorkspacePath(fromPath),
            to: this.toDaemonWorkspacePath(toPath),
          },
        },
        this.options.workspaceRoot ?? null,
      );
      if ("Error" in reply) {
        this.reportFsError(`rename: ${reply.Error.message}`);
        return;
      }
      // Update any open buffer-tab pointing at the renamed path so
      // the tab strip doesn't reference a stale path on next refresh.
      let changed = false;
      for (const tab of this.bufferTabs) {
        if (tab.kind === "file" && tab.path === fromPath) {
          tab.path = toPath;
          tab.title = toPath.split(/[\\/]/).pop() ?? newName;
          changed = true;
        }
      }
      if (changed) this.replayBufferTabs();
      this.refreshFileTreeAfterMutation();
    } catch (err) {
      this.reportFsError(`rename: ${String(err)}`);
    }
  }

  private async confirmDelete(path: string, isDir?: boolean): Promise<void> {
    if (this.modalChannelAvailable()) {
      this.wasmAdapter?.openFileTreeDeleteModal?.(path, isDir ?? null);
      this.focus();
      this.scheduleDraw();
      this.pumpModalOutcomes();
      return;
    }
    const ok = window.confirm(`Delete ${path}? This cannot be undone.`);
    if (!ok) return;
    await this.performDelete(path);
  }

  private async performDelete(path: string): Promise<void> {
    try {
      const reply = await this.options.client.requestFiles(
        { Delete: { path: this.toDaemonWorkspacePath(path) } },
        this.options.workspaceRoot ?? null,
      );
      if ("Error" in reply) {
        this.reportFsError(`delete: ${reply.Error.message}`);
        return;
      }
      // Close any buffer-tab that pointed at the deleted path.
      let removed = false;
      for (let i = this.bufferTabs.length - 1; i >= 0; i--) {
        const tab = this.bufferTabs[i];
        if (tab.kind === "file" && tab.path === path) {
          this.applyBufferTabPolicy("close_index", i);
          removed = true;
        }
      }
      if (removed) this.replayBufferTabs();
      this.refreshFileTreeAfterMutation();
    } catch (err) {
      this.reportFsError(`delete: ${String(err)}`);
    }
  }

  private refreshFileTreeAfterMutation(): void {
    this.wasmAdapter?.refreshFileTree?.();
    // The notes vault lives under the same workspace root; a file op may
    // have touched it. `refreshFileTree` flags the notes panel dirty in
    // the wasm chrome; pump the side-panel refreshes so the open Alt+N
    // panel re-fetches its listing this frame rather than next toggle.
    this.pumpSidePanelRefreshes();
    this.scheduleDraw();
  }

  private reportFsError(message: string): void {
    if (typeof console !== "undefined") {
      console.warn(`[file-tree] ${message}`);
    }
    window.alert(message);
  }

  /// Pull queued tab-strip clicks out of the wasm bridge. Activate
  /// swaps the visible content for the named tab; close splices the
  /// tab out of our bookkeeping list and replays `set_buffer_tabs` so
  /// the chrome's strip mirrors the new list.
  private drainBufferTabClicks(): void {
    const intents = this.wasmAdapter?.drainBufferTabIntents?.();
    if (!intents) return;
    let changed = false;
    let activated = false;
    if (intents.close.length > 0) {
      // Apply in descending order so earlier indices stay valid as
      // later ones are spliced out. Skip index 0 — that's the always-
      // present Terminal tab, which chrome's `close_at` already
      // refuses, but be defensive in case JS gets a stale index.
      const sorted = [...intents.close].sort((a, b) => b - a);
      for (const idx of sorted) {
        this.applyBufferTabPolicy("close_index", idx);
        changed = true;
      }
    }
    if (intents.activate !== null) {
      const idx = intents.activate;
      if (idx >= 0 && idx < this.bufferTabs.length) {
        this.activeTabIndex = idx;
        activated = true;
      }
    }
    if (changed) {
      this.replayBufferTabs();
      this.activateCurrentTabContents();
    } else if (activated) {
      this.replayBufferTabs();
      this.activateCurrentTabContents();
    }
    // Trailing "+" button — spawn a terminal tab, the same action
    // Ctrl+Shift+T / desktop's TabCreateNew drives.
    if (intents.newTab) {
      this.openTerminalTabPlaceholder();
      this.scheduleDraw();
    }
  }

  /** When each PTY session was bound to a tab — gates the post-attach
   *  backlog window in `ingestPty`. */
  private readonly ptyAttachedAt = new Map<string, number>();

  private registerTerminalSession(
    sessionId: string,
    activate: boolean,
    titleOverride?: string,
  ): void {
    if (!this.ptyAttachedAt.has(sessionId)) {
      this.ptyAttachedAt.set(sessionId, performance.now());
    }
    const existing = this.bufferTabs.findIndex(
      (tab) => tab.kind === "terminal" && tab.sessionId === sessionId,
    );
    if (existing >= 0) {
      if (activate) {
        this.activeTabIndex = existing;
      }
      return;
    }
    const unbound = this.bufferTabs.findIndex(
      (tab) => tab.kind === "terminal" && !tab.sessionId,
    );
    const title = this.stableTerminalTitle(titleOverride);
    if (unbound >= 0) {
      this.bufferTabs[unbound] = {
        ...this.bufferTabs[unbound],
        title: this.stableTerminalTitle(titleOverride ?? this.bufferTabs[unbound].title),
        sessionId,
      };
      if (activate) {
        this.activeTabIndex = unbound;
      }
    } else {
      this.bufferTabs.push({
        title: this.stableTerminalTitle(title),
        kind: "terminal",
        sessionId,
      });
      if (activate) {
        this.activeTabIndex = this.bufferTabs.length - 1;
      }
    }
    if (!this.ptyReplayBuffers.has(sessionId)) {
      this.ptyReplayBuffers.set(sessionId, new Uint8Array());
    }
  }

  private attachTerminalTabInPlace(sessionId: string, title?: string | null): void {
    if (!this.ptyAttachedAt.has(sessionId)) {
      this.ptyAttachedAt.set(sessionId, performance.now());
    }
    if (!this.ptyReplayBuffers.has(sessionId)) {
      this.ptyReplayBuffers.set(sessionId, new Uint8Array());
    }
    if (this.bufferTabs.some((tab) => tab.kind === "terminal" && tab.sessionId === sessionId)) {
      return;
    }
    this.bufferTabs.push({
      title: this.stableTerminalTitle(title),
      kind: "terminal",
      sessionId,
    });
  }

  private stableTabTitle(tab: WebBufferTab, index?: number): string {
    if (tab.kind !== "terminal") return tab.title;
    return this.stableTerminalTitle(tab.title, index);
  }

  private stableTerminalTitle(title?: string | null, ordinal?: number): string {
    const trimmed = title?.trim() ?? "";
    if (/^Terminal\s+\d+$/i.test(trimmed)) return trimmed;
    if (trimmed.length > 0 && !/^Route\s+\d+$/i.test(trimmed)) return trimmed;
    if (typeof ordinal === "number") return `Terminal ${ordinal}`;
    return `Terminal ${this.nextTerminalOrdinal()}`;
  }

  private nextTerminalOrdinal(): number {
    let max = 0;
    for (const tab of this.bufferTabs) {
      if (tab.kind !== "terminal") continue;
      const match = /^Terminal\s+(\d+)$/.exec(tab.title);
      if (match) {
        max = Math.max(max, Number(match[1]));
      }
    }
    return max + 1;
  }

  private knowsPtySession(sessionId: string): boolean {
    if (sessionId === this.options.sessionId) return true;
    return this.bufferTabs.some(
      (tab) => tab.kind === "terminal" && tab.sessionId === sessionId,
    );
  }

  private activePtySessionId(): string | null {
    const active = this.bufferTabs[this.activeTabIndex];
    return active?.kind === "terminal"
      ? active.sessionId ?? this.options.sessionId
      : null;
  }

  private activatePtySession(sessionId: string): void {
    const index = this.bufferTabs.findIndex(
      (tab) => tab.kind === "terminal" && tab.sessionId === sessionId,
    );
    if (index >= 0) {
      this.activeTabIndex = index;
      this.wasmAdapter?.setActiveTab?.(index);
      this.replayBufferTabs();
      this.activateCurrentTabContents(false);
      // Shared-PTY semantics: tell the shell our viewport so it
      // reflows/repaints for this client. For an adopted session
      // (desktop workspace opened from the modal) this also nudges an
      // immediate prompt redraw even when the replay buffer was thin.
      if (this.cols > 0 && this.rows > 0) {
        this.options.pty?.resize(sessionId, this.cols, this.rows);
      }
    }
    this.focus();
  }

  private rememberPtyBytes(sessionId: string, bytes: Uint8Array): void {
    if (bytes.length === 0) return;
    const existing = this.ptyReplayBuffers.get(sessionId) ?? new Uint8Array();
    const combined = new Uint8Array(existing.length + bytes.length);
    combined.set(existing, 0);
    combined.set(bytes, existing.length);
    if (combined.length <= MAX_REPLAY_BYTES_PER_PTY) {
      this.ptyReplayBuffers.set(sessionId, combined);
      return;
    }
    this.ptyReplayBuffers.set(
      sessionId,
      combined.slice(combined.length - MAX_REPLAY_BYTES_PER_PTY),
    );
  }

  /// Side panels (git diff, notes, file tree) resize the content
  /// column from INSIDE chrome — Esc-close, the X button, and the
  /// shared Alt+G/Alt+N handlers never pass through `handleResize`.
  /// Track the terminal rect each frame and re-run the resize
  /// contract when it moves.
  private lastTerminalRectKey = "";
  private keyboardInsetBottom = 0;
  private insetResizeTimer: number | null = null;
  private syncTerminalRectDependents(): void {
    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal) return;
    const key = `${terminal.x},${terminal.y},${terminal.w},${terminal.h}`;
    if (key === this.lastTerminalRectKey) return;
    const firstSync = this.lastTerminalRectKey === "";
    this.lastTerminalRectKey = key;
    if (!firstSync) {
      // Deduct the soft-keyboard inset — resizing back to the full
      // root height here would cancel the keyboard push-up the
      // MobileKeyboard insets handler just applied.
      this.handleResize(
        this.root.clientWidth,
        Math.max(1, this.root.clientHeight - this.keyboardInsetBottom),
      );
    }
  }

  private replayPtySession(sessionId: string): void {
    const replay = this.ptyReplayBuffers.get(sessionId);
    if (!replay) return;
    // The wasm bridge currently owns one rendered terminal surface.
    // Reset the parser/grid, then replay the selected session's saved
    // byte stream so tab switching presents the right shell screen.
    this.feedVisiblePtyBytes(TERMINAL_RESET_BYTES, false);
    if (replay.length > 0) {
      this.feedVisiblePtyBytes(replay, false);
    }
    // Replayed bytes can contain capability queries (DA1, DSR, kitty)
    // from a TUI that ran earlier. The parser queues responses for
    // them; flushing those to the PTY answers a question nobody is
    // asking anymore, and the shell just echoes the payload — the
    // `/62;4;6;22c` garbage in the scrollback. Drain and drop.
    this.wasmAdapter?.takePtyWrites();
  }

  /// Build and ship the `Editor` OpenBuffer envelope. The envelope
  /// shape matches `ServiceClientMessage::Editor { request_id,
  /// message }` in the daemon's server.rs; we don't wait for a reply.
  /// The daemon currently answers with "editor backend unavailable";
  /// the future native CodePane will service this wire.
  private sendEditorOpenBuffer(path: string): void {
    this.syncBreadcrumbsForPath(path);
    this.handleResize(this.root.clientWidth, this.root.clientHeight);
    this.editorSessionStarted = true;
    const externalId = this.activePaneExternalId();
    const surfaceId = externalId === null ? null : this.editorSurfaceId(externalId);
    if (externalId !== null) {
      this.bindEditorSurface(externalId, path);
    }
    this.requestPresenceSnapshot(
      presenceBufferIdForPath(path, this.options.workspaceRoot),
    );
    this.editorResizeBySurface.delete(surfaceId ?? "__primary__");
    this.sendEditorResize(this.cols, this.rows);
    this.sendEditorMessage({
      OpenBuffer: {
        path,
        ...(surfaceId ? { surface_id: surfaceId } : {}),
      },
    });
  }

  private sendEditorResize(cols: number, rows: number): void {
    if (!this.editorSessionStarted) return;
    const width = Math.max(1, Math.trunc(cols));
    const height = Math.max(1, Math.trunc(rows));
    const surfaceId = this.focusedEditorSurfaceId();
    const resizeKey = surfaceId ?? "__primary__";
    const previous = this.editorResizeBySurface.get(resizeKey);
    if (previous?.width === width && previous.height === height) {
      return;
    }
    this.editorGridCols = width;
    this.editorGridRows = height;
    this.editorResizeBySurface.set(resizeKey, { width, height });
    this.sendEditorMessage({
      Resize: {
        width,
        height,
        ...(surfaceId ? { surface_id: surfaceId } : {}),
      },
    });
  }

  private requestFileContent(path: string, tabIdx: number): void {
    if (!this.wasmAdapter) return;
    if (isMarkdownPath(path) && this.bufferTabs[tabIdx]?.path === path) {
      this.wasmAdapter.setTabContent?.(tabIdx, "Loading markdown...", path);
      this.renderMarkdownLayer(tabIdx, "Loading markdown...");
      this.scheduleDraw();
    }
    const requestId = nextFileReadRequestId++;
    this.pendingServiceMappers.set(requestId, (payload) => {
      if ("FileContent" in payload) {
        if (this.bufferTabs[tabIdx]?.path !== path) {
          return null;
        }
        const bytes = payload.FileContent.bytes;
        const decoded = new TextDecoder("utf-8", { fatal: false }).decode(
          new Uint8Array(bytes),
        );
        if (isMarkdownPath(path)) {
          this.markdownContentCache.set(path, decoded);
        }
        this.wasmAdapter?.setTabContent?.(tabIdx, decoded, path);
        if (isMarkdownPath(path)) {
          this.renderMarkdownLayer(tabIdx, decoded);
        } else {
          if (this.markdownLayerTabIndex === tabIdx) {
            this.clearMarkdownLayer();
          }
          // Route the fetched file into the chrome-hosted native
          // editor pane (code / notebook / draw) — desktop parity.
          // Re-opening the same path keeps live pane state (cursor,
          // undo, unsaved edits), so the refetch never clobbers.
          (this.wasmAdapter as {
            editorOpenFile?: (
              tabIdx: number,
              path: string,
              text: string,
            ) => string;
          })?.editorOpenFile?.(tabIdx, path, decoded);
          this.pumpCodeCrdt();
        }
        this.scheduleDraw();
        return decoded.length;
      }
      if ("Error" in payload) {
        const message = payload.Error.message;
        if (this.bufferTabs[tabIdx]?.path === path) {
          const errorText = `Could not read ${path}\n\n${message}`;
          this.wasmAdapter?.setTabContent?.(
            tabIdx,
            errorText,
            path,
          );
          if (isMarkdownPath(path)) {
            this.renderMarkdownLayer(tabIdx, errorText);
          }
        }
        this.pushInAppNotification("File open failed", message, "error");
        return null;
      }
      return null;
    });
    this.options.client.sendFiles(
      requestId,
      { ReadFile: { path: this.toDaemonWorkspacePath(path) } },
      this.options.workspaceRoot ?? null,
    );
  }

  private pollOpenMarkdownTabs(): void {
    if (!this.wasmAdapter || this.bufferTabs.length === 0) return;
    const markdownTabs = this.bufferTabs
      .map((tab, index) => ({ tab, index }))
      .filter(({ tab }) => tab.kind === "file" && !!tab.path && isMarkdownPath(tab.path));
    if (markdownTabs.length === 0) return;

    this.markdownReloadCursor %= markdownTabs.length;
    const { tab, index } = markdownTabs[this.markdownReloadCursor];
    this.markdownReloadCursor = (this.markdownReloadCursor + 1) % markdownTabs.length;
    if (!tab.path || this.markdownReloadInFlight.has(tab.path)) return;
    this.requestMarkdownLiveReload(tab.path, index);
  }

  private requestMarkdownLiveReload(path: string, tabIdx: number): void {
    this.markdownReloadInFlight.add(path);
    const requestId = nextFileReadRequestId++;
    this.pendingServiceMappers.set(requestId, (payload) => {
      this.markdownReloadInFlight.delete(path);
      if (this.bufferTabs[tabIdx]?.path !== path) {
        return null;
      }
      if ("FileContent" in payload) {
        const decoded = new TextDecoder("utf-8", { fatal: false }).decode(
          new Uint8Array(payload.FileContent.bytes),
        );
        if (this.markdownContentCache.get(path) === decoded) {
          return decoded.length;
        }
        this.markdownContentCache.set(path, decoded);
        this.wasmAdapter?.setTabContent?.(tabIdx, decoded, path);
        this.renderMarkdownLayer(tabIdx, decoded);
        this.scheduleDraw();
        return decoded.length;
      }
      if ("Error" in payload) {
        return null;
      }
      return null;
    });
    this.options.client.sendFiles(
      requestId,
      { ReadFile: { path: this.toDaemonWorkspacePath(path) } },
      this.options.workspaceRoot ?? null,
    );
  }

  private scheduleDraw(): void {
    if (this.rafHandle !== null) {
      return;
    }
    this.rafHandle = requestAnimationFrame(() => {
      this.rafHandle = null;
      this.draw();
    });
  }

  /// Lazily acquire the 2D rendering context. Returns null if the
  /// canvas has already been claimed by sugarloaf (WebGL2) or some
  /// other non-2D context.
  private ensureCtx(): CanvasRenderingContext2D | null {
    if (this.ctx) return this.ctx;
    this.ctx = this.canvas.getContext("2d");
    return this.ctx;
  }

  private draw(): void {
    // Drain any chrome-side intents that built up since the last
    // frame: file-tree opens (the panel saying "the user clicked a
    // file"), and tab-strip clicks (activate / close). Both translate
    // into buffer-tab bookkeeping updates on the JS side, which we
    // then replay back into the chrome via `set_buffer_tabs`.
    this.drainChromeIntents();
    this.syncTerminalRectDependents();

    if (this.isRendered()) {
      // sugarloaf owns the canvas — paint cells via wgpu and skip the
      // canvas2d stub entirely.
      this.wasmAdapter?.render();
      this.syncActiveMarkdownLayer();
      if (this.wasmAdapter?.animationsActive?.() === true) {
        this.scheduleDraw();
      }
      return;
    }

    // While wasm init is still pending we MUST NOT touch the canvas
    // (no 2D ctx, no width/height) — calling getContext("2d") would
    // lock the canvas to 2D and block sugarloaf's WebGL2 path.
    if (!this.wasmInitResolved) {
      return;
    }

    const ctx = this.ensureCtx();
    if (!ctx) return;

    const widthCss = this.canvas.clientWidth;
    const heightCss = this.canvas.clientHeight;

    ctx.fillStyle = "#000000";
    ctx.fillRect(0, 0, widthCss, heightCss);

    ctx.strokeStyle = "#1c2128";
    ctx.lineWidth = 1;
    for (let c = 0; c <= this.cols; c += 8) {
      const x = Math.floor(c * CELL_WIDTH) + 0.5;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, heightCss);
      ctx.stroke();
    }
    for (let r = 0; r <= this.rows; r += 4) {
      const y = Math.floor(r * CELL_HEIGHT) + 0.5;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(widthCss, y);
      ctx.stroke();
    }

    ctx.fillStyle = "#e6edf3";
    ctx.font = `12px ${this.fallbackFontFamily}`;
    ctx.textBaseline = "top";

    if (this.terminalInitError) {
      ctx.fillText("neoism terminal renderer failed to initialize", 8, 6);
      ctx.fillText("expected: ChromeBridge / Sugarloaf rendered path", 8, 22);
      ctx.fillText("diagnostic fallback: disabled", 8, 38);
      ctx.fillText(
        "set VITE_NEOISM_ALLOW_TERMINAL_STUB=1 only for diagnostic stub mode",
        8,
        54,
      );
      ctx.fillText(this.terminalInitError.slice(0, 180), 8, 78);
      return;
    }

    const snap: TerminalSnapshot = this.stubTerminal.snapshot();
    ctx.fillText(`session: ${this.options.sessionId}`, 8, 6);
    ctx.fillText(`grid: ${snap.cols} x ${snap.rows}`, 8, 22);
    ctx.fillText(`bytes ingested: ${snap.bytesIngested}`, 8, 38);
    ctx.fillText(`last bytes: ${snap.lastBytePreview || "<none>"}`, 8, 54);
    ctx.fillText(
      this.wasmAdapter
        ? "neoism-terminal-wasm: opt-in diagnostic data-only adapter; not rendered"
        : "neoism-terminal-wasm: opt-in diagnostic stub; not rendered",
      8,
      heightCss - 18,
    );

    const cx = snap.cursor.col * CELL_WIDTH;
    const cy = snap.cursor.row * CELL_HEIGHT;
    ctx.fillStyle = snap.cursor.visible ? "#7ee787" : "#30363d";
    ctx.fillRect(cx, cy, CELL_WIDTH, CELL_HEIGHT);
  }

  private activateCurrentTabContents(openEditorBuffer = true): void {
    const tab = this.bufferTabs[this.activeTabIndex];
    this.wasmAdapter?.setActiveTab?.(this.activeTabIndex);
    this.syncActiveBreadcrumbs();
    this.handleResize(this.root.clientWidth, this.root.clientHeight);
    this.syncActiveMarkdownLayer();
    if (!tab) {
      this.scheduleDraw();
      return;
    }
    if (tab.kind === "terminal") {
      this.setMarkdownLayerVisible(false);
      this.replayPtySession(tab.sessionId ?? this.options.sessionId);
      this.scheduleDraw();
      return;
    }
    if (tab.kind === "file" && tab.path) {
      if (isMarkdownPath(tab.path)) {
        this.requestFileContent(tab.path, this.activeTabIndex);
        this.syncActiveMarkdownLayer();
        this.scheduleDraw();
        return;
      }
      this.ensureSessionLayoutState();
      this.wasmAdapter?.focusEditorInput?.();
      this.requestFileContent(tab.path, this.activeTabIndex);
      this.assignActiveTabToFocusedEditorPane();
      if (openEditorBuffer) {
        this.openFileTabContent(tab.path);
      }
      this.scheduleDraw();
      return;
    }
    if (tab.kind === "neoism-agent") {
      this.assignActiveTabToFocusedEditorPane();
      this.wasmAdapter?.agentSetInput?.(this.agentInput);
      this.scheduleDraw();
      return;
    }
    this.scheduleDraw();
  }

  private handleKeyDown(event: KeyboardEvent): void {
    // Mode-locking during compose. Mirrors the desktop fork's
    // `Screen::process_key_event` early return when
    // `context.ime.preedit().is_some()` — while the IME owns the
    // keyboard, every keystroke (Enter to commit, Escape to cancel,
    // arrows to navigate the candidate list) belongs to the IME and
    // must not reach the pty or chrome routing. The
    // `event.isComposing` flag fires for the in-flight composition;
    // our own `imeComposing` field stays true through `compositionend`
    // so the final commit-cycle keydown is also swallowed.
    if (
      !event.metaKey &&
      !event.altKey &&
      this.activeTabIsMarkdown() &&
      this.useWasmMarkdown()
    ) {
      // Space-leader chord (Space then x closes the tab) — must run
      // before markdownKey so the leader Space isn't typed/treated as
      // a motion.
      if (this.handleMarkdownLeaderShortcut(event)) {
        event.preventDefault();
        return;
      }
      // Ctrl+S = daemon-owned save (the doc is shared; the daemon
      // writes the converged text). Must run before markdownKey so a
      // bare "s" never reaches normal-mode routing with ctrl held.
      if (event.ctrlKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        this.saveActiveMarkdown();
        return;
      }
      // Real-renderer markdown: desktop-breadth key routing through
      // the shared dispatcher (operators, visual mode, tables/lists,
      // title editing, `/` block menu, `[[` completion, `/` search).
      // Unhandled keys fall through to the normal routing below.
      const adapter = this.wasmAdapter as {
        markdownKey?: (key: string, ctrl: boolean) => boolean;
        markdownKeyFull?: (
          key: string,
          ctrl: boolean,
          shift: boolean,
          alt: boolean,
          meta: boolean,
        ) => boolean;
        markdownKeyFullSupported?: () => boolean;
        markdownDrainClipboardOut?: () => string | null;
        markdownDrainOpenIntents?: () => unknown;
        markdownSeedClipboard?: (text: string) => void;
        markdownInInsertMode?: () => boolean;
      };
      // Vim `p`/`P` paste from the system clipboard: browsers only
      // hand clipboard text over asynchronously, so seed the wasm
      // unnamed register in the background — key repeats and
      // follow-up pastes read fresh text (in-session yanks already
      // live in the pane's own registers).
      if (
        adapter?.markdownSeedClipboard &&
        (event.key === "p" || event.key === "P") &&
        !event.ctrlKey &&
        adapter.markdownInInsertMode?.() !== true
      ) {
        void navigator.clipboard
          ?.readText?.()
          .then((text) => adapter.markdownSeedClipboard?.(text))
          .catch(() => {});
      }
      const handled = adapter?.markdownKeyFullSupported?.()
        ? (adapter.markdownKeyFull?.(
            event.key,
            event.ctrlKey,
            event.shiftKey,
            false,
            false,
          ) ?? false)
        : (adapter?.markdownKey?.(event.key, event.ctrlKey) ?? false);
      if (handled) {
        event.preventDefault();
        // Yanks / copy chips / contact links queue clipboard text.
        const copyOut = adapter?.markdownDrainClipboardOut?.();
        if (copyOut) {
          void navigator.clipboard?.writeText?.(copyOut).catch(() => {});
        }
        // Link activations + committed title renames queue intents.
        const intents = adapter?.markdownDrainOpenIntents?.();
        if (Array.isArray(intents)) {
          for (const raw of intents) {
            if (!raw || typeof raw !== "object") continue;
            const intent = raw as {
              kind?: string;
              target?: string;
              line?: number;
            };
            const target = intent.target ?? "";
            if (target.length === 0) continue;
            if (intent.kind === "external") {
              window.open(target, "_blank", "noopener,noreferrer");
            } else if (intent.kind === "markdown" || intent.kind === "editor") {
              this.openActivatedPaths([target]);
            }
            // "rename": committed title-edit renames need a daemon
            // move op the web host doesn't expose yet — the buffer
            // text is already updated via the CRDT path.
          }
        }
        // Letter-by-letter outbound: the keystroke just mutated the
        // pane — flush it into the shared doc right away.
        this.pumpCrdtOutbox();
        this.scheduleDraw();
        this.pumpMarkdownAnimation();
        return;
      }
    }
    // Chrome-hosted native editor panes (code / notebook / draw) —
    // the desktop `dispatch_code_key` / notebook / draw key surfaces
    // adapted to the browser. Alt combos stay with the chrome
    // shortcuts EXCEPT Ctrl+Alt (multi-cursor caret stacking).
    if (
      !event.metaKey &&
      (!event.altKey || event.ctrlKey) &&
      // A focused chrome surface (palette / finder / tree / tabs /
      // composer) owns the keyboard first — desktop parity: keys
      // route by focused surface, not by the visible buffer.
      !this.isChromeKeyboardCaptureActive() &&
      this.activeEditorPaneKind() !== null
    ) {
      // Ctrl+S = save (daemon single-writer when doc-bound, WriteFile
      // fallback otherwise). Must run before editorKey so a bare "s"
      // never reaches insert-mode routing with ctrl held.
      if (event.ctrlKey && !event.altKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        this.saveActiveEditorPane();
        return;
      }
      const adapter = this.wasmAdapter as {
        editorKey?: (
          key: string,
          ctrl: boolean,
          shift: boolean,
          alt: boolean,
        ) => boolean;
        editorDrainClipboardOut?: () => string | null;
      };
      if (
        adapter?.editorKey?.(
          event.key,
          event.ctrlKey,
          event.shiftKey,
          event.altKey,
        )
      ) {
        event.preventDefault();
        // Yank/cut queued text for the system clipboard.
        const copyOut = adapter.editorDrainClipboardOut?.();
        if (copyOut) {
          void this.writeClipboard(copyOut);
        }
        // Letter-by-letter outbound for bound code docs.
        this.pumpCodeCrdt();
        // LSP host actions the key may have queued (rename prompt,
        // open-definition-target, deferred save completion).
        this.processEditorLspHostActions();
        this.scheduleDraw();
        return;
      }
    }
    if (
      shouldDropKeysDuringCompose(this.imeComposing) ||
      keyEventIsImeComposing(event)
    ) {
      return;
    }

    if (this.handleChromeShortcut(event)) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }

    if (this.routeKeyToChrome(event)) {
      event.preventDefault();
      return;
    }

    if (this.routeKeyToAgent(event)) {
      event.preventDefault();
      return;
    }

    // Terminal-grid shortcuts: selection copy + scrollback paging
    // (desktop bindings/defaults.rs + platform/linux.rs parity).
    if (
      this.activeSurface() === "terminal" &&
      this.handleTerminalGridShortcut(event)
    ) {
      event.preventDefault();
      return;
    }

    const bytes = encodePtyKeyEvent(event, this.activeSurface(), this.wasmAdapter);
    if (!bytes) {
      return;
    }
    event.preventDefault();
    if (
      this.activeSurface() === "terminal" &&
      !(this.wasmAdapter?.terminalShouldCaptureInput?.() ?? false)
    ) {
      // Desktop key_event.rs SendToPty arm: keys headed for the PTY
      // snap scrollback to the live tail and clear any selection.
      // Composer-owned keys never reach the PTY, so they're skipped.
      if (this.wasmAdapter?.terminalNotifyKeyInput?.()) {
        this.scheduleDraw();
      }
    }
    this.handleInputBytes(bytes);
  }

  /** Terminal-surface keyboard shortcuts that never reach the PTY:
   *
   *  - Ctrl+Shift+C (Linux/Windows) / Cmd+C (macOS) → copy the
   *    selection through the ClipboardService path. Consumed even
   *    with no selection, matching the desktop binding (otherwise
   *    Ctrl+Shift+C would leak 0x03 SIGINT into the shell).
   *  - Shift+PageUp / Shift+PageDown → scrollback paging outside the
   *    alt screen (defaults.rs:37-38); on the alt screen the desktop
   *    key encoder sends CSI 5;2~ / 6;2~ instead.
   *  - Plain PageUp / PageDown → \x1b[5~ / \x1b[6~ to the PTY
   *    (defaults.rs:134-135) — the generic keyEventToBytes table
   *    doesn't cover them.
   */
  private handleTerminalGridShortcut(event: KeyboardEvent): boolean {
    const adapter = this.wasmAdapter;
    if (!adapter) return false;
    const key = event.key.toLowerCase();
    // Terminal hint mode (desktop bindings/defaults.rs
    // create_hint_bindings — default binding Ctrl+Shift+O). While a
    // hint is being selected it owns EVERY key, mirroring desktop
    // key_event.rs:399-483 ("all key bindings are disabled while a
    // hint is being selected"): Escape stops, Backspace pops,
    // printable chars narrow the labels, a completed label opens.
    if (adapter.terminalHintActive?.() === true) {
      const flags = adapter.terminalHintKey?.(event.key) ?? 0;
      if (flags & 2) this.drainTerminalLinkOpens();
      this.scheduleDraw();
      return true;
    }
    if (
      event.ctrlKey &&
      event.shiftKey &&
      !event.altKey &&
      !event.metaKey &&
      key === "o"
    ) {
      if (adapter.terminalHintStart?.() === true) {
        this.scheduleDraw();
      }
      // Consumed either way, like the desktop binding (hint mode
      // cancels itself silently when nothing matches).
      return true;
    }
    const isMac = /Mac|iP(hone|ad|od)/.test(navigator.platform);
    const copyCombo =
      (event.ctrlKey &&
        event.shiftKey &&
        !event.altKey &&
        !event.metaKey &&
        key === "c") ||
      (isMac &&
        event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.shiftKey &&
        key === "c");
    if (copyCombo) {
      const text = adapter.terminalSelectedText?.();
      if (text) {
        void this.writeClipboard(text);
      }
      return true;
    }
    if (event.key === "PageUp" || event.key === "PageDown") {
      const up = event.key === "PageUp";
      if (
        event.shiftKey &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey
      ) {
        if (adapter.terminalScrollPage?.(up)) {
          this.scheduleDraw();
        } else {
          // Alt screen: desktop falls through to the shift-modified
          // key escape.
          this.handleInputBytes(
            new TextEncoder().encode(up ? "\x1b[5;2~" : "\x1b[6;2~"),
          );
        }
        return true;
      }
      if (!event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey) {
        this.handleInputBytes(
          new TextEncoder().encode(up ? "\x1b[5~" : "\x1b[6~"),
        );
        return true;
      }
    }
    return false;
  }

  /**
   * Browser `compositionstart` — open an IME session. Forwards the
   * shared `Composition::Start` event to chrome so panels that care
   * about preedit (status line, modals) can react.
   */
  private handleCompositionStart(_event: CompositionEvent): void {
    this.imeComposing = true;
    this.forwardChromeEvent(fromCompositionStart());
  }

  /**
   * Browser `compositionupdate` — the preedit string changed.
   * Forwards `Composition::Update { preedit, cursor }` to chrome.
   * The browser reports a single insertion-point cursor on the
   * `CompositionEvent` (no native explicit start/end offset), so we
   * place the caret at the end of the preedit string. The Rust side
   * (`Preedit::new`) clamps the byte offset into range, so an empty
   * preedit (cancel cycle) stays panic-safe.
   */
  private handleCompositionUpdate(event: CompositionEvent): void {
    this.imeComposing = true;
    const preedit = event.data ?? "";
    // Encode to UTF-8 to count bytes, matching the byte offset the
    // Rust side expects via `Preedit::new`.
    const cursorBytes = new TextEncoder().encode(preedit).length;
    this.forwardChromeEvent(fromCompositionUpdate(preedit, cursorBytes));
  }

  /**
   * Browser `compositionend` — the composition closed. `event.data`
   * is the committed string (empty on cancel-with-Escape). Forwards
   * `Composition::Commit(text)` followed by `Composition::End`, then
   * routes the committed bytes through `handleInputBytes` so the pty
   * receives the final text via the same path real keystrokes
   * + the system paste flow use. Mirrors the desktop fork's
   * `Ime::Commit -> screen.paste(text, count > 1)` pipeline.
   */
  private handleCompositionEnd(event: CompositionEvent): void {
    const committed = event.data ?? "";
    if (committed.length > 0) {
      this.forwardChromeEvent(fromCompositionCommit(committed));
      const dispatch = commitDispatch(committed);
      // Forward as a Text event too so chrome panels that only
      // listen for `UiEvent::Text` (status line, modals) receive
      // the committed string the same way they would for a paste.
      this.forwardChromeEvent(fromTextEvent(dispatch.text));
      // Route the bytes to the focused surface (pty / agent
      // input) via the shared input path. `handleInputBytes` already
      // dispatches on `activeSurface()`, so the IME commit lands
      // wherever a real paste would.
      const bytes = new TextEncoder().encode(dispatch.text);
      this.handleInputBytes(bytes);
    }
    this.forwardChromeEvent(fromCompositionEnd());
    this.imeComposing = false;
  }

  private handleChromeShortcut(event: KeyboardEvent): boolean {
    if (!this.wasmAdapter?.isChrome()) return false;

    if (event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
      if (event.key === "ArrowUp" && this.wasmAdapter.bufferTabsFocused?.()) {
        this.wasmAdapter.focusWorkspaceIsland?.();
        this.scheduleDraw();
        return true;
      }
      if (this.wasmAdapter.workspaceIslandFocused?.()) {
        if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
          if (this.wasmAdapter.moveWorkspaceIslandFocus?.(event.key === "ArrowLeft")) {
            this.scheduleDraw();
            return true;
          }
        }
        if (event.key === "ArrowUp") {
          this.scheduleDraw();
          return true;
        }
        if (event.key === "Enter") {
          if (this.wasmAdapter.activateWorkspaceIslandFocus?.()) {
            this.drainWorkspaceIslandIntents();
            this.scheduleDraw();
            return true;
          }
        }
      }
    }

    // File-tree CRUD shortcuts (task #68). Only fire when the tree
    // owns chrome focus so the keys keep their default meanings in
    // the terminal / editor / palette.
    //
    //   - F2     -> rename selected row
    //   - Delete -> delete selected row
    //
    // Both go through the same prompt/confirm flow the right-click
    // menu uses, so behavior stays consistent across input methods.
    if (
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.shiftKey &&
      this.wasmAdapter.fileTreeFocused?.()
    ) {
      if (event.key === "F2") {
        const path = this.wasmAdapter.fileTreeSelectedPath?.();
        if (path) {
          void this.promptRename(path);
          return true;
        }
      }
      if (event.key === "Delete") {
        const path = this.wasmAdapter.fileTreeSelectedPath?.();
        if (path) {
          void this.confirmDelete(path);
          return true;
        }
      }
    }

    // Web shortcut table — kept in sync with the desktop bindings so the
    // same muscle memory works in the browser. Where a desktop binding
    // would collide with a browser default (Cmd+W closes the tab,
    // Cmd+T opens a new tab, Cmd+1..9 switches browser tabs, Cmd+N
    // opens a new window, Cmd+Shift+N opens private browsing), we
    // either remap, or skip if the action only makes sense on the
    // desktop host (native file dialogs, window-level splits, etc.).
    //
    // Sources for the desktop side:
    //   * neoism-frontend/desktop/src/bindings/defaults.rs        (cross-platform)
    //   * neoism-frontend/desktop/src/bindings/platform/linux.rs  (Linux/BSD)
    //   * neoism-frontend/desktop/src/bindings/platform/macos.rs  (macOS)
    //   * neoism-frontend/desktop/src/screen/lifecycle.rs         (handle_app_global_shortcut)
    //   * neoism-frontend/desktop/src/screen/bridges/palette.rs   (is_command_palette_key)
    //   * neoism-frontend/desktop/src/screen/bridges/agent.rs     (is_command_neoism_agent_key)
    //
    // Panel toggles + global navigation.
    // ---------------------------------------------------------------
    // Alt+Shift+ArrowLeft/Right → move active buffer tab to previous /
    // next slot. Mirrors MoveActiveBufferTabToPrev/Next.
    if (event.altKey && event.shiftKey && !event.ctrlKey && !event.metaKey) {
      if (event.key === "ArrowLeft") {
        this.moveActiveBufferTab(-1);
        return true;
      }
      if (event.key === "ArrowRight") {
        this.moveActiveBufferTab(1);
        return true;
      }
    }

    // Alt + ... (matches Linux/X11 splash keybindings + cross-platform
    // chrome shortcuts in defaults.rs).
    if (event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
      // Alt+ArrowLeft/Right/Up/Down → chrome focus chain navigation.
      // Mirrors `is_chrome_focus_key` in screen/lifecycle.rs.
      if (isArrowKey(event.key)) {
        this.wasmAdapter.blurWorkspaceIsland?.();
        this.forwardChromeEvent(fromKeyboardEvent(event));
        this.scheduleDraw();
        return true;
      }
      // Alt+E → toggle file tree.
      // defaults.rs: `"e", ModifiersState::ALT; Action::ToggleFileTree`
      if (matchesKey(event, "KeyE", "e")) {
        this.wasmAdapter.blurWorkspaceIsland?.();
        this.wasmAdapter.toggleFileTree?.();
        this.wasmAdapter.refreshFileTree?.();
        this.scheduleDraw();
        return true;
      }
      // Alt+G → toggle the rich right-side git diff panel.
      // defaults.rs: `"g", ModifiersState::ALT; Action::ToggleGitDiffPanel`
      if (matchesKey(event, "KeyG", "g")) {
        this.wasmAdapter.blurWorkspaceIsland?.();
        this.toggleGitSidePanel();
        this.scheduleDraw();
        return true;
      }
      // Alt+N → toggle the notes sidebar.
      // defaults.rs: `"n", ModifiersState::ALT; Action::OpenNeoismNotes`
      if (matchesKey(event, "KeyN", "n")) {
        this.wasmAdapter.blurWorkspaceIsland?.();
        this.wasmAdapter.toggleNotesSidebar?.();
        // The refresh intent is queued chrome-side on open; pump it now
        // (same as Alt+G) so the notes listing fetch starts this frame
        // instead of waiting for the next draw — otherwise the sidebar
        // flashes empty on first open.
        this.pumpSidePanelRefreshes();
        this.scheduleDraw();
        return true;
      }
      // Alt+P -> command palette.
      if (matchesKey(event, "KeyP", "p")) {
        this.wasmAdapter.showCommandPalette?.();
        this.scheduleDraw();
        return true;
      }
      // Alt+S -> finder / file search.
      if (matchesKey(event, "KeyS", "s")) {
        (this.wasmAdapter.showFinderFiles ?? this.wasmAdapter.showFinder)?.call(this.wasmAdapter);
        this.scheduleDraw();
        return true;
      }
      // Alt+A → open / focus the Neoism Agent buffer tab.
      // screen/bridges/agent.rs::is_command_neoism_agent_key — desktop
      // uses Alt+A (not Cmd+A, which would collide with the browser's
      // "Select All").
      if (matchesKey(event, "KeyA", "a")) {
        this.openNeoismAgentTab();
        this.scheduleDraw();
        return true;
      }
    }

    // Ctrl+Cmd+Arrow* → resize the active editor split. Desktop's
    // macOS MoveDivider* binding uses CONTROL | SUPER.
    if (event.metaKey && event.ctrlKey && !event.altKey && !event.shiftKey) {
      if (isArrowKey(event.key)) {
        this.moveEditorDivider(arrowKeyDirection(event.key));
        return true;
      }
    }

    // Super (Cmd on macOS, Windows key on Linux) + ...
    // ---------------------------------------------------------------
    if (event.metaKey && !event.ctrlKey && !event.altKey) {
      // Cmd+P → command palette.
      // macos.rs: `"p", ModifiersState::SUPER; Action::OpenCommandPalette`
      // Also screen/bridges/palette.rs::is_command_palette_key.
      if (!event.shiftKey && matchesKey(event, "KeyP", "p")) {
        this.wasmAdapter.showCommandPalette?.();
        this.scheduleDraw();
        return true;
      }
      // Cmd+Shift+P → command palette (macOS alias).
      // macos.rs: `"p", SUPER | SHIFT; Action::OpenCommandPalette`
      if (event.shiftKey && matchesKey(event, "KeyP", "p")) {
        this.wasmAdapter.showCommandPalette?.();
        this.scheduleDraw();
        return true;
      }
      // Cmd+; / Cmd+: → command palette.
      // screen/lifecycle.rs::is_command_colon_key.
      if (matchesCommandColon(event)) {
        this.wasmAdapter.showCommandPalette?.();
        this.scheduleDraw();
        return true;
      }
      // Cmd+1..9 → select indexed tab, with 9 selecting the last tab.
      // This is a browser-tab shortcut too; when neoism owns focus, the
      // app gets desktop tab selection semantics.
      const commandDigit = digitKey(event);
      if (!event.shiftKey && commandDigit !== null && commandDigit > 0) {
        this.selectIndexedTab(commandDigit === 9 ? this.bufferTabs.length - 1 : commandDigit - 1);
        return true;
      }
      // Cmd+T → create a terminal tab. Desktop uses this for
      // TabCreateNew on macOS; the web terminal captures it when the
      // app owns keyboard focus.
      if (!event.shiftKey && matchesKey(event, "KeyT", "t")) {
        this.openTerminalTabPlaceholder();
        this.scheduleDraw();
        return true;
      }
      // Cmd+W → close current editor split, otherwise close the active
      // web tab. This mirrors CloseCurrentSplitOrTab.
      if (!event.shiftKey && matchesKey(event, "KeyW", "w")) {
        this.closeCurrentSplitOrTab();
        return true;
      }
      // Cmd+D / Cmd+Shift+D → split active editor pane right / down.
      if (!event.shiftKey && matchesKey(event, "KeyD", "d")) {
        this.splitEditorPane("horizontal");
        return true;
      }
      if (event.shiftKey && matchesKey(event, "KeyD", "d")) {
        this.splitEditorPane("vertical");
        return true;
      }
      // Cmd+[ / Cmd+] → focus previous / next editor split.
      if (!event.shiftKey && (event.code === "BracketLeft" || event.key === "[")) {
        this.focusEditorPane(true);
        return true;
      }
      if (!event.shiftKey && (event.code === "BracketRight" || event.key === "]")) {
        this.focusEditorPane(false);
        return true;
      }
      // Cmd+S → finder / file search.
      // screen/lifecycle.rs::is_command_files_key. macOS also has
      // `"s", SUPER; Action::SearchForward`, which we collapse to the
      // finder on the web since search-over-PTY isn't wired here.
      // Cmd+S also collides with the browser's "Save Page" — we
      // preventDefault unconditionally in the documentKeydownHandler
      // wrapper above.
      if (!event.shiftKey && matchesKey(event, "KeyS", "s")) {
        (this.wasmAdapter.showFinderFiles ?? this.wasmAdapter.showFinder)?.call(this.wasmAdapter);
        this.scheduleDraw();
        return true;
      }
      // Cmd+F → finder (macOS SearchForward alias). Browser's Find in
      // page also uses Cmd+F; preventDefault swallows it.
      // macos.rs: `"f", SUPER; Action::SearchForward`
      if (!event.shiftKey && matchesKey(event, "KeyF", "f")) {
        (this.wasmAdapter.showFinderGrep ?? this.wasmAdapter.showFinder)?.call(this.wasmAdapter);
        this.scheduleDraw();
        return true;
      }
      // Cmd+Shift+[ / Cmd+Shift+] → previous / next buffer tab.
      // macos.rs (with use_navigation_key_bindings):
      //   `"[", SUPER | SHIFT; Action::SelectPrevTab`
      //   `"]", SUPER | SHIFT; Action::SelectNextTab`
      // The web only models a single workspace, so this drives the
      // buffer-tab strip instead of top-level workspace tabs.
      if (event.shiftKey && (event.code === "BracketLeft" || event.key === "[" || event.key === "{")) {
        this.cycleBufferTab(-1);
        return true;
      }
      if (event.shiftKey && (event.code === "BracketRight" || event.key === "]" || event.key === "}")) {
        this.cycleBufferTab(1);
        return true;
      }
    }

    // Font zoom: Ctrl+= / Ctrl++ steps the cell size up, Ctrl+- steps
    // it down, Ctrl+0 resets to 1.0. Geometric ramp (×/÷ 1.1) so
    // repeated presses feel proportional rather than additive. Bridge
    // clamps too, but we clamp here so `currentFontScale` stays in
    // sync after it saturates.
    // platform/linux.rs:
    //   `"=", CONTROL; Action::IncreaseFontSize`
    //   `"-", CONTROL; Action::DecreaseFontSize`
    //   `"0", CONTROL; Action::ResetFontSize`
    // platform/macos.rs mirrors the same with SUPER.
    if (event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey) {
      // Ctrl+Tab → next tab.
      if (event.key === "Tab") {
        this.selectRelativeTab(1);
        return true;
      }
      if (
        event.key === "+" ||
        event.key === "=" ||
        event.code === "Equal"
      ) {
        const next = Math.min(3.0, this.currentFontScale * 1.1);
        this.applyFontScale(next);
        return true;
      }
      if (event.key === "-" || event.code === "Minus") {
        const next = Math.max(0.5, this.currentFontScale / 1.1);
        this.applyFontScale(next);
        return true;
      }
      if (event.key === "0" || event.code === "Digit0") {
        this.applyFontScale(1.0);
        return true;
      }
    }

    // Cmd+0 / Cmd+= / Cmd+- → font zoom (macOS SUPER variant).
    // platform/macos.rs: `"0/=/-", SUPER`. Browser zoom also binds
    // these — preventDefault swallows it.
    if (event.metaKey && !event.altKey && !event.ctrlKey && !event.shiftKey) {
      if (
        event.key === "+" ||
        event.key === "=" ||
        event.code === "Equal"
      ) {
        const next = Math.min(3.0, this.currentFontScale * 1.1);
        this.applyFontScale(next);
        return true;
      }
      if (event.key === "-" || event.code === "Minus") {
        const next = Math.max(0.5, this.currentFontScale / 1.1);
        this.applyFontScale(next);
        return true;
      }
      if (event.key === "0" || event.code === "Digit0") {
        this.applyFontScale(1.0);
        return true;
      }
    }

    // Ctrl+Shift bindings — Linux/X11 platform defaults, also kept as
    // fallbacks for hosts that don't have Alt/Super available
    // (e.g. browser tabs that swallow Alt for menu access).
    // Alt+W → create a NEW workspace, mirroring desktop's
    // Ctrl+Shift+W `create_tab` (a fresh top-level workspace tab).
    // The browser reserves Ctrl+Shift+W (close window) and never
    // delivers it to the page, so Alt+W is the binding that actually
    // works in a normal tab. The Workspaces PICKER stays on the
    // command palette ("Workspaces").
    if (
      event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.shiftKey &&
      matchesKey(event, "KeyW", "w")
    ) {
      this.options.onCreateWorkspace?.();
      return true;
    }
    if (event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey) {
      // Ctrl+Shift+Tab → previous tab.
      if (event.key === "Tab") {
        this.selectRelativeTab(-1);
        return true;
      }
      // Ctrl+Shift+P → command palette.
      // platform/linux.rs: `"p", CONTROL | SHIFT; Action::OpenCommandPalette`
      if (matchesKey(event, "KeyP", "p")) {
        this.wasmAdapter.showCommandPalette?.();
        this.scheduleDraw();
        return true;
      }
      // Ctrl+Shift+K → command composer (legacy chrome shortcut, no
      // direct match on the desktop bindings list but the bridge
      // method is exposed and useful from the keyboard).
      if (matchesKey(event, "KeyK", "k")) {
        this.wasmAdapter.showCommandComposer?.();
        this.scheduleDraw();
        return true;
      }
      // Ctrl+Shift+G → toggle git diff. Mirror of Alt+G for hosts that
      // swallow Alt.
      if (matchesKey(event, "KeyG", "g")) {
        this.toggleGitSidePanel();
        this.scheduleDraw();
        return true;
      }
      // Ctrl+Shift+B → toggle file tree. Mirror of Alt+E for hosts
      // that swallow Alt; matches VS Code muscle memory.
      if (matchesKey(event, "KeyB", "b")) {
        this.wasmAdapter.toggleFileTree?.();
        this.wasmAdapter.refreshFileTree?.();
        this.scheduleDraw();
        return true;
      }
      // Ctrl+Shift+F → finder / file search.
      // platform/linux.rs: `"f", CONTROL | SHIFT; Action::SearchForward`
      // The web finder covers the same use case.
      if (matchesKey(event, "KeyF", "f")) {
        (this.wasmAdapter.showFinderGrep ?? this.wasmAdapter.showFinder)?.call(this.wasmAdapter);
        this.scheduleDraw();
        return true;
      }
      // Ctrl+Shift+T → terminal-tab creation (desktop's workspace
      // terminal tab binding).
      if (matchesKey(event, "KeyT", "t")) {
        this.openTerminalTabPlaceholder();
        this.scheduleDraw();
        return true;
      }
      // Ctrl+Shift+W → create a new workspace (desktop parity:
      // `create_tab` spawns a fresh top-level workspace). NOTE:
      // browsers reserve Ctrl+Shift+W for "close window" and never
      // deliver it to the page — this branch only fires in
      // wrapped/PWA contexts. Alt+W above is the binding that works
      // in a normal browser tab.
      if (matchesKey(event, "KeyW", "w")) {
        this.options.onCreateWorkspace?.();
        return true;
      }
      // Ctrl+Shift+R / D → split active editor pane right / down.
      if (matchesKey(event, "KeyR", "r")) {
        this.splitEditorPane("horizontal");
        return true;
      }
      if (matchesKey(event, "KeyD", "d")) {
        this.splitEditorPane("vertical");
        return true;
      }
      // Ctrl+Shift+ArrowLeft/Right → previous / next buffer tab.
      // platform/linux.rs:
      //   `ArrowLeft, CONTROL | SHIFT; Action::SelectPrevBufferTab`
      //   `ArrowRight, CONTROL | SHIFT; Action::SelectNextBufferTab`
      if (event.key === "ArrowLeft") {
        this.cycleBufferTab(-1);
        return true;
      }
      if (event.key === "ArrowRight") {
        this.cycleBufferTab(1);
        return true;
      }
      // Ctrl+Shift+[ / Ctrl+Shift+] → previous / next buffer tab.
      // platform/linux.rs:
      //   `"[", CONTROL | SHIFT; Action::SelectPrevBufferTab`
      //   `"]", CONTROL | SHIFT; Action::SelectNextBufferTab`
      if (event.code === "BracketLeft" || event.key === "[" || event.key === "{") {
        if (this.activeSurface() === "editor") {
          this.focusEditorPane(true);
        } else {
          this.cycleBufferTab(-1);
        }
        return true;
      }
      if (event.code === "BracketRight" || event.key === "]" || event.key === "}") {
        if (this.activeSurface() === "editor") {
          this.focusEditorPane(false);
        } else {
          this.cycleBufferTab(1);
        }
        return true;
      }
    }
    // Ctrl+Shift+Alt+Arrow* → resize active editor split. Desktop's
    // Linux/Windows MoveDivider* binding lives here.
    if (event.ctrlKey && event.shiftKey && event.altKey && !event.metaKey) {
      if (isArrowKey(event.key)) {
        this.moveEditorDivider(arrowKeyDirection(event.key));
        return true;
      }
    }
    // Ctrl+Alt+Arrow* → resize the focused editor split via the
    // web-side session-layout policy.
    if (event.ctrlKey && event.altKey && !event.metaKey && !event.shiftKey) {
      if (event.key === "ArrowUp") {
        this.moveEditorDivider("up");
        return true;
      }
      if (event.key === "ArrowDown") {
        this.moveEditorDivider("down");
        return true;
      }
      if (event.key === "ArrowLeft") {
        this.moveEditorDivider("left");
        return true;
      }
      if (event.key === "ArrowRight") {
        this.moveEditorDivider("right");
        return true;
      }
    }
    return false;
  }

  /// Move the active buffer-tab selection by `delta` (clamped to the
  /// open tab list). Web-side mirror of desktop's `SelectPrev/NextBufferTab`
  /// actions — buffer tabs are owned by the JS bookkeeping in
  /// `this.bufferTabs`, so we just bump the index and replay the strip
  /// state into the wasm bridge so the chrome's tab visuals stay in
  /// sync. Wraps at both ends so repeated presses cycle.
  private cycleBufferTab(delta: number): void {
    this.applyBufferTabPolicy(delta < 0 ? "select_previous" : "select_next");
  }

  private applyBufferTabPolicy(operation: BufferTabPolicyOperation, index?: number): void {
    const raw = this.wasmAdapter?.applyBufferTabPolicy?.(
      JSON.stringify(this.bufferTabs),
      this.activeTabIndex,
      operation,
      index ?? null,
    );
    const result = parseBufferTabPolicyResult(raw);
    if (!result) {
      this.scheduleDraw();
      return;
    }

    if (typeof result.remove_index === "number") {
      const tab = this.bufferTabs[result.remove_index];
      if (tab?.kind === "terminal" && tab.sessionId) {
        this.options.pty?.close(tab.sessionId);
        this.ptyReplayBuffers.delete(tab.sessionId);
      }
      this.bufferTabs.splice(result.remove_index, 1);
      this.removeTabFromPaneState(result.remove_index);
    } else if (
      typeof result.move_from === "number" &&
      typeof result.move_to === "number" &&
      result.move_from !== result.move_to
    ) {
      const [tab] = this.bufferTabs.splice(result.move_from, 1);
      if (tab) {
        this.bufferTabs.splice(result.move_to, 0, tab);
        this.moveTabInPaneState(result.move_from, result.move_to);
      }
    }

    if (this.bufferTabs.length > 0) {
      this.activeTabIndex = Math.max(0, Math.min(result.active, this.bufferTabs.length - 1));
      this.wasmAdapter?.setActiveTab?.(this.activeTabIndex);
    } else {
      this.activeTabIndex = 0;
    }

    if (result.changed || typeof result.remove_index === "number" || typeof result.move_from === "number") {
      this.assignActiveTabToFocusedEditorPane();
      this.replayBufferTabs();
    }
    this.activateCurrentTabContents();
  }

  private routeKeyToChrome(event: KeyboardEvent): boolean {
    if (!this.isChromeKeyboardCaptureActive()) return false;
    this.forwardChromeEvent(fromKeyboardEvent(event));
    return true;
  }

  private routeKeyToAgent(event: KeyboardEvent): boolean {
    if (this.activeSurface() !== "agent") return false;
    const text =
      event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey
        ? event.key
        : "";
    return this.routeAgentKeyThroughShared(
      event.key,
      event.code,
      text,
      event.shiftKey,
      event.ctrlKey,
      event.altKey,
      event.metaKey,
    );
  }

  private routeInputBytesToAgent(bytes: Uint8Array): boolean {
    if (this.activeSurface() !== "agent") return false;
    if (bytes.length === 1 && bytes[0] === 0x03) {
      return this.routeAgentKeyThroughShared("c", "KeyC", "", false, true, false, false);
    }
    if (bytes.length === 1) {
      if (bytes[0] === 0x0d) {
        return this.routeAgentKeyThroughShared("Enter", "Enter", "", false, false, false, false);
      }
      if (bytes[0] === 0x7f || bytes[0] === 0x08) {
        return this.routeAgentKeyThroughShared(
          "Backspace",
          "Backspace",
          "",
          false,
          false,
          false,
          false,
        );
      }
      if (bytes[0] === 0x1b) {
        return this.routeAgentKeyThroughShared("Escape", "Escape", "", false, false, false, false);
      }
    }
    const text = new TextDecoder().decode(bytes);
    if (text.length > 0) {
      for (const char of text) {
        if (char === "\n" || char === "\r") {
          this.routeAgentKeyThroughShared(
            "Enter",
            "Enter",
            "",
            true,
            false,
            false,
            false,
          );
          continue;
        }
        this.routeAgentKeyThroughShared(char, "", char, false, false, false, false);
      }
    }
    return true;
  }

  private routeAgentKeyThroughShared(
    key: string,
    code: string,
    text: string,
    shift: boolean,
    ctrl: boolean,
    alt: boolean,
    meta: boolean,
  ): boolean {
    const handled =
      this.wasmAdapter?.agentHandleKey?.(key, code, text, shift, ctrl, alt, meta) ===
      true;
    if (!handled) return false;
    this.agentInput = this.wasmAdapter?.agentInput?.() ?? "";
    // Typing may have opened / extended an `@file` mention — make sure
    // the shared picker has candidates to rank.
    this.maybeFeedAgentFileMentions();
    this.scheduleDraw();
    return true;
  }

  private isChromeKeyboardCaptureActive(): boolean {
    return this.wasmAdapter?.chromeKeyboardCaptureActive?.() === true;
  }

  private canvasLogicalPoint(event: { clientX: number; clientY: number }): {
    x: number;
    y: number;
  } {
    const rect = this.canvas.getBoundingClientRect();
    return {
      x: event.clientX - rect.left,
      y: event.clientY - rect.top,
    };
  }

  private beginBufferTabDrag(event: PointerEvent): void {
    if (event.button !== 0) return;
    const adapter = this.wasmAdapter;
    const { x, y } = this.canvasLogicalPoint(event);
    let tabIndex = -1;
    if (adapter?.bufferTabBeginDrag) {
      // Shared strip pipeline: arms `BufferTabs::begin_drag` so the
      // strip itself paints the floating tab, reorders incrementally,
      // and reports the tear-out threshold.
      tabIndex = adapter.bufferTabBeginDrag(x, y);
    } else if (adapter?.bufferTabHitTest) {
      tabIndex = adapter.bufferTabHitTest(x, y);
    }
    if (tabIndex < 0 || tabIndex >= this.bufferTabs.length) {
      return;
    }
    const tab = this.bufferTabs[tabIndex];
    const draggable =
      this.isEditorLikeTab(tab) || (tab?.kind === "terminal" && !!tab.sessionId);
    if (!draggable) {
      adapter?.bufferTabCancelDrag?.();
      return;
    }
    this.bufferTabDrag = {
      pointerId: event.pointerId,
      tabIndex,
      startX: event.clientX,
      startY: event.clientY,
      active: false,
      target: null,
    };
    this.canvas.setPointerCapture?.(event.pointerId);
  }

  private updateBufferTabDrag(event: PointerEvent): boolean {
    const drag = this.bufferTabDrag;
    if (!drag || drag.pointerId !== event.pointerId) return false;
    const adapter = this.wasmAdapter;
    const { x, y } = this.canvasLogicalPoint(event);
    if (adapter?.bufferTabUpdateDrag) {
      // Shared pipeline: the strip reorders while the pointer stays in
      // the row; crossing the tear-out threshold switches to the pane
      // drop-zone preview (Rust-painted).
      if (adapter.bufferTabUpdateDrag(x, y)) {
        this.scheduleDraw();
      }
      const tearArmed = adapter.bufferTabDragTearArmed?.() === true;
      if (tearArmed) {
        if (!drag.active) {
          drag.active = true;
          adapter.paneGridBeginTabDrag?.();
        }
        drag.target = this.paneDropTargetAt(event.clientX, event.clientY);
        adapter.paneGridDragPreview?.(x, y);
        this.scheduleDraw();
      } else if (drag.active) {
        drag.active = false;
        drag.target = null;
        adapter.paneGridCancelDrag?.();
        this.scheduleDraw();
      }
      event.preventDefault();
      return true;
    }
    // Legacy fallback for stale bundles without the shared pipeline.
    if (!drag.active) {
      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      if (Math.hypot(dx, dy) < 4) return false;
      drag.active = true;
      adapter?.paneGridBeginTabDrag?.();
    }
    drag.target = this.paneDropTargetAt(event.clientX, event.clientY);
    adapter?.paneGridDragPreview?.(x, y);
    this.scheduleDraw();
    event.preventDefault();
    return true;
  }

  private paneDropTargetAt(clientX: number, clientY: number): WebPaneDropTarget | null {
    // Normalize against the chrome's terminal (content) rect — the
    // same rect the Rust pane grid solves against — instead of the
    // deleted DOM overlay's approximation.
    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal || terminal.w <= 0 || terminal.h <= 0) return null;
    const canvasRect = this.canvas.getBoundingClientRect();
    const bounds = {
      left: canvasRect.left + terminal.x,
      top: canvasRect.top + terminal.y,
      width: terminal.w,
      height: terminal.h,
    };
    const nx = (clientX - bounds.left) / bounds.width;
    const ny = (clientY - bounds.top) / bounds.height;
    // Shared-geometry hit test: the wasm bridge runs the SAME
    // `session_layout::geometry::drop_zone_at` (with the shared
    // `DEFAULT_EDGE_FRAC`) the desktop PaneGrid uses, so the edge
    // bands and the half-split preview can never drift from the
    // shared constants. Deliberately no TS fallback math — a stale
    // bundle just disables drag-to-split instead of desyncing.
    const zone = this.wasmAdapter?.paneDropTarget?.(
      JSON.stringify(
        this.paneLayoutPanes.map((pane) => ({
          external_id: pane.external_id,
          x: pane.x,
          y: pane.y,
          w: pane.w,
          h: pane.h,
        })),
      ),
      nx,
      ny,
    );
    if (!zone) return null;
    return {
      paneExternalId: zone.external_id,
      placement: zone.placement as WebPaneDropPlacement,
      rect: {
        x: bounds.left + zone.rect.x * bounds.width,
        y: bounds.top + zone.rect.y * bounds.height,
        w: zone.rect.w * bounds.width,
        h: zone.rect.h * bounds.height,
      },
    };
  }

  // ----------------------------------------------------------------
  // Shared PaneGrid pointer surface. Divider drag, focus-by-click and
  // the drag-to-split preview run inside the Rust grid; TS only
  // routes pointer events and drains the resulting side effects.
  // ----------------------------------------------------------------

  private paneGridDividerDragging = false;

  private paneGridHandlePointerDown(event: PointerEvent): boolean {
    if (event.button !== 0) return false;
    const adapter = this.wasmAdapter;
    if (!adapter?.paneGridPointerDown) return false;
    const { x, y } = this.canvasLogicalPoint(event);
    const flags = adapter.paneGridPointerDown(x, y);
    if ((flags & 1) === 0) return false;
    if (flags & 2) {
      this.paneGridDividerDragging = true;
      try {
        this.canvas.setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture is best-effort.
      }
    }
    if (flags & 4) {
      this.drainPaneGridUpdates(true);
    }
    if (flags & 8) {
      this.drainPaneTabIntents();
    }
    event.preventDefault();
    this.scheduleDraw();
    return true;
  }

  private paneGridHandlePointerMove(event: PointerEvent): boolean {
    const adapter = this.wasmAdapter;
    if (!adapter?.paneGridPointerMove || !this.paneGridDividerDragging) {
      return false;
    }
    const { x, y } = this.canvasLogicalPoint(event);
    const flags = adapter.paneGridPointerMove(x, y);
    if ((flags & 1) === 0) return false;
    if (flags & 2) {
      this.drainPaneGridUpdates(false);
    }
    event.preventDefault();
    this.scheduleDraw();
    return true;
  }

  private paneGridHandlePointerUp(event: PointerEvent): boolean {
    const adapter = this.wasmAdapter;
    if (!adapter?.paneGridPointerUp || !this.paneGridDividerDragging) {
      return false;
    }
    this.paneGridDividerDragging = false;
    this.canvas.releasePointerCapture?.(event.pointerId);
    const { x, y } = this.canvasLogicalPoint(event);
    const flags = adapter.paneGridPointerUp(x, y);
    if ((flags & 1) === 0) return false;
    this.drainPaneGridUpdates(false);
    event.preventDefault();
    this.scheduleDraw();
    return true;
  }

  /** Drain PaneGridAction side effects out of the Rust grid and pull
   *  the refreshed layout so the round-tripped `sessionLayoutStateJson`
   *  and normalized pane list stay in sync with the Rust-owned tree. */
  private drainPaneGridUpdates(activateFocused: boolean): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    // Capture the outgoing focus BEFORE processing focus actions so a
    // terminal pane losing focus keeps its session bound.
    const prevFocusedPane =
      this.paneLayoutPanes.find((pane) => pane.focused)?.external_id ?? null;
    const prevSession = this.activePtySessionId();
    const raw = adapter.drainPaneGridActions?.();
    if (Array.isArray(raw)) {
      for (const item of raw) {
        if (!item || typeof item !== "object") continue;
        const rec = item as Record<string, unknown>;
        if (
          rec.kind === "focus_pane" &&
          typeof rec.external_id === "number" &&
          activateFocused
        ) {
          if (
            prevFocusedPane !== null &&
            prevSession &&
            rec.external_id !== prevFocusedPane
          ) {
            this.paneSessionIds.set(prevFocusedPane, prevSession);
          }
          this.activatePaneExternalId(rec.external_id, true);
        } else if (rec.kind === "close_pane" && typeof rec.external_id === "number") {
          this.closeEditorSurface(rec.external_id);
        }
        // "open_pane" / "relayout" need no TS side effect beyond the
        // layout refresh below (keyboard splits allocate surfaces
        // through applySessionLayoutPolicy already).
      }
    }
    const result = parseSessionLayoutPolicyResult(adapter.paneGridLayoutResult?.());
    if (result) {
      this.sessionLayoutStateJson = result.state_json;
      this.paneLayoutPanes = result.panes;
      this.syncPaneRouteState(result.panes);
    }
    this.syncPaneTerminals();
  }

  /** Keep the per-pane wasm terminals in step with the visible split:
   *  seed newly visible terminal panes from their session's replay
   *  buffer, prune terminals whose panes went away, resize each bound
   *  session's PTY to its pane, and push the pane→surface descriptors
   *  the chrome's unfocused-pane renderer reads. Collapsing back to a
   *  single pane drops every pane terminal and restores the full-rect
   *  PTY size. */
  private readonly paneLastPtySize = new Map<number, string>();
  private syncPaneTerminals(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.feedPaneTerminal) {
      this.syncPaneSurfaces();
      return;
    }
    // Bindings for panes that no longer exist die with the pane.
    for (const externalId of [...this.paneSessionIds.keys()]) {
      if (!this.paneLayoutPanes.some((pane) => pane.external_id === externalId)) {
        this.paneSessionIds.delete(externalId);
        this.paneLastPtySize.delete(externalId);
      }
    }
    if (this.paneLayoutPanes.length <= 1) {
      adapter.prunePaneTerminals?.("[]");
      this.paneLastPtySize.clear();
      // Back to a single surface: restore the full-rect PTY size.
      const session = this.activePtySessionId();
      if (session && this.cols > 0 && this.rows > 0) {
        this.options.pty?.resize(session, this.cols, this.rows);
      }
      this.syncPaneSurfaces();
      return;
    }
    const terminalRect = this.wasmAdapter?.chromeLayout?.()?.terminal;
    const cellW =
      terminalRect && this.cols > 0 ? terminalRect.w / this.cols : 8;
    const cellH =
      terminalRect && this.rows > 0 ? terminalRect.h / this.rows : 16;
    const keep: number[] = [];
    for (const pane of this.paneLayoutPanes) {
      const sessionId = this.paneSessionIds.get(pane.external_id);
      if (!sessionId) continue;
      keep.push(pane.external_id);
      if (!adapter.paneTerminalExists?.(pane.external_id)) {
        // Seed the fresh pane terminal with the session's remembered
        // stream so it shows the live screen, not a blank grid.
        const replay = this.ptyReplayBuffers.get(sessionId);
        adapter.feedPaneTerminal(pane.external_id, replay ?? new Uint8Array());
      }
      // Size the pane's PTY to the pane, not the whole content rect.
      if (terminalRect && terminalRect.w > 0 && terminalRect.h > 0) {
        const cols = Math.max(2, Math.floor((pane.w * terminalRect.w) / cellW));
        const rows = Math.max(2, Math.floor((pane.h * terminalRect.h) / cellH));
        const key = `${cols}x${rows}`;
        if (this.paneLastPtySize.get(pane.external_id) !== key) {
          this.paneLastPtySize.set(pane.external_id, key);
          this.options.pty?.resize(sessionId, cols, rows);
        }
      }
    }
    adapter.prunePaneTerminals?.(JSON.stringify(keep));
    this.syncPaneSurfaces();
    this.scheduleDraw();
  }

  /** Push per-pane surface descriptors into the chrome so unfocused
   *  editor panes resolve their parked panes and placeholders carry
   *  honest labels — plus each pane's local tab strip (desktop
   *  pane_tabs parity; the chrome lays the strip out on stacked
   *  panes only, per the top-aligned rule). */
  private syncPaneSurfaces(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.setPaneSurfaces) return;
    const split = this.paneLayoutPanes.length > 1;
    const stripIds: number[] = [];
    const payload = this.paneLayoutPanes.map((pane) => {
      const state = this.paneTabState.get(pane.external_id);
      const tab =
        typeof state?.activeTabIndex === "number"
          ? this.bufferTabs[state.activeTabIndex]
          : null;
      const session = this.paneSessionIds.get(pane.external_id);
      const kind = session
        ? "terminal"
        : tab?.kind === "file"
          ? "editor"
          : (tab?.kind ?? pane.kind);
      if (split && adapter.setPaneTabs) {
        // Local strip entries: the pane's assigned editor-like tabs,
        // or a single sticky terminal tab for a bound shell.
        const entries: Array<{ title: string; path: string | null; kind: string }> =
          (state?.tabIndices ?? [])
            .filter(
              (ix) =>
                ix >= 0 &&
                ix < this.bufferTabs.length &&
                this.isEditorLikeTab(this.bufferTabs[ix]),
            )
            .map((ix) => ({
              title: this.bufferTabs[ix].title,
              path: this.bufferTabs[ix].path ?? null,
              kind: this.bufferTabs[ix].kind,
            }));
        if (entries.length === 0 && session) {
          const sessionTab = this.bufferTabs.find(
            (t) => t.kind === "terminal" && t.sessionId === session,
          );
          entries.push({
            title: sessionTab?.title ?? "Terminal",
            path: null,
            kind: "terminal",
          });
        }
        const activeWithin = Math.max(
          0,
          (state?.tabIndices ?? []).indexOf(state?.activeTabIndex ?? -1),
        );
        adapter.setPaneTabs(
          pane.external_id,
          JSON.stringify(entries),
          activeWithin,
        );
        if (entries.length > 0) stripIds.push(pane.external_id);
      }
      return {
        external_id: pane.external_id,
        kind,
        path: tab?.kind === "file" ? (tab.path ?? null) : null,
        title: tab?.title ?? pane.title ?? null,
      };
    });
    adapter.setPaneSurfaces(JSON.stringify(payload));
    adapter.retainPaneTabs?.(JSON.stringify(split ? stripIds : []));
  }

  /** Apply queued per-pane strip interactions (activate / close). */
  private drainPaneTabIntents(): void {
    const raw = this.wasmAdapter?.drainPaneTabIntents?.();
    if (!Array.isArray(raw)) return;
    for (const item of raw) {
      if (!item || typeof item !== "object") continue;
      const rec = item as Record<string, unknown>;
      const externalId =
        typeof rec.external_id === "number" ? rec.external_id : null;
      const index = typeof rec.index === "number" ? rec.index : 0;
      if (externalId === null) continue;
      const state = this.paneTabState.get(externalId);
      const paneTabs = (state?.tabIndices ?? []).filter(
        (ix) =>
          ix >= 0 &&
          ix < this.bufferTabs.length &&
          this.isEditorLikeTab(this.bufferTabs[ix]),
      );
      if (rec.kind === "activate") {
        const workspaceIx = paneTabs[index];
        if (state && typeof workspaceIx === "number") {
          state.activeTabIndex = workspaceIx;
        }
        this.focusEditorPaneByExternalId(externalId);
      } else if (rec.kind === "close") {
        const workspaceIx = paneTabs[index];
        if (state && typeof workspaceIx === "number") {
          state.tabIndices = state.tabIndices.filter((ix) => ix !== workspaceIx);
          if (state.activeTabIndex === workspaceIx) {
            state.activeTabIndex = state.tabIndices[0] ?? null;
          }
          if (state.tabIndices.length === 0 && !this.paneSessionIds.get(externalId)) {
            // Last tab left the pane — collapse the split cell.
            this.focusEditorPaneByExternalId(externalId);
            this.closeEditorPaneOrTab();
          } else {
            this.syncPaneTerminals();
          }
        }
      }
      // "new_tab" on a pane strip has no host mapping yet.
    }
    this.scheduleDraw();
  }

  /** Route one session's PTY bytes into every visible pane terminal
   *  bound to it (split panes render live from these). */
  private feedPaneTerminalBytes(sessionId: string, bytes: Uint8Array): void {
    if (this.paneLayoutPanes.length <= 1) return;
    const adapter = this.wasmAdapter;
    if (!adapter?.feedPaneTerminal) return;
    let fed = false;
    for (const [externalId, session] of this.paneSessionIds) {
      if (session !== sessionId) continue;
      if (!this.paneLayoutPanes.some((pane) => pane.external_id === externalId)) {
        continue;
      }
      adapter.feedPaneTerminal(externalId, bytes);
      fed = true;
    }
    if (fed) this.scheduleDraw();
  }

  private endBufferTabDrag(event: PointerEvent): boolean {
    const drag = this.bufferTabDrag;
    if (!drag || drag.pointerId !== event.pointerId) return false;
    this.bufferTabDrag = null;
    this.canvas.releasePointerCapture?.(event.pointerId);
    // Clear the Rust-painted drop preview.
    this.wasmAdapter?.paneGridCancelDrag?.();
    this.scheduleDraw();
    const releaseRaw = this.wasmAdapter?.bufferTabEndDrag?.();
    if (releaseRaw && typeof releaseRaw === "object") {
      const rec = releaseRaw as Record<string, unknown>;
      if (
        rec.kind === "reorder" &&
        typeof rec.from === "number" &&
        typeof rec.to === "number"
      ) {
        if (rec.from !== rec.to) {
          // The strip already reordered its own copy incrementally;
          // mirror the move into the canonical TS list through the
          // shared policy so active-index bookkeeping matches.
          this.applyBufferTabPolicy(
            "reorder",
            ((rec.from as number) << 16) | (rec.to as number),
          );
        }
        event.preventDefault();
        event.stopPropagation();
        return true;
      }
      if (rec.kind === "tear_out") {
        if (drag.target) {
          this.commitWebTabPaneDrop(drag.tabIndex, drag.target);
        } else {
          // Released past the strip but over no pane zone — restore
          // the strip (the replay puts the canonical list back).
          this.replayBufferTabs();
        }
        event.preventDefault();
        event.stopPropagation();
        return true;
      }
      // kind "none": plain click — activation flows through the
      // strip's own PointerDown handling.
      return false;
    }
    // Legacy fallback (stale bundle): pane drop only.
    if (!drag.active || !drag.target) return false;
    this.commitWebTabPaneDrop(drag.tabIndex, drag.target);
    event.preventDefault();
    event.stopPropagation();
    return true;
  }

  private commitWebTabPaneDrop(tabIndex: number, target: WebPaneDropTarget): void {
    const dropped = this.bufferTabs[tabIndex];
    if (dropped?.kind === "terminal" && dropped.sessionId) {
      // Terminal tab drop: an edge drop opens a terminal split bound
      // to the dragged shell's session; a center drop rebinds the
      // target pane to it.
      this.replayBufferTabs();
      if (target.placement === "center") {
        this.paneSessionIds.set(target.paneExternalId, dropped.sessionId);
        this.wasmAdapter?.removePaneTerminal?.(target.paneExternalId);
        this.syncPaneTerminals();
        this.focusEditorPaneByExternalId(target.paneExternalId);
        return;
      }
      this.focusEditorPaneByExternalId(target.paneExternalId);
      const paneId = this.nextWebPaneId++;
      const horizontal =
        target.placement === "left" || target.placement === "right";
      const before = target.placement === "left" || target.placement === "top";
      const result = this.applySessionLayoutPolicy(
        before ? "split_terminal_before" : "split_terminal",
        horizontal ? "horizontal" : "vertical",
        dropped.title ?? "Terminal",
        paneId,
      );
      if (!result) {
        this.nextWebPaneId -= 1;
        return;
      }
      this.paneSessionIds.set(paneId, dropped.sessionId);
      this.syncPaneTerminals();
      this.activatePaneExternalId(paneId, false);
      return;
    }
    if (!this.isEditorLikeTab(this.bufferTabs[tabIndex])) {
      this.replayBufferTabs();
      return;
    }
    for (const state of this.paneTabState.values()) {
      state.tabIndices = state.tabIndices.filter((index) => index !== tabIndex);
      if (state.activeTabIndex === tabIndex) {
        state.activeTabIndex = state.tabIndices[0] ?? null;
      }
    }

    if (target.placement === "center") {
      this.assignTabToPane(target.paneExternalId, tabIndex);
      this.focusEditorPaneByExternalId(target.paneExternalId);
      this.activatePaneExternalId(target.paneExternalId, true);
      return;
    }

    // Every edge drop creates a new split cell whose first tab is the
    // dragged tab. Focus the pointed pane before invoking the shared split
    // policy so nested layouts land at the correct depth.
    this.focusEditorPaneByExternalId(target.paneExternalId);
    const paneId = this.nextWebPaneId++;
    const horizontal = target.placement === "left" || target.placement === "right";
    const axis: WebPaneSplitAxis = horizontal ? "horizontal" : "vertical";
    const before = target.placement === "left" || target.placement === "top";
    const result = this.applySessionLayoutPolicy(
      before ? "split_before" : "split",
      axis,
      `Editor ${paneId}`,
      paneId,
    );
    if (!result) {
      this.nextWebPaneId -= 1;
      this.assignTabToPane(target.paneExternalId, tabIndex);
      return;
    }
    this.assignTabToPane(paneId, tabIndex);
    this.activatePaneExternalId(paneId, true);
    this.bindEditorSurfaceForTab(paneId, tabIndex);
    this.options.client.sendWorkspace({
      PaneLayoutOp: {
        pane_external_id: target.paneExternalId,
        op: {
          Split: {
            axis: horizontal ? "Horizontal" : "Vertical",
            placement: before ? "Before" : "After",
          },
        },
      },
    });
  }

  private handlePointerMove(event: PointerEvent): void {
    // Touch pointers are owned entirely by the touchstart/move/end
    // handlers; letting the parallel PointerEvent stream through
    // double-fires every action (a folder tap toggled open on
    // pointerdown, then closed again on the synthesized tap).
    if (event.pointerType === "touch") return;
    if (this.updateBufferTabDrag(event)) return;
    if (this.paneGridHandlePointerMove(event)) return;
    this.updateCustomCursorFromPointer(event, true);
    if (this.activeEditorPaneKind() !== null) {
      // Editor-pane drags: code selection / scrollbar, draw gestures
      // + graph hover. Consumed moves stay out of the chrome path.
      const { x, y } = this.canvasLogicalPoint(event);
      // Mouse-rest LSP hover: the wasm session arms a ~400ms candidate
      // on each move; with draw-on-demand nothing would tick after the
      // pointer stops, so schedule one delayed frame to mature it.
      if (this.activeEditorPaneKind() === "code") {
        if (this.editorLspHoverTimer !== null) {
          clearTimeout(this.editorLspHoverTimer);
        }
        this.editorLspHoverTimer = setTimeout(() => {
          this.editorLspHoverTimer = null;
          this.scheduleDraw();
        }, 450);
      }
      const adapter = this.wasmAdapter as {
        editorPointerMove?: (x: number, y: number) => boolean;
      };
      if (adapter?.editorPointerMove?.(x, y)) {
        event.preventDefault();
        this.scheduleDraw();
        return;
      }
    }
    if (
      this.activeTabIsMarkdown() &&
      this.useWasmMarkdown() &&
      (event.buttons & 1) !== 0
    ) {
      // Markdown drag: selection extend / block reorder — the
      // pointer-move half of desktop `handle_markdown_drag_move`.
      const { x, y } = this.canvasLogicalPoint(event);
      if (this.wasmAdapter?.markdownDragMove?.(x, y)) {
        event.preventDefault();
        this.scheduleDraw();
        return;
      }
    }
    if (this.activeSurface() === "agent") {
      const { x } = this.canvasLogicalPoint(event);
      if (this.wasmAdapter?.agentDragMarkdownHorizontalScrollbar?.(x)) {
        event.preventDefault();
        this.scheduleDraw();
        return;
      }
    }
    if (this.wasmAdapter?.splashMouseMove) {
      const { x, y } = this.canvasLogicalPoint(event);
      this.wasmAdapter.splashMouseMove(x, y);
      this.scheduleDraw();
    }
    // Selection drag / TUI motion reports on the terminal grid. While
    // a drag is active the bridge consumes the move (desktop skips
    // hint/hover work during selection too).
    if (this.handleTerminalGridPointerMove(event)) {
      return;
    }
    // Link hover underline (desktop draw_terminal_file_link_hover):
    // probe the grid under the pointer, redraw when the hover span
    // changes, and feed the daemon-backed link-existence cache.
    if (this.activeSurface() === "terminal") {
      const { x, y } = this.canvasLogicalPoint(event);
      const flags = this.wasmAdapter?.terminalHoverProbe?.(x, y) ?? 0;
      if (flags & 4) this.pumpTerminalLinkDirRequests();
      if (flags & 1) this.scheduleDraw();
    }
    this.forwardChromeEvent(fromPointerMoveEvent(event, this.canvas));
  }

  private handlePointerDown(event: PointerEvent): void {
    if (event.pointerType === "touch") return;
    this.beginBufferTabDrag(event);
    this.focusSurface();
    this.updateCustomCursorFromPointer(event, true);
    const islandPoint = this.canvasLogicalPoint(event);
    if (this.wasmAdapter?.workspaceIslandClick?.(islandPoint.x, islandPoint.y)) {
      event.preventDefault();
      this.drainWorkspaceIslandIntents();
      this.wasmAdapter.blurWorkspaceIsland?.();
      this.scheduleDraw();
      return;
    }
    // Shared PaneGrid pointer surface: divider grabs and clicks into
    // an unfocused pane route through the Rust grid BEFORE the
    // editor/terminal branches so a divider press near a caret can't
    // start a selection instead of a resize.
    if (this.paneGridHandlePointerDown(event)) return;
    if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
      // Real-renderer markdown: clicks place the caret (roster dots and
      // task checkboxes hit-test first, mirroring the desktop order).
      const rect = this.canvas.getBoundingClientRect();
      const adapter = this.wasmAdapter as {
        markdownClick?: (x: number, y: number) => boolean;
      };
      if (
        adapter?.markdownClick?.(
          event.clientX - rect.left,
          event.clientY - rect.top,
        )
      ) {
        event.preventDefault();
        this.scheduleDraw();
        return;
      }
    }
    if (this.activeEditorPaneKind() !== null) {
      // Chrome-hosted editor pane: caret placement, drag select,
      // scrollbar, notebook gutter actions, draw gestures — desktop
      // press semantics live in the wasm bridge.
      const { x, y } = this.canvasLogicalPoint(event);
      const adapter = this.wasmAdapter as {
        editorPointerDown?: (
          x: number,
          y: number,
          shift: boolean,
          ctrl: boolean,
          clickCount: number,
        ) => boolean;
      };
      if (
        adapter?.editorPointerDown?.(
          x,
          y,
          event.shiftKey,
          event.ctrlKey,
          event.detail || 1,
        )
      ) {
        event.preventDefault();
        this.pumpCodeCrdt();
        this.scheduleDraw();
        return;
      }
    }
    // Splash menu buttons are sticky-positioned in the terminal pane.
    // Hit-test before forwarding to other chrome so a button click
    // doesn't also send a terminal click to the PTY. Terminal surface
    // only — the cached splash rects survive tab switches and would
    // eat clicks meant for the agent pane's overlays.
    if (this.wasmAdapter?.splashClick && this.activeSurface() === "terminal") {
      const { x, y } = this.canvasLogicalPoint(event);
      if (this.wasmAdapter.splashClick(x, y)) {
        // Splash menu clicks are navigation actions, not terminal
        // submissions. Keep the splash armed so returning to an empty
        // terminal still shows it; real command submit handles dismiss.
        this.scheduleDraw();
        return;
      }
      // Even when no menu fired, give the wordmark a chance to pop.
      this.wasmAdapter.splashWordmarkClick?.(x, y);
    }
    if (this.activeSurface() === "agent") {
      const { x, y } = this.canvasLogicalPoint(event);
      if (this.agentPointerDownAt(x, y)) {
        this.scheduleDraw();
        return;
      }
      if (this.wasmAdapter?.agentWordmarkClick?.(x, y)) {
        this.scheduleDraw();
        return;
      }
    }
    {
      // Center-modal row clicks (mouse parity with the touch path).
      const { x, y } = this.canvasLogicalPoint(event);
      if ((this.wasmAdapter?.modalPointerDown?.(x, y) ?? 0) !== 0) {
        this.scheduleDraw();
        return;
      }
    }
    if (this.handleStatusLineClick(event)) {
      return;
    }
    if (this.handleTerminalGridPointerDown(event)) {
      return;
    }
    this.forwardChromeEvent(
      fromPointerDownEvent(event, event.detail || 1, this.canvas),
    );
    if (this.isMobileViewport() && event.pointerType !== "touch") {
      const { x, y } = this.canvasLogicalPoint(event);
      this.maybeRequestSoftKeyboardAfterTap(x, y);
    }
  }

  // ----------------------------------------------------------------
  // Terminal grid pointer surface (selection + TUI mouse reports).
  // Mirrors desktop app/window_event/mouse.rs; the wasm bridge owns
  // the click chain, selection state, and PTY report encoding.
  // ----------------------------------------------------------------

  private terminalSelectionScrollTimer: number | null = null;

  private handleTerminalGridPointerDown(event: PointerEvent): boolean {
    if (this.activeSurface() !== "terminal") return false;
    const adapter = this.wasmAdapter;
    if (!adapter?.terminalPointerDown) return false;
    const { x, y } = this.canvasLogicalPoint(event);
    if (event.button < 0 || event.button > 2) return false;
    const flags = adapter.terminalPointerDown(
      x,
      y,
      event.button,
      event.shiftKey,
      event.ctrlKey,
      event.altKey,
      performance.now(),
    );
    if ((flags & 1) === 0) return false;
    event.preventDefault();
    if (flags & 2) this.flushTerminalPointerBytes();
    if (flags & 8) {
      // Plain single-click landed on a link (desktop on_left_click's
      // link arm): open it instead of starting a selection.
      this.drainTerminalLinkOpens();
    }
    if (flags & 4) {
      // Selection drag started: keep move/up events flowing while the
      // pointer leaves the canvas, and run the desktop 15ms
      // edge-autoscroll tick.
      try {
        this.canvas.setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture is best-effort (detached canvas etc.).
      }
      this.startTerminalSelectionAutoscroll();
    }
    // Chrome still needs the press for focus bookkeeping (blurring
    // side panels), mirroring desktop's select_current_based_on_mouse
    // running alongside the terminal click.
    this.forwardChromeEvent(
      fromPointerDownEvent(event, event.detail || 1, this.canvas),
    );
    this.scheduleDraw();
    return true;
  }

  private handleTerminalGridPointerMove(event: PointerEvent): boolean {
    if (this.activeSurface() !== "terminal") return false;
    const adapter = this.wasmAdapter;
    if (!adapter?.terminalPointerMove) return false;
    const { x, y } = this.canvasLogicalPoint(event);
    const flags = adapter.terminalPointerMove(
      x,
      y,
      event.shiftKey,
      event.ctrlKey,
      event.altKey,
    );
    if ((flags & 1) === 0) return false;
    if (flags & 2) this.flushTerminalPointerBytes();
    this.scheduleDraw();
    return true;
  }

  private handleTerminalGridPointerUp(event: PointerEvent): boolean {
    const adapter = this.wasmAdapter;
    if (!adapter?.terminalPointerUp) return false;
    if (event.button < 0 || event.button > 2) return false;
    const { x, y } = this.canvasLogicalPoint(event);
    // Always deliver the release so the bridge's button/drag state
    // resets even when the press landed elsewhere.
    const flags = adapter.terminalPointerUp(
      x,
      y,
      event.button,
      event.shiftKey,
      event.ctrlKey,
      event.altKey,
    );
    try {
      this.canvas.releasePointerCapture(event.pointerId);
    } catch {
      // Not captured — fine.
    }
    if ((flags & 1) === 0) return false;
    if (flags & 2) this.flushTerminalPointerBytes();
    this.scheduleDraw();
    return true;
  }

  private startTerminalSelectionAutoscroll(): void {
    if (this.terminalSelectionScrollTimer !== null) return;
    // 15ms cadence matches desktop's SelectionScrolling scheduler.
    this.terminalSelectionScrollTimer = window.setInterval(() => {
      if (this.wasmAdapter?.terminalDragScrollTick?.()) {
        this.scheduleDraw();
      }
    }, 15);
  }

  private stopTerminalSelectionAutoscroll(): void {
    if (this.terminalSelectionScrollTimer !== null) {
      window.clearInterval(this.terminalSelectionScrollTimer);
      this.terminalSelectionScrollTimer = null;
    }
  }

  // ----------------------------------------------------------------
  // Terminal link opens + hint mode host effects (desktop
  // file_link_mouse.rs click routing / hints.rs command execution).
  // ----------------------------------------------------------------

  /** Drain link-open intents queued by a terminal link click or a
   *  hint-mode fire: URLs open in a browser tab (desktop
   *  open_hyperlink_uri), files open as buffer tabs with an optional
   *  deferred `file:line` jump (desktop open_path_in_editor /
   *  open_path_in_markdown routing lives in openActivatedPaths), and
   *  dirs reveal the file tree (desktop
   *  open_directory_link_in_file_tree). */
  private drainTerminalLinkOpens(): void {
    const raw = this.wasmAdapter?.terminalDrainLinkOpens?.();
    if (!Array.isArray(raw) || raw.length === 0) return;
    for (const entry of raw) {
      if (!entry || typeof entry !== "object") continue;
      const intent = entry as { kind?: string; target?: string; line?: number };
      const target = intent.target ?? "";
      if (target.length === 0) continue;
      if (intent.kind === "url") {
        window.open(target, "_blank", "noopener,noreferrer");
      } else if (intent.kind === "dir") {
        this.wasmAdapter?.showFileTree?.();
        this.scheduleDraw();
      } else if (intent.kind === "file") {
        this.openActivatedPaths([target]);
        if (typeof intent.line === "number" && intent.line > 0) {
          this.scheduleTerminalLinkLineJump(intent.line);
        }
      }
    }
  }

  /** Retry a `file:line` jump until the opened file's pane is live —
   *  file content arrives async from the daemon, so the jump defers
   *  the same way the LSP cross-file goto lands its cursor. */
  private scheduleTerminalLinkLineJump(line: number): void {
    let attempts = 0;
    const tryJump = () => {
      if (this.wasmAdapter?.terminalLinkGotoLine?.(line)) {
        this.scheduleDraw();
        return;
      }
      if (++attempts < 20) {
        window.setTimeout(tryJump, 160);
      }
    };
    window.setTimeout(tryJump, 60);
  }

  /** Fetch daemon listings for parent dirs the wasm link-existence
   *  probe requested. Reuses the completion seeding round-trip — the
   *  wasm seeds both the Tab-completion and link caches from one
   *  reply (terminal_seed_completion_dir). */
  private pumpTerminalLinkDirRequests(): void {
    const raw = this.wasmAdapter?.terminalDrainLinkDirRequests?.();
    if (!Array.isArray(raw)) return;
    for (const dir of raw) {
      if (typeof dir === "string" && dir.length > 0) {
        void this.seedCompletionDir(dir);
      }
    }
  }

  /** Pointer-side markdown drains: clipboard-out text and queued
   *  open intents (link activations, copy chips). Mirrors the
   *  keydown drain path so mouse-driven actions land too. */
  private drainMarkdownPointerEffects(): void {
    const adapter = this.wasmAdapter as {
      markdownDrainClipboardOut?: () => string | null;
      markdownDrainOpenIntents?: () => unknown;
    } | null;
    const copyOut = adapter?.markdownDrainClipboardOut?.();
    if (copyOut) {
      void navigator.clipboard?.writeText?.(copyOut).catch(() => {});
    }
    const intents = adapter?.markdownDrainOpenIntents?.();
    if (Array.isArray(intents)) {
      for (const raw of intents) {
        if (!raw || typeof raw !== "object") continue;
        const intent = raw as { kind?: string; target?: string };
        const target = intent.target ?? "";
        if (target.length === 0) continue;
        if (intent.kind === "external") {
          window.open(target, "_blank", "noopener,noreferrer");
        } else if (intent.kind === "markdown" || intent.kind === "editor") {
          this.openActivatedPaths([target]);
        }
      }
    }
  }

  private handleStatusLineClick(event: PointerEvent): boolean {
    const { x, y } = this.canvasLogicalPoint(event);
    return this.statusLineClickAt(x, y);
  }

  private statusLineClickAt(x: number, y: number): boolean {
    const adapter = this.wasmAdapter;
    if (!adapter?.statusLineClick) return false;
    const intent = adapter.statusLineClick(x, y);
    if (!intent) return false;

    switch (intent.kind) {
      case "toggle_git_diff":
        this.toggleGitSidePanel();
        break;
      case "toggle_split":
        // Legacy status-line intent. Split panes are first-class now and
        // remain visible; older WASM builds may still emit this value.
        break;
      case "diagnostic_jump":
        // Jump-to-line was an embedded-nvim SendKeys hop; the native
        // CodePane will own caret jumps.
        break;
      case "diagnostics_opened":
      case "consumed":
        break;
    }
    this.scheduleDraw();
    return true;
  }

  private handlePointerUp(event: PointerEvent): void {
    if (event.pointerType === "touch") return;
    // Any release ends a terminal selection drag's edge autoscroll
    // (desktop unschedules SelectionScrolling on button release).
    this.stopTerminalSelectionAutoscroll();
    if (this.endBufferTabDrag(event)) return;
    if (this.paneGridHandlePointerUp(event)) return;
    this.updateCustomCursorFromPointer(event, true);
    if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
      // Markdown pointer release: drop a reordered block / finish the
      // selection / open a queued block menu (desktop
      // handle_markdown_mouse_release), then run the pointer-side
      // drains — clipboard-out + link open intents previously only
      // drained on key events (the wasm side TTL-guards them, so a
      // release with nothing queued is a no-op).
      const consumed = this.wasmAdapter?.markdownMouseRelease?.() === true;
      this.drainMarkdownPointerEffects();
      if (consumed) {
        event.preventDefault();
        this.pumpCrdtOutbox();
        this.scheduleDraw();
        return;
      }
    }
    if (this.activeEditorPaneKind() !== null) {
      // Ends code selections / scrollbar drags, finalizes draw
      // gestures (which also snapshots undo history + dirty state).
      const adapter = this.wasmAdapter as { editorPointerUp?: () => boolean };
      if (adapter?.editorPointerUp?.()) {
        event.preventDefault();
        this.pumpCodeCrdt();
        this.scheduleDraw();
        return;
      }
    }
    if (this.wasmAdapter?.agentEndMarkdownHorizontalScrollbarDrag?.()) {
      event.preventDefault();
      this.scheduleDraw();
      return;
    }
    if (this.handleTerminalGridPointerUp(event)) {
      return;
    }
    this.forwardChromeEvent(fromPointerUpEvent(event, this.canvas));
  }

  // ----------------------------------------------------------------
  // Touch (C3 polish). `services/touchPolicy.ts` routes gesture
  // classification through the shared Rust `touch_policy` state
  // machine via the wasm `TouchGesturePolicy` export. We only do
  // platform-specific wiring here (coordinate translation, zone
  // hit-test, `preventDefault` gating, side-effect application).
  // ----------------------------------------------------------------

  /** Classify a touch point's canvas-local position into one of the
   *  shared `TouchZone` buckets. Used by `handleTouchStart` to seed the
   *  policy with the right gating hint (no pinch on chrome panels, no
   *  swipe-back on editor area, etc.). */
  private resolveTouchZone(x: number, y: number): TouchZone {
    const adapter = this.wasmAdapter;
    const layout = adapter?.chromeLayout?.();
    if (!layout) return "terminal-body";
    const candidates: Array<{ rect: ChromeRect | null | undefined; zone: TouchZone }> = [
      { rect: layout.command_palette, zone: "chrome-panel" },
      { rect: layout.finder, zone: "chrome-panel" },
      { rect: layout.git_diff, zone: "chrome-panel" },
      { rect: layout.command_composer, zone: "chrome-panel" },
      { rect: layout.buffer_tabs, zone: "chrome-panel" },
      { rect: layout.status_line, zone: "chrome-panel" },
      { rect: layout.file_tree, zone: "chrome-panel" },
      { rect: layout.terminal, zone: this.terminalZone() },
    ];
    for (const c of candidates) {
      if (c.rect && pointInRect({ x, y }, c.rect)) return c.zone;
    }
    return "terminal-body";
  }

  private mobileChromeTouchTarget(x: number, y: number): MobileChromeTouchTarget {
    const layout = this.wasmAdapter?.chromeLayout?.();
    if (!layout) return "other";
    if (pointInRect({ x, y }, layout.buffer_tabs)) return "buffer-tabs";
    if (layout.file_tree && pointInRect({ x, y }, layout.file_tree)) {
      return "file-tree";
    }
    const textRects = [
      layout.command_palette,
      layout.finder,
      layout.git_diff,
      layout.command_composer,
    ];
    if (textRects.some((rect) => rect && pointInRect({ x, y }, rect))) {
      return "text-entry";
    }
    return "other";
  }

  private shouldRequestSoftKeyboardForTap(x: number, y: number): boolean {
    if (!this.isMobileViewport()) return false;
    const layout = this.wasmAdapter?.chromeLayout?.();
    if (!layout) return false;
    const target = this.mobileChromeTouchTarget(x, y);
    if (target === "text-entry") return true;
    if (target === "buffer-tabs" || target === "file-tree") return false;
    const surface = this.activeSurface();
    // Editor and markdown docs are type-anywhere surfaces:
    // any tap inside the content rect keeps/raises the keyboard.
    if (surface === "editor" || surface === "markdown") {
      return pointInRect({ x, y }, layout.terminal);
    }
    if (!pointInRect({ x, y }, layout.terminal)) return false;
    if (surface === "agent") {
      // Hit-test the REAL input rect: the home screen centers it
      // mid-pane, while a conversation docks it to the bottom — a
      // bottom-band-only check left the start page tap-dead on mobile.
      const inputRect = this.wasmAdapter?.agentInputRect?.();
      if (inputRect) {
        const pad = 24;
        if (
          x >= inputRect[0] - pad &&
          x <= inputRect[0] + inputRect[2] + pad &&
          y >= inputRect[1] - pad &&
          y <= inputRect[1] + inputRect[3] + pad
        ) {
          return true;
        }
      }
      const inputBandHeight = Math.min(112, Math.max(64, layout.terminal.h * 0.24));
      return y >= layout.terminal.y + layout.terminal.h - inputBandHeight;
    }
    return surface === "terminal";
  }

  private maybeRequestSoftKeyboardAfterTap(x: number, y: number): void {
    if (this.shouldRequestSoftKeyboardForTap(x, y)) {
      this.requestSoftKeyboard();
    } else {
      this.dismissSoftKeyboard();
    }
  }

  private synthesizeCanvasTap(x: number, y: number): void {
    this.forwardChromeEvent({
      PointerDown: {
        button: "Left",
        x,
        y,
        modifiers: "",
        click_count: 1,
      },
    });
    this.forwardChromeEvent({
      PointerUp: {
        button: "Left",
        x,
        y,
        modifiers: "",
      },
    });
  }

  /** Tap places the markdown caret (roster dots / checkboxes first,
   *  mirroring the mouse path). */
  private markdownTapAt(x: number, y: number): boolean {
    if (!this.activeTabIsMarkdown() || !this.useWasmMarkdown()) return false;
    const adapter = this.wasmAdapter as {
      markdownClick?: (x: number, y: number) => boolean;
      markdownKey?: (key: string, ctrl: boolean) => boolean;
      markdown_in_insert_mode?: () => boolean;
      markdownInInsertMode?: () => boolean;
    };
    if (adapter?.markdownClick?.(x, y) !== true) return false;
    // Obsidian-style on mobile: the tap lands ready to type. The pane
    // reports its vim mode, so entering insert is exact (no double-i).
    if (this.isMobileViewport() && adapter.markdownInInsertMode?.() !== true) {
      adapter.markdownKey?.("i", false);
      this.pumpCrdtOutbox();
    }
    return true;
  }

  /** Touch-drag scroll for the markdown pane (1:1; release momentum
   *  comes from `finishMarkdownTouchScroll`). */
  private routeMarkdownTouchScroll(dyPixels: number): boolean {
    if (!this.activeTabIsMarkdown() || !this.useWasmMarkdown()) return false;
    const adapter = this.wasmAdapter as {
      markdownScroll?: (dy: number, vh: number) => boolean;
    };
    if (!adapter?.markdownScroll) return false;
    const rect = this.canvas.getBoundingClientRect();
    // Finger down (positive dy) reveals earlier content = DOM scroll up.
    adapter.markdownScroll(-dyPixels, rect.height);
    TerminalPanel.pushVelocitySample(
      (this.markdownTouchSamples ??= []),
      0,
      -dyPixels,
    );
    this.scheduleDraw();
    this.pumpMarkdownAnimation();
    return true;
  }

  private markdownTouchSamples: Array<{ t: number; dx: number; dy: number }> | null =
    null;
  private markdownKineticRaf: number | null = null;

  private finishMarkdownTouchScroll(): void {
    const samples = this.markdownTouchSamples;
    this.markdownTouchSamples = null;
    if (!samples || samples.length === 0) return;
    const { vy } = TerminalPanel.releaseVelocity(samples);
    if (Math.abs(vy) < 80) return;
    const adapter = this.wasmAdapter as {
      markdownScroll?: (dy: number, vh: number) => boolean;
    };
    if (!adapter?.markdownScroll) return;
    let velocity = vy;
    let lastT = performance.now();
    const step = () => {
      this.markdownKineticRaf = null;
      if (!this.activeTabIsMarkdown()) return;
      const now = performance.now();
      const dt = Math.min(0.05, (now - lastT) / 1000);
      lastT = now;
      velocity *= Math.exp(-dt / 0.28);
      if (Math.abs(velocity) < 40) return;
      const rect = this.canvas.getBoundingClientRect();
      adapter.markdownScroll?.(velocity * dt, rect.height);
      this.scheduleDraw();
      this.pumpMarkdownAnimation();
      this.markdownKineticRaf = requestAnimationFrame(step);
    };
    this.markdownKineticRaf = requestAnimationFrame(step);
  }

  private stopMarkdownKinetic(): void {
    if (this.markdownKineticRaf !== null) {
      cancelAnimationFrame(this.markdownKineticRaf);
      this.markdownKineticRaf = null;
    }
  }

  /** Run the desktop-priority agent click chain (picker rows, side
   *  panel, permission buttons, links, tool-card expand) and execute
   *  the host-side effects it returns. Shared by the mouse pointer
   *  path and the touch tap path. */
  private agentPointerDownAt(x: number, y: number): boolean {
    const result = this.wasmAdapter?.agentPointerDown?.(x, y);
    if (!result?.handled) return false;
    if (result.copy) {
      void this.writeClipboard(result.copy);
    }
    if (result.link) {
      if (/^https?:\/\//i.test(result.link)) {
        window.open(result.link, "_blank", "noopener");
      } else {
        const root = this.options.workspaceRoot?.replace(/\/+$/, "");
        const target = result.link.startsWith("/")
          ? result.link
          : root
            ? `${root}/${result.link}`
            : result.link;
        // Strip a trailing :line suffix the agent loves to emit.
        this.openActivatedPaths([target.replace(/:\d+(:\d+)?$/, "")]);
      }
    }
    this.scheduleDraw();
    return true;
  }

  /** Touch-drag the agent timeline 1:1 with the finger. Positive
   *  `dyPixels` (finger moving down) reveals older history, matching
   *  the shared timeline's sign convention. Returns true when the
   *  gesture is owned by the agent surface even if the timeline is
   *  pinned at an edge, so the delta never leaks into the chrome
   *  wheel path mid-drag. */
  private routeAgentTouchScroll(x: number, y: number, dyPixels: number): boolean {
    if (this.activeSurface() !== "agent") return false;
    const adapter = this.wasmAdapter;
    const terminal = adapter?.chromeLayout?.()?.terminal;
    if (!terminal || !pointInRect({ x, y }, terminal)) return false;
    // Position-aware drag: picker overlay / side panel / diff cards
    // consume the drag without timeline fling; the timeline records
    // fling samples for the release.
    const consumed = adapter?.agentDragAt
      ? adapter.agentDragAt(x, y, dyPixels)
      : adapter?.agentDragTimeline?.(dyPixels)
        ? 2
        : 0;
    debugAgentTimeline("touch-scroll", { x, y, dyPixels, consumed });
    if (consumed === 0 && !adapter?.agentDragAt) return false;
    this.scheduleDraw();
    if (consumed === 2) {
      const now = performance.now();
      const samples = (this.agentTouchScrollSamples ??= []);
      samples.push({ t: now, dy: dyPixels });
      while (samples.length > 0 && now - samples[0].t > 120) {
        samples.shift();
      }
    } else {
      this.agentTouchScrollSamples = null;
    }
    return true;
  }

  // ----------------------------------------------------------------
  // Kinetic wheel pump: iOS-style release momentum for chrome panels
  // that scroll via forwarded Wheel events (file tree, buffer tabs).
  // The agent timeline and the editor have their own springs; this
  // covers the panels that don't.
  // ----------------------------------------------------------------
  private kineticWheelPump: {
    raf: number;
    vx: number;
    vy: number;
    lastT: number;
  } | null = null;

  private startKineticWheel(vx: number, vy: number): void {
    this.stopKineticWheel();
    if (Math.hypot(vx, vy) < 80) return;
    const pump = { raf: 0, vx, vy, lastT: performance.now() };
    this.kineticWheelPump = pump;
    const step = () => {
      if (this.kineticWheelPump !== pump) return;
      const now = performance.now();
      const dt = Math.min(0.05, (now - pump.lastT) / 1000);
      pump.lastT = now;
      // Same decay half-life the agent timeline glide uses.
      const decay = Math.exp(-dt / 0.28);
      pump.vx *= decay;
      pump.vy *= decay;
      if (Math.hypot(pump.vx, pump.vy) < 40) {
        this.kineticWheelPump = null;
        return;
      }
      this.forwardChromeEvent({
        Wheel: {
          dx: pump.vx * dt,
          dy: pump.vy * dt,
          mode: "Pixel",
          modifiers: "",
        },
      });
      this.scheduleDraw();
      pump.raf = requestAnimationFrame(step);
    };
    pump.raf = requestAnimationFrame(step);
  }

  /** Stop the glide; returns true when one was actually running so
   *  the stopping tap can be swallowed (iOS stop-scroll semantics). */
  private stopKineticWheel(): boolean {
    const pump = this.kineticWheelPump;
    if (!pump) return false;
    cancelAnimationFrame(pump.raf);
    this.kineticWheelPump = null;
    return true;
  }

  private static releaseVelocity(
    samples: Array<{ t: number; dx: number; dy: number }>,
  ): { vx: number; vy: number } {
    const now = performance.now();
    let totalX = 0;
    let totalY = 0;
    let oldest = now;
    for (const sample of samples) {
      totalX += sample.dx;
      totalY += sample.dy;
      if (sample.t < oldest) oldest = sample.t;
    }
    const dt = (now - oldest) / 1000;
    if (dt < 0.005) return { vx: 0, vy: 0 };
    return { vx: totalX / dt, vy: totalY / dt };
  }

  private static pushVelocitySample(
    samples: Array<{ t: number; dx: number; dy: number }>,
    dx: number,
    dy: number,
  ): void {
    const now = performance.now();
    samples.push({ t: now, dx, dy });
    while (samples.length > 0 && now - samples[0].t > 120) {
      samples.shift();
    }
  }

  /** Finger lifted off an agent-timeline drag: launch a glide at the
   *  finger's release velocity (trailing-120ms average). */
  private finishAgentTouchScroll(): void {
    const samples = this.agentTouchScrollSamples;
    this.agentTouchScrollSamples = null;
    if (!samples || samples.length === 0) return;
    const adapter = this.wasmAdapter;
    if (this.activeSurface() !== "agent" || !adapter?.agentFlingTimeline) {
      return;
    }
    const now = performance.now();
    let total = 0;
    let oldest = now;
    for (const sample of samples) {
      total += sample.dy;
      if (sample.t < oldest) oldest = sample.t;
    }
    const dtSeconds = (now - oldest) / 1000;
    if (dtSeconds < 0.005) return;
    const velocity = total / dtSeconds;
    // A slow, deliberate drag should stop dead where the finger left
    // it; only a real flick keeps gliding.
    if (Math.abs(velocity) < 80) return;
    adapter.agentFlingTimeline(velocity);
    this.scheduleDraw();
  }

  private terminalZone(): TouchZone {
    // iOS model: on a phone, a single-finger drag is ALWAYS a scroll —
    // never a drag-select (which read as nvim visual-mode runaway when
    // a horizontal wobble crossed the 5px tap budget). Selection on
    // mobile stays on the long-press path. Desktop touchscreens keep
    // select-drag for the terminal body.
    if (this.isMobileViewport()) return "editor-area";
    const surface = this.activeSurface();
    return surface === "editor" || surface === "agent"
      ? "editor-area"
      : "terminal-body";
  }

  private touchSampleFromEvent(touch: Touch): TouchSample {
    const rect = this.canvas.getBoundingClientRect();
    return {
      id: touch.identifier,
      x: touch.clientX - rect.left,
      y: touch.clientY - rect.top,
      timeMs: performance.now(),
    };
  }

  private layoutSizeForTouchPolicy(): { width: number; height: number } {
    const rect = this.canvas.getBoundingClientRect();
    return { width: rect.width, height: rect.height };
  }

  private startTouchLongPressTimer(): void {
    if (this.touchLongPressTimer !== null) return;
    // 50ms tick: fast enough that the 500ms long-press fires within
    // one frame of the threshold without burning CPU on idle taps.
    this.touchLongPressTimer = setInterval(() => {
      if (!this.touchPolicy.isActive()) {
        this.stopTouchLongPressTimer();
        return;
      }
      const action = this.touchPolicy.tickLongPress(
        performance.now(),
        this.layoutSizeForTouchPolicy(),
      );
      if (action.kind !== "none") {
        this.applyTouchAction(action);
      }
    }, 50);
  }

  private stopTouchLongPressTimer(): void {
    if (this.touchLongPressTimer !== null) {
      clearInterval(this.touchLongPressTimer);
      this.touchLongPressTimer = null;
    }
  }

  private handleTouchStart(event: TouchEvent): void {
    // `changedTouches` contains the fingers that just landed in this
    // event; `touches` is every finger currently on the surface.
    let zoneForGesture: TouchZone | null = null;
    let firstSample: TouchSample | null = null;
    for (let i = 0; i < event.changedTouches.length; i += 1) {
      const t = event.changedTouches[i];
      const sample = this.touchSampleFromEvent(t);
      if (firstSample === null) firstSample = sample;
      const zone = this.resolveTouchZone(sample.x, sample.y);
      if (zoneForGesture === null) zoneForGesture = zone;
      const action = this.touchPolicy.start(sample, zone);
      this.applyTouchAction(action);
    }
    if (
      this.isMobileViewport() &&
      event.touches.length === 1 &&
      firstSample &&
      this.mobileChromeTouchTarget(firstSample.x, firstSample.y) === "buffer-tabs"
    ) {
      this.mobileBufferTabPan = {
        id: firstSample.id,
        start: firstSample,
        last: firstSample,
        panning: false,
        samples: [],
        suppressTap: this.stopKineticWheel(),
      };
      // Anchor chrome's pointer position so the pan's (and the
      // release glide's) coordinate-less Wheel events route here.
      this.forwardChromeEvent({
        PointerMove: { x: firstSample.x, y: firstSample.y, modifiers: "" },
      });
      this.touchPolicy.reset();
      this.focusSurface();
      this.dismissSoftKeyboard();
      event.preventDefault();
      return;
    }
    if (
      this.isMobileViewport() &&
      event.touches.length === 1 &&
      firstSample &&
      this.mobileChromeTouchTarget(firstSample.x, firstSample.y) === "file-tree"
    ) {
      this.mobileFileTreePan = {
        id: firstSample.id,
        start: firstSample,
        last: firstSample,
        scrolling: false,
        samples: [],
        suppressTap: this.stopKineticWheel(),
      };
      this.forwardChromeEvent({
        PointerMove: { x: firstSample.x, y: firstSample.y, modifiers: "" },
      });
      this.touchPolicy.reset();
      this.focusSurface();
      this.dismissSoftKeyboard();
      event.preventDefault();
      return;
    }
    // Any other touch-down halts a panel glide-in-flight.
    this.stopKineticWheel();
    this.stopMarkdownKinetic();
    if (zoneForGesture !== null) {
      this.touchSuppressSwipeBack =
        this.touchSuppressSwipeBack ||
        TouchPolicy.shouldSuppressSwipeBack(zoneForGesture);
    }
    // Touching a gliding agent timeline stops the glide (iOS
    // semantics) — and that tap must not turn into a click on lift.
    if (
      event.touches.length === 1 &&
      firstSample &&
      this.activeSurface() === "agent"
    ) {
      const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
      if (terminal && pointInRect(firstSample, terminal)) {
        this.agentTouchSuppressTap =
          this.wasmAdapter?.agentFlingTimeline?.(0) === true;
      }
    }
    // Keep key routing anchored without opening the soft keyboard.
    // Keyboard focus is requested after a tap lands on a text-entry
    // target, not on every touchstart.
    if (event.touches.length === 1) {
      this.focusSurface();
      if (firstSample && !this.shouldRequestSoftKeyboardForTap(firstSample.x, firstSample.y)) {
        this.dismissSoftKeyboard();
      }
    }
    // Multi-finger gestures must not trigger the browser's native
    // back/forward swipe. Single-finger touches are NOT defaulted-out
    // here: preventDefault on touchstart cancels the tap's user
    // activation in iOS Safari, which silently kills the programmatic
    // focus that summons the soft keyboard. Swipe-back suppression for
    // single-finger drags lives in `handleTouchMove` instead, which is
    // early enough to cancel the navigation gesture.
    if (event.touches.length >= 2) {
      event.preventDefault();
    }
    this.startTouchLongPressTimer();
  }

  private handleTouchMove(event: TouchEvent): void {
    if (this.mobileBufferTabPan) {
      const pan = this.mobileBufferTabPan;
      for (let i = 0; i < event.changedTouches.length; i += 1) {
        const t = event.changedTouches[i];
        const sample = this.touchSampleFromEvent(t);
        if (sample.id !== pan.id) continue;
        const dx = sample.x - pan.last.x;
        const totalDx = sample.x - pan.start.x;
        const totalDy = sample.y - pan.start.y;
        if (pan.panning || Math.abs(totalDx) > MAX_TAP_DISTANCE) {
          pan.panning = true;
          this.forwardChromeEvent({
            PointerMove: {
              x: sample.x,
              y: sample.y,
              modifiers: "",
            },
          });
          // Natural touch direction: the strip follows the finger
          // (drag left → tabs move left). The wheel path keeps its
          // own inverted mapping for trackpads.
          this.forwardChromeEvent({
            Wheel: {
              dx,
              dy: 0,
              mode: "Pixel",
              modifiers: "",
            },
          });
          TerminalPanel.pushVelocitySample(pan.samples, dx, 0);
        } else if (Math.abs(totalDy) > MAX_TAP_DISTANCE) {
          pan.panning = true;
        }
        pan.last = sample;
      }
      event.preventDefault();
      return;
    }
    if (this.mobileFileTreePan) {
      const pan = this.mobileFileTreePan;
      for (let i = 0; i < event.changedTouches.length; i += 1) {
        const t = event.changedTouches[i];
        const sample = this.touchSampleFromEvent(t);
        if (sample.id !== pan.id) continue;
        const dx = sample.x - pan.last.x;
        const dy = sample.y - pan.last.y;
        const totalDx = sample.x - pan.start.x;
        const totalDy = sample.y - pan.start.y;
        if (
          pan.scrolling ||
          Math.abs(totalDy) > MOBILE_SCROLL_TAP_SLOP ||
          Math.hypot(totalDx, totalDy) > MOBILE_SCROLL_TAP_SLOP
        ) {
          pan.scrolling = true;
          this.forwardChromeEvent({
            Wheel: {
              dx: -dx,
              dy: -dy,
              mode: "Pixel",
              modifiers: "",
            },
          });
          TerminalPanel.pushVelocitySample(pan.samples, -dx, -dy);
        }
        pan.last = sample;
      }
      event.preventDefault();
      return;
    }
    const layout = this.layoutSizeForTouchPolicy();
    let suppressDefault = this.touchSuppressSwipeBack || event.touches.length >= 2;
    for (let i = 0; i < event.changedTouches.length; i += 1) {
      const t = event.changedTouches[i];
      const sample = this.touchSampleFromEvent(t);
      let action = this.touchPolicy.move(sample, layout);
      // Promotion actions (tap→select / tap→scroll) require a re-feed
      // of the same sample so the new state's first delta lands.
      if (
        action.kind === "start-simulated-left-click" ||
        action.kind === "promote-tap-to-scroll"
      ) {
        this.applyTouchAction(action);
        action = this.touchPolicy.move(sample, layout);
      }
      if (action.kind === "suppress-native-gesture") {
        suppressDefault = true;
      }
      this.applyTouchAction(action);
    }
    if (suppressDefault) {
      event.preventDefault();
    }
  }

  private handleTouchEnd(event: TouchEvent): void {
    if (this.mobileBufferTabPan) {
      const pan = this.mobileBufferTabPan;
      let ended = false;
      for (let i = 0; i < event.changedTouches.length; i += 1) {
        const t = event.changedTouches[i];
        const sample = this.touchSampleFromEvent(t);
        if (sample.id !== pan.id) continue;
        if (pan.panning) {
          const { vx } = TerminalPanel.releaseVelocity(pan.samples);
          this.startKineticWheel(vx, 0);
        } else if (!pan.suppressTap) {
          this.synthesizeCanvasTap(sample.x, sample.y);
        }
        ended = true;
      }
      if (ended || event.touches.length === 0) {
        this.mobileBufferTabPan = null;
      }
      event.preventDefault();
      return;
    }
    if (this.mobileFileTreePan) {
      const pan = this.mobileFileTreePan;
      let ended = false;
      for (let i = 0; i < event.changedTouches.length; i += 1) {
        const t = event.changedTouches[i];
        const sample = this.touchSampleFromEvent(t);
        if (sample.id !== pan.id) continue;
        if (pan.scrolling) {
          const { vy } = TerminalPanel.releaseVelocity(pan.samples);
          this.startKineticWheel(0, vy);
        } else if (!pan.suppressTap) {
          this.synthesizeCanvasTap(sample.x, sample.y);
        }
        ended = true;
      }
      if (ended || event.touches.length === 0) {
        this.mobileFileTreePan = null;
      }
      event.preventDefault();
      return;
    }
    const layout = this.layoutSizeForTouchPolicy();
    let tapSummonedKeyboard = false;
    for (let i = 0; i < event.changedTouches.length; i += 1) {
      const t = event.changedTouches[i];
      const sample = this.touchSampleFromEvent(t);
      // Mirror the desktop fork's "re-feed motion before resolving end"
      // call pattern so the trailing delta extends the gesture before
      // the state machine drops it.
      const moveAction = this.touchPolicy.move(sample, layout);
      this.applyTouchAction(moveAction);
      const endAction = this.touchPolicy.end(sample, layout);
      this.applyTouchAction(endAction);
      if (
        endAction.kind === "end-simulated-left-click" &&
        this.shouldRequestSoftKeyboardForTap(endAction.x, endAction.y)
      ) {
        tapSummonedKeyboard = true;
      }
      if (this.agentTapConsumed) {
        this.agentTapConsumed = false;
        tapSummonedKeyboard = true;
      }
    }
    if (tapSummonedKeyboard) {
      // The tap just focused the soft-keyboard capture element. Without
      // preventDefault the browser follows up with compatibility mouse
      // events (mousedown focuses the canvas natively), which steals
      // focus back and closes the keyboard before it ever opens.
      event.preventDefault();
    }
    if (event.touches.length === 0) {
      this.touchSuppressSwipeBack = false;
      this.agentTouchSuppressTap = false;
      this.stopTouchLongPressTimer();
    }
  }

  /** Apply one `TouchAction` from the shared policy. Side effects are
   *  routed through the same chrome/wasm calls the pointer & wheel
   *  paths use so a tap is genuinely a click and a two-finger pan is
   *  genuinely a wheel event. */
  private applyTouchAction(action: TouchPolicyAction): void {
    const adapter = this.wasmAdapter;
    switch (action.kind) {
      case "none":
        return;
      case "start-simulated-left-click": {
        this.forwardChromeEvent({
          PointerDown: {
            button: "Left",
            x: action.x,
            y: action.y,
            modifiers: "",
            click_count: 1,
          },
        });
        return;
      }
      case "update-mouse-position": {
        this.forwardChromeEvent({
          PointerMove: {
            x: action.x,
            y: action.y,
            modifiers: "",
          },
        });
        return;
      }
      case "end-simulated-left-click": {
        if (this.agentTouchSuppressTap) {
          // This tap's only job was stopping an in-flight glide.
          this.agentTouchSuppressTap = false;
          return;
        }
        // Center modals overlay everything: row taps commit, taps on
        // the modal chrome keep it open (and raise the keyboard for
        // query typing).
        const modalHit = this.wasmAdapter?.modalPointerDown?.(action.x, action.y) ?? 0;
        if (modalHit !== 0) {
          this.agentTapConsumed = true;
          if (modalHit === 2) {
            this.requestSoftKeyboard();
          }
          this.scheduleDraw();
          return;
        }
        // Markdown caret tap — then summon the keyboard for typing.
        if (this.markdownTapAt(action.x, action.y)) {
          this.agentTapConsumed = true;
          this.requestSoftKeyboard();
          this.scheduleDraw();
          return;
        }
        // Splash menu rows (Open file tree / Neoism Agent / Search /
        // Command palette). Mirrors the mouse pointer chain — the
        // splash hit-test used to live only in handlePointerDown,
        // which no longer sees touch pointers. ONLY on the terminal
        // surface: the splash's cached hit rects survive tab switches,
        // and on the agent tab they sat underneath the slash-command
        // picker, eating its row taps ("click-through").
        if (this.activeSurface() === "terminal") {
          if (this.wasmAdapter?.splashClick?.(action.x, action.y)) {
            this.scheduleDraw();
            return;
          }
          this.wasmAdapter?.splashWordmarkClick?.(action.x, action.y);
        }
        if (
          this.activeSurface() === "agent" &&
          this.agentPointerDownAt(action.x, action.y)
        ) {
          // A picker row / tool card / link consumed the tap. Leave
          // the soft keyboard exactly as it is — picking a slash
          // command mid-composition must not dismiss it. The flag
          // makes handleTouchEnd preventDefault so the browser's
          // compatibility mousedown can't refocus the canvas and
          // close the keyboard either.
          this.agentTapConsumed = true;
          return;
        }
        if (this.statusLineClickAt(action.x, action.y)) {
          return;
        }
        this.forwardChromeEvent({
          PointerDown: {
            button: "Left",
            x: action.x,
            y: action.y,
            modifiers: "",
            click_count: 1,
          },
        });
        this.forwardChromeEvent({
          PointerUp: {
            button: "Left",
            x: action.x,
            y: action.y,
            modifiers: "",
          },
        });
        this.maybeRequestSoftKeyboardAfterTap(action.x, action.y);
        return;
      }
      case "end-select": {
        // Synthesise a release at the last-known cursor position.
        // Best-effort — the policy doesn't remember the final point,
        // so fall back to the canvas origin which is the safe no-op.
        this.forwardChromeEvent({
          PointerUp: {
            button: "Left",
            x: 0,
            y: 0,
            modifiers: "",
          },
        });
        return;
      }
      case "end-scroll":
        this.finishAgentTouchScroll();
        this.finishMarkdownTouchScroll();
        return;
      case "promote-tap-to-scroll":
        // No immediate side effect; the policy state has flipped and
        // the next move/end will produce the trailing action.
        return;
      case "scroll": {
        // Single-finger scroll: drive the wheel path so chrome /
        // editor scroll-spring code sees a familiar event shape. Sign
        // is inverted because dragging a finger down should scroll
        // the content up (natural touch scrolling).
        if (this.wasmAdapter?.modalScroll?.(action.x, action.y, -action.dy)) {
          this.scheduleDraw();
          return;
        }
        if (this.routeMarkdownTouchScroll(action.dy)) {
          return;
        }
        if (this.routeAgentTouchScroll(action.x, action.y, action.dy)) {
          return;
        }
        // Anchor chrome's pointer position so the coordinate-less
        // Wheel routes to the panel under the finger.
        this.forwardChromeEvent({
          PointerMove: { x: action.x, y: action.y, modifiers: "" },
        });
        this.forwardChromeEvent({
          Wheel: {
            dx: -action.dx,
            dy: -action.dy,
            mode: "Pixel",
            modifiers: "",
          },
        });
        return;
      }
      case "two-finger-scroll": {
        if (
          this.activeSurface() === "agent" &&
          this.wasmAdapter?.agentDragTimeline
        ) {
          if (this.wasmAdapter.agentDragTimeline(action.dy)) {
            this.scheduleDraw();
          }
          return;
        }
        this.forwardChromeEvent({
          Wheel: {
            dx: -action.dx,
            dy: -action.dy,
            mode: "Pixel",
            modifiers: "",
          },
        });
        return;
      }
      case "change-font-size": {
        if (!adapter) return;
        const current = this.currentFontScale;
        const step = action.direction === "increase" ? 0.1 : -0.1;
        const next = Math.max(0.5, Math.min(3.0, current + step));
        if (Math.abs(next - current) < 1e-3) return;
        this.applyFontScale(next);
        return;
      }
      case "open-context-menu": {
        // Long-press → right-click-equivalent context menu. Reuse the
        // mouse contextmenu pipeline so the file-tree path opens its
        // existing menu and other zones fall back to the browser's
        // default. We synthesise a MouseEvent so `handleContextMenu`
        // can read clientX / clientY for menu positioning.
        const rect = this.canvas.getBoundingClientRect();
        const synthetic = new MouseEvent("contextmenu", {
          clientX: rect.left + action.x,
          clientY: rect.top + action.y,
          button: 2,
          bubbles: true,
          cancelable: true,
        });
        this.canvas.dispatchEvent(synthetic);
        return;
      }
      case "suppress-native-gesture":
        // Caller already handled `preventDefault()`. Nothing else to
        // do here; the policy is consuming the gesture.
        return;
    }
  }

  private updateCustomCursorFromPointer(
    _event: { clientX: number; clientY: number },
    _visible: boolean,
  ): void {
    this.hideCustomCursor();
  }

  private handleWheel(event: WheelEvent): void {
    if (this.wasmAdapter?.modalScroll) {
      // Center modals (palette / finder) scroll their result lists.
      const { x, y } = this.canvasLogicalPoint(event);
      if (this.wasmAdapter.modalScroll(x, y, wheelDeltaYPixels(event))) {
        event.preventDefault();
        this.scheduleDraw();
        return;
      }
    }
    if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
      // Real-renderer markdown: the pane owns scrolling.
      const rect = this.canvas.getBoundingClientRect();
      const adapter = this.wasmAdapter as {
        markdownScroll?: (dy: number, vh: number) => boolean;
      };
      if (adapter?.markdownScroll?.(event.deltaY, rect.height)) {
        event.preventDefault();
        this.scheduleDraw();
        this.pumpMarkdownAnimation();
        return;
      }
    }
    if (this.activeEditorPaneKind() !== null) {
      // Editor pane owns its scroll (code glide / notebook eased
      // scroll / draw pan+zoom). The wasm side bounds-checks against
      // the terminal rect so tree/tab wheel still reaches chrome.
      const { x, y } = this.canvasLogicalPoint(event);
      const adapter = this.wasmAdapter as {
        editorScroll?: (
          x: number,
          y: number,
          deltaX: number,
          deltaY: number,
          ctrl: boolean,
        ) => boolean;
      };
      if (
        adapter?.editorScroll?.(
          x,
          y,
          event.deltaX,
          event.deltaY,
          event.ctrlKey,
        )
      ) {
        event.preventDefault();
        this.scheduleDraw();
        return;
      }
    }
    if (this.routeWheelToAgent(event)) {
      event.preventDefault();
      return;
    }
    // Terminal grid scrollback / TUI wheel. The bridge hit-tests the
    // chrome terminal rect itself, so wheels over the side panels
    // still fall through to the chrome route below.
    if (this.routeWheelToTerminalGrid(event)) {
      event.preventDefault();
      return;
    }
    if (this.wasmAdapter?.isChrome()) {
      event.preventDefault();
    }
    this.forwardChromeEvent(fromPointerMoveEvent(event, this.canvas));
    this.forwardChromeEvent(
      fromWheelEvent(event, { invertX: this.isWheelOverBufferTabs(event) }),
    );
  }

  /** Route a wheel event into the wasm terminal grid: scrollback via
   *  the shared TerminalScroll notch accumulator, or PTY wheel
   *  reports / arrow CSI when a TUI owns the screen — exactly the
   *  three arms of desktop `Screen::scroll`. Deltas are negated into
   *  the winit sign convention (positive = scroll up). */
  private routeWheelToTerminalGrid(event: WheelEvent): boolean {
    if (this.activeSurface() !== "terminal") return false;
    const adapter = this.wasmAdapter;
    if (!adapter?.terminalWheel) return false;
    const { x, y } = this.canvasLogicalPoint(event);
    const flags = adapter.terminalWheel(
      x,
      y,
      -wheelDeltaXPixels(event),
      -wheelDeltaYPixels(event),
      event.shiftKey,
    );
    if ((flags & 1) === 0) return false;
    if (flags & 2) this.flushTerminalPointerBytes();
    this.scheduleDraw();
    return true;
  }

  /** Drain PTY-bound mouse-report / CSI bytes queued by the bridge's
   *  wheel/pointer handlers into the PTY websocket (the web stand-in
   *  for desktop's `messenger.send_write`). */
  private flushTerminalPointerBytes(): void {
    const bytes = this.wasmAdapter?.takeTerminalPointerBytes?.();
    if (bytes && bytes.length > 0) this.sendPtyInput(bytes);
  }

  private routeWheelToAgent(event: WheelEvent): boolean {
    if (this.activeSurface() !== "agent") return false;
    const adapter = this.wasmAdapter;
    if (!adapter?.agentScrollTimeline) return false;
    // Sideways trackpad input, plus Shift+wheel, belongs only to rendered
    // Markdown code/table viewports. Keep it separate from the vertical
    // route so merely hovering an overflowing block never traps chat scroll.
    const horizontalPixels = (() => {
      const dx = wheelDeltaXPixels(event);
      const dy = wheelDeltaYPixels(event);
      if (Math.abs(dx) > Math.abs(dy) && Math.abs(dx) >= 0.5) return dx;
      if (event.shiftKey && Math.abs(dy) >= 0.5) return dy;
      return 0;
    })();
    if (horizontalPixels !== 0 && adapter.agentScrollHorizontalAt) {
      const { x, y } = this.canvasLogicalPoint(event);
      if (adapter.agentScrollHorizontalAt(x, y, horizontalPixels)) {
        this.scheduleDraw();
        return true;
      }
    }

    // The shared timeline uses the desktop (winit) sign convention:
    // positive delta scrolls UP into history, one wheel notch = 42px
    // (`agent_timeline_scroll_pixels`). DOM deltaY is positive when
    // scrolling DOWN, so negate it or the conversation scrolls
    // backwards.
    const deltaY =
      event.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? -event.deltaY * 42
        : -wheelDeltaYPixels(event);
    if (Math.abs(deltaY) < 0.5) return false;
    // Position-aware: pickers, the side panel, and diff/code cards
    // under the cursor scroll themselves before the timeline moves.
    if (adapter.agentScrollAt) {
      const { x, y } = this.canvasLogicalPoint(event);
      const moved = adapter.agentScrollAt(x, y, deltaY);
      debugAgentTimeline("wheel-scroll-at", {
        x,
        y,
        deltaY,
        rawDeltaY: event.deltaY,
        deltaMode: event.deltaMode,
        moved,
      });
      if (moved) {
        this.scheduleDraw();
        return true;
      }
      return false;
    }
    const moved = adapter.agentScrollTimeline(deltaY);
    debugAgentTimeline("wheel-scroll", {
      deltaY,
      rawDeltaY: event.deltaY,
      deltaMode: event.deltaMode,
      moved,
    });
    if (moved) this.scheduleDraw();
    return moved;
  }

  private isWheelOverBufferTabs(event: WheelEvent): boolean {
    const layout = this.wasmAdapter?.chromeLayout?.();
    if (!layout?.buffer_tabs) return false;
    return pointInRect(this.canvasLogicalPoint(event), layout.buffer_tabs);
  }

  private handlePaste(event: ClipboardEvent): void {
    const imageItems = Array.from(event.clipboardData?.items ?? []).filter(
      (item) => item.kind === "file" && item.type.startsWith("image/"),
    );
    const surface = this.activeSurface();
    if (imageItems.length > 0) {
      if (surface === "agent") {
        event.preventDefault();
        // Desktop parity (pane/input.rs attach_clipboard_image): the
        // image lands in the composer as an `[imageN]` token + chip and
        // is sent on the next Enter — not immediately. The legacy
        // send-immediately path only remains for wasm bundles that
        // pre-date the attach export.
        if (this.wasmAdapter?.agentAttachClipboardImage) {
          void this.attachPastedImagesToAgent(imageItems);
        } else {
          void this.submitPastedImages(
            imageItems,
            event.clipboardData?.getData("text/plain") ?? "",
          );
        }
        return;
      }
      if (surface === "editor") {
        // Materialise the image on the daemon side; the reply hands
        // back a daemon HTTP URL the user can open in a fresh tab.
        event.preventDefault();
        void this.submitEditorPastedImages(imageItems);
        return;
      }
      // Terminal surface (a shell PTY): we can't paste binary into a
      // shell sensibly, so we drop the image and fall through to any
      // accompanying text. Surface a brief notification so users know
      // why nothing happened.
      this.wasmAdapter?.pushNotification?.(
        JSON.stringify({
          title: "Clipboard",
          message:
            "Image paste isn't supported in terminal panes. Switch to an editor or agent pane.",
          severity: "info",
        }),
      );
    }
    const text = event.clipboardData?.getData("text/plain") ?? "";
    if (text.length === 0) return;
    event.preventDefault();
    if (surface === "editor" && this.activeEditorPaneKind() !== null) {
      // Chrome-hosted editor pane: paste inserts at the caret(s) and
      // seeds the vim unnamed register (so `p` repeats it).
      const adapter = this.wasmAdapter as {
        editorInsertPaste?: (text: string) => boolean;
      };
      if (adapter?.editorInsertPaste?.(text)) {
        this.pumpCodeCrdt();
        this.scheduleDraw();
        return;
      }
      // Not consumed (e.g. notebook/draw outside insert/editing):
      // swallow rather than leak the paste into the PTY byte path.
      return;
    }
    if (surface === "agent") {
      // Desktop parity (pane/input.rs paste path): pasted text goes
      // through the pane's insert_paste — picker-aware, large pastes
      // compact to a "[pasted N lines]" token. A false return means
      // the wasm bundle predates the export; fall through to bytes.
      if (this.wasmAdapter?.agentInsertPaste?.(text)) {
        this.agentInput = this.wasmAdapter.agentInput?.() ?? this.agentInput;
        // The pasted text may have opened / extended an `@` mention.
        this.maybeFeedAgentFileMentions();
        this.scheduleDraw();
        return;
      }
    }
    if (surface === "terminal") {
      const adapter = this.wasmAdapter;
      const composerOwnsInput =
        adapter?.terminalShouldCaptureInput?.() ??
        adapter?.terminalCommandComposerVisible?.() === true;
      if (composerOwnsInput && adapter?.terminalInputInsertPaste?.(text)) {
        // Desktop parity (Screen::paste, selection/file_link_mouse.rs
        // :349-363): while the composer owns the line, pasted text —
        // newlines included — lands in the composer via insert_paste
        // and never touches the PTY. Routing through the byte path
        // would submit at the first newline instead. A false return
        // means the wasm bundle predates the export — fall through to
        // the legacy byte path below.
        this.terminalInput = adapter.terminalInput?.() ?? this.terminalInput;
        this.scheduleDraw();
        return;
      }
      // Raw-PTY paste: frame per shared neoism_ui::paste_policy —
      // bracketed sentinels (payload scrubbed of ESC/ETX) when the
      // terminal has BRACKETED_PASTE set, CR-normalised raw bytes
      // otherwise — exactly like desktop's Screen::paste PTY branch.
      const payload = adapter?.terminalPastePayload?.(text);
      if (payload) {
        this.sendPtyInput(payload);
        return;
      }
    }
    this.pasteTextToActiveSurface(text);
  }

  /// Send each pasted image file to the daemon as
  /// `MaterializeClipboardImage`. The matching
  /// `ClipboardImageMaterialized` reply is handled in
  /// `ingestClipboardImageMaterialized` (recorded in
  /// `pendingClipboardImages` against an opaque `request_id`) so the
  /// reply can re-focus the pane that initiated the paste.
  private async submitEditorPastedImages(
    items: DataTransferItem[],
  ): Promise<void> {
    const originPaneId = this.activePaneExternalId();
    for (const item of items) {
      const file = item.getAsFile();
      if (!file) continue;
      const bytes = new Uint8Array(await file.arrayBuffer());
      const payload: ClipboardPayload = {
        mime_type: file.type || item.type || "image/png",
        text: null,
        bytes: Array.from(bytes),
        filename: file.name || null,
      };
      const requestId = `clip-${this.nextClipboardRequestId++}`;
      // Remember which pane started this paste so the async reply
      // dispatches `:edit` to the right surface even if focus moved.
      this.pendingClipboardImages.set(requestId, originPaneId);
      this.options.client.sendWorkspace({
        MaterializeClipboardImage: { payload, request_id: requestId },
      });
    }
  }

  private ingestClipboardImageMaterialized(payload: {
    path: string;
    mime_type: string;
    filename: string | null;
    request_id?: string | null;
  }): void {
    // Resolve the originating pane from the correlation table. If the
    // daemon stripped the id (older binary) or we never recorded one,
    // fall back to the focused surface — the legacy race is still
    // possible there but explicit so the fallback is easy to spot.
    let originPaneId: number | null = null;
    if (payload.request_id) {
      const recorded = this.pendingClipboardImages.get(payload.request_id);
      if (recorded !== undefined) {
        originPaneId = recorded;
        this.pendingClipboardImages.delete(payload.request_id);
      }
    }

    // Re-focus the originating pane, then surface the saved image.
    // `activatePaneExternalId` is a no-op if the pane is already
    // focused (or no longer exists).
    if (originPaneId !== null) {
      this.activatePaneExternalId(originPaneId, true);
    }

    // Web frontend has no shared filesystem with the daemon and no
    // sixel/kitty graphics protocol in the wasm terminal renderer, so
    // we can't preview the bytes inline. Surface a daemon HTTP URL
    // so the user can pop the image in a fresh tab — this is the
    // best we can do without bootstrapping a dedicated viewer pane.
    const filename = payload.path.split(/[\\/]/).pop() ?? "";
    const httpBase = this.options.client.getDaemonHttpBase();
    const url =
      filename && httpBase
        ? `${httpBase}/clipboard-image/${encodeURIComponent(filename)}`
        : null;
    this.wasmAdapter?.pushNotification?.(
      JSON.stringify({
        title: "Clipboard image saved",
        message: url ?? payload.path,
        severity: "info",
      }),
    );
    if (url) {
      try {
        window.open(url, "_blank", "noopener");
      } catch {
        // Popups blocked — the notification already carries the URL
        // so the user can click through manually.
      }
    }
  }

  private pasteTextToActiveSurface(text: string): void {
    this.handleInputBytes(new TextEncoder().encode(text));
  }

  private async submitPastedImages(
    items: DataTransferItem[],
    textFromClipboard: string,
  ): Promise<void> {
    const attachments: Attachment[] = [];
    for (const item of items) {
      const file = item.getAsFile();
      if (!file) continue;
      const bytes = new Uint8Array(await file.arrayBuffer());
      const payload: ClipboardPayload = {
        mime_type: file.type || item.type || "image/png",
        text: textFromClipboard || null,
        bytes: Array.from(bytes),
        filename: file.name || null,
      };
      if (!this.workspaceClipboardPayload) {
        this.workspaceClipboardPayload = payload;
      }
      this.options.client.sendWorkspace({ StoreClipboard: { payload } });
      attachments.push({
        kind: payload.mime_type,
        path: file.name || null,
        bytes: payload.bytes ?? [],
      });
    }
    if (attachments.length === 0) return;

    const text = (this.agentInput || textFromClipboard).trim();
    this.wasmAdapter?.agentSendMessageWithAttachments?.(
      text,
      JSON.stringify(attachments),
    );
    this.agentInput = "";
    this.scheduleDraw();
  }

  /// Desktop-parity paste flow: each clipboard image becomes an
  /// `[imageN]` composer token + chip via the shared pane's
  /// `attach_clipboard_image`; the prompt (image included) sends on
  /// the next Enter through the ordinary submit path. Files are
  /// snapshotted synchronously — `DataTransferItem`s are neutered
  /// once the paste handler returns.
  private async attachPastedImagesToAgent(
    items: DataTransferItem[],
  ): Promise<void> {
    const files = items
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    let attached = false;
    for (const file of files) {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const ok =
        this.wasmAdapter?.agentAttachClipboardImage?.(
          file.name ?? "",
          file.type || "image/png",
          bytes,
        ) === true;
      if (ok) {
        attached = true;
      } else {
        this.pushInAppNotification(
          "Attachment failed",
          `Could not attach ${file.name || "pasted image"} (empty or over 20MB).`,
          "warn",
        );
      }
    }
    if (attached) {
      this.agentInput = this.wasmAdapter?.agentInput?.() ?? this.agentInput;
      this.scheduleDraw();
    }
  }

  /// Allow dropping files onto the agent pane (dragover must
  /// preventDefault for the drop event to fire). Other surfaces keep
  /// the browser default.
  private handleAgentDragOver(event: DragEvent): void {
    if (this.activeSurface() !== "agent") return;
    if (!this.wasmAdapter?.agentAttachFile) return;
    const transfer = event.dataTransfer;
    if (!transfer || !Array.from(transfer.types).includes("Files")) return;
    event.preventDefault();
    transfer.dropEffect = "copy";
  }

  /// Drag-and-drop a file onto the agent pane → composer attachment —
  /// the web analogue of desktop's `DroppedFile` → `attach_path`
  /// (app/window_event/dnd.rs). Bytes are read host-side and attached
  /// through the shared pane, so tokens/chips/20MB-cap match desktop.
  private async handleAgentFileDrop(event: DragEvent): Promise<void> {
    if (this.activeSurface() !== "agent") return;
    const adapter = this.wasmAdapter;
    if (!adapter?.agentAttachFile) return;
    const files = Array.from(event.dataTransfer?.files ?? []);
    if (files.length === 0) return;
    event.preventDefault();
    let attached = false;
    for (const file of files) {
      const bytes = new Uint8Array(await file.arrayBuffer());
      // Empty mime → the shared pane sniffs from the file extension.
      const ok = adapter.agentAttachFile(file.name || "file", file.type ?? "", bytes);
      if (ok) {
        attached = true;
      } else {
        this.pushInAppNotification(
          "Attachment failed",
          `Could not attach ${file.name || "file"} (empty or over 20MB).`,
          "warn",
        );
      }
    }
    if (attached) {
      this.agentInput = adapter.agentInput?.() ?? this.agentInput;
      this.scheduleDraw();
    }
  }

  /// Keep the shared `@file` mention picker supplied with candidates.
  /// The pane exposes the active mention query; when one is live and
  /// the cached workspace file list is stale (or absent), fetch it via
  /// the daemon's Files surface and feed it in. Ranking/filtering per
  /// keystroke happens pane-side (desktop `fuzzy_score` policy), so
  /// this only refreshes the LIST, not the match set.
  private maybeFeedAgentFileMentions(): void {
    const adapter = this.wasmAdapter;
    if (!adapter?.agentFileMentionQuery || !adapter.agentSetFileMentionCandidates) {
      return;
    }
    if (adapter.agentFileMentionQuery() === null) return;
    const now = Date.now();
    if (
      this.agentMentionFileCache !== null &&
      now - this.agentMentionFileCache.at < 15000
    ) {
      return;
    }
    if (this.agentMentionFetchInFlight) return;
    this.agentMentionFetchInFlight = true;
    void this.collectAgentMentionFiles()
      .then((paths) => {
        this.agentMentionFileCache = { paths, at: Date.now() };
        if (
          this.wasmAdapter?.agentSetFileMentionCandidates?.(JSON.stringify(paths))
        ) {
          this.scheduleDraw();
        }
      })
      .catch(() => {
        // Daemon hiccup — the next `@` keystroke retries.
      })
      .finally(() => {
        this.agentMentionFetchInFlight = false;
      });
  }

  /// Lightweight workspace file listing for `@` mentions: recursive
  /// daemon `ListDir` (the same surface the notes sidebar walks),
  /// skipping hidden entries plus desktop's historical mention
  /// exclude set (`file_mention_ignored_component`), bounded by
  /// depth 8 / 2000 files / 400 directory reads.
  private async collectAgentMentionFiles(): Promise<string[]> {
    const ignored = new Set([
      ".git",
      ".claude",
      ".cache",
      ".direnv",
      ".neoism",
      ".next",
      "build",
      "dist",
      "node_modules",
      "target",
    ]);
    const files: string[] = [];
    let dirBudget = 400;
    const listDir = async (dir: string, depth: number): Promise<void> => {
      if (depth > 8 || files.length >= 2000 || dirBudget <= 0) return;
      dirBudget -= 1;
      let reply: unknown;
      try {
        reply = await this.options.client.requestFiles(
          { ListDir: { path: dir } },
          this.options.workspaceRoot ?? null,
        );
      } catch {
        return;
      }
      if (!reply || typeof reply !== "object" || !("DirListing" in reply)) {
        return;
      }
      const entries = (
        reply as {
          DirListing: { entries: Array<{ name: string; is_dir: boolean }> };
        }
      ).DirListing.entries;
      const subdirs: string[] = [];
      for (const entry of entries) {
        if (entry.name.startsWith(".") || ignored.has(entry.name)) continue;
        const path = dir === "." ? entry.name : `${dir}/${entry.name}`;
        if (entry.is_dir) {
          subdirs.push(path);
        } else if (files.length < 2000) {
          files.push(path);
        }
      }
      for (const subdir of subdirs) {
        await listDir(subdir, depth + 1);
      }
    };
    await listDir(".", 0);
    return files;
  }

  private handleInputBytes(bytes: Uint8Array): void {
    if (this.routeInputBytesToChrome(bytes)) {
      return;
    }
    // Buffer-tab routing: the bridge exposes `active_surface()` as the
    // single source of truth so JS and Rust agree without an extra
    // cache.
    const surface = this.activeSurface();
    if (surface === "agent" && this.routeInputBytesToAgent(bytes)) {
      return;
    }
    if (surface === "markdown" && this.routeInputBytesToMarkdown(bytes)) {
      return;
    }
    if (surface === "editor") {
      // Chrome-hosted editor panes consume file-tab bytes (the soft
      // keyboard path — desktop keydowns already routed above). Bytes
      // are swallowed either way: leaking them into the PTY would
      // type into a shell the user isn't looking at.
      this.routeInputBytesToEditor(bytes);
      return;
    }
    // Prefer the live capture check: it reads shell state directly, so
    // it's correct during the fresh-terminal boot window (before the
    // first OSC 133 prompt) and while the composer holds a pending
    // command — cases where the render-synced visibility flag can lag
    // and let early keystrokes leak to the raw PTY, splitting the typed
    // command across two sinks. Fall back to the visibility flag for
    // adapters built before the capture export existed.
    const composerOwnsInput =
      this.wasmAdapter?.terminalShouldCaptureInput?.() ??
      this.wasmAdapter?.terminalCommandComposerVisible?.() === true;
    if (
      surface === "terminal" &&
      composerOwnsInput === true &&
      this.routeTerminalComposerInput(bytes)
    ) {
      return;
    }
    this.mirrorTerminalInput(bytes);
    this.sendPtyInput(bytes);
  }

  /// Route shell input through the formal `PtyService` when supplied,
  /// falling back to the raw protocol client for back-compat with hosts
  /// that wired the panel up before the service existed.
  private sendPtyInput(bytes: Uint8Array): void {
    const sessionId = this.activePtySessionId() ?? this.options.sessionId;
    if (this.options.pty) {
      this.options.pty.sendInput(sessionId, bytes);
      return;
    }
    this.options.client.sendInput(sessionId, bytes);
  }

  /// Same back-compat shape as `sendPtyInput` for SIGWINCH-style
  /// resize notifications.
  private resizePty(cols: number, rows: number): void {
    if (this.options.pty) {
      const sessions = new Set<string>();
      for (const tab of this.bufferTabs) {
        if (tab.kind === "terminal" && tab.sessionId) {
          sessions.add(tab.sessionId);
        }
      }
      if (sessions.size === 0) {
        sessions.add(this.options.sessionId);
      }
      for (const sessionId of sessions) {
        this.options.pty.resize(sessionId, cols, rows);
      }
      return;
    }
    this.options.client.resize(this.activePtySessionId() ?? this.options.sessionId, cols, rows);
  }

  private routeTerminalComposerInput(bytes: Uint8Array): boolean {
    if (bytes.length === 0) return true;
    const adapter = this.wasmAdapter;
    if (!adapter) return false;

    const syncInput = () => {
      this.terminalInput = adapter.terminalInput?.() ?? "";
      this.scheduleDraw();
    };
    const key = keyNameFromTerminalBytes(bytes);
    if (key) {
      if (key === "Enter") {
        const command = adapter.terminalInput?.() ?? this.terminalInput;
        const payload = adapter.terminalSubmitPayload?.() ?? new Uint8Array();
        if (isClearCommand(command)) {
          adapter.resetTerminalSplash?.();
        } else {
          adapter.dismissTerminalSplash?.();
        }
        syncInput();
        if (payload.length > 0) this.sendPtyInput(payload);
        return true;
      }
      if (key === "Ctrl+L") {
        // Desktop parity (block_overlay.rs:810-824): form-feed to the
        // PTY so the shell repaints a fresh prompt, drop block history
        // + scroll anchor in wasm, and bring the splash back like a
        // `clear`. Composer text is preserved, as on desktop.
        adapter.terminalInputKey?.(key);
        adapter.resetTerminalSplash?.();
        syncInput();
        this.sendPtyInput(Uint8Array.of(0x0c));
        return true;
      }
      if (key === "Ctrl+C") {
        // Desktop parity (block_overlay.rs:756-777): with the composer
        // owning the line there is no foreground readline to interrupt.
        // wasm shows the ^C notice (clearing pending text first); the
        // ETX is swallowed in both the empty and non-empty case.
        adapter.terminalInputKey?.(key);
        syncInput();
        return true;
      }
      if (key === "Ctrl+D") {
        if ((adapter.terminalInput?.() ?? this.terminalInput).length === 0) {
          // Empty composer: EOF belongs to the shell — desktop writes
          // the 0x04 straight through (block_overlay.rs:789-801).
          return false;
        }
        // Non-empty: delete-forward, like desktop (not clear-line).
        adapter.terminalInputKey?.(key);
        syncInput();
        return true;
      }
      if (key === "Escape") {
        // Consumed only when a completion menu was dismissed;
        // otherwise ESC belongs to the PTY (desktop :850-858).
        const consumed = adapter.terminalInputKey?.(key) === true;
        syncInput();
        return consumed;
      }
      // Everything else named by keyNameFromTerminalBytes — including
      // Ctrl+W/U/K/A/E (line editing), Ctrl+R (history picker),
      // Ctrl+F (favorites picker), and Shift+Enter (newline) — is
      // handled inside wasm terminal_input_key and always consumed by
      // the composer, matching desktop block_overlay.rs:713-935.
      adapter.terminalInputKey?.(key);
      syncInput();
      return true;
    }

    if (bytes[0] === 0x1b) return false;
    const text = new TextDecoder().decode(bytes);
    if (!text || /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(text)) return false;
    const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    const newline = normalized.search(/\n/);
    if (newline >= 0) {
      const prefix = normalized.slice(0, newline);
      if (prefix) adapter.terminalInputInsert?.(prefix);
      const command = adapter.terminalInput?.() ?? this.terminalInput;
      const payload = adapter.terminalSubmitPayload?.() ?? new Uint8Array();
      if (isClearCommand(command)) {
        adapter.resetTerminalSplash?.();
      } else {
        adapter.dismissTerminalSplash?.();
      }
      syncInput();
      if (payload.length > 0) this.sendPtyInput(payload);
      return true;
    }
    adapter.terminalInputInsert?.(normalized);
    syncInput();
    return true;
  }

  private mirrorTerminalInput(bytes: Uint8Array): void {
    if (bytes.length === 0) return;
    if (bytes.length === 1) {
      const byte = bytes[0];
      if (byte === 0x0d) {
        const command = this.terminalInput;
        this.wasmAdapter?.recordTerminalSubmit?.(command);
        if (isClearCommand(command)) {
          this.wasmAdapter?.resetTerminalSplash?.();
        } else {
          this.wasmAdapter?.dismissTerminalSplash?.();
        }
        this.setTerminalInput("");
        return;
      }
      if (byte === 0x0c) {
        this.wasmAdapter?.resetTerminalSplash?.();
        this.setTerminalInput("");
        return;
      }
      if (byte === 0x03 || byte === 0x04) {
        this.setTerminalInput("");
        return;
      }
      if (byte === 0x7f || byte === 0x08) {
        this.setTerminalInput(this.terminalInput.slice(0, -1));
        return;
      }
      if (byte < 0x20 || byte === 0x7f) return;
    } else if (bytes[0] === 0x1b) {
      return;
    }

    const text = new TextDecoder().decode(bytes);
    if (!text || /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(text)) return;
    const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    const newline = normalized.search(/\n/);
    if (newline >= 0) {
      const command = `${this.terminalInput}${normalized.slice(0, newline)}`;
      this.wasmAdapter?.recordTerminalSubmit?.(command);
      if (isClearCommand(command)) {
        this.wasmAdapter?.resetTerminalSplash?.();
      } else {
        this.wasmAdapter?.dismissTerminalSplash?.();
      }
      this.setTerminalInput("");
      return;
    }
    this.setTerminalInput(this.terminalInput + normalized);
  }

  private setTerminalInput(text: string): void {
    this.terminalInput = text;
    this.wasmAdapter?.setTerminalInput?.(text);
    this.scheduleDraw();
  }

  /** Soft-keyboard bytes → markdown pane keystrokes. The pane's vim
   *  keymap takes `event.key`-style names, so map the control bytes
   *  the contenteditable capture emits and feed printable text char
   *  by char. Markdown owns its bytes either way — leaking them to
   *  the PTY typed into a shell nobody can see. */
  private routeInputBytesToMarkdown(bytes: Uint8Array): boolean {
    if (!this.useWasmMarkdown()) return false;
    const adapter = this.wasmAdapter as {
      markdownKey?: (key: string, ctrl: boolean) => boolean;
    };
    if (!adapter?.markdownKey) return false;
    const text = new TextDecoder().decode(bytes);
    let handled = false;
    for (const ch of text) {
      const key =
        ch === "\r" || ch === "\n"
          ? "Enter"
          : ch === "\x7f" || ch === "\b"
            ? "Backspace"
            : ch === "\x1b"
              ? "Escape"
              : ch === "\t"
                ? "Tab"
                : ch;
      if (adapter.markdownKey(key, false)) {
        handled = true;
      }
    }
    if (handled) {
      this.pumpCrdtOutbox();
      this.scheduleDraw();
      this.pumpMarkdownAnimation();
    }
    return true;
  }

  /** Soft-keyboard bytes → editor-pane keystrokes (the mobile twin of
   *  the keydown routing; desktop browsers never reach this because
   *  `editorKey` consumed the keydown). Bytes are always swallowed. */
  private routeInputBytesToEditor(bytes: Uint8Array): void {
    if (this.activeEditorPaneKind() === null) return;
    const adapter = this.wasmAdapter as {
      editorKey?: (
        key: string,
        ctrl: boolean,
        shift: boolean,
        alt: boolean,
      ) => boolean;
    };
    if (!adapter?.editorKey) return;
    const text = new TextDecoder().decode(bytes);
    let handled = false;
    for (const ch of text) {
      const key =
        ch === "\r" || ch === "\n"
          ? "Enter"
          : ch === "\x7f" || ch === "\b"
            ? "Backspace"
            : ch === "\x1b"
              ? "Escape"
              : ch === "\t"
                ? "Tab"
                : ch;
      if (adapter.editorKey(key, false, false, false)) {
        handled = true;
      }
    }
    if (handled) {
      this.pumpCodeCrdt();
      this.scheduleDraw();
    }
  }

  private routeInputBytesToChrome(bytes: Uint8Array): boolean {
    if (!this.isChromeKeyboardCaptureActive()) return false;
    if (bytes.length === 1) {
      if (bytes[0] === 0x0d) {
        this.forwardChromeEvent(fromKeyPressEvent({ key: "Enter" }));
        return true;
      }
      if (bytes[0] === 0x7f) {
        this.forwardChromeEvent(fromKeyPressEvent({ key: "Backspace" }));
        return true;
      }
      if (bytes[0] === 0x1b) {
        this.forwardChromeEvent(fromKeyPressEvent({ key: "Escape" }));
        return true;
      }
    }
    const text = new TextDecoder().decode(bytes);
    if (text.length > 0) {
      this.forwardChromeEvent(fromTextEvent(text));
    }
    return true;
  }
}

function matchesKey(event: KeyboardEvent, code: string, key: string): boolean {
  return event.code === code || event.key.toLowerCase() === key;
}

function digitKey(event: KeyboardEvent): number | null {
  if (/^Digit[0-9]$/.test(event.code)) {
    return Number(event.code.slice("Digit".length));
  }
  if (/^[0-9]$/.test(event.key)) {
    return Number(event.key);
  }
  return null;
}

function pointInRect(
  point: { x: number; y: number },
  rect: ChromeRect,
): boolean {
  return (
    point.x >= rect.x &&
    point.y >= rect.y &&
    point.x < rect.x + rect.w &&
    point.y < rect.y + rect.h
  );
}

function wheelDeltaYPixels(event: WheelEvent): number {
  switch (event.deltaMode) {
    case WheelEvent.DOM_DELTA_LINE:
      return event.deltaY * 16;
    case WheelEvent.DOM_DELTA_PAGE:
      return event.deltaY * window.innerHeight;
    default:
      return event.deltaY;
  }
}

function wheelDeltaXPixels(event: WheelEvent): number {
  switch (event.deltaMode) {
    case WheelEvent.DOM_DELTA_LINE:
      return event.deltaX * 48;
    case WheelEvent.DOM_DELTA_PAGE:
      return event.deltaX * window.innerWidth;
    default:
      return event.deltaX;
  }
}

function debugAgentTimeline(event: string, payload: Record<string, unknown>): void {
  try {
    const enabled = localStorage.getItem("neoism_debug_agent_timeline");
    if (enabled !== "1" && enabled?.toLowerCase() !== "true") return;
    console.warn("[neoism-agent-timeline:web]", event, payload);
  } catch {
    // Debug logging is best-effort; localStorage can be unavailable.
  }
}

function isArrowKey(key: string): boolean {
  return (
    key === "ArrowLeft" ||
    key === "ArrowRight" ||
    key === "ArrowUp" ||
    key === "ArrowDown"
  );
}

function arrowKeyDirection(key: string): "up" | "down" | "left" | "right" {
  switch (key) {
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    default:
      throw new Error(`not an arrow key: ${key}`);
  }
}

function matchesCommandColon(event: KeyboardEvent): boolean {
  return event.code === "Semicolon" || event.key === ":" || event.key === ";";
}

function isClearCommand(command: string): boolean {
  const trimmed = command.trim();
  return trimmed === "clear" || trimmed.startsWith("clear ");
}

interface ChromeDiffLine {
  Context?: string;
  Added?: string;
  Removed?: string;
}

interface ChromeDiffHunk {
  old_start: number;
  new_start: number;
  lines: ChromeDiffLine[];
}

interface ChromeDiffFile {
  path: string;
  hunks: ChromeDiffHunk[];
  added: number;
  removed: number;
}

function diffFilesFromWire(hunks: WireDiffHunk[]): ChromeDiffFile[] {
  const byPath = new Map<string, ChromeDiffFile>();
  for (const hunk of hunks) {
    let file = byPath.get(hunk.path);
    if (!file) {
      file = { path: hunk.path, hunks: [], added: 0, removed: 0 };
      byPath.set(hunk.path, file);
    }
    const lines: ChromeDiffLine[] = [];
    for (const line of hunk.patch.split("\n")) {
      if (line.startsWith("@@")) continue;
      if (line.startsWith("+") && !line.startsWith("+++")) {
        lines.push({ Added: line.slice(1) });
        file.added += 1;
      } else if (line.startsWith("-") && !line.startsWith("---")) {
        lines.push({ Removed: line.slice(1) });
        file.removed += 1;
      } else {
        lines.push({ Context: line.startsWith(" ") ? line.slice(1) : line });
      }
    }
    file.hunks.push({
      old_start: hunk.old_start,
      new_start: hunk.new_start,
      lines,
    });
  }
  return Array.from(byPath.values());
}

/**
 * PTY key encoder entry point (desktop parity).
 *
 * For the terminal surface, delegates to the wasm export
 * `encode_terminal_key`, which walks the exact desktop pipeline —
 * bindings Esc table (DECCKM SS3 arrows/Home/End, `ESC[2~`-style
 * tildes, Backspace family, F1–F4 SS3, Shift+Tab) → alt-as-meta
 * masking → the shared `should_build_key_sequence` fork between the
 * kitty keyboard protocol builder and the raw UTF-8 path — against the
 * LIVE terminal modes. This is what gives web F-keys, Home/End/Insert/
 * Delete, modified arrows, Alt/Meta ESC prefixes, app-cursor SS3 and
 * kitty protocol support identical to desktop.
 *
 * Non-terminal surfaces (agent / markdown / editor byte routers) keep
 * the legacy `keyEventToBytes` vocabulary their decoders expect, as
 * does any host running a stale wasm bundle without the export.
 *
 * Returns null when the key produces no PTY-bound bytes (the event is
 * left to the browser / other handlers, matching the previous
 * behavior).
 */
function encodePtyKeyEvent(
  event: KeyboardEvent,
  surface: string,
  adapter: {
    encodeTerminalKey?: (
      key: string,
      code: string,
      ctrl: boolean,
      alt: boolean,
      shift: boolean,
      meta: boolean,
      repeat: boolean,
    ) => Uint8Array | null;
    terminalShouldCaptureInput?: () => boolean;
    terminalCommandComposerVisible?: () => boolean;
  } | null,
): Uint8Array | null {
  if (surface !== "terminal" || !adapter?.encodeTerminalKey) {
    // Stale-wasm-bundle safety + non-terminal surfaces: legacy table.
    return keyEventToBytes(event);
  }
  // Composer-only Shift+Enter transport: while the composer owns the
  // line, emit the CSI-u disambiguated form so the byte-name router
  // maps it to "Shift+Enter" (newline instead of submit). Desktop
  // handles Shift+Enter inside block_overlay BEFORE byte encoding, so
  // this never reaches the PTY — routeTerminalComposerInput consumes
  // it. Outside composer capture the wasm encoder decides (legacy
  // 0x0d, kitty ESC[13;2u), exactly like desktop.
  if (
    event.key === "Enter" &&
    event.shiftKey &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.metaKey &&
    (adapter.terminalShouldCaptureInput?.() ??
      adapter.terminalCommandComposerVisible?.() === true)
  ) {
    return new TextEncoder().encode("\x1b[13;2u");
  }
  const bytes = adapter.encodeTerminalKey(
    event.key,
    event.code,
    event.ctrlKey,
    event.altKey,
    event.shiftKey,
    event.metaKey,
    event.repeat,
  );
  if (bytes === null || bytes === undefined) {
    // Adapter present but export missing (older bundle): legacy table.
    return keyEventToBytes(event);
  }
  // Empty = not PTY-bound in the current terminal mode (consumed
  // host-side on desktop, or no representation). Swallow nothing:
  // returning null leaves the event to the browser, matching the
  // legacy null path.
  return bytes.length > 0 ? bytes : null;
}

/**
 * Legacy hand-rolled fallback table. ONLY used when the wasm bundle
 * predates `encode_terminal_key`, and for non-terminal surfaces whose
 * byte routers (agent / markdown / editor) decode this fixed
 * vocabulary. The terminal PTY path goes through `encodePtyKeyEvent`
 * above — do not extend this table for terminal keys.
 */
function keyEventToBytes(event: KeyboardEvent): Uint8Array | null {
  if (event.ctrlKey && event.key.length === 1) {
    const code = event.key.toLowerCase().charCodeAt(0);
    if (code >= 97 && code <= 122) {
      return Uint8Array.of(code - 96);
    }
  }
  switch (event.key) {
    case "Enter":
      return Uint8Array.of(0x0d);
    case "Backspace":
      return Uint8Array.of(0x7f);
    case "Tab":
      if (event.shiftKey) return new TextEncoder().encode("\x1b[Z");
      return Uint8Array.of(0x09);
    case "Escape":
      return Uint8Array.of(0x1b);
    case "ArrowUp":
      return new TextEncoder().encode("\x1b[A");
    case "ArrowDown":
      return new TextEncoder().encode("\x1b[B");
    case "ArrowRight":
      return new TextEncoder().encode("\x1b[C");
    case "ArrowLeft":
      return new TextEncoder().encode("\x1b[D");
    default:
      break;
  }
  if (event.key.length === 1 && !event.metaKey && !event.altKey) {
    return new TextEncoder().encode(event.key);
  }
  return null;
}

function keyNameFromTerminalBytes(bytes: Uint8Array): string | null {
  if (bytes.length === 1) {
    switch (bytes[0]) {
      case 0x01:
        return "Ctrl+A";
      case 0x03:
        return "Ctrl+C";
      case 0x04:
        return "Ctrl+D";
      case 0x05:
        return "Ctrl+E";
      case 0x06:
        return "Ctrl+F";
      case 0x09:
        return "Tab";
      case 0x0b:
        return "Ctrl+K";
      case 0x0c:
        return "Ctrl+L";
      case 0x0d:
        return "Enter";
      case 0x12:
        return "Ctrl+R";
      case 0x15:
        return "Ctrl+U";
      case 0x17:
        return "Ctrl+W";
      case 0x1b:
        return "Escape";
      case 0x7f:
      case 0x08:
        return "Backspace";
      default:
        return null;
    }
  }
  const text = new TextDecoder().decode(bytes);
  switch (text) {
    // Both the CSI and the DECCKM app-cursor SS3 form: shells (zsh
    // zle) set application cursor keys at the prompt, so the
    // desktop-parity encoder emits ESC O A there — the composer must
    // recognize both spellings.
    case "\x1b[A":
    case "\x1bOA":
      return "ArrowUp";
    case "\x1b[B":
    case "\x1bOB":
      return "ArrowDown";
    case "\x1b[C":
    case "\x1bOC":
      return "ArrowRight";
    case "\x1b[D":
    case "\x1bOD":
      return "ArrowLeft";
    case "\x1b[H":
    case "\x1bOH":
    case "\x1b[1~":
      return "Home";
    case "\x1b[F":
    case "\x1bOF":
    case "\x1b[4~":
      return "End";
    case "\x1b[3~":
      return "Delete";
    case "\x1b[Z":
      return "Shift+Tab";
    // Shift+Enter in the two standard disambiguated encodings —
    // CSI-u (kitty) and xterm modifyOtherKeys. The legacy encoding
    // collapses Shift+Enter to plain 0x0d, which is indistinguishable
    // from Enter here; the keydown encoder must emit one of these for
    // the composer's newline-instead-of-submit to engage.
    case "\x1b[13;2u":
    case "\x1b[27;2;13~":
      return "Shift+Enter";
    default:
      return null;
  }
}

function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}
