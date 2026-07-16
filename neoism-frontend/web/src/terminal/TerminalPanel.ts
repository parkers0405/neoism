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
import { NvimCanvasLayer } from "../editor/nvim/NvimCanvasLayer";
import type { NvimGridSnapshot } from "../editor/nvim/NvimGridModel";
import { MarkdownPresenceOverlay } from "./MarkdownPresenceOverlay";
import {
  localPresenceIdentity,
  presenceBufferIdForPath,
} from "../presence/presence";
import {
  PresencePublisher,
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
  EditorServerMessage,
} from "../workspace/types";
import type { SearchBridge } from "../services/SearchService";

const CELL_WIDTH = 8;
const CELL_HEIGHT = 16;
const MIN_COLS = 20;
const MIN_ROWS = 6;
const MAX_REPLAY_BYTES_PER_PTY = 2 * 1024 * 1024;
const MOBILE_SCROLL_TAP_SLOP = 10;
const TERMINAL_RESET_BYTES = new TextEncoder().encode("\x1bc\x1b[3J\x1b[H\x1b[2J");
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

function luaStringLiteral(value: string): string {
  return `"${value
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t")}"`;
}

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

interface EditorWheelAnchor {
  x: number;
  y: number;
  modifier: string;
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

interface EditorGridCellSnapshot {
  ch: string;
  fg: number;
  bg: number;
  attrs: number;
}

interface EditorGridSnapshot {
  width: number;
  height: number;
  cells: EditorGridCellSnapshot[];
  cursor: [number, number] | null;
  default_fg: number;
  default_bg: number;
}

interface NvimCanvasPixelStats {
  width: number;
  height: number;
  sampledPixels: number;
  nonBackgroundPixels: number;
}

interface NvimSmokeSnapshot {
  surfaceId: string | null;
  width: number;
  height: number;
  mode: string | null;
  cursor: [number, number] | null;
  text: string;
  nonBlankCells: number;
  error: string | null;
  canvas: NvimCanvasPixelStats | null;
}

interface WebNvimSmokeHook {
  openNvimBuffer(path: string): void;
  closeNvimBuffer(path: string): void;
  sendNvimKeys(keys: string): void;
  snapshot(surfaceId?: string | null): NvimSmokeSnapshot | null;
  canvasStats(): NvimCanvasPixelStats | null;
  activeSurfaceId(): string | null;
  cachedSurfaceIds(): string[];
}

declare global {
  interface Window {
    __neoismE2E?: {
      terminal?: WebNvimSmokeHook;
    };
  }
}

function editorReplySurfaceId(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const rec = payload as Record<string, unknown>;
  const first = Object.values(rec)[0];
  if (!first || typeof first !== "object") return null;
  const surfaceId = (first as Record<string, unknown>).surface_id;
  return typeof surfaceId === "string" && surfaceId.length > 0 ? surfaceId : null;
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

function parseEditorGridSnapshotJson(json: string | undefined): EditorGridSnapshot | null {
  if (!json) return null;
  try {
    const raw = JSON.parse(json);
    if (!raw || typeof raw !== "object") return null;
    const rec = raw as Record<string, unknown>;
    const width = rec.width;
    const height = rec.height;
    const cells = rec.cells;
    if (
      typeof width !== "number" ||
      typeof height !== "number" ||
      !Number.isFinite(width) ||
      !Number.isFinite(height) ||
      !Array.isArray(cells)
    ) {
      return null;
    }
    const parsedCells: EditorGridCellSnapshot[] = [];
    for (const rawCell of cells) {
      if (!rawCell || typeof rawCell !== "object") {
        parsedCells.push({ ch: " ", fg: 0, bg: 0, attrs: 0 });
        continue;
      }
      const cell = rawCell as Record<string, unknown>;
      parsedCells.push({
        ch: typeof cell.ch === "string" ? cell.ch : " ",
        fg: typeof cell.fg === "number" ? cell.fg : 0,
        bg: typeof cell.bg === "number" ? cell.bg : 0,
        attrs: typeof cell.attrs === "number" ? cell.attrs : 0,
      });
    }
    if (parsedCells.length < width * height) return null;
    const cursor = Array.isArray(rec.cursor)
      ? rec.cursor.length >= 2 &&
        typeof rec.cursor[0] === "number" &&
        typeof rec.cursor[1] === "number"
        ? ([rec.cursor[0], rec.cursor[1]] as [number, number])
        : null
      : null;
    return {
      width: Math.max(0, Math.trunc(width)),
      height: Math.max(0, Math.trunc(height)),
      cells: parsedCells,
      cursor,
      default_fg: typeof rec.default_fg === "number" ? rec.default_fg : 0xe6edf3,
      default_bg: typeof rec.default_bg === "number" ? rec.default_bg : 0x000000,
    };
  } catch {
    return null;
  }
}

function packedRgbCss(value: number): string {
  const n = Math.max(0, Math.min(0xffffff, Math.trunc(value)));
  return `#${n.toString(16).padStart(6, "0")}`;
}

function nvimSnapshotText(snapshot: NvimGridSnapshot): string {
  const rows: string[] = [];
  for (let row = 0; row < snapshot.height; row += 1) {
    const start = row * snapshot.width;
    rows.push(
      snapshot.cells
        .slice(start, start + snapshot.width)
        .map((cell) => cell.ch || " ")
        .join("")
        .trimEnd(),
    );
  }
  return rows.join("\n").trimEnd();
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
  onBridgeReady?: (bridge: SearchBridge) => void;
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
  private readonly nvimLayer: NvimCanvasLayer;
  private e2eHook: WebNvimSmokeHook | null = null;
  private readonly paneOverlay: HTMLDivElement;
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
  private activeThemeName: (typeof WEB_IDE_THEMES)[number] = "pastel_dark";
  private fallbackFontFamily =
    "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', 'Apple Color Emoji', 'Segoe UI Emoji', 'Noto Color Emoji', monospace";
  private activeShaderFilter: string | null = null;
  private paneOverlaySuppressed = false;
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
  // owns so nvim / pty never see the candidate-list navigation keys.
  private imeComposing = false;
  private readonly compositionStartHandler: (event: CompositionEvent) => void;
  private readonly compositionUpdateHandler: (event: CompositionEvent) => void;
  private readonly compositionEndHandler: (event: CompositionEvent) => void;
  private readonly pasteHandler: (event: ClipboardEvent) => void;
  private readonly contextMenuHandler: (event: MouseEvent) => void;
  // Touch handlers — C3 polish. The classifier (`touchPolicy.ts`)
  // mirrors `neoism-frontend/shared/src/touch_policy.rs` 1:1 so the
  // tap-vs-drag-vs-pinch-vs-pan state machine and long-press timer
  // match the desktop fork's behaviour. Side effects are applied by
  // `applyTouchAction`. `touchLongPressTimer` polls
  // `tickLongPress` while a finger is held inside the tap radius.
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
    // publishes (scroll-driven), and client-side TTL pruning. Cursor
    // moves additionally pump synchronously from `editorReply`.
    this.presenceTimer = setInterval(() => {
      this.pumpPresence();
      // Safety-net CRDT flush: keystrokes pump synchronously from the
      // key handler; this catches mutations from other entry points
      // (paste, checkbox toggles, drags) within a frame or two.
      this.pumpCrdtOutbox();
      this.pollOpenMarkdownTabs();
      if (this.remotePresence.pruneStale(Date.now(), 15_000)) {
        this.syncMarkdownPresenceOverlay();
        this.scheduleDraw();
      }
    }, 250);
    this.nvimLayer = new NvimCanvasLayer({
      mount: this.root,
      sendEditor: (message) => this.sendEditorMessage(message),
      activeSurfaceId: () => this.focusedEditorSurfaceId(),
      focusHost: () => this.focus(),
    });
    this.nvimLayer.setVisible(false);
    this.installE2eHook();
    this.paneOverlay = document.createElement("div");
    this.paneOverlay.className = "terminal-pane-layout-overlay";
    this.root.appendChild(this.paneOverlay);

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
            this.wasmAdapter as unknown as SearchBridge,
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
        // keyboard animates; reflowing nvim/PTY for every frame of
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
      this.editorPointerDragging = false;
      this.hideCustomCursor();
      this.forwardChromeEvent(pointerLeaveEvent());
    };
    this.wheelHandler = (event) => this.handleWheel(event);
    this.pasteHandler = (event) => this.handlePaste(event);
    this.contextMenuHandler = (event) => this.handleContextMenu(event);
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
    // reaches nvim or the pty — the keydown path only fires for
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
    this.clearEditorLeaderPending(false);
    this.uninstallE2eHook();
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

  private installE2eHook(): void {
    if (typeof window === "undefined") return;
    const hook: WebNvimSmokeHook = {
      openNvimBuffer: (path) => this.openNvimBufferForE2e(path),
      closeNvimBuffer: (path) => this.closeNvimBufferForE2e(path),
      sendNvimKeys: (keys) => this.sendEditorSendKeys(new TextEncoder().encode(keys)),
      snapshot: (surfaceId) => this.nvimSmokeSnapshot(surfaceId),
      canvasStats: () => this.nvimCanvasPixelStats(),
      activeSurfaceId: () => this.focusedEditorSurfaceId(),
      cachedSurfaceIds: () => this.nvimLayer.surfaceIds(),
    };
    this.e2eHook = hook;
    window.__neoismE2E = {
      ...(window.__neoismE2E ?? {}),
      terminal: hook,
    };
  }

  private uninstallE2eHook(): void {
    if (typeof window === "undefined" || !this.e2eHook) return;
    if (window.__neoismE2E?.terminal === this.e2eHook) {
      delete window.__neoismE2E.terminal;
    }
    this.e2eHook = null;
  }

  private openNvimBufferForE2e(path: string): void {
    const trimmed = path.trim();
    if (trimmed.length === 0) return;
    this.ensureSessionLayoutState();

    const existing = this.bufferTabs.findIndex((tab) => tab.path === trimmed);
    const tabIndex =
      existing >= 0
        ? existing
        : this.bufferTabs.push({
            title: trimmed.split(/[\\/]/).pop() ?? trimmed,
            kind: "file",
            path: trimmed,
          }) - 1;

    if (existing < 0) {
      this.requestFileContent(trimmed, tabIndex);
    }
    this.activeTabIndex = tabIndex;
    this.wasmAdapter?.setActiveTab?.(tabIndex);
    this.assignActiveTabToFocusedEditorPane();
    this.replayBufferTabs();
    if (!isMarkdownPath(trimmed) && this.focusedEditorSurfaceId() === null) {
      // The hook can be installed before ChromeBridge finishes booting.
      // Keep the host tab state, then let adapter-ready synchronization
      // open the buffer once a real editor surface can be bound.
      this.scheduleDraw();
      return;
    }
    this.openFileTabContent(trimmed);
    this.syncNvimLayerVisibility();
    this.scheduleDraw();
  }

  private closeNvimBufferForE2e(path: string): void {
    const trimmed = path.trim();
    if (trimmed.length === 0) return;
    const index = this.bufferTabs.findIndex((tab) => tab.path === trimmed);
    if (index < 0) return;
    this.applyBufferTabPolicy("close_index", index);
    this.scheduleDraw();
  }

  private nvimSmokeSnapshot(surfaceId?: string | null): NvimSmokeSnapshot | null {
    const requestedSurfaceId =
      surfaceId === undefined ? this.focusedEditorSurfaceId() : surfaceId;
    const snapshot =
      this.nvimLayer.snapshotForSurface(requestedSurfaceId) ??
      this.nvimLayer.activeSnapshot();
    if (!snapshot) return null;
    return {
      surfaceId: snapshot.surfaceId,
      width: snapshot.width,
      height: snapshot.height,
      mode: snapshot.mode,
      cursor: snapshot.cursor,
      text: nvimSnapshotText(snapshot),
      nonBlankCells: snapshot.cells.filter((cell) => cell.ch.trim().length > 0).length,
      error: snapshot.error,
      canvas: this.nvimCanvasPixelStats(),
    };
  }

  private nvimCanvasPixelStats(): NvimCanvasPixelStats | null {
    this.syncNvimLayerVisibility();
    const canvas = this.nvimLayer.canvas;
    const wasHidden = canvas.hidden;
    try {
      if (wasHidden) {
        this.nvimLayer.setVisible(true);
      } else {
        this.nvimLayer.render();
      }
      if (canvas.width <= 0 || canvas.height <= 0) return null;
      const ctx = canvas.getContext("2d");
      if (!ctx) return null;
      const { width, height } = canvas;
      const image = ctx.getImageData(0, 0, width, height);
      const data = image.data;
      const bgR = data[0] ?? 0;
      const bgG = data[1] ?? 0;
      const bgB = data[2] ?? 0;
      const bgA = data[3] ?? 0;
      let nonBackgroundPixels = 0;
      for (let i = 0; i < data.length; i += 4) {
        if (
          data[i] !== bgR ||
          data[i + 1] !== bgG ||
          data[i + 2] !== bgB ||
          data[i + 3] !== bgA
        ) {
          nonBackgroundPixels += 1;
        }
      }
      return {
        width,
        height,
        sampledPixels: width * height,
        nonBackgroundPixels,
      };
    } catch {
      return null;
    } finally {
      if (wasHidden) {
        this.nvimLayer.setVisible(false);
      }
    }
  }

  // ---------------------------------------------------------------

  /// Hand a daemon `EditorReply` (nvim redraw frame) to the bridge so
  /// the file-viewer pane updates its grid snapshot. The bridge parses
  /// the JSON and pushes it onto `Chrome::editor_grid`, which the next
  /// `Chrome::draw` paints. We never round-trip through serviceReply
  /// because editor frames are unsolicited and route by panel, not by
  /// request id.
  editorReply(payload: unknown): void {
    if (!payload) return;
    try {
      const json = JSON.stringify(payload);
      // Single-hop diagnostic: log the variant tag so we can confirm in
      // devtools that EditorReply frames are arriving and being handed
      // to the wasm bridge. Truncate the JSON so long GridUpdate cell
      // arrays don't flood the console.
      const tag =
        payload && typeof payload === "object"
          ? Object.keys(payload as Record<string, unknown>)[0] ?? "<unknown>"
          : "<non-object>";
      console.debug(
        `[nvim-trace] TerminalPanel.editorReply variant=${tag} bytes=${json.length}`,
      );
      const surfaceId = editorReplySurfaceId(payload);
      const focusedSurfaceId = this.focusedEditorSurfaceId();
      const shouldRemainLive = this.shouldApplyEditorRedraw(surfaceId);
      const liveBeforeUpdate = this.liveEditorGridSurfaceId ?? focusedSurfaceId;
      this.nvimLayer.ingest(payload as EditorServerMessage);
      this.requestPresenceSnapshotForSurface(surfaceId ?? focusedSurfaceId);
      this.pumpPresence();
      if (shouldRemainLive) {
        this.wasmAdapter?.editorGridUpdate?.(json);
      } else {
        // Background surface (another tab): cache the frame without
        // touching the live grid — applying it live and restoring
        // afterwards flashed the other tab's buffer whenever the
        // restore had no cached snapshot to fall back to (e.g. the
        // redraw storm after a font-scale resize).
        this.wasmAdapter?.editorGridUpdatePassive?.(json);
      }
      this.refreshEditorGridSurfaceSnapshotIds();
      if (shouldRemainLive) {
        this.noteLiveEditorGridSurface(surfaceId);
      } else {
        console.debug(
          `[nvim-trace] editorReply cached inactive surface=${surfaceId ?? "<primary>"}`,
        );
        const restoreSurface = liveBeforeUpdate ?? focusedSurfaceId;
        if (
          restoreSurface &&
          this.wasmAdapter?.activateEditorGridSurface?.(restoreSurface) === true
        ) {
          this.noteLiveEditorGridSurface(restoreSurface);
        } else {
          this.noteLiveEditorGridSurface(surfaceId);
        }
      }
      this.scheduleDraw();
    } catch (err) {
      console.warn("[nvim-trace] editorReply: failed to forward frame", err);
      // Drop malformed frames silently — the next redraw will catch up.
    }
  }

  private crdtInboundLogAt = new Map<string, number>();
  crdtReply(payload: CrdtServerMessage): void {
    // Wave 8D: document plane first — snapshots seed the wasm pane's
    // doc binding, syncs splice remote keystrokes into the visible
    // text (echo-guarded in wasm), `Saved` clears the doc dirty bit.
    const textChanged =
      this.wasmAdapter?.crdtApply?.(JSON.stringify(payload)) === true;
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
    this.nvimLayer.ingestPresence(payload, this.crdtPeerId);
    if (changed) {
      this.syncMarkdownPresenceOverlay();
      this.syncEditorPresenceOverlay();
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
    // Install the outbound nvim-keys bridge. The bridge's stub passes
    // a single base64 string per call; we wrap it in an
    // `EditorClientMessage::SendKeys { bytes }` envelope under the
    // `Editor` service tag the daemon dispatches in `server.rs`.
    adapter.setNvimSend?.((bytesB64) => {
      const bytes = base64ToBytes(bytesB64);
      const surfaceId = this.editorInputSurfaceId();
      this.options.client.sendEditor(
        {
          SendKeys: {
            bytes: Array.from(bytes),
            ...(surfaceId ? { surface_id: surfaceId } : {}),
          },
        },
        this.options.workspaceRoot ?? null,
      );
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
        if (this.restoreEditorGridSnapshotForSurface(externalId)) {
          this.noteLiveEditorGridSurface(surface.surface_id);
        }
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
    this.cachedEditorGridSurfaceIds.delete(surfaceId);
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
    if (typeof prefs.theme === "string" && WEB_IDE_THEMES.includes(prefs.theme as (typeof WEB_IDE_THEMES)[number])) {
      this.setIdeTheme(prefs.theme as (typeof WEB_IDE_THEMES)[number]);
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

  private clearEditorLeaderPending(flushSpace: boolean): void {
    if (this.editorLeaderTimer !== null) {
      window.clearTimeout(this.editorLeaderTimer);
      this.editorLeaderTimer = null;
    }
    const wasPending = this.editorLeaderPendingAt !== null;
    this.editorLeaderPendingAt = null;
    if (flushSpace && wasPending) {
      this.sendEditorSendKeys(new TextEncoder().encode(" "));
    }
  }

  private editorIsNormalMode(): boolean {
    const surface = this.focusedEditorSurfaceId();
    const snapshot =
      (surface ? this.nvimLayer.snapshotForSurface(surface) : null) ??
      this.nvimLayer.activeSnapshot();
    const mode = (snapshot?.mode ?? "normal").toLowerCase();
    return mode === "n" || mode === "normal" || mode.startsWith("normal");
  }

  /** Markdown mirror of the nvim Space-leader: Space then x closes
   *  the tab. Only in normal mode — insert-mode spaces are text. */
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
    };
    if (adapter?.markdownInInsertMode?.() === true) {
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

  private handleEditorLeaderShortcut(event: KeyboardEvent): boolean {
    if (this.activeSurface() !== "editor") {
      this.clearEditorLeaderPending(false);
      return false;
    }
    if (event.altKey || event.ctrlKey || event.metaKey) {
      this.clearEditorLeaderPending(true);
      return false;
    }
    if (!this.editorIsNormalMode()) {
      this.clearEditorLeaderPending(false);
      return false;
    }

    const now = performance.now();
    if (
      this.editorLeaderPendingAt !== null &&
      now - this.editorLeaderPendingAt > 900
    ) {
      this.clearEditorLeaderPending(true);
    }

    if (this.editorLeaderPendingAt !== null) {
      this.clearEditorLeaderPending(false);
      if (!event.shiftKey && matchesKey(event, "KeyX", "x")) {
        this.closeActiveBufferTab();
        return true;
      }
      this.sendEditorSendKeys(new TextEncoder().encode(" "));
      return false;
    }

    if (!event.shiftKey && event.code === "Space") {
      this.editorLeaderPendingAt = now;
      this.editorLeaderTimer = window.setTimeout(() => {
        this.clearEditorLeaderPending(true);
      }, 900);
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
    this.nvimLayer.setViewport({
      x: chromeTerminal?.x ?? 0,
      y: chromeTerminal?.y ?? 0,
      w: terminalWidth,
      h: terminalHeight,
    });
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
  private nextWebPaneId = 2;
  private agentInput = "";
  private agentLastAttachAt = 0;
  private terminalInput = "";
  private editorSessionStarted = false;
  private editorGridCols = 0;
  private editorGridRows = 0;
  private editorWheelRaf: number | null = null;
  private editorWheelAnchor: EditorWheelAnchor | null = null;
  private editorPointerDragging = false;
  private editorLeaderPendingAt: number | null = null;
  private editorLeaderTimer: number | null = null;
  private workspaceSessionId: string | null = null;
  private readonly editorSurfaceBindings = new Map<string, EditorSurfaceSummary>();
  private readonly editorResizeBySurface = new Map<string, { width: number; height: number }>();
  private readonly cachedEditorGridSurfaceIds = new Set<string>();
  private liveEditorGridSurfaceId: string | null = null;

  private drainChromeIntents(): void {
    this.drainTopBarActions();
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
      }
    }
    if (acted) this.scheduleDraw();
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
    this.syncNvimLayerVisibility();
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
  /// (same path the file-tree open intents take) plus, for grep / git
  /// hits, a follow-up `SendKeys` envelope that jumps the embedded
  /// nvim's cursor to the matching line.
  private drainFinderOpenIntents(): void {
    const intents = this.wasmAdapter?.drainFinderOpenIntents?.();
    if (!intents || intents.length === 0) return;
    let changed = false;
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
      // Grep / git hits carry a 1-based line. Send `:<n>\n` as raw
      // input bytes so nvim's `:` ex command moves the cursor; the
      // resulting `grid_cursor_goto` redraw routes back through the
      // bridge's `editor_grid_update` path.
      if (intent.line && intent.line > 0) {
        const cmd = `:${intent.line}\n`;
        this.sendEditorSendKeys(new TextEncoder().encode(cmd));
      }
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
          if (intent.query.length > 0) {
            this.sendEditorSendKeys(
              this.paletteSearchCommitKeys(intent.query, intent.match_location),
            );
          }
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
      adapter.enterPaletteThemesMode?.(JSON.stringify(WEB_IDE_THEMES));
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
    // Send the ex command verbatim to nvim by prefixing `:` and
    // terminating with `<CR>`. Matches the desktop's vim_run_ex_command
    // path for commands the host does not intercept.
    this.sendEditorSendKeys(new TextEncoder().encode(`:${trimmed}\n`));
  }

  private paletteSearchCommitKeys(
    query: string,
    matchLocation: [number, number] | null,
  ): Uint8Array {
    const location =
      matchLocation &&
      matchLocation[0] > 0 &&
      matchLocation[1] > 0 &&
      Number.isFinite(matchLocation[0]) &&
      Number.isFinite(matchLocation[1])
        ? ([Math.trunc(matchLocation[0]), Math.trunc(matchLocation[1])] as const)
        : null;
    const quoted = luaStringLiteral(query);
    const args = location ? `${quoted}, ${location[0]}, ${location[1]}` : quoted;
    return new TextEncoder().encode(
      `:lua pcall(function() require('rio.search').commit(${args}) end)\n`,
    );
  }

  /// Map a stable PaletteAction variant name (as serialized by the
  /// wasm bridge) onto the matching host-side handler. Mirrors the
  /// `Screen::execute_palette_action` arm-by-arm on desktop. Shared
  /// Rust owns the command list and palette UI; this host layer owns
  /// transport-specific effects such as spawning daemon PTYs, opening
  /// browser windows, or forwarding nvim commands.
  private dispatchPaletteAction(action: string): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    switch (action) {
      case "ToggleGitDiffPanel":
        this.toggleGitSidePanel();
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
                (tab.kind === "neoism-agent" ? "Neoism Agent" : "Terminal"),
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
        if (this.activeSurface() === "editor") {
          this.sendEditorSendKeys(
            new TextEncoder().encode(
              ":lua pcall(function() require('rio.clipboard').copy_active() end)\n",
            ),
          );
        } else {
          void this.writeClipboard(this.terminalInput || this.agentInput);
        }
        break;
      case "Paste":
        void this.readClipboard().then((text) => {
          if (text) this.pasteTextToActiveSurface(text);
        });
        break;
      case "SaveDocument":
        if (this.activeTabIsMarkdown() && this.useWasmMarkdown()) {
          this.saveActiveMarkdown();
        } else {
          this.sendEditorSendKeys(new TextEncoder().encode(":w\n"));
        }
        break;
      case "SearchForward":
        this.sendEditorSendKeys(new TextEncoder().encode("/"));
        break;
      case "SearchBackward":
        this.sendEditorSendKeys(new TextEncoder().encode("?"));
        break;
      case "ConfigEditor":
        this.sendEditorSendKeys(new TextEncoder().encode(":edit $MYVIMRC\n"));
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
        adapter.enterPaletteThemesMode?.(JSON.stringify(WEB_IDE_THEMES));
        break;
      case "OpenShaders":
        adapter.enterPaletteShadersMode?.(JSON.stringify(WEB_SHADER_FILTERS));
        break;
      case "LspRename":
        this.sendEditorSendKeys(new TextEncoder().encode(":Rename "));
        break;
      case "LspHover":
      case "LspCodeAction":
      case "LspFormat":
      case "LspDefinition":
      case "LspReferences":
      case "LspDocumentSymbols":
      case "LspWorkspaceSymbols":
      case "ToggleInlayHints":
      case "ToggleMinimap":
        // These map to user-command names the embedded nvim exposes
        // (Hover, CodeAction, LspFormat, Definition, References,
        // Rename, DocumentSymbols, WorkspaceSymbols, InlayHints,
        // Minimap). Run them as ex commands the same way desktop does
        // via `send_editor_command`.
        {
          const name =
            action === "LspWorkspaceSymbols"
              ? "WorkspaceSymbols"
              : action === "LspFormat"
              ? "LspFormat"
              : action.startsWith("Lsp")
                ? action.slice(3)
                : action === "ToggleInlayHints"
                  ? "InlayHints"
                  : "Minimap";
          this.sendEditorSendKeys(new TextEncoder().encode(`:${name}\n`));
        }
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
    if (!(WEB_IDE_THEMES as readonly string[]).includes(name)) {
      this.notifyPaletteUnavailable(`Unknown IDE theme "${name}".`, adapter);
      return;
    }
    this.setIdeTheme(name as (typeof WEB_IDE_THEMES)[number]);
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

  private sendEditorWindowCommand(command: string): void {
    const adapter = this.wasmAdapter;
    if (!adapter) return;
    if (this.activeSurface() !== "editor") {
      this.notifyPaletteUnavailable(
        `Editor window command "${command}" needs an editor buffer.`,
        adapter,
      );
      return;
    }
    this.sendEditorSendKeys(new TextEncoder().encode(`:${command}\n`));
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

  private editorInputSurfaceId(): string | null {
    if (this.liveEditorGridSurfaceId !== null) return this.liveEditorGridSurfaceId;
    const focused = this.focusedEditorSurfaceId();
    if (focused !== null) return focused;
    if (this.paneLayoutPanes.length === 1) {
      return this.editorSurfaceId(this.paneLayoutPanes[0].external_id);
    }
    if (this.editorSurfaceBindings.size === 1) {
      return this.editorSurfaceBindings.keys().next().value ?? null;
    }
    return null;
  }

  private activeEditorGridSize(
    surfaceId: string | null = this.focusedEditorSurfaceId(),
  ): { cols: number; rows: number } {
    const snapshot = surfaceId ? this.editorGridSnapshotForSurface(surfaceId) : null;
    const cols = Math.max(
      1,
      Math.trunc(snapshot?.width ?? (this.editorGridCols || this.cols)),
    );
    const rows = Math.max(
      1,
      Math.trunc(snapshot?.height ?? (this.editorGridRows || this.rows)),
    );
    return { cols, rows };
  }

  private shouldApplyEditorRedraw(surfaceId: string | null): boolean {
    if (!surfaceId) {
      return true;
    }
    const externalId = this.externalIdFromEditorSurface(surfaceId);
    if (
      externalId === null ||
      !this.paneLayoutPanes.some((pane) => pane.external_id === externalId)
    ) {
      return false;
    }
    const focused = this.focusedEditorSurfaceId();
    return focused === null || focused === surfaceId;
  }

  private noteLiveEditorGridSurface(surfaceId: string | null): void {
    const nextSurface = surfaceId ?? this.focusedEditorSurfaceId();
    if (this.liveEditorGridSurfaceId === nextSurface) return;
    this.liveEditorGridSurfaceId = nextSurface;
    this.nvimLayer.setActiveSurfaceId(nextSurface);
    this.renderPaneLayoutOverlay();
  }

  private refreshEditorGridSurfaceSnapshotIds(): void {
    const idsJson = this.wasmAdapter?.editorGridSurfaceIdsJson?.();
    if (idsJson) {
      try {
        const ids = JSON.parse(idsJson);
        if (Array.isArray(ids)) {
          this.cachedEditorGridSurfaceIds.clear();
          for (const id of ids) {
            if (typeof id === "string" && id.length > 0) {
              this.cachedEditorGridSurfaceIds.add(id);
            }
          }
        }
      } catch {
        // Cache hint only. The bridge getter below remains authoritative.
      }
    }
    for (const id of this.nvimLayer.surfaceIds()) {
      this.cachedEditorGridSurfaceIds.add(id);
    }
  }

  private hasEditorGridSnapshotForSurface(surfaceId: string): boolean {
    if (this.cachedEditorGridSurfaceIds.has(surfaceId)) {
      return true;
    }
    const snapshot = this.wasmAdapter?.editorGridSnapshotForSurfaceJson?.(surfaceId);
    if (snapshot) {
      this.cachedEditorGridSurfaceIds.add(surfaceId);
      return true;
    }
    if (this.nvimLayer.hasSnapshotForSurface(surfaceId)) {
      this.cachedEditorGridSurfaceIds.add(surfaceId);
      return true;
    }
    this.refreshEditorGridSurfaceSnapshotIds();
    return this.cachedEditorGridSurfaceIds.has(surfaceId);
  }

  private editorGridSnapshotForSurface(surfaceId: string): EditorGridSnapshot | null {
    const snapshot = parseEditorGridSnapshotJson(
      this.wasmAdapter?.editorGridSnapshotForSurfaceJson?.(surfaceId),
    );
    if (snapshot) {
      this.cachedEditorGridSurfaceIds.add(surfaceId);
      return snapshot;
    }
    const nvimSnapshot = this.nvimLayer.snapshotForSurface(surfaceId);
    if (nvimSnapshot) {
      this.cachedEditorGridSurfaceIds.add(surfaceId);
      return {
        width: nvimSnapshot.width,
        height: nvimSnapshot.height,
        cells: nvimSnapshot.cells,
        cursor: nvimSnapshot.cursor,
        default_fg: nvimSnapshot.default_fg,
        default_bg: nvimSnapshot.default_bg,
      };
    }
    this.refreshEditorGridSurfaceSnapshotIds();
    return null;
  }

  private restoreEditorGridSnapshotForSurface(externalId: number): boolean {
    const adapter = this.wasmAdapter;
    const surfaceId = this.editorSurfaceId(externalId);
    if (!this.hasEditorGridSnapshotForSurface(surfaceId)) {
      return false;
    }
    if (adapter?.activateEditorGridSurface && !adapter.activateEditorGridSurface(surfaceId)) {
      return false;
    }
    this.nvimLayer.setActiveSurfaceId(surfaceId);
    this.noteLiveEditorGridSurface(surfaceId);
    this.scheduleDraw();
    return true;
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

  private requestPresenceSnapshotForSurface(surfaceId: string | null): void {
    const snapshot =
      this.nvimLayer.snapshotForSurface(surfaceId) ?? this.nvimLayer.activeSnapshot();
    if (snapshot?.bufferId) {
      this.requestPresenceSnapshot(presenceBufferIdForPath(snapshot.bufferId));
    }
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
   * Where is the local user? Markdown viewer tabs publish their
   * reading position (top visible source line — the web markdown view
   * is read-only, so the scroll position IS the cursor). Nvim editor
   * surfaces publish the real grid cursor. Terminal/agent tabs
   * publish nothing, which makes the publisher clear any previous
   * presence.
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
    // Editor-like file tabs run through the daemon's nvim plane; the
    // grid cursor is the genuine local cursor. (Checked off the tab
    // model, not the wasm adapter, so presence still publishes when
    // the bridge is running in stub mode.)
    if (!this.isEditorLikeTab(this.bufferTabs[this.activeTabIndex])) return null;
    const snapshot =
      this.nvimLayer.snapshotForSurface(this.focusedEditorSurfaceId()) ??
      this.nvimLayer.activeSnapshot();
    if (!snapshot?.bufferId || !snapshot.cursorState?.visible) return null;
    // Prefer the BUFFER-coordinate caret from win_viewport's
    // curline/curcol — remote screens with different scroll positions
    // then draw it at the true line. Screen row/col is the fallback
    // for older daemons that don't forward them.
    const line = snapshot.curline ?? snapshot.cursorState.row;
    const column = snapshot.curcol ?? snapshot.cursorState.col;
    const mode = (snapshot.mode ?? "normal").toLowerCase();
    return {
      bufferId: presenceBufferIdForPath(snapshot.bufferId),
      cursor: { line, column, offset: null },
      selection: null,
      insert: mode.startsWith("insert") || mode === "i" || mode.startsWith("replace"),
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

  /** 7C-2: feed remote carets for the ACTIVE editor (nvim) surface
   *  into the wasm grid painter — buffer-coordinate lines; the bridge
   *  folds the viewport topline out before drawing. */
  private syncEditorPresenceOverlay(): void {
    const adapter = this.wasmAdapter as {
      setEditorRemoteCursors?: (peers: unknown) => void;
    };
    if (!adapter?.setEditorRemoteCursors) return;
    const snapshot =
      this.nvimLayer.snapshotForSurface(this.focusedEditorSurfaceId()) ??
      this.nvimLayer.activeSnapshot();
    const bufferId = snapshot?.bufferId
      ? presenceBufferIdForPath(snapshot.bufferId)
      : null;
    const peers = bufferId
      ? this.remotePresence.cursorsFor(bufferId).map((p) => ({
          name: p.display_name,
          color: [p.color.r, p.color.g, p.color.b],
          rainbow: p.rainbow ?? false,
          line: p.cursor.line,
          col: p.cursor.column,
          insert: p.insert ?? false,
        }))
      : [];
    const counts = adapter.setEditorRemoteCursors(peers) as
      | number[]
      | undefined;
    if (peers.length > 0 || (counts?.[1] ?? 0) > 0) {
      console.info(
        `[carets] buffer=${bufferId} peers=${peers.length} visible=${counts?.[0] ?? "?"} roster=${counts?.[1] ?? "?"}`,
      );
    }
  }

  /** Repaint the markdown DOM overlay from the presence store. */
  private syncMarkdownPresenceOverlay(): void {
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
    this.wasmAdapter?.clearActiveEditorGrid?.();
    this.liveEditorGridSurfaceId = null;
    if (this.markdownLayerTabIndex === this.activeTabIndex) {
      this.setMarkdownLayerVisible(true);
    } else {
      this.markdownLayer.textContent = "Loading markdown…";
      this.setMarkdownLayerVisible(true);
    }
  }

  private showMarkdownTab(): void {
    this.wasmAdapter?.clearActiveEditorGrid?.();
    this.liveEditorGridSurfaceId = null;
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
    if (
      typeof tabIndex === "number" &&
      tabIndex >= 0 &&
      tabIndex < this.bufferTabs.length &&
      this.isEditorLikeTab(this.bufferTabs[tabIndex])
    ) {
      this.activeTabIndex = tabIndex;
      this.wasmAdapter?.setActiveTab?.(tabIndex);
      const tab = this.bufferTabs[tabIndex];
      const restored = this.restoreEditorGridSnapshotForSurface(externalId);
      if (openEditorBuffer && tab?.kind === "file" && tab.path) {
        this.openFileTabContent(tab.path);
      } else {
        this.bindEditorSurfaceForTab(externalId, tabIndex);
      }
      if (restored) {
        this.noteLiveEditorGridSurface(this.editorSurfaceId(externalId));
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
      const restored = this.restoreEditorGridSnapshotForSurface(externalId);
      if (openEditorBuffer && tab?.kind === "file" && tab.path) {
        this.openFileTabContent(tab.path);
      } else {
        this.bindEditorSurfaceForTab(externalId, fallback);
      }
      if (restored) {
        this.noteLiveEditorGridSurface(this.editorSurfaceId(externalId));
      }
      this.replayBufferTabs();
    }
  }

  private paneTitle(pane: WebPaneRect): string {
    const state = this.paneTabState.get(pane.external_id);
    const tab =
      typeof state?.activeTabIndex === "number"
        ? this.bufferTabs[state.activeTabIndex]
        : null;
    return tab?.title || pane.title || `Pane ${pane.external_id}`;
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
    this.nextWebPaneId = Math.max(
      this.nextWebPaneId,
      1,
      ...result.active_external_ids.map((id) => id + 1),
    );
    this.renderPaneLayoutOverlay();
    return result;
  }

  private splitEditorPane(axis: WebPaneSplitAxis): void {
    const command = axis === "horizontal" ? "vsplit" : "split";
    if (this.activeSurface() !== "editor") {
      this.sendEditorWindowCommand(command);
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
    this.sendEditorWindowCommand(command);
  }

  private focusEditorPane(previous: boolean): void {
    if (this.activeSurface() !== "editor") {
      this.sendEditorWindowCommand(previous ? "wincmd W" : "wincmd w");
      return;
    }
    const result = this.applySessionLayoutPolicy(previous ? "focus_prev" : "focus_next");
    if (typeof result?.focused_external_id === "number") {
      this.activatePaneExternalId(result.focused_external_id, true);
    }
    this.sendEditorWindowCommand(previous ? "wincmd W" : "wincmd w");
  }

  private closeEditorPaneOrTab(): void {
    if (this.activeSurface() !== "editor") {
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
      this.sendEditorWindowCommand("close");
      return;
    }
    this.closeActiveBufferTab();
  }

  private renderPaneLayoutOverlay(): void {
    this.paneOverlay.replaceChildren();
    if (
      this.paneOverlaySuppressed ||
      this.paneLayoutPanes.length <= 1 ||
      this.activeSurface() !== "editor"
    ) {
      this.paneOverlay.hidden = true;
      return;
    }
    this.paneOverlay.hidden = false;
    for (const pane of this.paneLayoutPanes) {
      const ownsLiveGrid =
        this.liveEditorGridSurfaceId === this.editorSurfaceId(pane.external_id);
      const el = document.createElement("div");
      el.className = `terminal-pane-layout-cell${pane.focused ? " is-focused" : ""}${
        ownsLiveGrid ? " is-live-grid" : ""
      }`;
      el.style.left = `${pane.x * 100}%`;
      el.style.top = `${pane.y * 100}%`;
      el.style.width = `${pane.w * 100}%`;
      el.style.height = `${pane.h * 100}%`;
      el.dataset.paneId = String(pane.external_id);
      el.title = this.paneTitle(pane);
      const label = document.createElement("span");
      label.className = "terminal-pane-layout-label";
      label.textContent = `${this.paneTitle(pane)}${ownsLiveGrid ? " · live" : ""}`;
      el.appendChild(label);
      if (!ownsLiveGrid) {
        this.appendEditorPanePreview(el, pane);
      }
      el.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        event.stopPropagation();
        this.focusEditorPaneByExternalId(pane.external_id);
      });
      this.paneOverlay.appendChild(el);
    }
  }

  private appendEditorPanePreview(parent: HTMLElement, pane: WebPaneRect): void {
    const surfaceId = this.editorSurfaceId(pane.external_id);
    const snapshot = this.editorGridSnapshotForSurface(surfaceId);
    const preview = document.createElement("div");
    preview.className = "terminal-pane-editor-preview";
    if (!snapshot || snapshot.width <= 0 || snapshot.height <= 0) {
      preview.classList.add("is-empty");
      preview.textContent = "Waiting for editor grid";
      parent.appendChild(preview);
      return;
    }
    preview.style.backgroundColor = packedRgbCss(snapshot.default_bg);
    preview.style.color = packedRgbCss(snapshot.default_fg);
    this.populateEditorPanePreview(preview, snapshot);
    parent.appendChild(preview);
  }

  private populateEditorPanePreview(
    preview: HTMLElement,
    snapshot: EditorGridSnapshot,
  ): void {
    const maxRows = Math.min(snapshot.height, 80);
    const maxCols = Math.min(snapshot.width, 160);
    const cursorRow = snapshot.cursor?.[0] ?? -1;
    const cursorCol = snapshot.cursor?.[1] ?? -1;
    for (let row = 0; row < maxRows; row += 1) {
      const line = document.createElement("div");
      line.className = "terminal-pane-editor-preview-row";
      let currentSpan: HTMLSpanElement | null = null;
      let currentFg = Number.NaN;
      let currentBg = Number.NaN;
      let currentAttrs = Number.NaN;
      for (let col = 0; col < maxCols; col += 1) {
        const cell = snapshot.cells[row * snapshot.width + col];
        if (!cell) continue;
        const isCursor = row === cursorRow && col === cursorCol;
        if (
          !currentSpan ||
          currentFg !== cell.fg ||
          currentBg !== cell.bg ||
          currentAttrs !== cell.attrs ||
          isCursor
        ) {
          currentSpan = document.createElement("span");
          currentSpan.style.color = packedRgbCss(cell.fg || snapshot.default_fg);
          if (cell.bg !== snapshot.default_bg || isCursor) {
            currentSpan.style.backgroundColor = isCursor
              ? "color-mix(in srgb, var(--neoism-accent) 55%, transparent)"
              : packedRgbCss(cell.bg);
          }
          if ((cell.attrs & 1) !== 0) currentSpan.style.fontWeight = "700";
          if ((cell.attrs & 2) !== 0) currentSpan.style.fontStyle = "italic";
          if ((cell.attrs & 4) !== 0) currentSpan.style.textDecoration = "underline";
          line.appendChild(currentSpan);
          currentFg = cell.fg;
          currentBg = cell.bg;
          currentAttrs = cell.attrs;
        }
        currentSpan.textContent += cell.ch.length > 0 ? cell.ch : " ";
        if (isCursor) {
          currentSpan = null;
        }
      }
      preview.appendChild(line);
    }
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
    const command =
      direction === "up"
        ? "resize -2"
        : direction === "down"
          ? "resize +2"
          : direction === "left"
            ? "vertical resize -5"
            : "vertical resize +5";
    if (this.activeSurface() === "editor") {
      this.applySessionLayoutPolicy("resize", direction);
    }
    this.sendEditorWindowCommand(command);
  }

  private setIdeTheme(name: (typeof WEB_IDE_THEMES)[number]): void {
    this.activeThemeName = name;
    this.wasmAdapter?.setIdeTheme?.(name);
    // Presence broadcasts MY cursor color — switching themes updates
    // what peers see within a heartbeat.
    this.applyPresenceThemeColor(name);
  }

  private cycleIdeTheme(delta: number): void {
    const current = WEB_IDE_THEMES.indexOf(this.activeThemeName);
    const next = (current + delta + WEB_IDE_THEMES.length) % WEB_IDE_THEMES.length;
    this.setIdeTheme(WEB_IDE_THEMES[next]);
  }

  /// Wrap raw input bytes in an `EditorClientMessage::SendKeys`
  /// envelope and ship them to the daemon's embedded nvim. Used by the
  /// finder / palette picks for cursor jumps + ex commands. Matches
  /// the existing `nvimSendKeys` shape but skips the wasm-bridge round
  /// trip (the bytes come from JS-side intent processing).
  private sendEditorSendKeys(bytes: Uint8Array): void {
    if (bytes.length === 0) return;
    this.editorSessionStarted = true;
    const surfaceId = this.editorInputSurfaceId();
    this.sendEditorMessage({
      SendKeys: {
        bytes: Array.from(bytes),
        ...(surfaceId ? { surface_id: surfaceId } : {}),
      },
    });
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
        title: "Neoism Agent",
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
        // Also ask the daemon's embedded nvim to open the same path,
        // so once keystrokes start routing to "editor" the nvim grid
        // already has the buffer loaded. Fire-and-forget — the daemon
        // emits its own `BufferOpened` / `GridUpdate` reply that the
        // bridge consumes via `editor_grid_update`.
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
          if (ctx.target?.path) void this.confirmDelete(ctx.target.path);
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

  private async promptCreateFile(parentDir: string): Promise<void> {
    const name = window.prompt(`New file in ${parentDir}`, "untitled.txt");
    if (name === null) return;
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    try {
      const reply = await this.options.client.requestFiles(
        {
          CreateFile: {
            dir: this.toDaemonWorkspacePath(parentDir),
            name: trimmed,
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
    const name = window.prompt(`New folder in ${parentDir}`, "untitled");
    if (name === null) return;
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    try {
      const reply = await this.options.client.requestFiles(
        {
          CreateDir: {
            dir: this.toDaemonWorkspacePath(parentDir),
            name: trimmed,
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
    const oldName = fromPath.split(/[\\/]/).pop() ?? fromPath;
    const next = window.prompt(`Rename ${oldName}`, oldName);
    if (next === null) return;
    const trimmed = next.trim();
    if (trimmed.length === 0 || trimmed === oldName) return;
    const parent = fromPath.slice(0, fromPath.length - oldName.length);
    const toPath = `${parent}${trimmed}`;
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
          tab.title = trimmed;
          changed = true;
        }
      }
      if (changed) this.replayBufferTabs();
      this.refreshFileTreeAfterMutation();
    } catch (err) {
      this.reportFsError(`rename: ${String(err)}`);
    }
  }

  private async confirmDelete(path: string): Promise<void> {
    const ok = window.confirm(`Delete ${path}? This cannot be undone.`);
    if (!ok) return;
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
  /// Without this, the daemon nvim grid keeps its old cols and the
  /// editor paint stretches/squeezes glyphs to the new rect. Track the
  /// terminal rect each frame and re-run the resize contract when it
  /// moves.
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

  /// Build and ship the `Editor` envelope that tells the daemon's
  /// embedded nvim to `:edit <path>`. The envelope shape matches
  /// `ServiceClientMessage::Editor { request_id, message }` in the
  /// daemon's server.rs; we don't wait for a reply (the bridge picks
  /// up the resulting `GridUpdate` via `editor_grid_update`).
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
    this.assumedNvimInsertMode = false;
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

  private sendEditorMouseInput(args: {
    button: string;
    action: string;
    modifier: string;
    grid: number;
    row: number;
    col: number;
    count: number;
  }): void {
    this.editorSessionStarted = true;
    const surfaceId = this.editorInputSurfaceId();
    this.sendEditorMessage({
      MouseInput: {
        button: args.button,
        action: args.action,
        modifier: args.modifier,
        grid: Math.trunc(args.grid),
        row: Math.trunc(args.row),
        col: Math.trunc(args.col),
        count: Math.max(1, Math.trunc(args.count)),
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
        } else if (this.markdownLayerTabIndex === tabIdx) {
          this.clearMarkdownLayer();
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
    this.syncNvimLayerVisibility();
    this.syncTerminalRectDependents();

    if (this.isRendered()) {
      // sugarloaf owns the canvas — paint cells via wgpu and skip the
      // canvas2d stub entirely.
      this.wasmAdapter?.render();
      this.syncActiveMarkdownLayer();
      if (
        this.wasmAdapter?.animationsActive?.() === true ||
        this.wasmAdapter?.editorScrollAnimating?.() === true
      ) {
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

  private syncNvimLayerVisibility(): void {
    const focused = this.focusedEditorSurfaceId();
    if (focused) {
      this.nvimLayer.setActiveSurfaceId(focused);
    }
    // The shared wasm Chrome renderer is the visual source of truth for
    // nvim grids now. Keep NvimCanvasLayer as a model/cache for smoke
    // snapshots, presence, and surface bookkeeping, but never let its
    // DOM canvas paint or capture pointer events on top of the shared
    // renderer. Leaving it visible caused double cursors and transient
    // blank bands when it displayed intermediate GridScroll states.
    this.nvimLayer.setVisible(false);
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
        this.wasmAdapter?.clearActiveEditorGrid?.();
        this.liveEditorGridSurfaceId = null;
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
      this.syncNvimLayerVisibility();
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
    // must not reach nvim, the pty, or chrome routing. The
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
      // Real-renderer markdown: full vim-mode key routing in the wasm
      // pane (motions, insert typing, Ctrl+U/D). Unhandled keys fall
      // through to the normal routing below.
      const adapter = this.wasmAdapter as {
        markdownKey?: (key: string, ctrl: boolean) => boolean;
      };
      if (adapter?.markdownKey?.(event.key, event.ctrlKey)) {
        event.preventDefault();
        // Letter-by-letter outbound: the keystroke just mutated the
        // pane — flush it into the shared doc right away.
        this.pumpCrdtOutbox();
        this.scheduleDraw();
        this.pumpMarkdownAnimation();
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

    if (this.routeKeyToEditor(event)) {
      event.preventDefault();
      return;
    }

    const bytes = keyEventToBytes(event);
    if (!bytes) {
      return;
    }
    event.preventDefault();
    this.handleInputBytes(bytes);
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
   * routes the committed bytes through `handleInputBytes` so nvim /
   * the pty receive the final text via the same path real keystrokes
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
      // Route the bytes to the focused surface (nvim / pty / agent
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

    if (this.handleEditorLeaderShortcut(event)) {
      return true;
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
      // Cmd+D / Cmd+Shift+D → split active embedded nvim right / down.
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
      // Ctrl+Shift+R / D → split active embedded nvim right / down.
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
    // Ctrl+Alt+Arrow* → resize the focused nvim split. The desktop
    // path ultimately delegates to pane-border resize bookkeeping;
    // web's current editor split authority is embedded nvim, so route
    // the same intent through `:resize` / `:vertical resize`.
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

    let removedActiveEditorLikeTab = false;
    if (typeof result.remove_index === "number") {
      const tab = this.bufferTabs[result.remove_index];
      removedActiveEditorLikeTab =
        result.remove_index === this.activeTabIndex && this.isEditorLikeTab(tab);
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
    if (removedActiveEditorLikeTab) {
      this.liveEditorGridSurfaceId = null;
      this.wasmAdapter?.clearActiveEditorGrid?.();
    }
    this.activateCurrentTabContents();
  }

  private routeKeyToChrome(event: KeyboardEvent): boolean {
    if (!this.isChromeKeyboardCaptureActive()) return false;
    this.forwardChromeEvent(fromKeyboardEvent(event));
    return true;
  }

  private routeKeyToEditor(event: KeyboardEvent): boolean {
    if (this.activeSurface() !== "editor") return false;
    const input = editorKeyEventToNvimInput(event);
    if (input === null) return false;
    this.editorSessionStarted = true;
    this.wasmAdapter?.nvimSendKeys?.(bytesToBase64(new TextEncoder().encode(input)));
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
    this.scheduleDraw();
    return true;
  }

  private isChromeKeyboardCaptureActive(): boolean {
    return this.wasmAdapter?.chromeKeyboardCaptureActive?.() === true;
  }

  private isEditorInputModalActive(): boolean {
    return this.wasmAdapter?.editorInputModalActive?.() === true;
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

  private handlePointerMove(event: PointerEvent): void {
    // Touch pointers are owned entirely by the touchstart/move/end
    // handlers; letting the parallel PointerEvent stream through
    // double-fires every action (a folder tap toggled open on
    // pointerdown, then closed again on the synthesized tap).
    if (event.pointerType === "touch") return;
    this.updateCustomCursorFromPointer(event, true);
    if (this.editorPointerDragging && this.routePointerToEditor(event, "drag")) {
      event.preventDefault();
      return;
    }
    if (this.wasmAdapter?.splashMouseMove) {
      const { x, y } = this.canvasLogicalPoint(event);
      this.wasmAdapter.splashMouseMove(x, y);
      this.scheduleDraw();
    }
    this.forwardChromeEvent(fromPointerMoveEvent(event, this.canvas));
  }

  private handlePointerDown(event: PointerEvent): void {
    if (event.pointerType === "touch") return;
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
    if (this.routePointerToEditor(event, "press")) {
      this.editorPointerDragging = true;
      try {
        this.canvas.setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture can fail for synthetic/browser-cancelled events.
      }
      event.preventDefault();
      return;
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
    this.forwardChromeEvent(
      fromPointerDownEvent(event, event.detail || 1, this.canvas),
    );
    if (this.isMobileViewport() && event.pointerType !== "touch") {
      const { x, y } = this.canvasLogicalPoint(event);
      this.maybeRequestSoftKeyboardAfterTap(x, y);
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
        this.paneOverlaySuppressed = !this.paneOverlaySuppressed;
        this.renderPaneLayoutOverlay();
        break;
      case "diagnostic_jump":
        this.sendEditorSendKeys(
          new TextEncoder().encode(`:normal! ${intent.line}G\n`),
        );
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
    this.updateCustomCursorFromPointer(event, true);
    if (this.editorPointerDragging && this.routePointerToEditor(event, "release")) {
      this.editorPointerDragging = false;
      try {
        this.canvas.releasePointerCapture(event.pointerId);
      } catch {
        // Already released by the browser.
      }
      event.preventDefault();
      return;
    }
    this.editorPointerDragging = false;
    this.forwardChromeEvent(fromPointerUpEvent(event, this.canvas));
  }

  // ----------------------------------------------------------------
  // Touch (C3 polish). The shared classifier in
  // `services/touchPolicy.ts` mirrors `shared/src/touch_policy.rs`.
  // We only do platform-specific wiring here (coordinate translation,
  // zone hit-test, `preventDefault` gating, side-effect application).
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
    // nvim buffers and markdown docs are type-anywhere surfaces:
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

  /** Tap places the nvim cursor at the touched cell — a synthetic
   *  press+release through the same daemon mouse pipeline desktop
   *  clicks use. */
  private editorTapAt(x: number, y: number): boolean {
    if (this.activeSurface() !== "editor") return false;
    if (this.isEditorInputModalActive()) return false;
    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal || !pointInRect({ x, y }, terminal)) return false;
    const hit = this.wasmAdapter?.editorPointerIntent?.(x, y);
    if (!hit) return false;
    this.wasmAdapter?.focusEditorInput?.();
    for (const action of ["press", "release"] as const) {
      this.sendEditorMouseInput({
        button: "left",
        action,
        modifier: "",
        grid: 0,
        row: hit.row,
        col: hit.col,
        count: 1,
      });
    }
    // Obsidian-style mobile editing: a tap lands ready to type. nvim
    // doesn't echo its mode over this transport, so track what we
    // sent: "i" flips us into insert; an Escape from the soft
    // keyboard's toolbar flips back (see handleInputBytes).
    if (this.isMobileViewport() && !this.assumedNvimInsertMode) {
      this.wasmAdapter?.nvimSendKeys?.(
        bytesToBase64(new TextEncoder().encode("i")),
      );
      this.assumedNvimInsertMode = true;
    }
    return true;
  }

  /** Best-effort mobile-side mirror of nvim's insert state. Only
   *  consulted (and only mutated) on the touch path. */
  private assumedNvimInsertMode = false;

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

  private routeEditorTouchScroll(x: number, y: number, dyPixels: number): boolean {
    if (this.activeSurface() !== "editor") return false;
    if (this.isChromeKeyboardCaptureActive()) return false;
    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal || !pointInRect({ x, y }, terminal)) return false;

    // kinetic=true: touch drags ride the editor's pixel-scroll glide
    // (same spring trackpads use) so release momentum feels like iOS.
    this.pushEditorWheelIntent(x, y, dyPixels, 0, "", true);
    return true;
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
        // nvim buffer tap: place the cursor at the cell and bring up
        // the keyboard, same flow as the agent input.
        if (this.editorTapAt(action.x, action.y)) {
          this.agentTapConsumed = true;
          this.requestSoftKeyboard();
          this.scheduleDraw();
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
        if (this.routeEditorTouchScroll(action.x, action.y, -action.dy)) {
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
    if (this.routeWheelToEditor(event)) {
      event.preventDefault();
      return;
    }
    if (this.routeWheelToAgent(event)) {
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

  private routeWheelToAgent(event: WheelEvent): boolean {
    if (this.activeSurface() !== "agent") return false;
    const adapter = this.wasmAdapter;
    if (!adapter?.agentScrollTimeline) return false;
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

  private routeWheelToEditor(event: WheelEvent): boolean {
    if (this.activeSurface() !== "editor") return false;
    if (this.isEditorInputModalActive()) return false;

    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal) return false;

    const point = this.canvasLogicalPoint(event);
    if (!pointInRect(point, terminal)) return false;

    if (event.deltaY === 0) return true;

    this.wasmAdapter?.focusEditorInput?.();
    this.pushEditorWheelIntent(
      point.x,
      point.y,
      event.deltaY,
      event.deltaMode,
      nvimMouseModifier(event),
      event.deltaMode === WheelEvent.DOM_DELTA_PIXEL,
    );
    return true;
  }

  private pushEditorWheelIntent(
    x: number,
    y: number,
    deltaY: number,
    deltaMode: number,
    modifier: string,
    kinetic: boolean,
  ): void {
    this.editorWheelAnchor = { x, y, modifier };
    const intent = this.wasmAdapter?.editorWheelIntent?.(
      x,
      y,
      deltaY,
      deltaMode,
    );
    if (intent) {
      this.sendEditorWheelRows(intent.row, intent.col, intent.rows, modifier);
    }

    if (kinetic || this.wasmAdapter?.editorScrollAnimating?.() === true) {
      this.scheduleEditorWheelGlide();
    }
  }

  private sendEditorWheelRows(
    row: number,
    col: number,
    wholeRows: number,
    modifier: string,
  ): void {
    this.sendEditorMouseInput({
      button: "wheel",
      action: wholeRows > 0 ? "down" : "up",
      modifier,
      grid: 0,
      row,
      col,
      count: Math.abs(wholeRows),
    });
  }

  private scheduleEditorWheelGlide(): void {
    if (this.editorWheelRaf !== null) return;
    this.editorWheelRaf = requestAnimationFrame(() =>
      this.tickEditorWheelGlide(),
    );
  }

  private tickEditorWheelGlide(): void {
    this.editorWheelRaf = null;
    const anchor = this.editorWheelAnchor;
    if (!anchor || this.activeSurface() !== "editor") {
      this.resetEditorWheelState();
      return;
    }
    const intent = this.wasmAdapter?.editorTickWheelIntent?.(anchor.x, anchor.y);
    if (intent) {
      this.sendEditorWheelRows(
        intent.row,
        intent.col,
        intent.rows,
        anchor.modifier,
      );
    }
    if (this.wasmAdapter?.editorScrollAnimating?.() === true) {
      this.scheduleEditorWheelGlide();
    } else {
      this.editorWheelAnchor = null;
    }
  }

  private resetEditorWheelState(): void {
    this.wasmAdapter?.editorResetWheel?.();
    this.editorWheelAnchor = null;
    if (this.editorWheelRaf !== null) {
      cancelAnimationFrame(this.editorWheelRaf);
      this.editorWheelRaf = null;
    }
  }

  private routePointerToEditor(
    event: PointerEvent,
    action: "press" | "drag" | "release",
  ): boolean {
    if (this.activeSurface() !== "editor") return false;
    if (this.isEditorInputModalActive()) return false;
    if (event.button !== 0 && action !== "drag") return false;

    const terminal = this.wasmAdapter?.chromeLayout?.()?.terminal;
    if (!terminal) return false;

    const point = this.canvasLogicalPoint(event);
    if (!pointInRect(point, terminal)) return false;

    const hit = this.wasmAdapter?.editorPointerIntent?.(point.x, point.y);
    if (!hit) return false;
    this.wasmAdapter?.focusEditorInput?.();
    this.sendEditorMouseInput({
      button: "left",
      action,
      modifier: nvimMouseModifier(event),
      grid: 0,
      row: hit.row,
      col: hit.col,
      count: Math.max(1, event.detail || 1),
    });
    return true;
  }

  private handlePaste(event: ClipboardEvent): void {
    const imageItems = Array.from(event.clipboardData?.items ?? []).filter(
      (item) => item.kind === "file" && item.type.startsWith("image/"),
    );
    const surface = this.activeSurface();
    if (imageItems.length > 0) {
      if (surface === "agent") {
        event.preventDefault();
        void this.submitPastedImages(
          imageItems,
          event.clipboardData?.getData("text/plain") ?? "",
        );
        return;
      }
      if (surface === "editor") {
        // Materialise the image on the daemon side and `:edit <path>`
        // it once the daemon replies. Falls back to the text payload
        // (if any) for the same paste event so users still get the
        // accompanying caption when the daemon write fails.
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
    this.pasteTextToActiveSurface(text);
  }

  /// Send each pasted image file to the daemon as
  /// `MaterializeClipboardImage`. The matching
  /// `ClipboardImageMaterialized` reply is handled in
  /// `ingestClipboardImageMaterialized`, which dispatches `:edit <path>`
  /// against the pane that *initiated* the paste (recorded in
  /// `pendingClipboardImages` against an opaque `request_id`) — not
  /// the focused pane at reply time, which the user may have switched
  /// while the daemon was writing.
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

    // Try to activate the originating pane so the `:edit` keystrokes
    // land in the right nvim. `activatePaneExternalId` is a no-op if
    // the pane is already focused (or no longer exists).
    if (originPaneId !== null) {
      this.activatePaneExternalId(originPaneId, true);
    }

    const surface = this.activeSurface();
    if (surface !== "editor") {
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
      return;
    }
    // Send `:edit <path>` to the focused nvim editor surface. Image
    // plugins (like `image.nvim` / `kitty-image.nvim`) hook into
    // `BufReadPost` to render the preview inline; without one nvim
    // simply opens the binary buffer at the saved path.
    const escaped = payload.path.replace(/ /g, "\\ ").replace(/"/g, '\\"');
    this.sendEditorSendKeys(new TextEncoder().encode(`:edit ${escaped}\n`));
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

  private handleInputBytes(bytes: Uint8Array): void {
    if (this.routeInputBytesToChrome(bytes)) {
      return;
    }
    // Buffer-tab routing: tab 0 is the always-present Terminal (shell
    // PTY); any other tab is a file backed by the daemon's embedded
    // nvim. The bridge exposes `active_surface()` as the single source
    // of truth so JS and Rust agree without an extra cache.
    const surface = this.activeSurface();
    if (surface === "agent" && this.routeInputBytesToAgent(bytes)) {
      return;
    }
    if (surface === "markdown" && this.routeInputBytesToMarkdown(bytes)) {
      return;
    }
    if (surface === "editor" && bytes.includes(0x1b)) {
      // Soft-keyboard Esc — nvim leaves insert mode; keep the mobile
      // tap-to-type mirror in sync.
      this.assumedNvimInsertMode = false;
    }
    if (surface === "editor" && this.wasmAdapter?.nvimSendKeys) {
      // Soft-keyboard bytes must be rewritten into nvim_input's <>
      // notation: a raw DEL byte (0x7f) lands as nvim's internal
      // `<80>ku` keycode — Backspace literally pressed Up.
      const text = new TextDecoder().decode(bytes);
      let notated = "";
      for (const ch of text) {
        if (ch === "\x7f" || ch === "\b") notated += "<BS>";
        else if (ch === "\r" || ch === "\n") notated += "<CR>";
        else if (ch === "\x1b") notated += "<Esc>";
        else if (ch === "\t") notated += "<Tab>";
        else if (ch === "<") notated += "<lt>";
        else notated += ch;
      }
      this.wasmAdapter.nvimSendKeys(
        bytesToBase64(new TextEncoder().encode(notated)),
      );
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
        adapter.resetTerminalSplash?.();
        adapter.clearTerminalInput?.();
        syncInput();
        return true;
      }
      if (key === "Ctrl+C" || key === "Ctrl+D") {
        if ((adapter.terminalInput?.() ?? this.terminalInput).length === 0) {
          return false;
        }
        adapter.clearTerminalInput?.();
        syncInput();
        return true;
      }
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

function debugAgentTimeline(event: string, payload: Record<string, unknown>): void {
  try {
    const enabled = localStorage.getItem("neoism_debug_agent_timeline");
    if (enabled !== "1" && enabled?.toLowerCase() !== "true") return;
    console.warn("[neoism-agent-timeline:web]", event, payload);
  } catch {
    // Debug logging is best-effort; localStorage can be unavailable.
  }
}

function nvimMouseModifier(event: MouseEvent): string {
  let modifier = "";
  if (event.shiftKey) modifier += "S-";
  if (event.ctrlKey) modifier += "C-";
  if (event.altKey) modifier += "M-";
  if (event.metaKey) modifier += "D-";
  return modifier;
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

function editorKeyEventToNvimInput(event: KeyboardEvent): string | null {
  if (event.metaKey || event.altKey) return null;
  if (event.ctrlKey && event.key.length === 1) {
    return `<C-${event.key.toLowerCase()}>`;
  }
  switch (event.key) {
    case "Enter":
      return "<CR>";
    case "Backspace":
      return "<BS>";
    case "Tab":
      return event.shiftKey ? "<S-Tab>" : "<Tab>";
    case "Escape":
      return "<Esc>";
    case "ArrowUp":
      return "<Up>";
    case "ArrowDown":
      return "<Down>";
    case "ArrowRight":
      return "<Right>";
    case "ArrowLeft":
      return "<Left>";
    case "PageUp":
      return "<PageUp>";
    case "PageDown":
      return "<PageDown>";
    case "Home":
      return "<Home>";
    case "End":
      return "<End>";
    case "Delete":
      return "<Del>";
    default:
      break;
  }
  if (event.key.length === 1 && !event.ctrlKey) {
    return event.key;
  }
  return null;
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
      case 0x03:
        return "Ctrl+C";
      case 0x04:
        return "Ctrl+D";
      case 0x09:
        return "Tab";
      case 0x0c:
        return "Ctrl+L";
      case 0x0d:
        return "Enter";
      case 0x7f:
      case 0x08:
        return "Backspace";
      default:
        return null;
    }
  }
  const text = new TextDecoder().decode(bytes);
  switch (text) {
    case "\x1b[A":
      return "ArrowUp";
    case "\x1b[B":
      return "ArrowDown";
    case "\x1b[C":
      return "ArrowRight";
    case "\x1b[D":
      return "ArrowLeft";
    case "\x1b[H":
    case "\x1b[1~":
      return "Home";
    case "\x1b[F":
    case "\x1b[4~":
      return "End";
    case "\x1b[3~":
      return "Delete";
    case "\x1b[Z":
      return "Shift+Tab";
    default:
      return null;
  }
}

/// `btoa`/`atob` are the right primitives for the JsValue boundary,
/// but feeding `String.fromCharCode(...bytes)` to `btoa` blows the
/// argument list past `Function.apply` limits on multi-KB chunks. We
/// build the binary string in a chunked loop so this stays safe for
/// paste-sized payloads.
function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    const slice = bytes.subarray(i, Math.min(i + CHUNK, bytes.length));
    binary += String.fromCharCode(...slice);
  }
  return btoa(binary);
}

function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}
