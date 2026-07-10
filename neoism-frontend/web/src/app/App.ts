import { ProtocolClient, ProtocolStatus } from "../workspace/ProtocolClient";
import { ConnectionScreen } from "./ConnectionScreen";
import type { ConnectionScreenOptions } from "./ConnectionScreen";
import { TerminalPanel } from "../terminal/TerminalPanel";
import {
  workspaceChromeActionsForVisibility,
  type WorkspaceChromeActionKind,
} from "../terminal/createTerminal";
import { PtyService } from "../services/PtyService";
import { SearchService } from "../services/SearchService";
import { WorkspaceService } from "../services/WorkspaceService";
import {
  DiagnosticsService,
  type DiagnosticsBridge,
} from "../services/DiagnosticsService";
import {
  WorkplaceService,
  WorkplaceSwitcher,
  type DaemonId,
  type DiscoveredWorkplace,
  type WorkplacePreferences,
  type WorkplaceEntry,
} from "../services/WorkplaceService";
import type {
  WorkspaceServerMessage,
  WorkspaceSummary,
  WorkspaceTabSummary,
} from "../workspace/types";
import type {
  WorkspacesModalPayload,
  WorkspacesModalPeerHost,
  WorkspacesModalWorkspace,
} from "../terminal/createTerminal";

const DEFAULT_DAEMON_URL = "ws://127.0.0.1:7878/session";
const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;
const WEB_CONTROLLER_HOST_ID = "web-controller";
const WORKSPACE_SUBSCRIPTIONS_STORAGE_KEY = "neoism.workspace-subscriptions.v1";

function normalizedDaemonUrl(url: string): string {
  return url.replace(/\/+$/, "");
}

type WorkspaceStripSnapshot = Array<{
  title: string;
  kind: string;
  path: string | null;
  sessionId: string | null;
  active: boolean;
}>;

/**
 * Top-level controller. Owns the persistent `WorkplaceService`, swaps
 * between the connection screen and the terminal panel, and routes server
 * frames through the active workplace.
 *
 * The `WorkplaceService` is the single source of truth for "which
 * daemon are we talking to right now"; this controller subscribes to
 * its `active-changed` event so terminal panels are re-mounted
 * automatically when the operator picks a different workplace from the
 * command-palette switcher.
 */
export class App {
  private connectionScreen: ConnectionScreen | null = null;
  private terminalPanel: TerminalPanel | null = null;
  private client: ProtocolClient | null = null;
  private activeSessionId: string | null = null;
  private ptyService: PtyService | null = null;
  private searchService: SearchService | null = null;
  private workspaceService: WorkspaceService | null = null;
  /** Set by the Alt+W create-workspace flow; the daemon upsert ack is
   *  followed by an explicit active-workspace switch. */
  private pendingCreateWorkspaceId: string | null = null;
  /** Workspace selected from a different daemon. The connection is switched
   * first; adoption completes only after that daemon publishes its tree. */
  private pendingRemoteWorkspaceId: string | null = null;
  private pendingRemoteWorkspaceRoot: string | null = null;
  /** The daemon host-workspace this panel currently lives in — the
   *  target for tab publishes. Set on switch/adopt/create. */
  private activeHostWorkspaceId: string | null = null;
  /** PTY fallback is blocked until the daemon chooses the startup workspace. */
  private initialWorkspaceResolved = false;
  /** Web must not restore PTYs/editor tabs until the user explicitly picks a workspace. */
  private workspaceGateSatisfied = false;
  /** Daemon workspace root used to absolutize relative tab paths. */
  private activeWorkspaceRootPath: string | null = null;
  /** Browser-local tab views. Workspace root/host state is shared by the
   *  daemon; open tabs/focus remain local only while this page is alive. */
  private workspaceStrips = new Map<string, WorkspaceStripSnapshot>();
  private suppressWorkspaceTabSync = false;
  private subscribedWorkspaceIds = new Set<string>();
  private workspaceSubscriptionOrder: string[] = [];
  /** Last daemon-tracked live cwd per PTY session id (from `SessionCwd`
   *  pushes). The daemon broadcasts every session's cwd to every client,
   *  so this accumulates roots for all live shells — letting a workspace
   *  jump re-root to the target's terminal cwd even before we focus it,
   *  and letting a `cd` in the active terminal move the tree live. */
  private sessionCwds = new Map<string, string>();
  private diagnosticsService: DiagnosticsService | null = null;
  private readonly workplaceService: WorkplaceService;
  private workplaceSwitcherOverlay: HTMLDivElement | null = null;
  private workplaceSwitcher: WorkplaceSwitcher | null = null;
  private workplaceSwitcherKeydown: ((event: KeyboardEvent) => void) | null =
    null;
  private fallbackSpawnTimer: number | null = null;
  /** Session ids already swapped out after an `unknown session` reply —
   *  one respawn per dead id, so a burst of queued Resize/PtyInput
   *  errors for the same session can't fork a shell per frame. */
  private staleSessionRecovery = new Set<string>();
  // request_id-0 service pushes (git branch / change counts) that
  // arrived before the terminal panel existed. The daemon only
  // re-sends them on change, so they must be replayed, not dropped.
  private pendingStatusPushes: unknown[] = [];
  private activeWorkspaceId: string | null = null;
  // Surface-id -> daemon-assigned diagnostics route_id. Populated from
  // `EditorSurfaceList` / `EditorSurfaceChanged` so subscribes follow
  // the route the daemon picked for each pane, replacing the legacy
  // hard-coded `ACTIVE_EDITOR_ROUTE_ID = 1` that assumed a single
  // editor surface per web client.
  private readonly surfaceRouteIds = new Map<string, number>();

  constructor(private readonly root: HTMLElement) {
    this.root.classList.add("app-root");
    this.loadWorkspaceSubscriptions();
    this.workplaceService = new WorkplaceService();
    this.workplaceService.subscribe((event) => {
      if (event.kind === "rehome") {
        this.handleRehome(event);
        return;
      }
      if (event.kind !== "preferences") return;
      if (this.activeWorkspaceId && event.workspaceId !== this.activeWorkspaceId) {
        return;
      }
      this.terminalPanel?.applyWorkplacePreferences(event.prefs);
    });
    this.showConnectionScreen();
  }

  private showConnectionScreen(): void {
    this.clearTerminal();
    this.connectionScreen = new ConnectionScreen(
      this.connectionScreenOptions(this.defaultConnectionUrl()),
    );
  }

  private connectionScreenOptions(defaultUrl: string): ConnectionScreenOptions {
    return {
      mount: this.root,
      defaultUrl,
      onSubmit: (values) => this.connectManual(values.url, values.authToken),
      onWorkspacePick: (workspace) => this.acceptWorkspaceGate(workspace),
      onCreateWorkspace: () => this.createWorkspaceOnConnectedHost(),
    };
  }

  /** Pick a sensible URL to pre-fill the connection screen with. Falls
   *  back to `DEFAULT_DAEMON_URL` if the registry is empty. */
  private defaultConnectionUrl(): string {
    const entries = this.workplaceService.listWorkplaces();
    if (entries.length === 0) return DEFAULT_DAEMON_URL;
    // Most recently added is at the tail of insertion order; sorted
    // listing is alphabetical so we look up the raw map via `find`.
    return entries[0].url;
  }

  /**
   * Manual connection path from the `ConnectionScreen` form. We
   * promote the typed URL into the registry (so subsequent loads can
   * skip the form) and then route through `connectViaService` like
   * every other path.
   */
  private connectManual(url: string, authToken: string): void {
    const entry = this.workplaceService.addWorkplace(
      {
        url,
        label: friendlyLabelFromUrl(url),
        transport: "manual",
      },
      authToken,
    );
    this.connectViaService(entry.id);
  }

  /** Discovered-peer path from the switcher widget. Promotes the
   *  candidate into the registry then connects through it. */
  private handleDiscoveredPick(
    entry: DiscoveredWorkplace,
    pairingToken: string,
  ): void {
    const promoted = this.workplaceService.addWorkplace(
      {
        id: entry.id,
        url: entry.url,
        label: entry.label,
        transport: "tailscale",
      },
      pairingToken,
    );
    this.connectViaService(promoted.id);
  }

