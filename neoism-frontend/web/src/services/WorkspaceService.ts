// JS-side bridge for `neoism-protocol::workspace` — workspace
// identity, project registry, per-session cwd, and the active-session
// pointer.
//
// Workspace messages are push-style on the wire: every variant
// includes its own routing key (workspace id or session id), so there
// is no per-request id. The service exposes both fire-and-forget
// senders (the common case — "open this workspace", "list sessions")
// and an event subscription so the UI can react to
// `ProjectRootChanged` / `SessionCreated` / `ProjectRootList` pushes.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type {
  ClipboardPayload,
  EditorSurfaceSummary,
  WorkspaceSummary,
  HostSummary,
  PaneFocusDir,
  PaneLayoutSnapshot,
  PaneSplitAxis,
  PaneSplitPlacement,
  SessionSummary,
  WorkspaceAction,
  WorkspaceClientMessage,
  WorkspaceServerMessage,
  WorkspaceTabSummary,
  ProjectRootSummary,
} from "../workspace/types";

export type {
  ClipboardPayload,
  EditorSurfaceSummary,
  WorkspaceSummary,
  HostSummary,
  PaneFocusDir,
  PaneLayoutSnapshot,
  PaneSplitAxis,
  PaneSplitPlacement,
  SessionSummary,
  WorkspaceAction,
  WorkspaceServerMessage,
  WorkspaceTabSummary,
  ProjectRootSummary,
};

/**
 * Listener for daemon-pushed workspace events. The handler gets the
 * full `WorkspaceServerMessage` so it can narrow on the variant it
 * cares about (`ProjectRootList`, `ProjectRootChanged`, `SessionList`,
 * etc.).
 */
export type ProjectRootListener = (msg: WorkspaceServerMessage) => void;

export class WorkspaceService {
  private readonly listeners = new Set<ProjectRootListener>();
  private hosts: HostSummary[] = [];
  private workspaces: WorkspaceSummary[] = [];
  private workspaceTabs: WorkspaceTabSummary[] = [];

  constructor(private readonly client: ProtocolClient) {}

  /** Subscribe to daemon `WorkspaceReply` pushes. */
  subscribe(listener: ProjectRootListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /**
   * Hand a daemon-pushed `WorkspaceServerMessage` to every listener.
   * Wired into `ProtocolClient.onWorkspaceReply`.
   */
  ingestServerMessage(msg: WorkspaceServerMessage): void {
    this.ingestTreeMessage(msg);
    for (const listener of this.listeners) {
      try {
        listener(msg);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[workspace] listener threw", err);
        }
      }
    }
  }

  private ingestTreeMessage(msg: WorkspaceServerMessage): void {
    if ("HostList" in msg) {
      this.hosts = msg.HostList.hosts;
      return;
    }
    if ("HostWorkspaceList" in msg) {
      const byId = new Map(this.workspaces.map((workspace) => [workspace.id, workspace]));
      for (const workspace of msg.HostWorkspaceList.workspaces) {
        byId.set(workspace.id, workspace);
      }
      this.workspaces = Array.from(byId.values());
      return;
    }
    if ("WorkspaceTabList" in msg) {
      const workspaceIds = new Set(msg.WorkspaceTabList.tabs.map((tab) => tab.workspace_id));
      this.workspaceTabs = this.workspaceTabs.filter(
        (tab) => !workspaceIds.has(tab.workspace_id),
      );
      this.workspaceTabs.push(...msg.WorkspaceTabList.tabs);
      return;
    }
    if ("HostWorkspaceTree" in msg) {
      this.hosts = msg.HostWorkspaceTree.hosts;
      this.workspaces = msg.HostWorkspaceTree.workspaces;
      this.workspaceTabs = msg.HostWorkspaceTree.tabs;
      return;
    }
    if ("HostWorkspaceChanged" in msg && msg.HostWorkspaceChanged.workspace_id) {
      this.requestHostWorkspaceTree();
      return;
    }
    if ("InitialWorkspaceResolved" in msg) {
      this.upsertWorkspace(msg.InitialWorkspaceResolved.workspace);
      return;
    }
    if ("HostWorkspaceUpserted" in msg) {
      this.upsertWorkspace(msg.HostWorkspaceUpserted.workspace);
      return;
    }
    if ("WorkspaceControlChanged" in msg) {
      this.upsertWorkspace(msg.WorkspaceControlChanged.workspace);
      return;
    }
    if ("WorkspaceTabMoved" in msg) {
      this.workspaceTabs = this.workspaceTabs.map((tab) =>
        tab.id === msg.WorkspaceTabMoved.tab.id ? msg.WorkspaceTabMoved.tab : tab,
      );
      if (!this.workspaceTabs.some((tab) => tab.id === msg.WorkspaceTabMoved.tab.id)) {
        this.workspaceTabs.push(msg.WorkspaceTabMoved.tab);
      }
    }
  }