  /** Click-on-existing-workplace path. The chrome may currently be
   *  showing the connection screen or a terminal — either way we
   *  short-circuit the form and dial the picked daemon. */
  private handleSwitchTo(entry: WorkplaceEntry): void {
    this.connectViaService(entry.id);
  }

  /**
   * Drive a (re-)connection through `WorkplaceService`. Disconnects
   * whatever client is currently live, tears down the existing
   * terminal panel, then asks the service to construct a fresh
   * `ProtocolClient` with our handlers bound. The chrome falls back to
   * the connection screen briefly while the new client dials so the
   * user sees the status transitions.
   */
  private connectViaService(id: DaemonId): void {
    // Tear the current session down. We don't go through
    // `showConnectionScreen()` because that would reset the form's
    // default URL — instead we mount it ourselves below so the URL
    // input shows the picked workplace's URL.
    this.clearTerminal();

    const url = this.workplaceService.listWorkplaces().find((e) => e.id === id)
      ?.url;
    this.connectionScreen = new ConnectionScreen(
      this.connectionScreenOptions(url ?? this.defaultConnectionUrl()),
    );
    this.connectionScreen.setBusy(true);
    this.connectionScreen.setStatus("Opening WebSocket...");

    // Wave 4B: let the service auto-re-dial with our handler bundle when
    // the active workspace is re-homed. Installed before `connect` so a
    // very-early tree push that flips the home can already follow.
    this.workplaceService.setRehomeHandlers(this.buildClientHandlers());

    let client: ProtocolClient;
    try {
      client = this.workplaceService.connect(id, this.buildClientHandlers());
    } catch (err) {
      this.connectionScreen.setBusy(false);
      this.connectionScreen.setStatus(
        `Connect failed: ${err instanceof Error ? err.message : String(err)}`,
      );
      return;
    }
    this.client = client;
    // Service-level subscribers (PTY/workspace/diagnostics) must be
    // alive before we open the socket so the daemon's first frames
    // route correctly. Bridge-bound services (search/diagnostics
    // sinks) are installed later in `handlePtyCreated` once
    // `TerminalPanel` exposes the wasm bridge.
    this.ptyService = new PtyService(client);
    this.ptyService.subscribe({
      onCreated: (sessionId, workspaceRoot) =>
        this.handlePtyCreated(sessionId, workspaceRoot),
      onOutput: (sessionId, bytes) => this.handlePtyOutput(sessionId, bytes),
      onClosed: (sessionId, exitCode) =>
        this.handlePtyClosed(sessionId, exitCode),
      onError: (message) => this.handlePtyError(message),
    });
    this.workspaceService = new WorkspaceService(client);
    this.installWorkspaceTreeSubscription();
    this.diagnosticsService = new DiagnosticsService(client);
    client.connect();
  }

  /**
   * Build the handler bundle a `ProtocolClient` needs. Pulled out so
   * `connectViaService` (and any future re-connect paths) can share
   * one definition without duplicating the fan-out.
   */
  private buildClientHandlers() {
    return {
      onStatus: (status: ProtocolStatus, detail?: string) =>
        this.handleStatus(status, detail),
      // PTY frames are funnelled through `PtyService` so the wire
      // stays in one place and future multi-pane web frontends can
      // route by `session_id`. App registers its own listener below
      // for the spawn→panel handoff and lifecycle bookkeeping.
      onPtyCreated: (sessionId: string, workspaceRoot: string | null) => {
        if (!this.workspaceGateSatisfied) return;
        this.ptyService?.ingestCreated(sessionId, workspaceRoot);
      },
      onPtyOutput: (sessionId: string, bytes: Uint8Array) => {
        if (!this.workspaceGateSatisfied) return;
        this.ptyService?.ingestOutput(sessionId, bytes);
      },
      onPtyClosed: (sessionId: string, exitCode: number | null) => {
        if (!this.workspaceGateSatisfied) return;
        this.ptyService?.ingestClosed(sessionId, exitCode);
      },
      onSessionCwd: (sessionId: string, cwd: string) =>
        this.workspaceGateSatisfied && this.handleSessionCwd(sessionId, cwd),
      onServerError: (message: string) => {
        this.ptyService?.ingestError(message);
        this.connectionScreen?.setStatus(`Daemon error: ${message}`);
      },
      onProtocolError: (message: string) =>
        this.connectionScreen?.setStatus(`Protocol error: ${message}`),
      onServiceReply: (requestId: number, payload: unknown) => {
        if (!this.workspaceGateSatisfied) return;
        if (requestId === 0 && !this.terminalPanel) {
          this.pendingStatusPushes.push(payload);
          return;
        }
        this.terminalPanel?.serviceReply(
          requestId,
          payload as Parameters<TerminalPanel["serviceReply"]>[1],
        );
      },
      onAgentReply: (_requestId: number, payload: unknown) => {
        if (!this.workspaceGateSatisfied) return;
        this.terminalPanel?.agentReply(
          payload as Parameters<TerminalPanel["agentReply"]>[0],
        );
      },
      onEditorReply: (_requestId: number, payload: unknown) => {
        if (!this.workspaceGateSatisfied) return;
        this.terminalPanel?.editorReply(payload);
      },
      onSearchReply: (payload: unknown) =>
        this.searchService?.ingestServerMessage(
          payload as Parameters<SearchService["ingestServerMessage"]>[0],
        ),
      onWorkspaceReply: (payload: WorkspaceServerMessage) => {
        this.maybeRequestWorkspacePreferences(payload);
        this.workspaceService?.ingestServerMessage(payload);
        if (this.workspaceGateSatisfied) {
          this.terminalPanel?.workspaceReply(payload);
          this.routeDiagnosticsFromWorkspace(payload);
        }
        // Wave 4B: after the tree mutation lands, feed the latest
        // workspace summaries into the re-home watcher so the client
        // follows the active workspace if it was promoted to a new host.
        this.observeWorkspaceHoming(payload);
      },
      onDiagnosticsReply: (payload: unknown) => {
        if (!this.workspaceGateSatisfied) return;
        this.diagnosticsService?.ingestServerMessage(
          payload as Parameters<
            DiagnosticsService["ingestServerMessage"]
          >[0],
        );
        this.terminalPanel?.diagnosticsReply(
          payload as Parameters<TerminalPanel["diagnosticsReply"]>[0],
        );
      },
      onCursorOverlayReply: (_requestId: number, payload: unknown) => {
        if (!this.workspaceGateSatisfied) return;
        this.terminalPanel?.cursorOverlayReply(
          payload as Parameters<TerminalPanel["cursorOverlayReply"]>[0],
        );
      },
      onCrdtReply: (_requestId: number, payload: unknown) => {
        if (!this.workspaceGateSatisfied) return;
        this.terminalPanel?.crdtReply(
          payload as Parameters<TerminalPanel["crdtReply"]>[0],
        );
      },
    };
  }

  private handleStatus(status: ProtocolStatus, detail?: string): void {
    if (this.connectionScreen) {
      const suffix = detail ? ` (${detail})` : "";
      this.connectionScreen.setStatus(`Socket ${status}${suffix}`);
    }
    if (status === "open" && this.client) {
      // Fresh connection, fresh recovery budget — the persisted tree
      // can hand us the same stale session id again.
      this.staleSessionRecovery.clear();
      this.initialWorkspaceResolved = false;
      this.workspaceGateSatisfied = false;
      this.workspaceService?.requestFullSnapshot();
      this.workspaceService?.requestHostWorkspaceTree();
      this.connectionScreen?.showWorkspaces(this.workspaceService?.getHostWorkspaceTree() ?? null);
      this.connectionScreen?.setStatus("Choose a workspace to continue...");
      // Diagnostics subscriptions are no longer keyed by a static
      // route id. Each `EditorSurfaceChanged` / `EditorSurfaceList`
      // frame from the daemon carries a fresh `route_id`; we wire the
      // subscription up in `routeDiagnosticsFromWorkspace` below as
      // surfaces arrive. The daemon backfills the inventory in
      // response to the `ListEditorSurfaces` call right below, so we
      // don't miss the first diagnostics frame for surfaces opened by
      // another client before this connection.
      //
      // Phone-control parity: ask the daemon for its current session
      // / editor-surface inventory so panes opened by other clients
      // (e.g. neoism-agent on a paired phone) before we connected
      // materialise in the local chrome.
      this.surfaceRouteIds.clear();
      // Drive the daemon's inventory rebroadcast through the
      // `WorkplaceService` helper so the same code path runs whether
      // we're booting a fresh connection or rehydrating after a
      // `switchTo()` swap. The helper sends `ListSessions` +
      // `ListEditorSurfaces` against the active `ProtocolClient`; the
      // daemon's push-style `SessionList` / `EditorSurfaceList`
      // replies land in `routeDiagnosticsFromWorkspace` below and
      // wire up per-surface diagnostics subscriptions.
      // Do not request pane/session restore until the user picks a workspace.
    } else if (status === "closed" || status === "errored") {
      this.connectionScreen?.setBusy(false);
    }
  }

  private scheduleFallbackPtySpawn(): void {
    if (this.fallbackSpawnTimer !== null) {
      window.clearTimeout(this.fallbackSpawnTimer);
    }
    this.fallbackSpawnTimer = window.setTimeout(() => {
      this.fallbackSpawnTimer = null;
      if (!this.initialWorkspaceResolved) {
        return;
      }
      if (!this.workspaceGateSatisfied) {
        return;
      }
      if (this.terminalPanel || !this.ptyService) {
        return;
      }
      if (this.tryAttachExistingWorkspaceSession()) {
        return;
      }
      this.ptyService.spawn({
        cwd: this.activeWorkspaceRootPath ?? null,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
      });
      this.connectionScreen?.setStatus("Requesting PTY...");
    }, 150);
  }

  private spawnWorkspaceTerminal(workspaceRoot: string | null): void {
    if (this.terminalPanel || !this.ptyService) {
      return;
    }
    this.ptyService.spawn({
      cwd: workspaceRoot,
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
    });
    this.connectionScreen?.setStatus("Requesting PTY...");
  }

  private tryAttachExistingWorkspaceSession(): boolean {
    const tree = this.workspaceService?.getHostWorkspaceTree();
    const workspaceId = this.activeHostWorkspaceId;
    const workspaceTabs =
      workspaceId && tree
        ? tree.tabs.filter((candidate) => candidate.workspace_id === workspaceId)
        : [];
    const tab = workspaceTabs.find((candidate) => candidate.active && candidate.session_id)
      ?? workspaceTabs.find((candidate) => candidate.session_id);
    if (!tab?.session_id) {
      return false;
    }
    this.activeHostWorkspaceId = tab.workspace_id;
    this.syncCommandPaletteWorkspaceVisibility();
    const workspace = tree?.workspaces.find(
      (candidate) => candidate.id === tab.workspace_id,
    );
    this.handlePtyCreated(tab.session_id, workspace?.root_dir ?? null);
    this.terminalPanel?.applyWorkspaceLayoutSnapshot(workspace?.layout_snapshot);
    this.terminalPanel?.applyWorkspaceTabs(
      tree?.tabs.filter((candidate) => candidate.workspace_id === tab.workspace_id) ?? [],
      workspace?.active_tab_id,
    );
    return true;
  }

  private handlePtyCreated(
    sessionId: string,
    _rootHint: string | null = null,
  ): void {
    if (!this.client) {
      return;
    }
    if (this.terminalPanel) {
      this.terminalPanel.ptyCreated(sessionId);
      return;
    }
    this.activeSessionId = sessionId;
    const workspaceRoot = this.activeWorkspaceId
      ? this.workspaceService
          ?.getHostWorkspaceTree()
          .workspaces.find((workspace) => workspace.id === this.activeWorkspaceId)
          ?.root_dir ?? null
      : null;
    this.connectionScreen?.dispose();
    this.connectionScreen = null;
    this.terminalPanel = new TerminalPanel({
      client: this.client,
      pty: this.ptyService ?? undefined,
      workspaceRoot,
      sessionId,
      mount: this.root,
      onBridgeReady: (bridge) => {
        // Bridge is ready — bind the search service to it. Search
        // needs the bridge because the chrome panels surface
        // `IoError::Pending(req_id)` and resume only when JS calls
        // `bridge.service_reply(...)`.
        if (!this.client) return;
        this.searchService = new SearchService(this.client, bridge);
        this.searchService.install();
        // Diagnostics also needs the bridge now — each
        // `DiagnosticsServerMessage` variant maps to a specific
        // `set_diagnostics` / `hide_diagnostics` / `set_status_lsp_*`
        // bridge call. The service still keeps its pure-listener fan-out
        // so panels that subscribed before the bridge was up still see
        // the frame.
        this.diagnosticsService?.bindBridge(
          bridge as unknown as DiagnosticsBridge,
        );
        {
          const prefs = this.activeWorkspaceId
            ? this.workplaceService.getPreferences(this.activeWorkspaceId)
            : undefined;
          if (prefs) this.terminalPanel?.applyWorkplacePreferences(prefs);
          if (typeof prefs?.font_size !== "number") {
            const stored = this.storedFontSize();
            if (stored !== null) {
              this.terminalPanel?.applyWorkplacePreferences({
                font_size: stored,
              });
            }
          }
        }
        // Warm the tree cache so the first Cmd+P → Workspaces open is
        // instant. The modal itself is NOT auto-opened (it stole the
        // first keystrokes and got in the way); the picker is always
        // a Cmd+P away.
        this.workspaceService?.requestHostWorkspaceTree();
        this.renderWorkspaceChrome();
      },
      onFontSizeChanged: (fontSize) => this.persistActiveFontSize(fontSize),
      onShowWorkplaces: () => this.showWorkplaceSwitcherOverlay(),
      getWorkspacesModalPayload: () => this.buildWorkspacesModalPayload(),
      onWorkspaceSelected: (workspaceId) =>
        this.switchToWorkspaceById(workspaceId),
      onWorkspaceIslandIntent: (intent) => this.handleWorkspaceIslandIntent(intent),
      onCreateWorkspace: () => this.createWorkspaceOnConnectedHost(),
      onBufferTabsChanged: (tabs) => {
        if (this.suppressWorkspaceTabSync) return;
        this.publishActiveWorkspaceTabs(tabs);
        this.rememberCurrentStrip();
      },
    });
    // Replay git-status pushes that landed while the boot picker /
    // connection screen was still up.
    if (this.pendingStatusPushes.length > 0) {
      const pushes = this.pendingStatusPushes;
      this.pendingStatusPushes = [];
      for (const payload of pushes) {
        this.terminalPanel.serviceReply(
          0,
          payload as Parameters<TerminalPanel["serviceReply"]>[1],
        );
      }
    }
    if (workspaceRoot) {
      this.activeWorkspaceRootPath = workspaceRoot;
      this.terminalPanel?.setWorkspaceRoot(workspaceRoot);
    }
    // Establish this connection's project-root workspace on the daemon.
    // The daemon scopes editor-surface binds, pane-layout ops, and
    // session inventory to a per-connection active workspace; without
    // opening one, every file/tab open bounced with a "no active
    // workspace" error toast.
    if (workspaceRoot) {
      this.workspaceService?.openProjectRoot(workspaceRoot, false);
    }
  }

  /**
   * Alt+W (desktop Ctrl+Shift+W parity): create a fresh workspace. A
   * workspace IS a directory, so we declare one up front — the daemon
   * creates it if missing and roots everything there. The dir defaults to
   * the current workspace root so "new workspace" is one keystroke, but
   * you can point it anywhere to open a different folder.
   */
  private createWorkspaceOnConnectedHost(): void {
    const service = this.workspaceService;
    if (!service) return;
    const suggested = this.activeWorkspaceRootPath ?? "";
    const dir = window
      .prompt("New workspace directory (absolute path, empty for daemon default):", suggested)
      ?.trim();
    if (dir === undefined || dir === null) return; // cancelled
    const rootDir = dir.length > 0 ? dir : null;
    const title = rootDir ? rootDir.split("/").filter(Boolean).pop() ?? "Workspace" : "Workspace";
    const workspaceId = crypto.randomUUID();
    this.pendingCreateWorkspaceId = workspaceId;
    service.createWorkspace(title, rootDir, workspaceId);
    service.requestHostWorkspaceTree();
  }