  private upsertWorkspace(workspace: WorkspaceSummary): void {
    this.workspaces = this.workspaces.map((candidate) =>
      candidate.id === workspace.id ? workspace : candidate,
    );
    if (!this.workspaces.some((candidate) => candidate.id === workspace.id)) {
      this.workspaces.push(workspace);
    }
  }

  // Fire-and-forget senders --------------------------------------------

  getHostWorkspaceTree(): {
    hosts: HostSummary[];
    workspaces: WorkspaceSummary[];
    tabs: WorkspaceTabSummary[];
  } {
    return {
      hosts: [...this.hosts],
      workspaces: [...this.workspaces],
      tabs: [...this.workspaceTabs],
    };
  }

  openProjectRoot(path: string, initIfMissing = false): void {
    this.send({ OpenProjectRoot: { path, init_if_missing: initIfMissing } });
  }

  closeProjectRoot(id: string): void {
    this.send({ CloseProjectRoot: { id } });
  }

  listProjectRoots(): void {
    this.send("ListProjectRoots");
  }

  switchProjectRoot(id: string): void {
    this.send({ SwitchProjectRoot: { id } });
  }

  getProjectRootInfo(id: string): void {
    this.send({ GetProjectRootInfo: { id } });
  }

  renameProjectRoot(id: string, name: string): void {
    this.send({ RenameProjectRoot: { id, name } });
  }

  forgetProjectRoot(id: string): void {
    this.send({ ForgetProjectRoot: { id } });
  }

  listHosts(): void {
    this.send("ListHosts");
  }

  upsertHost(host: HostSummary): void {
    this.send({ UpsertHost: { host } });
  }

  listHostWorkspaces(hostId: string | null = null): void {
    this.send({ ListHostWorkspaces: { host_id: hostId } });
  }

  listWorkspaceTabs(workspaceId: string): void {
    this.send({ ListWorkspaceTabs: { workspace_id: workspaceId } });
  }

  requestHostWorkspaceTree(): void {
    this.send("RequestHostWorkspaceTree");
  }

  resolveInitialWorkspace(preferredHostId: string | null = null): void {
    this.send({
      ResolveInitialWorkspace: {
        preferred_host_id: preferredHostId,
      },
    });
  }

  /** Replace one workspace's tab list in the daemon tree — how the
   *  web (a controller with no host entry to snapshot) records its
   *  open tabs so desktop can adopt this workspace with its buffers
   *  and live sessions intact. */
  publishWorkspaceTabs(workspaceId: string, tabs: WorkspaceTabSummary[]): void {
    this.send({ PublishWorkspaceTabs: { workspace_id: workspaceId, tabs } });
  }

  createHostWorkspace(
    hostId: string,
    title: string | null = null,
    rootDir: string | null = null,
    workspaceId: string | null = null,
  ): void {
    this.send({
      CreateHostWorkspace: {
        host_id: hostId,
        workspace_id: workspaceId,
        title,
        root_dir: rootDir,
      },
    });
  }

  createWorkspace(
    title: string | null = null,
    rootDir: string | null = null,
    workspaceId: string | null = null,
  ): void {
    this.send({
      CreateWorkspace: {
        workspace_id: workspaceId,
        title,
        root_dir: rootDir,
      },
    });
  }

  closeHostWorkspace(workspaceId: string): void {
    this.send({ CloseHostWorkspace: { workspace_id: workspaceId } });
  }

  switchHostWorkspace(workspaceId: string): void {
    this.send({ SwitchHostWorkspace: { workspace_id: workspaceId } });
  }

  /** Re-point an existing workspace's directory (the explicit `cd`).
   *  The daemon resolves and validates it, then broadcasts a tree change
   *  so every client in the workspace re-roots its Explorer. */
  setWorkspaceRoot(workspaceId: string, rootDir: string): void {
    this.send({
      SetWorkspaceRoot: { workspace_id: workspaceId, root_dir: rootDir },
    });
  }

  shareWorkspace(workspaceId: string): void {
    this.send({ ShareWorkspace: { workspace_id: workspaceId } });
  }

  stopSharingWorkspace(workspaceId: string): void {
    this.send({ StopSharingWorkspace: { workspace_id: workspaceId } });
  }

  sendWorkspaceToDockerSandbox(workspaceId: string): void {
    this.send({ SendWorkspaceToDockerSandbox: { workspace_id: workspaceId } });
  }

  sendWorkspaceToCloud(workspaceId: string): void {
    this.send({ SendWorkspaceToCloud: { workspace_id: workspaceId } });
  }

  subscribeWorkspace(workspaceId: string): void {
    this.send({ SubscribeWorkspace: { workspace_id: workspaceId } });
  }

  unsubscribeWorkspace(workspaceId: string): void {
    this.send({ UnsubscribeWorkspace: { workspace_id: workspaceId } });
  }

  controlWorkspace(workspaceId: string, controllerHostId: string): void {
    this.send({
      ControlWorkspace: {
        workspace_id: workspaceId,
        controller_host_id: controllerHostId,
      },
    });
  }

  releaseWorkspaceControl(workspaceId: string, controllerHostId: string): void {
    this.send({
      ReleaseWorkspaceControl: {
        workspace_id: workspaceId,
        controller_host_id: controllerHostId,
      },
    });
  }

  moveWorkspaceToHost(workspaceId: string, targetHostId: string): void {
    this.send({
      MoveWorkspaceToHost: {
        workspace_id: workspaceId,
        target_host_id: targetHostId,
      },
    });
  }

  moveTabToWorkspace(tabId: string, targetWorkspaceId: string): void {
    this.send({
      MoveTabToWorkspace: {
        tab_id: tabId,
        target_workspace_id: targetWorkspaceId,
      },
    });
  }

  moveTabToHostWorkspace(
    tabId: string,
    targetHostId: string,
    targetWorkspaceId: string,
  ): void {
    this.send({
      MoveTabToHostWorkspace: {
        tab_id: tabId,
        target_host_id: targetHostId,
        target_workspace_id: targetWorkspaceId,
      },
    });
  }

  listSessions(): void {
    this.send("ListSessions");
  }

  requestFullSnapshot(): void {
    this.send({ RequestFullSnapshot: {} });
  }

  switchSession(sessionId: string): void {
    this.send({ SwitchSession: { session_id: sessionId } });
  }

  newSession(cwd: string | null = null, label: string | null = null): void {
    this.send({ NewSession: { cwd, label } });
  }

  closeSession(sessionId: string): void {
    this.send({ CloseSession: { session_id: sessionId } });
  }

  getSessionState(sessionId: string): void {
    this.send({ GetSessionState: { session_id: sessionId } });
  }

  setCwd(sessionId: string, path: string): void {
    this.send({ SetCwd: { session_id: sessionId, path } });
  }

  renameSession(sessionId: string, label: string): void {
    this.send({ RenameSession: { session_id: sessionId, label } });
  }

  bindEditorSurface(
    surfaceId: string,
    sessionId: string,
    path: string | null = null,
  ): void {
    this.send({
      BindEditorSurface: {
        surface_id: surfaceId,
        session_id: sessionId,
        path,
      },
    });
  }

  listEditorSurfaces(): void {
    this.send("ListEditorSurfaces");
  }

  closeEditorSurface(surfaceId: string): void {
    this.send({ CloseEditorSurface: { surface_id: surfaceId } });
  }

  runWorkspaceAction(action: WorkspaceAction): void {
    this.send({ RunWorkspaceAction: { action } });
  }

  storeClipboard(payload: ClipboardPayload): void {
    this.send({ StoreClipboard: { payload } });
  }

  loadClipboard(): void {
    this.send("LoadClipboard");
  }

  /**
   * Build the daemon HTTP URL that serves a previously-materialised
   * clipboard image. `filename` should be the basename the daemon
   * returned in `ClipboardImageMaterialized.path` (e.g.
   * `paste-<uuid>.png`). Returns `null` if the protocol client can't
   * derive an HTTP base — in that case callers should fall back to
   * the daemon-side filesystem path (which only works when the
   * daemon and the frontend share a filesystem, i.e. the desktop
   * shell-out case).
   *
   * Used by the web frontend to open clipboard images in a fresh
   * browser tab as a fallback for "I pasted into a non-editor pane"
   * — see `TerminalPanel.ingestClipboardImageMaterialized`.
   */
  getClipboardImageUrl(filename: string): string | null {
    const base = this.client.getDaemonHttpBase();
    if (!base) return null;
    return `${base}/clipboard-image/${encodeURIComponent(filename)}`;
  }