  /**
   * Publish the panel's tab strip as the active workspace's tab list
   * in the daemon tree. This is what makes a web workspace ADOPTABLE:
   * desktop's pick rebuilds it from exactly these entries — terminals
   * by session id, files by path.
   */
  private publishActiveWorkspaceTabs(
    tabs: Array<{
      title: string;
      kind: string;
      path: string | null;
      sessionId: string | null;
      active: boolean;
    }>,
  ): void {
    const workspaceId = this.activeHostWorkspaceId;
    const service = this.workspaceService;
    if (!workspaceId || !service) return;
    const now = Math.floor(Date.now() / 1000);
    const root = this.activeWorkspaceRootPath;
    const summaries: WorkspaceTabSummary[] = tabs.map((tab, index) => {
      let cwd = tab.path;
      if (cwd && !cwd.startsWith("/") && root) {
        cwd = `${root.replace(/\/$/, "")}/${cwd}`;
      }
      const kind =
        tab.kind === "file"
          ? /\.(md|markdown|mdx)$/i.test(tab.path ?? "")
            ? "markdown"
            : "editor"
          : tab.kind;
      return {
        id: `${workspaceId}-web-${index}`,
        workspace_id: workspaceId,
        title: tab.title,
        kind,
        session_id: tab.sessionId,
        surface_id: null,
        cwd,
        active: tab.active,
        last_active: now,
      };
    });
    service.publishWorkspaceTabs(workspaceId, summaries);
  }

  /**
   * React to daemon workspace pushes: live-refresh the open Workspaces
   * modal whenever tree data lands, and finish the Alt+W
   * create-workspace flow (switch + fresh terminal tab) once the
   * daemon confirms the new workspace id.
   */
  private installWorkspaceTreeSubscription(): void {
    this.workspaceService?.subscribe((msg) => {
      if ("HostWorkspaceUpserted" in msg) {
        const workspaceId = msg.HostWorkspaceUpserted.workspace.id;
        if (this.pendingCreateWorkspaceId === workspaceId) {
          this.pendingCreateWorkspaceId = null;
          this.workspaceGateSatisfied = true;
          this.initialWorkspaceResolved = true;
          // Entering the brand-new workspace: swap to ITS (empty)
          // strip — which lands in a fresh shell — instead of
          // appending a terminal to the previous workspace's tabs.
          this.switchToWorkspaceById(workspaceId, { terminalOnly: true });
          this.workplaceService.requestPaneSnapshot();
          this.terminalPanel?.resetToWorkspaceTabs([], null);
          this.scheduleFallbackPtySpawn();
        }
      }
      if ("HostWorkspaceUpserted" in msg) {
        this.maybeReRootActiveWorkspace();
      }
      if ("HostWorkspaceTree" in msg && this.pendingRemoteWorkspaceId) {
        const workspace = msg.HostWorkspaceTree.workspaces.find(
          (candidate) =>
            candidate.id === this.pendingRemoteWorkspaceId &&
            !!candidate.root_dir &&
            candidate.root_dir.startsWith("/"),
        );
        if (workspace) {
          const expectedRoot = this.pendingRemoteWorkspaceRoot;
          if (expectedRoot && workspace.root_dir !== expectedRoot) {
            console.error("Refusing remote workspace whose root changed during join", {
              expectedRoot,
              receivedRoot: workspace.root_dir,
              workspaceId: workspace.id,
            });
            this.pendingRemoteWorkspaceId = null;
            this.pendingRemoteWorkspaceRoot = null;
            return;
          }
          this.pendingRemoteWorkspaceId = null;
          this.pendingRemoteWorkspaceRoot = null;
          this.acceptWorkspaceGate(workspace);
        }
      }
      if (
        "HostWorkspaceTree" in msg ||
        "HostWorkspaceList" in msg ||
        "HostList" in msg
      ) {
        this.renderWorkspaceChrome();
        this.terminalPanel?.refreshWorkspacesModal();
        this.connectionScreen?.showWorkspaces(this.workspaceService!.getHostWorkspaceTree());
        // The daemon re-points a workspace's root_dir when its terminal
        // cd's; if that's the workspace we're in, follow it live.
        this.maybeReRootActiveWorkspace();
      }
      if ("InitialWorkspaceResolved" in msg) {
        // Browser startup is gated on an explicit workspace choice. The daemon
        // can still publish an initial workspace, but replaying it before web
        // has a selected root restores editor tabs as unsafe absolute paths.
        if (this.workspaceGateSatisfied) {
          this.initialWorkspaceResolved = true;
          this.switchToWorkspaceById(msg.InitialWorkspaceResolved.workspace.id);
          this.scheduleFallbackPtySpawn();
        }
      }
    });
  }

  private acceptWorkspaceGate(workspace: WorkspaceSummary): void {
    this.workspaceGateSatisfied = true;
    this.initialWorkspaceResolved = true;
    this.connectionScreen?.setStatus(`Opening ${workspace.title || workspace.id}...`);
    this.switchToWorkspaceById(workspace.id, { terminalOnly: true });
    this.workplaceService.requestPaneSnapshot();
    this.scheduleFallbackPtySpawn();
  }

  /**
   * Build the wasm Workspaces-modal payload from the latest
   * `HostWorkspaceTree`. Returns `null` only when there is no
   * workspace service at all (the legacy DOM switcher then covers
   * connect/create flows). An empty/late tree still yields a payload —
   * the modal opens immediately and live-refreshes when the tree
   * arrives (see installWorkspaceTreeSubscription).
   *
   * Mirrors desktop's `open_daemon_workspaces_picker`: kick a tree
   * refresh for next time, then render the last-known snapshot now.
   * The "local" host is resolved by matching each host's advertised
   * `daemon_url` against the workplace we're currently dialed into —
   * the web client's moral equivalent of the desktop's own machine.
   */
  private buildWorkspacesModalPayload(): WorkspacesModalPayload | null {
    const service = this.workspaceService;
    if (!service) return null;
    service.requestHostWorkspaceTree();
    const tree = service.getHostWorkspaceTree();

    const activeHost = wsUrlHost(this.workplaceService.getActiveUrl());
    const hostIndex = new Map(tree.hosts.map((host) => [host.id, host]));
    const isLocalHost = (hostId: string): boolean => {
      const url = hostIndex.get(hostId)?.daemon_url;
      const host = wsUrlHost(url ?? null);
      return host !== null && activeHost !== null && host === activeHost;
    };

    const workspaces: WorkspacesModalWorkspace[] = tree.workspaces.map(
      (workspace) => {
        const host = hostIndex.get(workspace.host_id);
        const local = isLocalHost(workspace.host_id);
        return {
          title: workspace.title,
          detail:
            workspace.root_dir && workspace.root_dir.length > 0
              ? workspace.root_dir
              : "daemon workspace",
          workspace_id: workspace.id,
          host_id: workspace.host_id,
          host_label: host?.label ?? workspace.host_id,
          host_kind:
            workspace.host_kind === "cloud_sandbox" ||
            workspace.host_kind === "docker_sandbox"
              ? "cloud"
              : local
                ? "local"
                : "remote",
          workspace_host_kind: workspace.host_kind ?? "local",
          workspace_visibility: workspace.visibility ?? "private",
          // The local host's URL is implicit (we're dialed into it);
          // matches the desktop picker, which hides the URL for ⌂.
          daemon_url: local ? null : host?.daemon_url ?? null,
          host_online: host?.online ?? local,
        };
      },
    );

    // Hosts with zero workspaces still get a header row so the tree
    // shows every known machine (drag-to-move onto them is view-only
    // on web for now — no move dispatch is wired).
    const populated = new Set(tree.workspaces.map((w) => w.host_id));
    const peer_hosts: WorkspacesModalPeerHost[] = tree.hosts
      .filter((host) => !populated.has(host.id))
      .map((host) => ({
        host_id: host.id,
        label: host.label,
        kind: isLocalHost(host.id) ? ("local" as const) : ("remote" as const),
        daemon_url: host.daemon_url ?? null,
        online: host.online ?? isLocalHost(host.id),
      }));

    // First open races the async tree request — synthesize a header
    // row for the daemon we're dialed into so the modal never opens
    // blank. The workspace-service subscription live-refreshes the
    // modal the moment the real tree lands.
    if (workspaces.length === 0 && peer_hosts.length === 0) {
      peer_hosts.push({
        host_id: "local",
        label: activeHost ?? "this daemon",
        kind: "local",
        daemon_url: null,
        online: true,
      });
    }

    return { workspaces, peer_hosts };
  }

  /** Workspace pick from the wasm Workspaces modal. Resolve its owning daemon
   * before issuing workspace/session/file requests. */
  private switchToWorkspaceById(
    workspaceId: string,
    options: { terminalOnly?: boolean } = {},
  ): void {
    const tree = this.workspaceService?.getHostWorkspaceTree();
    const workspace = tree?.workspaces.find(
      (candidate) => candidate.id === workspaceId,
    );
    if (!workspace) return;
    const host = tree?.hosts.find((candidate) => candidate.id === workspace.host_id);
    const targetUrl = host?.daemon_url ?? null;
    const activeUrl = this.workplaceService.getActiveUrl();
    if (
      targetUrl &&
      (!activeUrl ||
        normalizedDaemonUrl(targetUrl) !== normalizedDaemonUrl(activeUrl))
    ) {
      const target = this.workplaceService.addWorkplace({
        id: host?.id ?? workspace.host_id,
        label: host?.label ?? workspace.host_id,
        url: targetUrl,
        transport: "tailscale",
      });
          this.pendingRemoteWorkspaceId = workspace.id;
          this.pendingRemoteWorkspaceRoot = workspace.root_dir ?? null;
          this.connectViaService(target.id);
      return;
    }
    this.switchToWorkspace(workspace, options);
  }

  /** Switch the active daemon workspace and re-attach the terminal
   *  panel to its live session/tabs. Shared by the wasm Workspaces
   *  modal and the legacy DOM switcher overlay. */
  private switchToWorkspace(
    workspace: WorkspaceSummary,
    options: { terminalOnly?: boolean } = {},
  ): void {
    this.rememberCurrentStrip();
    this.activeHostWorkspaceId = workspace.id;
    this.addWorkspaceSubscription(workspace.id);
    this.persistWorkspaceSubscriptions();
    this.syncCommandPaletteWorkspaceVisibility();
    this.renderWorkspaceChrome();
    this.workspaceService?.switchHostWorkspace(workspace.id);
    this.workspaceService?.listWorkspaceTabs(workspace.id);
    const tree = this.workspaceService?.getHostWorkspaceTree();
    const tabs =
      tree?.tabs.filter((candidate) => candidate.workspace_id === workspace.id) ?? [];
    const tab =
      tabs.find((candidate) => candidate.active && candidate.session_id) ??
      tabs.find((candidate) => candidate.session_id);
    const newRoot = workspace.root_dir ?? null;
    if (!newRoot || !newRoot.startsWith("/")) {
      console.error("Refusing to switch workspace without an absolute daemon root", workspace);
      return;
    }
    this.activeWorkspaceRootPath = newRoot;
    this.terminalPanel?.setWorkspaceRoot(newRoot);
    if (tab?.session_id) {
      this.handlePtyCreated(tab.session_id, newRoot);
      this.activeSessionId = tab.session_id;
    }
    // A workspace IS its directory. Root the Explorer at the daemon-owned
    // root_dir; PTY cwd is pane-local state and must never redefine it.
    this.terminalPanel?.applyWorkspaceLayoutSnapshot(workspace.layout_snapshot);
    // Daemon tabs are the source of truth on workspace entry/reload. A
    // browser-local strip is only an in-memory fallback for workspaces that
    // have not published tabs yet.
    const remembered = this.workspaceStrips.get(workspace.id);
    const liveSessionIds = new Set(
      tabs
        .map((candidate) => candidate.session_id)
        .filter((id): id is string => typeof id === "string" && id.length > 0),
    );
    if (tabs.length > 0) {
      this.withSuppressedWorkspaceTabSync(() => {
        this.terminalPanel?.resetToWorkspaceTabs(tabs, workspace.active_tab_id, options);
      });
    } else if (remembered && remembered.length > 0) {
      this.withSuppressedWorkspaceTabSync(() => {
        this.terminalPanel?.restoreStripSnapshot(remembered, liveSessionIds);
      });
    } else {
      this.withSuppressedWorkspaceTabSync(() => {
        this.terminalPanel?.resetToWorkspaceTabs(tabs, workspace.active_tab_id);
      });
    }
    if (!tab?.session_id) {
      this.spawnWorkspaceTerminal(newRoot);
    }
  }

  /** Re-root the Explorer when the daemon broadcasts that the workspace
   *  we're in changed its directory (its main terminal cd'd, or it was
   *  re-pointed). The workspace's daemon-owned `root_dir` is the single
   *  source of truth — same value desktop and every other client see. */
  private maybeReRootActiveWorkspace(): void {
    const id = this.activeHostWorkspaceId;
    if (!id) return;
    const root =
      this.workspaceService
        ?.getHostWorkspaceTree()
        .workspaces.find((w) => w.id === id)?.root_dir ?? null;
    if (root && root.startsWith("/") && root !== this.activeWorkspaceRootPath) {
      this.activeWorkspaceRootPath = root;
      this.terminalPanel?.setWorkspaceRoot(root);
    }
  }

  /** Daemon-pushed live cwd for a PTY session (the shell `cd`'d, or it
   *  just reported its initial directory). The daemon/desktop main-terminal
   *  path owns workspace root changes; web only caches per-terminal cwd and
   *  follows daemon workspace broadcasts for Explorer re-rooting. */
  private handleSessionCwd(sessionId: string, cwd: string): void {
    if (!cwd || !cwd.startsWith("/")) {
      return;
    }
    this.sessionCwds.set(sessionId, cwd);
  }

  private renderWorkspaceChrome(): void {
    const tree = this.workspaceService?.getHostWorkspaceTree();
    if (!tree) return;
    this.syncCommandPaletteWorkspaceVisibility();
    if (this.activeHostWorkspaceId) {
      this.addWorkspaceSubscription(this.activeHostWorkspaceId);
    }
    const subscribed = tree.workspaces.filter((workspace) =>
      this.subscribedWorkspaceIds.has(workspace.id),
    );
    const workspaceById = new Map(tree.workspaces.map((workspace) => [workspace.id, workspace]));
    const ordered = this.workspaceSubscriptionOrder
      .filter((id) => this.subscribedWorkspaceIds.has(id) && workspaceById.has(id));
    for (const workspace of subscribed) {
      if (!ordered.includes(workspace.id)) ordered.push(workspace.id);
    }
    this.workspaceSubscriptionOrder = ordered;
    this.terminalPanel?.setWorkspaceIslandTabs(JSON.stringify({
      tabs: ordered.flatMap((id) => {
        const workspace = workspaceById.get(id);
        if (!workspace) return [];
        return [{
          id: workspace.id,
          title: workspace.title || workspace.root_dir || "Workspace",
          host_kind:
            workspace.host_kind === "local" &&
            (workspace.visibility === "shared" || workspace.visibility === "team")
              ? "shared"
              : workspace.host_kind ?? "local",
        }];
      }),
      active_id: this.activeHostWorkspaceId,
    }));
  }

  private handleWorkspaceIslandIntent(intent: {
    kind: "activate" | "context_menu" | "open_workspaces";
    workspace_id?: string | null;
    x?: number | null;
    y?: number | null;
  }): void {
    if (intent.kind === "open_workspaces") {
      this.terminalPanel?.showWorkspacesModal();
      return;
    }
    const tree = this.workspaceService?.getHostWorkspaceTree();
    const workspace = tree?.workspaces.find((item) => item.id === intent.workspace_id);
    if (!workspace) return;
    if (intent.kind === "activate") {
      this.switchToWorkspaceById(workspace.id);
      return;
    }
    this.openWorkspaceContextMenu(intent.x ?? 0, intent.y ?? 0, workspace);
  }