  // Phone-control helpers ----------------------------------------------
  //
  // Sugar around `BindEditorSurface` for the "leave laptop open, drive
  // from phone" use case. The daemon broadcasts the resulting
  // `EditorSurfaceChanged` back to every connected client, so the
  // laptop's `TerminalPanel.ingestEditorSurfaceChanged` materialises
  // the matching pane (via the `ensure_external` session-layout
  // policy) if it doesn't have one already.

  /// Bind `path` to the editor surface identified by `paneExternalId`,
  /// creating the surface server-side if it doesn't exist yet. The
  /// pane comes from the local session-layout policy and is exposed
  /// via the integer leaf id used by the chrome's pane overlay.
  openFileInPane(
    paneExternalId: number,
    sessionId: string,
    path: string,
  ): void {
    this.bindEditorSurface(String(paneExternalId), sessionId, path);
  }

  /// Drop the surface binding for a pane (mirror of
  /// `closeEditorSurface` with the integer external_id phone clients
  /// already speak). The local pane stays alive — close it via the
  /// session-layout policy if needed.
  closePaneBinding(paneExternalId: number): void {
    this.closeEditorSurface(String(paneExternalId));
  }

  // Remote pane-layout control ----------------------------------------
  //
  // Sugar around the `PaneLayoutOp` client message — the phone-control
  // surface the daemon uses to drive `SessionLayout` mutations on
  // paired laptop chromes. Every helper resolves to a single
  // `WorkspaceClientMessage::PaneLayoutOp` envelope; the daemon
  // validates the pane and broadcasts a sibling
  // `PaneLayoutChanged` reply that every connected client materialises.

  /// Split the pane identified by `paneExternalId` along `axis`,
  /// placing the new pane according to `placement`.
  remoteSplitPane(
    paneExternalId: number,
    axis: PaneSplitAxis,
    placement: PaneSplitPlacement,
  ): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: { Split: { axis, placement } },
      },
    });
  }

  /// Move keyboard focus from `paneExternalId` in `dir`.
  remoteFocusPane(paneExternalId: number, dir: PaneFocusDir): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: { Focus: { dir } },
      },
    });
  }

  /// Close the pane identified by `paneExternalId`.
  remoteClosePane(paneExternalId: number): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: "Close",
      },
    });
  }

  /// Nudge the split ratio between `paneExternalId` and its neighbour
  /// by `delta` (range `-0.5..=0.5`).
  remoteResizePane(paneExternalId: number, delta: number): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: { ResizeRatio: { delta } },
      },
    });
  }

  /// Move a tab inside `paneExternalId` from index `from` to `to`.
  remoteMoveTab(paneExternalId: number, from: number, to: number): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: { MoveTab: { from, to } },
      },
    });
  }

  /// Move a pane/surface to an explicit edge of another pane.
  remoteMovePane(
    paneExternalId: number,
    targetPaneExternalId: number,
    axis: PaneSplitAxis,
    placement: PaneSplitPlacement,
  ): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: {
          MovePane: {
            target_pane_external_id: targetPaneExternalId,
            axis,
            placement,
          },
        },
      },
    });
  }

  /// Center-drop a pane/surface into another pane's tab group.
  remoteAdoptPaneAsTab(paneExternalId: number, targetPaneExternalId: number): void {
    this.send({
      PaneLayoutOp: {
        pane_external_id: paneExternalId,
        op: { AdoptPaneAsTab: { target_pane_external_id: targetPaneExternalId } },
      },
    });
  }

  /// Publish a host-materialized recursive split/tab tree verbatim. This is
  /// the exact-tree counterpart to the smaller remote mutation helpers.
  publishPaneLayout(layout: PaneLayoutSnapshot): void {
    this.send({ PublishPaneLayout: { layout } });
  }

  // ------------------------------------------------------------------

  private send(message: WorkspaceClientMessage): void {
    this.client.sendWorkspace(message);
  }
}

// TODO(W4-pushes): the daemon does not yet ship dedicated push surfaces
// for breadcrumbs (`bridge.setBreadcrumbs`), minimap snapshots
// (`bridge.setMinimap`), completion menus (`bridge.setCompletionMenu`),
// global notifications (`bridge.pushNotification`), or the dedicated
// git branch pill (`bridge.setGitBranchPill`). The bridge already
// exposes optional accessors for these (added in this wave; see
// `ChromeBridgeInstance` in `createTerminal.ts`); once the daemon
// emits matching `WorkspaceServerMessage` / `EditorServerMessage`
// variants, wire them through here / `DiagnosticsService` so the
// chrome panels paint without waiting on a follow-up push.
//