  private syncCommandPaletteWorkspaceVisibility(): void {
    const tree = this.workspaceService?.getHostWorkspaceTree();
    const workspace = tree?.workspaces.find((item) => item.id === this.activeHostWorkspaceId);
    this.terminalPanel?.setCommandPaletteWorkspaceVisibility(workspace?.visibility ?? "private");
  }

  private openWorkspaceContextMenu(x: number, y: number, workspace: WorkspaceSummary): void {
    document.querySelector(".workspace-context-menu")?.remove();
    const menu = document.createElement("div");
    menu.className = "workspace-context-menu";
    menu.style.left = `${x}px`;
    menu.style.top = `${y}px`;
    const addAction = (label: string, action: () => void) => {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = label;
      button.addEventListener("click", () => {
        menu.remove();
        action();
      });
      menu.appendChild(button);
    };
    if (this.subscribedWorkspaceIds.has(workspace.id)) {
      addAction("Unsubscribe here", () => {
        this.workspaceService?.unsubscribeWorkspace(workspace.id);
        this.removeWorkspaceSubscription(workspace.id);
      });
    } else {
      addAction("Subscribe here", () => {
        this.workspaceService?.subscribeWorkspace(workspace.id);
        this.addWorkspaceSubscription(workspace.id);
        this.persistWorkspaceSubscriptions();
        this.renderWorkspaceChrome();
      });
    }
    const runSharedAction = (kind: WorkspaceChromeActionKind) => {
      switch (kind) {
        case "share":
          this.workspaceService?.shareWorkspace(workspace.id);
          break;
        case "stop_sharing":
          this.workspaceService?.stopSharingWorkspace(workspace.id);
          break;
        case "send_to_docker_sandbox":
          void this.sendWorkspaceToDockerSandbox(workspace);
          break;
        case "send_to_cloud":
          this.workspaceService?.sendWorkspaceToCloud(workspace.id);
          break;
      }
    };
    workspaceChromeActionsForVisibility(workspace.visibility ?? "private").forEach((action) => {
      addAction(action.label, () => runSharedAction(action.kind));
    });
    document.body.appendChild(menu);
    const close = () => menu.remove();
    setTimeout(() => window.addEventListener("click", close, { once: true }), 0);
  }

  private async sendWorkspaceToDockerSandbox(workspace: WorkspaceSummary): Promise<void> {
    const modal = this.showWorkspaceOperationModal(
      "Sending to Docker sandbox",
      "Exporting snapshot, starting container, and importing workspace...",
      true,
    );
    try {
      const activeUrl = this.workplaceService.getActiveUrl();
      if (!activeUrl) throw new Error("no active daemon URL");
      const base = activeUrl.replace(/^ws/i, "http").replace(/\/session\/?$/, "");
      const response = await fetch(`${base}/workspace/docker-sandbox`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ workspace_id: workspace.id }),
      });
      const text = await response.text();
      if (!response.ok) {
        throw new Error(text || `HTTP ${response.status}`);
      }
      this.workspaceService?.requestHostWorkspaceTree();
      modal.update("Docker sandbox ready", text || "Workspace imported into Docker sandbox.", false);
    } catch (err) {
      modal.update(
        "Docker sandbox failed",
        err instanceof Error ? err.message : String(err),
        false,
      );
    }
  }

  private showWorkspaceOperationModal(
    titleText: string,
    bodyText: string,
    busy: boolean,
  ): { update: (title: string, body: string, busy: boolean) => void } {
    document.querySelector(".workspace-operation-backdrop")?.remove();
    const backdrop = document.createElement("div");
    backdrop.className = "workspace-confirm-backdrop workspace-operation-backdrop";
    const modal = document.createElement("div");
    modal.className = "workspace-confirm-modal workspace-operation-modal";
    const title = document.createElement("h2");
    const body = document.createElement("p");
    const spinner = document.createElement("div");
    spinner.className = "workspace-operation-spinner";
    const actions = document.createElement("div");
    actions.className = "workspace-confirm-actions";
    const close = document.createElement("button");
    close.type = "button";
    close.textContent = "Close";
    close.disabled = busy;
    close.addEventListener("click", () => backdrop.remove());
    actions.appendChild(close);
    modal.append(spinner, title, body, actions);
    backdrop.appendChild(modal);
    document.body.appendChild(backdrop);
    const update = (nextTitle: string, nextBody: string, nextBusy: boolean) => {
      title.textContent = nextTitle;
      body.textContent = nextBody;
      spinner.hidden = !nextBusy;
      close.disabled = nextBusy;
    };
    update(titleText, bodyText, busy);
    return { update };
  }

  private rememberCurrentStrip(): void {
    const workspaceId = this.activeHostWorkspaceId;
    const panel = this.terminalPanel;
    if (!workspaceId || !panel) return;
    this.workspaceStrips.set(workspaceId, panel.captureStripSnapshot());
  }

  private withSuppressedWorkspaceTabSync(action: () => void): void {
    const previous = this.suppressWorkspaceTabSync;
    this.suppressWorkspaceTabSync = true;
    try {
      action();
    } finally {
      this.suppressWorkspaceTabSync = previous;
    }
  }

  private persistWorkspaceSubscriptions(): void {
    try {
      window.localStorage.setItem(
        WORKSPACE_SUBSCRIPTIONS_STORAGE_KEY,
        JSON.stringify(this.workspaceSubscriptionOrder.filter((id) => this.subscribedWorkspaceIds.has(id))),
      );
    } catch {
      // Keep in-memory subscriptions for this browser tab.
    }
  }

  private addWorkspaceSubscription(workspaceId: string): void {
    this.subscribedWorkspaceIds.add(workspaceId);
    if (!this.workspaceSubscriptionOrder.includes(workspaceId)) {
      this.workspaceSubscriptionOrder.push(workspaceId);
    }
  }

  private removeWorkspaceSubscription(workspaceId: string): void {
    this.subscribedWorkspaceIds.delete(workspaceId);
    this.workspaceSubscriptionOrder = this.workspaceSubscriptionOrder.filter((id) => id !== workspaceId);
    if (this.activeHostWorkspaceId === workspaceId) {
      const tree = this.workspaceService?.getHostWorkspaceTree();
      const nextWorkspace = tree?.workspaces.find((workspace) =>
        this.subscribedWorkspaceIds.has(workspace.id),
      );
      this.activeHostWorkspaceId = nextWorkspace?.id ?? null;
      if (nextWorkspace) {
        this.workspaceService?.switchHostWorkspace(nextWorkspace.id);
        this.workspaceService?.listWorkspaceTabs(nextWorkspace.id);
      }
    }
    this.persistWorkspaceSubscriptions();
    this.syncCommandPaletteWorkspaceVisibility();
    this.renderWorkspaceChrome();
  }

  private loadWorkspaceSubscriptions(): void {
    try {
      const raw = window.localStorage.getItem(WORKSPACE_SUBSCRIPTIONS_STORAGE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) {
        for (const id of parsed) {
          if (typeof id === "string" && id.length > 0) {
            this.subscribedWorkspaceIds.add(id);
            if (!this.workspaceSubscriptionOrder.includes(id)) {
              this.workspaceSubscriptionOrder.push(id);
            }
          }
        }
      }
    } catch {
      // Corrupt or inaccessible storage: start with only the active workspace.
    }
  }

  private handlePtyOutput(sessionId: string, bytes: Uint8Array): void {
    if (!this.terminalPanel) {
      return;
    }
    this.terminalPanel.ingestPty(sessionId, bytes);
  }

  /**
   * Self-heal a stale session attach. The persisted host workspace
   * tree outlives daemon restarts but live PTYs don't, so
   * `tryAttachExistingWorkspaceSession` / `switchToWorkspace` can bind
   * the panel to a session id the daemon no longer knows. Every
   * Resize/PtyInput then bounces with `unknown session <id>` and the
   * terminal sits dead — no prompt marks, no composer, keystrokes go
   * nowhere. Swap the dead binding for a fresh shell; the `PtyCreated`
   * reply re-attaches through the normal pty-created path.
   */
  private handlePtyError(message: string): void {
    const match = /^unknown session (\S+)$/.exec(message);
    if (!match || !this.ptyService || !this.terminalPanel) {
      return;
    }
    const staleId = match[1];
    if (this.staleSessionRecovery.has(staleId)) {
      return;
    }
    this.staleSessionRecovery.add(staleId);
    if (this.terminalPanel.respawnDeadPtySession(staleId)) {
      if (this.activeSessionId === staleId) {
        this.activeSessionId = null;
      }
    }
  }

  private handlePtyClosed(sessionId: string, _exitCode: number | null): void {
    if (this.terminalPanel) {
      const stillRunning = this.terminalPanel.ptyClosed(sessionId);
      if (stillRunning) {
        return;
      }
    } else if (sessionId !== this.activeSessionId) {
      return;
    }
    this.clearTerminal();
    this.workplaceService.disconnect();
    this.client = null;
    this.activeSessionId = null;
    this.showConnectionScreen();
    this.connectionScreen?.setStatus("Session closed");
    this.connectionScreen?.setBusy(false);
  }

  private clearTerminal(): void {
    if (this.fallbackSpawnTimer !== null) {
      window.clearTimeout(this.fallbackSpawnTimer);
      this.fallbackSpawnTimer = null;
    }
    this.hideWorkplaceSwitcherOverlay();
    this.terminalPanel?.dispose();
    this.terminalPanel = null;
    this.connectionScreen?.dispose();
    this.connectionScreen = null;
    this.ptyService = null;
    this.searchService = null;
    this.workspaceService = null;
    this.diagnosticsService = null;
    this.surfaceRouteIds.clear();
    this.activeWorkspaceId = null;
    if (!this.pendingRemoteWorkspaceId) {
      this.pendingRemoteWorkspaceRoot = null;
    }
    this.initialWorkspaceResolved = false;
    this.workspaceGateSatisfied = false;
  }

  private noteActiveWorkspace(workspaceId: string): void {
    if (this.activeHostWorkspaceId !== workspaceId) {
      this.activeHostWorkspaceId = workspaceId;
      this.renderWorkspaceChrome();
    }
    const workspaceChanged = this.activeWorkspaceId !== workspaceId;
    if (workspaceChanged) this.activeWorkspaceId = workspaceId;
    this.applyEffectiveWorkspacePreferences(workspaceId);
    if (!workspaceChanged) return;
    this.workplaceService.requestPreferences(workspaceId);
    // Wave 4B: arm the re-home watcher on the workspace we're now
    // viewing, seeding the baseline with its current home host so a
    // later move can be told apart from the first observation.
    const summary = this.workspaceService
      ?.getHostWorkspaceTree()
      .workspaces.find((w) => w.id === workspaceId);
    this.workplaceService.setFollowedWorkspace(
      workspaceId,
      summary?.running_on_host_id ?? summary?.host_id ?? null,
    );
  }

  /**
   * Wave 4B: feed the workspace summaries carried by a tree / control
   * push into the `WorkplaceService` re-home watcher. The service is the
   * persistent owner of the active connection, so it's the right place
   * to decide "the workspace I'm viewing moved hosts — re-dial".
   */
  private observeWorkspaceHoming(payload: WorkspaceServerMessage): void {
    // Cache each host's advertised daemon_url (Wave 4E) so a re-home can
    // resolve `running_on_host_id` -> dialable URL directly.
    const hosts = hostsFromMessage(payload);
    if (hosts.length > 0) this.workplaceService.recordHostDaemonUrls(hosts);
    const summaries = workspaceSummariesFromMessage(payload);
    if (summaries.length === 0) return;
    this.workplaceService.observeWorkspaceHoming(summaries);
  }

  /**
   * Wave 4B: react to a re-home the `WorkplaceService` detected. When
   * the move resolved to a connectable host the service has already
   * re-dialled (via the handlers we installed with `setRehomeHandlers`),
   * so here we re-arm the per-connection services against the fresh
   * client and re-request the workspace tree so the daemon re-ships the
   * pane inventory at the new home. When the move did NOT resolve we
   * surface a status line so the user knows to reconnect by hand.
   */
  private handleRehome(
    event: Extract<
      Parameters<Parameters<WorkplaceService["subscribe"]>[0]>[0],
      { kind: "rehome" }
    >,
  ): void {
    if (!event.resolved || !event.targetId) {
      this.connectionScreen?.setStatus(
        `Workspace moved to host "${event.newHostId}" — reconnect manually (no reachable address for that host yet).`,
      );
      return;
    }
    // The service swapped `activeClient` to the new host. Re-bind the
    // per-connection services to it and re-drive the connection bring-up
    // so the daemon re-ships sessions / surfaces / tree at the new home,
    // exactly like a manual `switchTo`.
    const client = this.workplaceService.getActiveClient();
    if (!client) return;
    this.clearTerminal();
    this.client = client;
    this.ptyService = new PtyService(client);
    this.ptyService.subscribe({
      onCreated: (sessionId, workspaceRoot) =>
        this.handlePtyCreated(sessionId, workspaceRoot),
      onOutput: (sessionId, bytes) => this.handlePtyOutput(sessionId, bytes),
      onClosed: (sessionId, exitCode) =>
        this.handlePtyClosed(sessionId, exitCode),
      onError: (message) => this.handlePtyError(message),
    });
    this.workspaceService = new WorkspaceService(client);
    this.installWorkspaceTreeSubscription();
    this.diagnosticsService = new DiagnosticsService(client);
    this.connectionScreen = new ConnectionScreen(
      this.connectionScreenOptions(event.targetUrl ?? this.defaultConnectionUrl()),
    );
    this.connectionScreen.setBusy(true);
    this.connectionScreen.setStatus(
      `Workspace moved — following to ${event.targetUrl ?? event.newHostId}...`,
    );
    client.connect();
  }

  private showWorkplaceSwitcherOverlay(): void {
    this.hideWorkplaceSwitcherOverlay();
    const overlay = document.createElement("div");
    overlay.className = "workplace-switcher-overlay";
    overlay.setAttribute("role", "dialog");
    overlay.setAttribute("aria-label", "Workplaces");

    const popover = document.createElement("div");
    popover.className = "workplace-switcher-popover";
    overlay.appendChild(popover);

    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "workplace-switcher-close";
    closeButton.setAttribute("aria-label", "Close workplaces");
    closeButton.textContent = "Close";
    closeButton.addEventListener("click", () =>
      this.hideWorkplaceSwitcherOverlay(),
    );
    popover.appendChild(closeButton);

    const switcherMount = document.createElement("div");
    popover.appendChild(switcherMount);

    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) {
        this.hideWorkplaceSwitcherOverlay();
      }
    });
    this.workplaceSwitcherKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        this.hideWorkplaceSwitcherOverlay();
      }
    };
    document.addEventListener("keydown", this.workplaceSwitcherKeydown);

    this.root.appendChild(overlay);
    this.workplaceSwitcherOverlay = overlay;
    this.workplaceSwitcher = new WorkplaceSwitcher({
      mount: switcherMount,
      service: this.workplaceService,
      workspaceService: this.workspaceService,
      onPick: (entry, token) => {
        this.hideWorkplaceSwitcherOverlay();
        this.handleDiscoveredPick(entry, token);
      },
      onSwitch: (entry) => {
        this.hideWorkplaceSwitcherOverlay();
        this.handleSwitchTo(entry);
      },
      onWorkspaceSwitch: (workspace) => {
        this.hideWorkplaceSwitcherOverlay();
        this.switchToWorkspaceById(workspace.id);
      },
      onWorkspaceControl: (workspace) => {
        this.noteActiveWorkspace(workspace.id);
        this.workspaceService?.controlWorkspace(workspace.id, WEB_CONTROLLER_HOST_ID);
        this.workspaceService?.switchHostWorkspace(workspace.id);
        this.workspaceService?.listWorkspaceTabs(workspace.id);
        this.terminalPanel?.applyWorkspaceLayoutSnapshot(workspace.layout_snapshot);
        const tree = this.workspaceService?.getHostWorkspaceTree();
        this.terminalPanel?.applyWorkspaceTabs(
          tree?.tabs.filter((candidate) => candidate.workspace_id === workspace.id) ?? [],
          workspace.active_tab_id,
        );
      },
      onWorkspaceCreate: (hostId, title, rootDir) => {
        this.workspaceService?.createHostWorkspace(hostId, title, rootDir);
        this.workspaceService?.requestHostWorkspaceTree();
      },
    });
    closeButton.focus();
  }

  private hideWorkplaceSwitcherOverlay(): void {
    this.workplaceSwitcher?.dispose();
    this.workplaceSwitcher = null;
    this.workplaceSwitcherOverlay?.remove();
    this.workplaceSwitcherOverlay = null;
    if (this.workplaceSwitcherKeydown) {
      document.removeEventListener("keydown", this.workplaceSwitcherKeydown);
      this.workplaceSwitcherKeydown = null;
    }
  }

  private maybeRequestWorkspacePreferences(payload: WorkspaceServerMessage): void {
    if ("SessionCreated" in payload) {
      this.noteActiveWorkspace(payload.SessionCreated.session.workspace_id);
      return;
    }
    if ("SessionState" in payload) {
      this.noteActiveWorkspace(payload.SessionState.workspace_id);
      return;
    }
    if ("SessionList" in payload) {
      const sessions = payload.SessionList.sessions;
      const current =
        (this.activeSessionId &&
          sessions.find((session) => session.id === this.activeSessionId)) ||
        [...sessions].sort((a, b) => (b.last_active ?? 0) - (a.last_active ?? 0))[0];
      if (current) this.noteActiveWorkspace(current.workspace_id);
    }
  }

  private static readonly FONT_SIZE_STORAGE_KEY = "neoism.font_size";

  private applyEffectiveWorkspacePreferences(workspaceId: string): void {
    const cached = this.workplaceService.getPreferences(workspaceId);
    const storedFontSize = this.storedFontSize();
    if (!cached && storedFontSize === null) return;
    const prefs: WorkplacePreferences = cached ? { ...cached } : {};
    if (prefs.font_size === undefined && storedFontSize !== null) {
      prefs.font_size = storedFontSize;
    }
    this.terminalPanel?.applyWorkplacePreferences(prefs);
  }

  private persistActiveFontSize(fontSize: number): void {
    // Always keep a browser-local copy: the workspace-pref path needs
    // an active workspace id + daemon round-trip, and zoom done before
    // either exists was silently lost on refresh.
    try {
      localStorage.setItem(App.FONT_SIZE_STORAGE_KEY, String(fontSize));
    } catch {
      // Storage can be unavailable (private mode) — workspace prefs
      // below still cover the common case.
    }
    const workspaceId = this.activeWorkspaceId;
    if (!workspaceId) return;
    const current = this.workplaceService.getPreferences(workspaceId) ?? {};
    const prefs: WorkplacePreferences = {
      ...current,
      font_size: fontSize,
    };
    this.workplaceService.setPreferences(workspaceId, prefs);
  }

  /** Browser-local zoom fallback for boots where workspace prefs are
   *  absent or don't carry a font size. */
  private storedFontSize(): number | null {
    try {
      const raw = localStorage.getItem(App.FONT_SIZE_STORAGE_KEY);
      const parsed = raw === null ? NaN : Number(raw);
      return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
    } catch {
      return null;
    }
  }

  /**
   * Mirror the daemon's editor-surface inventory into per-surface
   * diagnostics subscriptions. The daemon assigns a fresh `route_id`
   * to every `BindEditorSurface` and broadcasts it back inside
   * `EditorSurfaceList` / `EditorSurfaceChanged`; the chrome's
   * previous "hard-code route 1" path didn't survive multi-pane
   * frontends where each pane has its own LSP route.
   *
   * Idempotent: `DiagnosticsService.watch` already de-duplicates via a
   * Set so re-binds (path retargets) don't churn the wire.
   */
  private routeDiagnosticsFromWorkspace(payload: WorkspaceServerMessage): void {
    const diagnostics = this.diagnosticsService;
    if (!diagnostics) return;
    if ("EditorSurfaceList" in payload) {
      for (const surface of payload.EditorSurfaceList.surfaces) {
        this.bindSurfaceRoute(surface.surface_id, surface.route_id ?? null);
      }
      return;
    }
    if ("EditorSurfaceChanged" in payload) {
      const surface = payload.EditorSurfaceChanged.surface;
      this.bindSurfaceRoute(surface.surface_id, surface.route_id ?? null);
      return;
    }
    if ("EditorSurfaceClosed" in payload) {
      const surfaceId = payload.EditorSurfaceClosed.surface_id;
      const routeId = this.surfaceRouteIds.get(surfaceId);
      if (routeId !== undefined) {
        this.surfaceRouteIds.delete(surfaceId);
        diagnostics.unwatch(routeId);
      }
    }
  }

  private bindSurfaceRoute(
    surfaceId: string,
    routeId: number | null | undefined,
  ): void {
    const diagnostics = this.diagnosticsService;
    if (!diagnostics) return;
    if (routeId === null || routeId === undefined) {
      // Older daemons don't populate `route_id` on the wire. Without a
      // daemon-assigned id we have nothing to subscribe to; leave the
      // surface unsubscribed rather than re-introduce the legacy "always
      // route 1" assumption that broke once the daemon learned to assign
      // distinct ids per surface.
      return;
    }
    const previous = this.surfaceRouteIds.get(surfaceId);
    if (previous === routeId) return;
    if (previous !== undefined) {
      diagnostics.unwatch(previous);
    }
    this.surfaceRouteIds.set(surfaceId, routeId);
    diagnostics.watch(routeId);
  }
}

/**
 * Wave 4B: pull every `WorkspaceSummary` out of a daemon workspace push
 * so the re-home watcher can see `running_on_host_id` changes regardless
 * of which message variant carried them. Covers the three variants that
 * ship summaries: the full `HostWorkspaceTree`, the per-host
 * `HostWorkspaceList`, and the single-workspace `WorkspaceControlChanged`
 * (which the daemon fans out on a `MoveWorkspaceToHost`). Returns `[]`
 * for every other variant.
 */
function workspaceSummariesFromMessage(
  payload: WorkspaceServerMessage,
): WorkspaceSummary[] {
  if ("HostWorkspaceTree" in payload) {
    return payload.HostWorkspaceTree.workspaces;
  }
  if ("HostWorkspaceList" in payload) {
    return payload.HostWorkspaceList.workspaces;
  }
  if ("WorkspaceControlChanged" in payload) {
    return [payload.WorkspaceControlChanged.workspace];
  }
  return [];
}

/** Hosts carried by a workspace-tree message. Only `HostWorkspaceTree`
 *  ships the host list; we read each host's advertised `daemon_url`
 *  (Wave 4E) to resolve `running_on_host_id` -> dialable URL for re-dial.
 *  Typed structurally so this needs no extra import. */
function hostsFromMessage(
  payload: WorkspaceServerMessage,
): ReadonlyArray<{ id: string; daemon_url?: string | null }> {
  if ("HostWorkspaceTree" in payload) {
    return payload.HostWorkspaceTree.hosts;
  }
  return [];
}

/** `host:port` of a ws(s) daemon URL, lowercased for comparison, or
 *  `null` when absent/unparseable. Used to decide which tree host is
 *  "local" (= the daemon this client is currently dialed into). */
function wsUrlHost(url: string | null): string | null {
  if (!url) return null;
  try {
    const parsed = new URL(url);
    return parsed.host ? parsed.host.toLowerCase() : null;
  } catch {
    return null;
  }
}

/** Best-effort short label for a daemon URL. Strips the path and
 *  protocol so the switcher row reads `host:port` instead of the full
 *  `ws://host:port/session`. Falls back to the raw URL if parsing
 *  fails. */
function friendlyLabelFromUrl(url: string): string {
  try {
    const parsed = new URL(url);
    return parsed.host || url;
  } catch {
    return url;
  }
}
