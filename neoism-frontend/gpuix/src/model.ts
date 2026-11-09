import {HistoryUpdates} from "./agent/history-updates";
import {sessionErrorMessage} from "./agent/session-error";
import {mergeHistoryHead} from "./agent/merge-history";
import {stepHistoryEdit,type HistoryEdit,type EditableHistory} from "./agent/history-edit";
import { recoveredEditorState } from "./editor/recovery-state";
import { profileStart, profileEnd } from "./profiling";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "./animation/frame-scheduler";
import {DocumentController} from "./collaboration/DocumentController";
import {PresenceController} from "./collaboration/PresenceController";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import type { AppConfig } from "./config";
import { DaemonConnection } from "./daemon/client";
import {recoveredStartupDirectory} from "./daemon/startup-directory";
import { NativeEditor, coreCall, type EditorSnapshot } from "./native-core";
import { NativeMarkdown } from "./markdown/NativeMarkdown";
import type { SavedSession } from "./session-store";
import { WorkspaceLayout, type LayoutSnapshot, type SplitAxis, type FocusDirection } from "./layout/WorkspaceLayout";
import { relative, isAbsolute, posix, win32 } from "node:path";
import {hostPaths} from "./workspaces/host-paths";
import { closeTerminalSession, disposeTerminalSessions,onTerminalUnavailable,terminalSession } from "./terminal/sessions";
import { absoluteTerminalDirectory,mainTerminalSurface,followsTerminalDirectory,terminalDirectoryFollowsWorkspace } from "./terminal/workspace";
import {getWindowWorkspaces,includeWindowWorkspace,openWorkspaceIds,reorderWindowWorkspace} from "./workspaces/window-tabs";
import {localWorkspaceSession} from "./workspaces/window-sessions";
import { AgentSdk, sdkModel, sdkPrompt,sdkThread } from "./daemon/sdk";
import {localPresenceIdentity} from "./agent/local-presence";
import {catalogThread} from "./agent/catalog-session";
import {historyPageCursor} from "./agent/history-cursor";
import {catalogAgentMessage} from "./agent/catalog-event";
import {FamilyStreamOwnership,CompletedTranscriptRefresh} from "./agent/transcript-sync";
import {agentFamily,agentFamilyRoot,noteAgentFamily} from "./agent/family";
import type { GitReviewDescriptor } from "./git/GitReviewController";
import { DatabaseController } from "./database/DatabaseController";
import type { DatabaseConnection } from "./database/types";
import { isDatabasePath } from "./database/file-path";
import { workspaceSidebarNavigation, type WorkspaceSidebar } from "./ui/workspace-sidebar";
import { DirectorySchema, FileSchema, HistoryMessageSchema, ThreadSchema, TreeSchema, WorkspaceSchema,
  UsageSchema, TodoSchema, ProviderSchema, AgentSchema, SkillSchema, ProviderStateSchema, expectVariant, isRecord, variant,
  type ConnectionState, type Directory, type HistoryMessage, type Thread, type Todo, type Usage,
  type Provider, type ProviderState, type AgentInfo, type SkillInfo, type Attachment,
  type WireEvent, type Workspace, type WorkspaceTab, type AgentClientMessage } from "./daemon/protocol";

export type Surface = {
  id: string; title: string; workspaceId: string;
  kind: "file" | "agent" | "terminal" | "page" | "welcome" | "git";
  page?: "extensions" | "database"; databaseTable?: string; document?: { kind: "config"; revision: string; writable: boolean };
  path?: string; rootDir?: string; sessionId?: string; agentRootSessionId?: string;
  /** Home keeps its PTY in sessionId; Agent mode owns a separate conversation. */
  homeMode?: "terminal" | "agent"; homeAgentSessionId?: string;
  git?: GitReviewDescriptor; gitReveal?: number; gitEditorIds?: string[];
  /** Canonical source owner for inline Git editing, without a second visible tab. */
  hidden?: boolean;
};
export function agentSurface(surface:Surface):Surface;
export function agentSurface(surface:Surface|undefined):Surface|undefined;
export function agentSurface(surface:Surface|undefined):Surface|undefined {
  return surface?.kind==="welcome"?{...surface,kind:"agent",sessionId:surface.homeAgentSessionId}:surface;
}
export interface PendingQuestion { sessionId: string; requestId: string; request: Record<string, unknown>; responding: boolean; error?: string }
export interface PromptOptions { attachments?: readonly Attachment[]; delivery?: "steer" | "queue"; selection?: ProviderState; messageId?:string }
export interface Permission { sessionId: string; requestId: string; tool: string; title: string; responding?: boolean; error?: string }
export interface ComposerDraft { text: string; attachments: Array<Attachment & { token: string; name: string }> }
export interface SessionGoalView { text: string; paused: boolean; status: "active" | "complete" | "blocked"; summary: string }
export interface AppState {
  ui: Record<string, unknown>;
  composerDrafts: Readonly<Record<string, ComposerDraft>>;
  connection: ConnectionState;
  workspaces: readonly Workspace[];
  workspaceTabs: readonly WorkspaceTab[];
  activeWorkspace?: Workspace;
  surfaces: readonly Surface[];
  activeSurfaceId?: string;
  directories: Readonly<Record<string, Directory>>;
  expanded: ReadonlySet<string>;
  notesDirectories: Readonly<Record<string, Directory>>;
  notesExpanded: ReadonlySet<string>;
  files: Readonly<Record<string, string>>;
  threads: readonly Thread[];
  history: Readonly<Record<string, readonly HistoryMessage[]>>;
  pendingAgentSessions: Readonly<Record<string,string>>;
  usage: Readonly<Record<string, Usage>>;
  todos: Readonly<Record<string, readonly Todo[]>>;
  goals: Readonly<Record<string, SessionGoalView | null>>;
  streaming: Readonly<Record<string, string>>;
  sessionErrors: Readonly<Record<string, string | undefined>>;
  permissions: readonly Permission[];
  questions: readonly PendingQuestion[];
  providers: readonly Provider[];
  /** Raw `/v2/providers/configured` reply (connected providers only), for the desktop model projection. */
  providerCatalog?: unknown;
  agents: readonly AgentInfo[];
  skills: readonly SkillInfo[];
  /** Fresh Agent defaults, scoped to workspace and directory; user drafts override them. */
  agentDefaults?: Readonly<Record<string,ProviderState>>;
  agentDefaultsLoading?: boolean;
  providerStates: Readonly<Record<string, ProviderState>>;
  drafts: Readonly<Record<string, ProviderState>>;
  queues: Readonly<Record<string, { count: number; preview?: string | null }>>;
  catalogsLoading: boolean;
  catalogsError?: string;
  catalogRetryAfterMs?: number;
  loading: boolean;
  creatingWorkspace?: boolean;
  error?: string;
}
type WorkspaceNavigation = Pick<AppState, "directories" | "expanded" | "notesDirectories" | "notesExpanded" | "threads">;
export class WorkspaceModel {
  readonly collaboration:DocumentController;
  readonly presence:PresenceController;
  private listeners = new Set<() => void>();
  private surfaceClosedListeners = new Set<(surface:Surface)=>void>();
  private workspaceClosedListeners = new Set<(workspaceId:string)=>void>();
  private closingWorkspaces = new Set<string>();
  private workspaceCloseEpochs = new Map<string,number>();
  private closedTranscriptRoots = new Set<string>();
  private cleanups: Array<() => void> = [];
  private abort = new AbortController();
  private epoch = 0;
  private disposed = false;
  private seededWorkspaces = new Set<string>();
  private startupWelcomePending = true;
  private workspaceNavigation = new Map<string, WorkspaceNavigation>();
  private terminalNotesVaults = new Map<string, Pick<Workspace, "notes_vault_dir" | "linked_vault_dir">>();
  private fileLoads = new Map<string, Promise<void>>();
  private directoryVersions = new Map<string,number>();
  private directoryLoads = new Map<string, Promise<void>>();
  private editorSnapshots = new WeakMap<NativeEditor, { version: number; frame: NativeEditor["frame"]; snapshot: ReturnType<NativeEditor["snapshot"]> }>();
  private updateDepth = 0;
  private updatePending = false;
  readonly editors: Map<string, NativeEditor>;
  private readonly databases = new Map<string, DatabaseController>();
  private readonly databaseSchemas = new Map<string, DatabaseController>();
  readonly agentSdk: AgentSdk;
  readonly layouts = new Map<string, WorkspaceLayout>();
  private editorViews = new Map<string, { surfaceId: string; editor: NativeEditor }>();
  private recoveredViews: Record<string, unknown> = {};
  private markdownViews = new Map<string, { surfaceId: string; markdown: NativeMarkdown }>();
  private recoveredMarkdownViews: Record<string, unknown> = {};
  private observedEditors = new WeakSet<NativeEditor>();
  private syncingDocuments = new Set<string>();
  private sessionWorkspaces = new Map<string, string>();
  private agentTitles=new Map<string,string>();
  private selectionTasks = new Map<string, Promise<void>>();
  private workspaceCreations: Promise<unknown> = Promise.resolve();
  private pendingWorkspaceCreations = new Set<string>();
  // Only this window's never-used creations are eligible for automatic pruning.
  private disposableWorkspaces = new Set<string>();
  private workspaceDirectoryChanges = new Set<string>();
  state: AppState = {
    ui: {}, composerDrafts: {}, connection: { phase: "closed", attempt: 0 }, workspaces: [], workspaceTabs: [],
    surfaces: [], notesDirectories: {}, notesExpanded: new Set([""]), directories: {}, expanded: new Set([""]), files: {}, threads: [],
    history: {}, pendingAgentSessions:{}, usage: {}, todos: {}, goals: {}, streaming: {}, sessionErrors: {}, permissions: [], questions: [], providers: [], agents: [], skills: [], providerStates: {}, drafts: {}, queues: {}, catalogsLoading: false, loading: false,
  };
  constructor(readonly client: DaemonConnection, readonly config: AppConfig, seed?: { state: AppState; editors: Map<string, NativeEditor>; recovery?: SavedSession }) {
    this.agentSdk = new AgentSdk(config);
    this.presence = new PresenceController(client,config);
    this.collaboration=new DocumentController(client,config,this.presence,this.report);
    this.cleanups.push(() => { for (const database of [...this.databases.values(), ...this.databaseSchemas.values()]) database.dispose(); this.databases.clear(); this.databaseSchemas.clear(); });
    this.editors = seed?.editors || new Map();
    if (seed) {
      // Hot remounts retain the current view; a cold recovery opens Welcome.
      this.startupWelcomePending = false;
      this.state = { ...this.state, ...seed.state, surfaces:seed.state.surfaces.map(surface=>surface.kind==="welcome"?{...surface,title:"Home"}:surface), ui:this.withoutTerminalSplash(seed.state.ui), connection: { phase: "closed", attempt: 0 } };
      if (seed.recovery) this.recoveredViews = seed.recovery.editors;
      const markdownViews = seed.recovery?.ui.markdownViews ?? this.state.ui.markdownViews;
      if (isRecord(markdownViews)) this.recoveredMarkdownViews = markdownViews;
      if (this.state.activeWorkspace) this.seededWorkspaces.add(this.state.activeWorkspace.id);
    }
  }
  setVimMode = (enabled: boolean) => {
    this.config.vim = enabled;
    // Hidden restored views have not allocated an editor yet. Use the engine's
    // toggle transition so recovery keeps its undo history and valid caret.
    for (const [key, snapshot] of Object.entries(this.recoveredViews)) {
      const editor = new NativeEditor("", "", enabled);
      try {
        editor.call("restore", { snapshot });
        if (editor.frame.vim !== enabled) editor.call("toggle-vim", { enabled });
        this.recoveredViews[key] = editor.snapshot();
      } catch {} finally { editor.dispose(); }
    }
    // This is the native Markdown recovery schema. Mutate the saved object so
    // an already created rich view waiting for its first layout sees it too.
    for (const snapshot of Object.values(this.recoveredMarkdownViews)) {
      if (isRecord(snapshot) && snapshot.version === 1 && snapshot.vim !== enabled)
        Object.assign(snapshot, { vim: enabled, mode: enabled ? "Normal" : "Insert" });
    }
    const editors = new Set([...this.editors.values(), ...[...this.editorViews.values()].map((view) => view.editor)]);
    for (const editor of editors) if (editor.frame.vim !== enabled) editor.call("toggle-vim", { enabled });
  };
  private withoutTerminalSplash(ui:Record<string,unknown>){const{terminalSplash,...current}=ui;return current;}
  setUI = (patch: Record<string, unknown>) => {
    if (Object.entries(patch).every(([key, value]) => JSON.stringify(this.state.ui[key]) === JSON.stringify(value))) return;
    this.update({ ui: { ...this.state.ui, ...patch } });
  };
  composerDraftKey = (sessionId?: string,surfaceId?:string) => {
    if(sessionId)return sessionId;
    const surface=surfaceId?this.state.surfaces.find(surface=>surface.id===surfaceId):undefined;
    return surface?.kind==="welcome"||surface?.kind==="agent"&&!surface.sessionId&&surface.id!==`agent-home:${surface.workspaceId}`?`home:${surface.id}`:`home:${surface?.workspaceId||this.state.activeWorkspace?.id||""}`;
  };
  setHomeMode = (surfaceId:string,mode:"terminal"|"agent") => {
    const surface=this.state.surfaces.find(surface=>surface.id===surfaceId&&surface.kind==="welcome");
    if(!surface||surface.homeMode===mode)return;
    this.update({surfaces:this.state.surfaces.map(current=>current.id===surfaceId?{...current,homeMode:mode}:current)});
    if(mode==="agent"&&surface.homeAgentSessionId)this.resumeThread(surface.homeAgentSessionId);
  };
  readonly composerHistory:ComposerDraft[]=[];
  rememberComposerPrompt(draft:ComposerDraft){
    this.composerHistory.push(draft);if(this.composerHistory.length>100)this.composerHistory.shift();
  }
  setComposerDraft = (key: string, draft: ComposerDraft) => {
    if(draft.text||draft.attachments.length)for(const id of this.disposableWorkspaces)if(key.includes(id))this.disposableWorkspaces.delete(id);
    if (JSON.stringify(this.state.composerDrafts[key]) === JSON.stringify(draft)) return;
    this.update({ composerDrafts: { ...this.state.composerDrafts, [key]: draft } });
  };
  private recoveryFrame(editor: NativeEditor) {
    const saved=this.editorSnapshots.get(editor),frame=editor.frame;
    // Scroll springs do not change recovery-owned document/history state.
    // Keep the last checkpoint during motion; capture the final viewport on
    // settling. Keys, edits and remote revisions always bypass this reuse.
    if(saved&&saved.version===editor.recoveryVersion&&saved.frame.revision===frame.revision&&saved.frame.dirty===frame.dirty
      && saved.frame.mode===frame.mode && saved.frame.pendingKeys===frame.pendingKeys && saved.frame.collaborationBound===frame.collaborationBound
      && (frame.scrollAnimating || saved.frame.scrollY===frame.scrollY
        && saved.frame.cursor.line===frame.cursor.line && saved.frame.cursor.col===frame.cursor.col
        && JSON.stringify(saved.frame.selection)===JSON.stringify(frame.selection)))return saved.frame;
    return frame;
  }
  private snapshotEditor(editor: NativeEditor, force = false) {
    let saved = this.editorSnapshots.get(editor);
    if (!saved || saved.frame !== (force ? editor.frame : this.recoveryFrame(editor)) || saved.version !== editor.recoveryVersion) {
      saved = { version: editor.recoveryVersion, frame: editor.frame, snapshot: editor.snapshot() }; this.editorSnapshots.set(editor, saved);
    }
    return saved.snapshot;
  }
  /** Identities of recovery-owned state. Chat paint/scroll frames do not alter
   * these, so a periodic checkpoint need not serialize every inactive editor. */
  sessionDependencies(): unknown[] {
    return [this.config.vim, this.state.activeWorkspace?.id, this.state.activeSurfaceId,
      this.state.surfaces, this.state.ui, this.state.providerStates, this.state.drafts, this.state.composerDrafts,
      ...Object.entries(this.recoveredViews).flat(), ...Object.entries(this.recoveredMarkdownViews).flat(),
      ...[...this.agentFamilyWatermarks].flat(),
      ...[...this.editors].flatMap(([key, editor]) => [key, editor.recoveryVersion, this.recoveryFrame(editor)]),
      ...[...this.editorViews].flatMap(([key, view]) => [key, view.editor.recoveryVersion, this.recoveryFrame(view.editor)]),
      ...[...this.markdownViews].flatMap(([key, view]) => [key, view.markdown.frame])];
  }
  captureSession(force = true): SavedSession {
    return { version: 1, activeWorkspaceId: this.state.activeWorkspace?.id, activeSurfaceId: this.state.activeSurfaceId,
      surfaces: [...this.state.surfaces], ui: { ...this.state.ui,agentFamilyVersions:{...(isRecord(this.state.ui.agentFamilyVersions)?this.state.ui.agentFamilyVersions:{}),...Object.fromEntries(this.agentFamilyWatermarks)}, markdownViews: { ...this.recoveredMarkdownViews, ...Object.fromEntries([...this.markdownViews].map(([key, view]) => [key, view.markdown.snapshot()])) } }, providerStates: this.state.providerStates, drafts: this.state.drafts,
      composerDrafts: { ...this.state.composerDrafts }, editors: { ...this.recoveredViews, ...Object.fromEntries([...this.editors].map(([id, editor]) => [id, this.snapshotEditor(editor,force)]).concat([...this.editorViews].map(([id, view]) => ["view:" + id, this.snapshotEditor(view.editor,force)]))) } };
  }
  restoreSession(session: SavedSession) {
    this.agentFamilyWatermarks.clear();
    this.startupWelcomePending = true;
    this.recoveredViews = { ...session.editors };
    this.recoveredMarkdownViews = isRecord(session.ui.markdownViews) ? session.ui.markdownViews : {};
    const files: Record<string, string> = {};
    const providers: Record<string, ProviderState> = {}, drafts: Record<string, ProviderState> = {};
    for (const [id, raw] of Object.entries(session.providerStates)) try { providers[id] = Schema.decodeUnknownSync(ProviderStateSchema)(raw); } catch {}
    for (const [id, raw] of Object.entries(session.drafts)) try { drafts[id] = Schema.decodeUnknownSync(ProviderStateSchema)(raw); } catch {}
    const composerDrafts: Record<string, ComposerDraft> = {};
    for (const [key, draft] of Object.entries(session.composerDrafts)) {
      const attachments = (draft.attachments || []).filter((v): v is ComposerDraft["attachments"][number] => isRecord(v)
        && typeof v.kind === "string" && typeof v.path === "string" && typeof v.token === "string" && typeof v.name === "string"
        && Array.isArray(v.bytes) && v.bytes.every((b) => Number.isInteger(b) && b >= 0 && b <= 255));
      composerDrafts[key] = { text: draft.text, attachments };
    }
    // Retain recovery data without constructing and painting every hidden tab.
    // The first getEditor restores native state; dirty guards use its saved metadata.
    for (const surface of session.surfaces) {
      if (surface.kind !== "file" || !session.editors[surface.id]) continue;
      const recovered = recoveredEditorState(session.editors[surface.id]);
      if (recovered) files[surface.id] = recovered.text;
    }
    this.restoredWorkspaceId = session.activeWorkspaceId;
    this.update({ surfaces: session.surfaces.map(surface=>surface.kind==="welcome"?{...surface,title:"Home"}:surface), activeSurfaceId: session.activeSurfaceId, files, providerStates: providers, drafts, composerDrafts, ui: this.withoutTerminalSplash(session.ui) });
  }
  private restoredWorkspaceId?: string;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  update(patch: Partial<AppState>) {
    if (this.disposed || Object.entries(patch).every(([key, value]) => this.state[key as keyof AppState] === value)) return;
    this.state = { ...this.state, ...patch };
    if (this.updateDepth) this.updatePending = true;
    else for (const listener of this.listeners) listener();
  }
  private transaction<T>(work: () => T): T {
    this.updateDepth++;
    try { return work(); }
    finally {
      if (--this.updateDepth === 0 && this.updatePending) {
        this.updatePending = false; for (const listener of this.listeners) listener();
      }
    }
  }
  start() {
    this.cleanups.push(onTerminalUnavailable(this.client,this.retireTerminal));
    this.cleanups.push(this.client.subscribe(this.enqueueReceive));
    this.cleanups.push(this.client.subscribeStatus((connection) => {
      this.update({ connection });
      if (connection.phase === "connected") for (const database of [...this.databases.values(), ...this.databaseSchemas.values()]) void database.initialize();
      if (connection.phase === "connected") void this.hydrate();
      else {this.presence.leaveWorkspace();this.epoch++;}
    }));
  }
  onDispose(cleanup:()=>void){this.cleanups.push(cleanup);}
  onSurfaceClosed(listener:(surface:Surface)=>void){this.surfaceClosedListeners.add(listener);return()=>{this.surfaceClosedListeners.delete(listener);};}
  onWorkspaceClosed(listener:(workspaceId:string)=>void){this.workspaceClosedListeners.add(listener);return()=>{this.workspaceClosedListeners.delete(listener);};}
  dispose(preserveEditors = false) { this.historyUpdates.clear(); this.collaboration.dispose(); this.presence.dispose(); this.historyRequests.clear();this.queuedHistory.clear();this.historyCursors.clear(); clearTimeout(this.completedHistoryTimer);this.completedHistory.clear();this.agentCatalogStream?.abort.abort();this.agentCatalogStream=undefined; this.incomingAgentFrames.dispose(); clearTimeout(this.incomingAgentTimer);this.incomingAgentTimer=undefined;this.incomingAgentEvents=[];this.incomingAgentHead=0; for (const view of this.markdownViews.values()) view.markdown.dispose(); this.markdownViews.clear(); disposeTerminalSessions(this.client); this.disposed = true; this.epoch++; this.abort.abort(); this.agentSdk.dispose(); this.cleanups.forEach((cleanup) => cleanup()); this.listeners.clear(); for (const layout of this.layouts.values()) layout.dispose(); this.layouts.clear(); for (const view of this.editorViews.values()) view.editor.dispose(); this.editorViews.clear(); if (!preserveEditors) { for (const editor of this.editors.values()) editor.dispose(); this.editors.clear(); } }
  getLayout(workspaceId = this.state.activeWorkspace?.id): WorkspaceLayout {
    if (!workspaceId) throw new Error("Select a workspace first");
    let layout = this.layouts.get(workspaceId);
    if (layout) return layout;
    const stored = isRecord(this.state.ui.layouts) ? this.state.ui.layouts[workspaceId] : undefined;
    try { layout = new WorkspaceLayout(stored as LayoutSnapshot | undefined); } catch { layout = new WorkspaceLayout(); }
    this.layouts.set(workspaceId, layout);
    const workspace = this.state.workspaces.find((w) => w.id === workspaceId) || this.state.activeWorkspace;
    if (!stored && workspace?.layout_snapshot) {
      const surfaces: Surface[] = [];
      try { layout.restoreDaemon(workspace.layout_snapshot, (_externalId, path, leaf) => {
        const tab = this.state.workspaceTabs.find((tab) => tab.workspace_id === workspaceId && (tab.surface_id === leaf.surface_id || tab.id === leaf.surface_id || tab.session_id === leaf.session_id));
        // Desktop terminal snapshots also carry their cwd in `path`. A cwd is
        // not an editor document; the tab kind disambiguates that wire field.
        const terminal = tab?.kind === "terminal" || path?.replace(/[\\/]$/, "") === workspace.root_dir?.replace(/[\\/]$/, "");
        const sessionId = tab?.session_id || leaf.session_id;
        if(terminal&&sessionId&&isRecord(this.state.ui.unavailableTerminals)&&this.state.ui.unavailableTerminals[sessionId])return;
        const existing = this.state.surfaces.find((s) => s.workspaceId === workspaceId && (path && !terminal ? s.path === path : s.sessionId === sessionId));
        if (existing) return existing;
        let surface: Surface | undefined;
        if (path && !terminal && workspace.root_dir) {
          const scopedPath = isAbsolute(path) ? relative(workspace.root_dir, path) : path;
          if (!scopedPath.startsWith("../")) surface = { id: `file:${workspaceId}:${scopedPath}`, kind: "file", title: scopedPath.split("/").pop() || scopedPath, path: scopedPath, rootDir: workspace.root_dir, workspaceId };
        } else if (sessionId && !sessionId.startsWith("desktop:")) surface = { id: `terminal:${sessionId}`, kind: "terminal", title: tab?.title || "Terminal", sessionId, workspaceId };
        if (surface) surfaces.push(surface);
        return surface;
      }); } catch {}
      if (surfaces.length) this.update({ surfaces: [...this.state.surfaces, ...surfaces.filter((s) => !this.state.surfaces.some((old) => old.id === s.id))] });
    }
    // Layout and surfaces persist separately; a tab whose surface is gone would
    // paint a loading skeleton forever and could not be closed. Drop it.
    const known = new Set(this.state.surfaces.map((s) => s.id));
    for (const leaf of [...layout.frame.leaves]) if (!leaf.surfaceId || !known.has(leaf.surfaceId)) layout.close(leaf.leaf);
    // Older saved routes predate page metadata; recover it from the real surface,
    // not from a table title or a database-looking route id.
    for (const surface of this.state.surfaces.filter(s => s.workspaceId === workspaceId && s.page)) {
      if (layout.frame.leaves.some(leaf => leaf.surfaceId === surface.id && leaf.page !== surface.page)) layout.rename(surface);
    }
    if (!layout.frame.leaves.length) for (const surface of this.state.surfaces.filter((s) => s.workspaceId === workspaceId && !s.hidden)) layout.open(surface);
    // The layout owns this listener and clears it on dispose.
    layout.subscribe((frame) => {
      const layouts = isRecord(this.state.ui.layouts) ? this.state.ui.layouts : {};
      const patch: Partial<AppState> = { ui: { ...this.state.ui, layouts: { ...layouts, [workspaceId]: layout!.snapshot() } } };
      if (this.state.activeWorkspace?.id === workspaceId) patch.activeSurfaceId = frame.focusedSurfaceId;
      const focused = patch.activeSurfaceId && patch.activeSurfaceId !== this.state.activeSurfaceId
        ? agentSurface(this.state.surfaces.find(surface=>surface.id===patch.activeSurfaceId)) : undefined;
      this.update(patch);
      // Layout-driven tab/pane navigation publishes focus before selectSurface.
      // Reconcile warm chats here too, while the actual transition is known.
      if(focused?.kind==="agent"&&focused.sessionId)this.resumeThread(focused.sessionId);
    });
    return layout;
  }
  focusPane = (direction: FocusDirection) => { const frame = this.getLayout().focusDirection(direction); if (frame.focusedSurfaceId) this.selectSurface(frame.focusedSurfaceId, true); };
  splitSurface = (axis: SplitAxis) => { void this.newTerminal(axis); };
  openPage = (page: "extensions") => {
    const workspace = this.state.activeWorkspace; if (!workspace) throw new Error("Select a workspace first");
    this.addSurface({ id: `page:${workspace.id}:${page}`, kind: "page", page, title: "Extensions", workspaceId: workspace.id });
  };
  getDatabase = (surface: Surface): DatabaseController => {
    if (surface.page !== "database" || !surface.path || !surface.rootDir) throw new Error("Missing database path");
    let controller = this.databases.get(surface.id);
    if (!controller) {
      let dirty = false;
      controller = new DatabaseController(this.client, surface.rootDir, surface.path, () => {
        const next = !!controller?.state.pending || !!controller?.state.applying;
        if (next !== dirty) { dirty = next; this.update({ surfaces: [...this.state.surfaces] }); }
      }, surface.databaseTable);
      this.databases.set(surface.id, controller);
    }
    if (this.client.state.phase === "connected") void controller.initialize();
    return controller;
  };
  databaseConnections = (workspaceId = this.state.activeWorkspace?.id): DatabaseConnection[] => {
    if (!workspaceId) return [];
    const stored = isRecord(this.state.ui.databaseConnections) ? this.state.ui.databaseConnections[workspaceId] : undefined;
    const connections = new Map<string, DatabaseConnection>();
    if (Array.isArray(stored)) for (const item of stored) {
      if (isRecord(item) && typeof item.id === "string" && typeof item.title === "string" && typeof item.path === "string" && typeof item.rootDir === "string" && item.workspaceId === workspaceId)
        connections.set(item.id, item as unknown as DatabaseConnection);
    }
    for (const surface of this.state.surfaces) if (surface.page === "database" && surface.workspaceId === workspaceId && surface.path && surface.rootDir) {
      const id = `database:${workspaceId}:${surface.rootDir}:${surface.path}`;
      if (!connections.has(id) && ![...connections.values()].some(connection=>connection.path===surface.path&&connection.rootDir===surface.rootDir)) connections.set(id, { id, workspaceId, path: surface.path, rootDir: surface.rootDir, title: surface.path.split(/[\\/]/).pop() || surface.path });
    }
    return [...connections.values()];
  };
  activeDatabaseConnection = (workspaceId = this.state.activeWorkspace?.id): DatabaseConnection | undefined => {
    const selected = isRecord(this.state.ui.databaseSelections) && workspaceId ? this.state.ui.databaseSelections[workspaceId] : undefined;
    const connections = this.databaseConnections(workspaceId);
    return connections.find(connection => connection.id === selected) || connections[0];
  };
  selectDatabaseConnection = (id: string, workspaceId = this.state.activeWorkspace?.id) => {
    if (!workspaceId || !this.databaseConnections(workspaceId).some(connection => connection.id === id)) return;
    const selected = isRecord(this.state.ui.databaseSelections) ? this.state.ui.databaseSelections : {};
    if (selected[workspaceId] !== id) this.setUI({ databaseSelections: { ...selected, [workspaceId]: id } });
  };
  getDatabaseSchema = (connection: DatabaseConnection): DatabaseController => {
    let controller = this.databaseSchemas.get(connection.id);
    if (!controller) { controller = new DatabaseController(this.client, connection.rootDir, connection.path, undefined, null); this.databaseSchemas.set(connection.id, controller); }
    if (this.client.state.phase === "connected") void controller.initialize();
    return controller;
  };
  openDatabaseTable = (connection: DatabaseConnection, table: string) => {
    if (this.disposed || this.closingWorkspaces.has(connection.workspaceId) || !this.state.workspaces.some(workspace => workspace.id === connection.workspaceId)) return;
    const schema = this.getDatabaseSchema(connection);
    if (!schema.state.objects.some(object => object.name === table)) return;
    const id = `${connection.id}:table:${encodeURIComponent(table)}`;
    this.addSurface({ id, kind: "page", page: "database", databaseTable: table, title: table,
      path: connection.path, rootDir: connection.rootDir, workspaceId: connection.workspaceId });
    schema.highlightTable(table);
    this.selectDatabaseConnection(connection.id, connection.workspaceId);
  };
  openGitReview = (descriptor: GitReviewDescriptor) => {
    const workspace = this.state.activeWorkspace;
    if (!workspace || !descriptor.root) throw new Error("Select a repository workspace first");
    const detail = isRecord(descriptor.detail) ? descriptor.detail : {};
    const summary = isRecord(detail.summary) ? detail.summary : detail;
    const identity = descriptor.mode === "working" ? "working" : descriptor.mode === "commit"
      ? descriptor.revision || String(detail.id || detail.oid || "commit")
      : String(summary.number || summary.id || descriptor.mode);
    const id = `git:${workspace.id}:${encodeURIComponent(descriptor.root)}:${descriptor.mode}:${identity}`;
    const before = this.state.surfaces.find(surface => surface.id === id);
    const title = descriptor.mode === "working" ? "Uncommitted Changes" : descriptor.mode === "commit"
      ? `Commit ${(descriptor.revision || String(detail.id || detail.oid || "")).slice(0, 7)}`
      : descriptor.mode === "pull-request-inbox" ? "Pull Requests" : descriptor.mode === "pull-request-create"
        ? "Create Pull Request" : `PR #${summary.number || summary.id || ""}`;
    const surface: Surface = { ...before, id, kind: "git", title, workspaceId: workspace.id,
      rootDir: descriptor.root, git: { ...descriptor }, gitReveal: (before?.gitReveal || 0) + 1 };
    this.transaction(() => {
      if (before) this.update({ surfaces: this.state.surfaces.map(current => current.id === id ? surface : current) });
      this.addSurface(surface);
    });
    return surface;
  };
  openConfigDocument = (document: { content: string; display_path: string; revision: string; writable: boolean }) => {
    const workspace = this.state.activeWorkspace; if (!workspace) throw new Error("Select a workspace first");
    const id = `config:${workspace.id}`, existing = this.editors.get(id);
    // Reopening the singleton must retain any unsaved editor bytes.
    if (!existing?.frame.dirty) {
      if (existing && existing.text() !== document.content) { existing.call("replace-source", { text: document.content }); existing.call("saved", { revision: existing.frame.revision }); }
      this.update({ files: { ...this.state.files, [id]: document.content }, surfaces: this.state.surfaces.map((surface) => surface.id === id ? { ...surface, document: { kind: "config", revision: document.revision, writable: document.writable } } : surface) });
    }
    this.addSurface({ id, kind: "file", title: "config.json", path: document.display_path, rootDir: workspace.root_dir || "", workspaceId: workspace.id,
      document: existing?.frame.dirty ? this.state.surfaces.find((s) => s.id === id)?.document : { kind: "config", revision: document.revision, writable: document.writable } });
  };
  closePane = (id: string, leaf: number, discard = false): boolean => {
    const surface=this.state.surfaces.find(surface=>surface.id===id);
    if(!surface){
      // An orphaned tab (its surface is gone) still closes.
      const layout=this.state.activeWorkspace?this.getLayout():undefined;
      if(!layout?.frame.leaves.some(entry=>entry.leaf===leaf))return false;
      layout.close(leaf);if(layout.frame.focusedSurfaceId)this.selectSurface(layout.frame.focusedSurfaceId,true);
      return true;
    }
    const layout=this.getLayout(surface.workspaceId);if(!layout.frame.leaves.some(entry=>entry.leaf===leaf&&entry.surfaceId===id))return false;
    if(this.databases.get(id)?.state.applying){this.report(new Error("Wait for the database transaction to finish before closing."));return false;}
    if(!discard&&this.surfaceDirty(id)&&layout.frame.leaves.filter(entry=>entry.surfaceId===id).length<2){this.report(new Error(this.databases.has(id)?"Apply or discard the database changes before closing.":"Unsaved changes. Save this file or use :q! to discard it."));return false;}
    this.transaction(()=>{
      const key=`${id}@${leaf}`,view=this.editorViews.get(key);this.markdownViews.get(key)?.markdown.dispose();this.markdownViews.delete(key);delete this.recoveredMarkdownViews[key];if(view)this.collaboration.detach(view.editor);view?.editor.dispose();this.editorViews.delete(key);delete this.recoveredViews[`view:${key}`];
      layout.close(leaf);
      if(!layout.frame.leaves.some(entry=>entry.surfaceId===id))this.closeSurface(id,discard);
      if(this.state.activeWorkspace?.id===surface.workspaceId&&layout.frame.focusedSurfaceId)this.selectSurface(layout.frame.focusedSurfaceId,true);
    });
    return true;
  };
  closeCurrentTab = (discard = false): boolean => {
    if(!this.state.activeWorkspace)return false;
    const frame=this.getLayout().frame;
    return frame.focusedSurfaceId!==undefined&&frame.focusedLeaf!==null?this.closePane(frame.focusedSurfaceId,frame.focusedLeaf,discard):false;
  };
  clearError = () => this.update({ error: undefined });
  clearSessionError = (sessionId: string) => {
    if(this.state.sessionErrors[sessionId])this.update({sessionErrors:{...this.state.sessionErrors,[sessionId]:undefined}});
  };
  private reportSessionError(sessionId: string, message: string, terminal = false) {
    this.update({sessionErrors:{...this.state.sessionErrors,[sessionId]:message},...(terminal?{streaming:{...this.state.streaming,[sessionId]:"Idle"}}:{})});
    this.report(new Error(message));
    const row:HistoryMessage={id:`error:${Date.now()}:${crypto.randomUUID()}`,role:"System",kind:"System",title:"Agent error",text:message};
    this.upsertMessage(sessionId,row);
    for(const listener of this.agentEventListeners){try{listener({MessageUpdated:{session_id:sessionId,message:row}});}catch(error){this.report(error);}}
  }
  report = (error: unknown) => {
    // Also to stderr: UI toasts vanish, and a field report needs the stack.
    if (process.env.NODE_ENV !== "test") console.error("[neoism-gpuix] reported:", error instanceof Error ? error.stack || error.message : error);
    if (!this.disposed) this.update({ error: error instanceof Error ? error.message : "Operation failed" });
  };
  private async run<A, E>(effect: Effect.Effect<A, E>): Promise<A> {
    return Effect.runPromise(effect, { signal: this.abort.signal });
  }
  private homeTerminalRequests = new Map<string, Promise<void>>();
  private homeTerminalOwners() {
    return isRecord(this.state.ui.homeTerminalSessions) ? this.state.ui.homeTerminalSessions : {};
  }
  private closeOwnedPty(sessionId:string) {
    closeTerminalSession(this.client,sessionId);
    // Cleanup must outlive the model's abort signal (including create replies
    // arriving after dispose). Never use this for an attached foreign session.
    return Effect.runPromise(this.client.request("Pty",{ClosePty:{session_id:sessionId}})).then(()=>{},this.report);
  }
  private releaseHomeTerminal(surfaceId:string, close=true) {
    this.homeTerminalRequests.delete(surfaceId);
    const owners={...this.homeTerminalOwners()},sessionId=owners[surfaceId];delete owners[surfaceId];
    const pending={...(isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{})};delete pending[surfaceId];
    if(typeof sessionId==="string"){
      if(close)void this.closeOwnedPty(sessionId);else closeTerminalSession(this.client,sessionId);
      const main={...(isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{})};
      const cwds={...(isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{})};
      for(const [workspace,id]of Object.entries(main))if(id===sessionId)delete main[workspace];
      delete cwds[sessionId];
      this.setUI({mainTerminals:main,terminalCwds:cwds});
      this.update({surfaces:this.state.surfaces.map(surface=>surface.id===surfaceId&&surface.sessionId===sessionId?{...surface,sessionId:undefined}:surface)});
    }
    this.setUI({homeTerminalSessions:owners,pendingTerminalBindings:pending});
  }
  /** Allocate a shell for an existing Home without selecting/creating a tab.
   * The first shell owns cwd follow; UI unmount is not closure. */
  ensureHomeTerminal = (surfaceId:string):Promise<void> => {
    const pending=this.homeTerminalRequests.get(surfaceId);if(pending)return pending;
    if(this.disposed)return Promise.resolve();
    const surface=this.state.surfaces.find(surface=>surface.id===surfaceId&&surface.kind==="welcome");
    if(!surface)return Promise.resolve();
    if(surface.sessionId){
      if(this.homeTerminalOwners()[surfaceId]!==surface.sessionId||this.state.surfaces.some(other=>other.id!==surfaceId&&other.sessionId===surface.sessionId))
        this.report(new Error("Home cannot take ownership of an attached terminal session"));
      return Promise.resolve();
    }
    // Use this surface's declared workspace, never whichever workspace happens
    // to be active when an asynchronous create finishes.
    const declared=this.state.workspaces.find(workspace=>workspace.id===surface.workspaceId);
    const workspace=declared&&this.workspaceView(declared);
    if(!workspace?.root_dir){this.report(new Error("Home terminal requires a declared workspace directory"));return Promise.resolve();}
    let task!:Promise<void>;
    const valid=()=>!this.disposed&&this.homeTerminalRequests.get(surfaceId)===task
      &&this.state.surfaces.some(current=>current.id===surfaceId&&current.kind==="welcome"&&current.workspaceId===workspace.id&&!current.sessionId)
      &&this.state.workspaces.some(current=>current.id===workspace.id&&this.workspaceView(current).root_dir===workspace.root_dir);
    task=Promise.resolve().then(async()=>{
      let fresh:string|undefined;
      const available=()=>!isRecord(this.state.ui.unavailableTerminals)||!this.state.ui.unavailableTerminals[fresh!];
      try{
        if(!valid())return;
        // Do not abort create on dispose: we must observe its fresh session ID
        // and close it, rather than orphaning a daemon PTY.
        const result=expectVariant(await Effect.runPromise(this.client.request("Pty",{CreatePty:{cwd:workspace.root_dir!,cols:100,rows:30,presentation:"blocks",
          ...(this.config.shell?.program?{shell:this.config.shell.program}:{}),...(this.config.shell?.args!==undefined?{shell_args:this.config.shell.args}:{})}})),"PtyCreated");
        if(!isRecord(result)||typeof result.session_id!=="string")throw new Error("Invalid terminal session");
        fresh=result.session_id;
        if(result.presentation!=="blocks")throw new Error("The running daemon is too old for this terminal. Update or restart the workspace daemon, then reconnect.");
        if(!valid()||!available())return;
        if(this.state.activeWorkspace?.id===workspace.id)
          await Effect.runPromise(this.client.request("Workspace",{BindEditorSurface:{surface_id:surfaceId,session_id:fresh,path:null}}));
        if(!valid()||!available())return;
        const sessionId=fresh;
        this.transaction(()=>{
          if(!this.mainTerminalSurface(workspace.id)){const main=isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{};this.setUI({mainTerminals:{...main,[workspace.id]:sessionId}});}
          // Queue if selection changed while create or bind was in flight.
          if(this.state.activeWorkspace?.id!==workspace.id){
            const bindings=isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{};
            this.setUI({pendingTerminalBindings:{...bindings,[surfaceId]:{workspaceId:workspace.id,sessionId}}});
          }
          this.setUI({homeTerminalSessions:{...this.homeTerminalOwners(),[surfaceId]:sessionId}});
          this.update({surfaces:this.state.surfaces.map(current=>current.id===surfaceId?{...current,sessionId,rootDir:workspace.root_dir!}:current)});
        });
        fresh=undefined;
        this.followMainTerminal();
      }catch(error){this.report(error);}
      // PtyClosed may beat the create/bind reply. The daemon has already
      // removed that fresh PTY; closing it again only produces "unknown session".
      finally{if(fresh&&available())await this.closeOwnedPty(fresh);}
    }).finally(()=>{if(this.homeTerminalRequests.get(surfaceId)===task)this.homeTerminalRequests.delete(surfaceId);});
    this.homeTerminalRequests.set(surfaceId,task);return task;
  };
  private async hydrate() {
    const epoch = ++this.epoch;
    this.update({ loading: true, error: undefined });
    try {
      const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await this.run(this.client.request("Workspace", "RequestHostWorkspaceTree")), "HostWorkspaceTree"));
      if (epoch !== this.epoch) return;
      this.update({ workspaces: tree.workspaces, workspaceTabs: tree.tabs });
      const hostId=this.state.connection.connectedHostId;
      const recoverLocal=!!hostId&&!this.config.workspaceId&&localWorkspaceSession({config:this.config,model:this});
      const eligible=(workspace:Workspace)=>!recoverLocal||workspace.host_id===hostId;
      // Remote tabs belong to separate connections. Old default selection could
      // persist a foreign row in this local window; hide that membership only,
      // retaining its catalog, layouts, editors and unsent drafts for recovery.
      const remembered=openWorkspaceIds(this.state.ui);
      if(recoverLocal&&remembered){
        const ids=remembered.filter(id=>{const workspace=tree.workspaces.find(workspace=>workspace.id===id);return !workspace||eligible(workspace);});
        if(ids.length!==remembered.length)this.setUI({openWorkspaceIds:ids});
      }
      const preferred = this.state.activeWorkspace?.id || this.config.workspaceId || this.restoredWorkspaceId;
      const rememberedWindow=openWorkspaceIds(this.state.ui);
      if(!this.config.workspaceId&&remembered?.length===0){
        await this.syncWindowWorkspaces([]);
        this.update({activeWorkspace:undefined,activeSurfaceId:undefined});
        return;
      }
      const opened=this.windowWorkspaces().filter(eligible);
      let workspace = !this.config.workspaceId&&rememberedWindow!==undefined
        ? opened.find(workspace=>workspace.id===preferred)||opened[0]
        : tree.workspaces.find((w) => w.id === preferred&&eligible(w));
      if (!workspace) {
        workspace = await this.resolveHostWorkspace();
      }
      if (epoch !== this.epoch || !workspace) return;
      const explicitRoots=isRecord(this.state.ui.explicitWorkspaceRoots)?this.state.ui.explicitWorkspaceRoots:{};
      const home=this.startupWelcomePending&&recoverLocal&&(!workspace.host_kind||workspace.host_kind==="local")&&explicitRoots[workspace.id]!==workspace.root_dir
        ? recoveredStartupDirectory(this.workspaceView(workspace).root_dir) : undefined;
      if(home){
        // Keep the old workspace and all its shells/dirty recovery data intact.
        // Only replace this window's mistaken startup membership with a fresh Home.
        const previous=workspace.id;
        const recovered=await this.createWorkspace({...workspace,root_dir:home});
        if(recovered){
          const ids=openWorkspaceIds(this.state.ui)||[];
          if(ids.includes(previous))this.setUI({openWorkspaceIds:ids.filter(id=>id!==recovered.id).map(id=>id===previous?recovered.id:id)});
          await this.syncWindowWorkspaces();
        }
        return;
      }
      await this.activateWorkspace(workspace, true);
    } catch (error) { if (epoch === this.epoch) this.report(error); }
    finally { if (epoch === this.epoch) this.update({ loading: false }); }
  }
  windowWorkspaces=(state:Pick<AppState,"workspaces"|"ui">=this.state)=>getWindowWorkspaces(state);
  private async syncWindowWorkspaces(ids=this.windowWorkspaces().map(workspace=>workspace.id),pruneEmpty?:string){
    if(!this.state.connection.workspaceLifecycle){
      if(pruneEmpty)this.report(new Error("Workspace hidden. Update and restart the daemon to enable unused-workspace cleanup."));
      return {workspaces:this.state.workspaces,tabs:this.state.workspaceTabs};
    }
    return Schema.decodeUnknownSync(TreeSchema)(expectVariant(await this.run(this.client.request("Workspace",{
      SetWindowWorkspaces:{workspace_ids:ids,prune_empty:pruneEmpty||null},
    })),"HostWorkspaceTree"));
  }
  /** Ask the daemon for its own host's default, never borrow another host's root. */
  resolveHostWorkspace = async ():Promise<Workspace> => {
    const hostId=this.state.connection.connectedHostId;
    const reply=expectVariant(await this.run(this.client.request("Workspace",{ResolveInitialWorkspace:{preferred_host_id:hostId||null}})),"InitialWorkspaceResolved");
    if(!isRecord(reply))throw new Error("Invalid default workspace reply");
    const workspace=Schema.decodeUnknownSync(WorkspaceSchema)(reply.workspace);
    if(hostId&&workspace.host_id!==hostId)throw new Error("The daemon returned a workspace on a different host; restart or update the local daemon");
    this.update({workspaces:this.state.workspaces.some(entry=>entry.id===workspace.id)?this.state.workspaces.map(entry=>entry.id===workspace.id?workspace:entry):[...this.state.workspaces,workspace]});
    return workspace;
  };
  reorderWorkspace = (dragId:string,targetId:string,position:"before"|"after"="before"):boolean => {
    const visible=this.windowWorkspaces();if(!visible.some(workspace=>workspace.id===dragId)||!visible.some(workspace=>workspace.id===targetId))return false;
    const ids=reorderWindowWorkspace(this.state.ui,dragId,targetId,position),previous=openWorkspaceIds(this.state.ui)||[];
    if(ids.every((id,index)=>id===previous[index]))return false;
    this.setUI({openWorkspaceIds:ids});return true;
  };
  newWorkspace = (parent?:Workspace): Promise<Workspace | undefined> => {
    const id=crypto.randomUUID();
    const hostId=this.state.connection.connectedHostId;
    const eligible=(workspace:Workspace|undefined)=>workspace&&(!hostId||workspace.host_id===hostId)?workspace:undefined;
    const source=parent||eligible(this.state.activeWorkspace)||this.state.workspaces.find(workspace=>!hostId||workspace.host_id===hostId);
    const inherited=source&&this.workspaceView(source);
    this.pendingWorkspaceCreations.add(id);this.update({creatingWorkspace:true});
    const task=this.workspaceCreations.then(async()=>{
      if(this.disposed)return;
      if(inherited)return this.createWorkspace(inherited,id);
      const current=eligible(this.state.activeWorkspace)||this.state.workspaces.find(workspace=>!hostId||workspace.host_id===hostId);
      if(current)return this.createWorkspace(this.workspaceView(current),id);
      // An empty daemon resolves one default; do not immediately create a second.
      const workspace=await this.resolveHostWorkspace();
      await this.activateWorkspace(workspace);return workspace;
    }).catch(error=>{this.report(error);return undefined;}).finally(()=>{
      this.pendingWorkspaceCreations.delete(id);this.update({creatingWorkspace:this.pendingWorkspaceCreations.size>0});
    });
    this.workspaceCreations=task;
    return task;
  };
  private async createWorkspace(parent:Workspace,id=crypto.randomUUID()):Promise<Workspace|undefined>{
    const root=parent.root_dir??null;
    const navigation=this.state.activeWorkspace&&this.workspaceKey(this.state.activeWorkspace)===this.workspaceKey(parent)
      ? this.state : this.workspaceNavigation.get(this.workspaceKey(parent));
    const title=(root?(win32.isAbsolute(root)?win32.basename(root):posix.basename(root)):"")||"Workspace";
    this.pendingWorkspaceCreations.add(id);
    this.update({creatingWorkspace:true});
      try{
        if(this.disposed)return;
        const body=expectVariant(await this.run(this.client.request("Workspace",{CreateHostWorkspace:{host_id:parent.host_id,workspace_id:id,title,root_dir:root}})),"HostWorkspaceUpserted");
        if(!isRecord(body))throw new Error("Invalid created workspace");
        const workspace=Schema.decodeUnknownSync(WorkspaceSchema)(body.workspace);
        if(workspace.id!==id||workspace.host_id!==parent.host_id)throw new Error("The daemon returned a different workspace");
        if(this.disposed)return;
        this.disposableWorkspaces.add(workspace.id);
        // The + action starts in the same host/folder. Its known directory
        // listing can paint immediately while the new workspace refreshes it.
        // Expansion, selection, scroll and conversation state remain its own.
        if(workspace.root_dir===parent.root_dir&&navigation?.directories[""]){
          this.workspaceNavigation.set(this.workspaceKey(workspace),{
            directories:{...navigation.directories},expanded:new Set([""]),
            notesDirectories:{},notesExpanded:new Set([""]),threads:[],
          });
        }
        let activation!:Promise<void>;
        this.transaction(()=>{
          this.update({workspaces:this.state.workspaces.some(entry=>entry.id===id)?this.state.workspaces.map(entry=>entry.id===id?workspace:entry):[...this.state.workspaces,workspace]});
          // Prepare the new layout before activation publishes its first frame.
          // Creating a workspace opens Welcome and does not allocate a shell.
          this.addSurface({id:`welcome:${id}`,kind:"welcome",title:"Home",workspaceId:id});
          activation=this.activateWorkspace(workspace);
        });
        await activation;
        return workspace;
      }catch(error){this.report(error);return undefined;}
      finally{this.pendingWorkspaceCreations.delete(id);this.update({creatingWorkspace:this.pendingWorkspaceCreations.size>0});}
  }
  closeWorkspace = (discard=false,workspaceId=this.state.activeWorkspace?.id):Promise<boolean> => {
    const task=this.workspaceCreations.then(async()=>{
      if(this.disposed||!workspaceId)return false;
      const opened=this.windowWorkspaces(),index=opened.findIndex(workspace=>workspace.id===workspaceId),workspace=opened[index];
      if(!workspace)return false;
      const localSurfaces=()=>this.state.surfaces.filter(surface=>surface.workspaceId===workspaceId);
      const databaseApplying=()=>localSurfaces().some(surface=>this.databases.get(surface.id)?.state.applying);
      if(databaseApplying()){this.report(new Error("Wait for the database transaction to finish before closing this workspace."));return false;}
      const draftKeys=new Set([`home:${workspaceId}`,...localSurfaces().flatMap(surface=>surface.kind==="welcome"?[this.composerDraftKey(surface.homeAgentSessionId,surface.id)]:!surface.sessionId?[this.composerDraftKey(undefined,surface.id)]:[])]);
      const hasUnsaved=()=>localSurfaces().some(surface=>this.surfaceDirty(surface.id))||[...draftKeys].some(key=>{const draft=this.state.composerDrafts[key];return !!draft&&(!!draft.text||draft.attachments.length>0);});
      if(!discard&&hasUnsaved()){this.report(new Error("Unsaved changes or unsent drafts. Save this workspace or use :qall! to discard it."));return false;}
      if(!this.state.connection.workspaceLifecycle){this.report(new Error("Update and restart the daemon to close this workspace and its resources safely."));return false;}
      const wasActive=this.state.activeWorkspace?.id===workspaceId;
      const adjacent=opened[index+1]||opened[index-1];
      const remaining=opened.filter(entry=>entry.id!==workspaceId).map(entry=>entry.id);
      this.closingWorkspaces.add(workspaceId);
      this.workspaceCloseEpochs.set(workspaceId,(this.workspaceCloseEpochs.get(workspaceId)||0)+1);
      try{
        // Unmount this workspace's inputs before awaiting host teardown. A failed
        // close restores the view without disposing its documents or tabs.
        if(wasActive){
          if(adjacent)await this.activateWorkspace(adjacent);
          else{++this.epoch;this.presence.leaveWorkspace();this.update({activeWorkspace:undefined,activeSurfaceId:undefined,loading:false});}
        }
        if(!discard&&hasUnsaved())throw new Error("Unsaved changes or unsent drafts. Save this workspace or use :qall! to discard it.");
        if(databaseApplying())throw new Error("Wait for the database transaction to finish before closing this workspace.");
        const tree=Schema.decodeUnknownSync(TreeSchema)(expectVariant(await this.run(this.client.request("Workspace",{CloseWindowWorkspace:{workspace_id:workspaceId}})),"HostWorkspaceTree"));
        if(this.disposed)return false;
        this.transaction(()=>{
          this.update({workspaces:tree.workspaces,workspaceTabs:tree.tabs});
          this.setUI({openWorkspaceIds:remaining.filter(id=>tree.workspaces.some(entry=>entry.id===id))});
          for(const surface of localSurfaces()){
            // The host decides whether a shell is still owned by another window.
            // Locally release its view; never kill a shared shell as a side effect.
            if(surface.kind==="welcome")this.releaseHomeTerminal(surface.id,false);
            this.closeSurface(surface.id,true);
          }
          this.layouts.get(workspaceId)?.dispose();this.layouts.delete(workspaceId);
          for(const connection of this.databaseConnections(workspaceId)){this.databaseSchemas.get(connection.id)?.dispose();this.databaseSchemas.delete(connection.id);}
          for(const key of this.workspaceNavigation.keys())if(JSON.parse(key)[0]===workspaceId)this.workspaceNavigation.delete(key);
          this.seededWorkspaces.delete(workspaceId);
          const ui:Record<string,unknown>={};
          for(const key of ["layouts","workspaceSidebars","databaseSidebarOrigins","mainTerminals","terminalWorkspaceRoots","explicitWorkspaceRoots","databaseConnections","databaseSelections"]){
            if(isRecord(this.state.ui[key])){const map={...this.state.ui[key]};delete map[workspaceId];ui[key]=map;}
          }
          this.setUI(ui);
          const composerDrafts={...this.state.composerDrafts};for(const key of draftKeys)delete composerDrafts[key];
          this.update({composerDrafts,...(!this.state.activeWorkspace?{directories:{},notesDirectories:{},threads:[],loading:false}:{})});
        });
        this.requestedAgentCatalogScopes.delete(workspaceId);
        if(this.agentCatalogStream?.workspaceId===workspaceId){this.agentCatalogStream.abort.abort();this.agentCatalogStream=undefined;}
        this.disposableWorkspaces.delete(workspaceId);
        for(const listener of this.workspaceClosedListeners){try{listener(workspaceId);}catch(error){this.report(error);}}
        return true;
      }catch(error){
        this.closingWorkspaces.delete(workspaceId);
        if(!this.disposed&&wasActive&&(!this.state.activeWorkspace||this.state.activeWorkspace.id===adjacent?.id))await this.activateWorkspace(workspace);
        this.report(error);return false;
      }finally{this.closingWorkspaces.delete(workspaceId);}
    }).catch(error=>{this.report(error);return false;});
    this.workspaceCreations=task;
    return task;
  };
  private workspaceKey(workspace: Workspace) {
    return JSON.stringify([workspace.id, workspace.root_dir || "", workspace.notes_vault_dir || "",workspace.host_kind||"local"]);
  }
  mainTerminalSurface=(workspaceId=this.state.activeWorkspace?.id)=>{
    const workspace=this.state.workspaces.find(workspace=>workspace.id===workspaceId)||this.state.activeWorkspace;
    const remembered=isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals[workspaceId||""]:undefined;
    const ended=isRecord(this.state.ui.unavailableTerminals)?this.state.ui.unavailableTerminals:{};
    const homes=this.homeTerminalOwners();
    return mainTerminalSurface(workspace,this.state.surfaces.filter(surface=>!ended[surface.sessionId||""]&&(surface.kind!=="welcome"||this.config.terminalBlockUi&&homes[surface.id]===surface.sessionId)),remembered);
  };
  setMainTerminal=(surfaceId:string)=>{
    const surface=this.state.surfaces.find(surface=>surface.id===surfaceId);
    if(this.disposed||!surface?.sessionId||surface.workspaceId!==this.state.activeWorkspace?.id)return;
    if(surface.kind!=="terminal"&&(surface.kind!=="welcome"||!this.config.terminalBlockUi||this.homeTerminalOwners()[surface.id]!==surface.sessionId))return;
    if(isRecord(this.state.ui.unavailableTerminals)&&this.state.ui.unavailableTerminals[surface.sessionId])return;
    if(this.workspaceDirectoryChanges.has(surface.workspaceId))return;
    const main=isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{};
    this.setUI({mainTerminals:{...main,[surface.workspaceId]:surface.sessionId}});
    this.followMainTerminal();
  };
  /** Explicit folder selection: change the existing first shell through the
   * host's controlled channel, then publish only its confirmed actual cwd. */
  changeWorkspaceDirectory=async(path:string,workspaceId=this.state.activeWorkspace?.id):Promise<void>=>{
    const captured=this.state.workspaces.find(workspace=>workspace.id===workspaceId);
    if(!captured)throw new Error("Select a workspace first");
    if(!absoluteTerminalDirectory(path)||/[\x00-\x1f\x7f]/.test(path))throw new Error("Choose an absolute directory without control characters");
    if(this.workspaceDirectoryChanges.has(captured.id))throw new Error("This workspace is still waiting for its directory change");
    this.disposableWorkspaces.delete(captured.id);
    this.workspaceDirectoryChanges.add(captured.id);
    try{
    const terminal=this.mainTerminalSurface(captured.id);
    const actual=terminal?.sessionId?await terminalSession(this.client,terminal.sessionId,this.config.theme,this.report).changeDirectory(path):path;
    if(this.disposed)return;
    const body=expectVariant(await this.run(this.client.request("Workspace",{SetWorkspaceRoot:{workspace_id:captured.id,root_dir:actual}})),"HostWorkspaceUpserted");
    if(!isRecord(body))throw new Error("Invalid workspace directory reply");
    const workspace=Schema.decodeUnknownSync(WorkspaceSchema)(body.workspace);
    if(workspace.id!==captured.id)throw new Error("The host returned a different workspace directory");
    this.transaction(()=>{
      this.update({workspaces:this.state.workspaces.map(entry=>entry.id===workspace.id?workspace:entry)});
      const roots=isRecord(this.state.ui.explicitWorkspaceRoots)?this.state.ui.explicitWorkspaceRoots:{};
      this.setUI({explicitWorkspaceRoots:{...roots,[workspace.id]:workspace.root_dir}});
      if(terminal?.sessionId){const cwds=isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{};this.setUI({terminalCwds:{...cwds,[terminal.sessionId]:actual}});}
    });
    if(this.state.activeWorkspace?.id===captured.id)await this.activateWorkspace(workspace);
    }finally{this.workspaceDirectoryChanges.delete(captured.id);}
  };
  private workspaceView(workspace:Workspace):Workspace{
    const roots=isRecord(this.state.ui.terminalWorkspaceRoots)?this.state.ui.terminalWorkspaceRoots:{},entry=roots[workspace.id];
    if(!terminalDirectoryFollowsWorkspace(workspace,this.config)||!isRecord(entry)||entry.declaredRoot!==(workspace.root_dir||null)||!absoluteTerminalDirectory(entry.root)||entry.root===workspace.root_dir)return workspace;
    return {...workspace,root_dir:entry.root,notes_vault_dir:undefined,linked_vault_dir:undefined,...this.terminalNotesVaults.get(entry.root)};
  }
  private followMainTerminal(){
    const workspace=this.state.activeWorkspace,surface=this.state.surfaces.find(surface=>surface.id===this.state.activeSurfaceId);
    if(!workspace||!followsTerminalDirectory(workspace,surface,this.mainTerminalSurface(),this.config))return;
    const cwds=isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{},cwd=cwds[surface!.sessionId!];
    if(!absoluteTerminalDirectory(cwd)||cwd===workspace.root_dir)return;
    const declared=this.state.workspaces.find(entry=>entry.id===workspace.id)||workspace;
    const roots=isRecord(this.state.ui.terminalWorkspaceRoots)?this.state.ui.terminalWorkspaceRoots:{};
    this.setUI({terminalWorkspaceRoots:{...roots,[workspace.id]:{declaredRoot:declared.root_dir||null,root:cwd}}});
    // Refresh only this frontend's Explorer/notes/catalog scope. Never publish a
    // host SetWorkspaceRoot or send a cd command into any running shell.
    void this.activateWorkspace(declared);
  }
  activateWorkspace = async (workspace: Workspace, force = false) => {
    if(this.closingWorkspaces.has(workspace.id))return;
    const declared=this.state.workspaces.find(entry=>entry.id===workspace.id)||workspace;
    workspace=this.workspaceView(declared);
    const previous = this.state.activeWorkspace, key = this.workspaceKey(workspace);
    if (!force && previous && this.workspaceKey(previous) === key) {
      if(this.presence.workspaceId()===workspace.id)this.presence.setWorkspace(workspace);
      this.setUI({openWorkspaceIds:includeWindowWorkspace(this.state.ui,workspace.id)});
      return;
    }
    this.presence.leaveWorkspace();
    if (previous) this.workspaceNavigation.set(this.workspaceKey(previous), {
      directories: this.state.directories, expanded: this.state.expanded, notesDirectories: this.state.notesDirectories,
      notesExpanded: this.state.notesExpanded, threads: this.state.threads,
    });
    const cached = this.workspaceNavigation.get(key), epoch = ++this.epoch;
    const startupWelcome = this.startupWelcomePending;
    const activeSurfaceId = this.state.surfaces.find((s) => s.id === this.state.activeSurfaceId && s.workspaceId === workspace.id)?.id || this.state.surfaces.find((s) => s.workspaceId === workspace.id)?.id;
    this.transaction(() => {
      this.setUI({openWorkspaceIds:includeWindowWorkspace(this.state.ui,workspace.id)});
      this.update({ activeWorkspace: workspace, directories: cached?.directories || {}, notesDirectories: cached?.notesDirectories || {},
        notesExpanded: cached?.notesExpanded || new Set([""]), threads: cached?.threads || [], expanded: cached?.expanded || new Set([""]),
        loading: !cached?.directories[""], error: undefined, activeSurfaceId,agentDefaultsLoading:!this.state.agentDefaults?.[JSON.stringify([workspace.id,workspace.root_dir||""])]?.model });
      const layout = this.getLayout(workspace.id);
      if (layout.frame.focusedSurfaceId) this.update({ activeSurfaceId: layout.frame.focusedSurfaceId });
      if (startupWelcome) {
        this.ensureWelcome();
        this.startupWelcomePending = false;
      } else if (!activeSurfaceId) {
        // A never-shown workspace would paint one empty frame until the
        // post-subscribe ensureWelcome; its Welcome tab is local, add it now.
        this.ensureWelcome();
      }
    });
    // Agent defaults use the workspace-scoped SDK route, independently of the
    // websocket subscription and any old terminal binding acknowledgements.
    // A slow subscription must not leave a usable empty Agent on placeholders.
    void this.refreshCatalogs();
    void this.refreshThreadCatalog(workspace,epoch);
    if(this.requestedAgentCatalogScopes.has(workspace.id))this.ensureAgentCatalogStream(workspace.id,this.requestedAgentCatalogScopes.get(workspace.id));
    // File/directory operations carry explicit roots and need not wait for the
    // workspace subscription acknowledgement. Cached navigation stays visible.
    const subscribed = this.syncWindowWorkspaces().then(()=>this.run(this.client.request("Workspace", { SubscribeWorkspace: { workspace_id: workspace.id } })));
    const listing = this.listDirectory("", epoch);
    if (workspace.notes_vault_dir) void this.listDirectory("", epoch, true);
    if(workspace.root_dir!==declared.root_dir){
      const root=workspace.root_dir!;
      void this.run(this.client.request("Workspace",{ResolveNotesVault:{root_dir:root}})).then(reply=>{
        const vault=expectVariant(reply,"NotesVaultResolved");
        if(!isRecord(vault)||typeof vault.notes_vault_dir!=="string")throw new Error("Invalid notes vault reply");
        if(this.disposed||epoch!==this.epoch)return;
        const notes={notes_vault_dir:vault.notes_vault_dir,linked_vault_dir:typeof vault.linked_vault_dir==="string"?vault.linked_vault_dir:null};
        this.terminalNotesVaults.set(root,notes);
        const changed=this.state.activeWorkspace?.notes_vault_dir!==notes.notes_vault_dir;
        workspace={...workspace,...notes};
        this.update({activeWorkspace:workspace,...(changed?{notesDirectories:{},notesExpanded:new Set([""])}:{})});
        void this.listDirectory("",epoch,true);
      }).catch(error=>{if(!this.disposed&&epoch===this.epoch)this.report(error);});
    }
    for (const pane of this.getLayout(workspace.id).frame.panes) {
      const surface = this.state.surfaces.find((s) => s.id === pane.surfaceId);
      if (surface?.kind === "file") void this.loadFile(surface);
    }
    try {
      await subscribed;
      if (epoch !== this.epoch) return;
      this.presence.setWorkspace(workspace);
      for (const pane of this.getLayout(workspace.id).frame.panes) {
        const surface = agentSurface(this.state.surfaces.find((s) => s.id === pane.surfaceId));
        if (surface?.kind === "agent" && surface.sessionId) this.resumeThread(surface.sessionId);
      }
      await this.bindPendingTerminals(workspace.id);
      if(epoch!==this.epoch)return;
      if(!this.state.activeSurfaceId)this.ensureWelcome();
      await listing;
      this.followMainTerminal();
    } catch (error) { if (epoch === this.epoch) this.report(error); }
    finally { if (epoch === this.epoch) this.update({ loading: false }); }
  };
  listDirectory = (path: string, epoch = this.epoch, notes = false): Promise<void> => {
    const root = notes ? this.state.activeWorkspace?.notes_vault_dir : this.state.activeWorkspace?.root_dir;
    if (!root) return Promise.resolve();
    const version=this.directoryVersions.get(root)||0;
    const key = JSON.stringify([epoch, root, path, notes,version]), pending = this.directoryLoads.get(key);
    if (pending) return pending;
    const task = (async () => {
      try {
        const reply = await this.run(this.client.request("Files", { ListDir: { path } }, root));
        const directory = Schema.decodeUnknownSync(DirectorySchema)(expectVariant(reply, "DirListing"));
        if (epoch !== this.epoch || version !== (this.directoryVersions.get(root)||0) || root !== (notes ? this.state.activeWorkspace?.notes_vault_dir : this.state.activeWorkspace?.root_dir)) return;
        const entries = [...directory.entries].sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || a.name.localeCompare(b.name));
        const before = (notes ? this.state.notesDirectories : this.state.directories)[path];
        if (before && before.entries.length === entries.length && before.entries.every((entry, index) => entry.name === entries[index].name && entry.is_dir === entries[index].is_dir && entry.size === entries[index].size)) return;
        if (notes) this.update({ notesDirectories: { ...this.state.notesDirectories, [path]: { path, entries } } });
        else this.update({ directories: { ...this.state.directories, [path]: { path, entries } } });
      } catch (error) { if (epoch === this.epoch) this.report(error); }
      finally { this.directoryLoads.delete(key); }
    })();
    this.directoryLoads.set(key, task); return task;
  };
  refreshTreeDirectory = (path:string,notes=false) => {
    const root=notes?this.state.activeWorkspace?.notes_vault_dir:this.state.activeWorkspace?.root_dir;
    if(root)this.directoryVersions.set(root,(this.directoryVersions.get(root)||0)+1);
    return this.listDirectory(path,undefined,notes);
  };
  toggleDirectory = (path: string, notes = false) => {
    const expanded = new Set(notes ? this.state.notesExpanded : this.state.expanded);
    if (expanded.has(path)) expanded.delete(path);
    else { expanded.add(path); if (!(notes ? this.state.notesDirectories : this.state.directories)[path]) void this.listDirectory(path, this.epoch, notes); }
    this.update(notes ? { notesExpanded: expanded } : { expanded });
  };
  /** A tree drop is a host-scoped rename, including for joined workspaces. */
  moveTreePath = async (from: string, directory: string, notes = false, newName?:string) => {
    const workspace=this.state.activeWorkspace,epoch=this.epoch;
    const root=notes?workspace?.notes_vault_dir:workspace?.root_dir;
    if(!workspace||!root)return;
    const paths=hostPaths(root);
    const relativePath=(value:string)=>paths.relative(root,paths.resolve(root,value)).replaceAll("\\","/");
    from=relativePath(from);directory=relativePath(directory);
    const outside=(path:string)=>path===".."||path.startsWith("../")||paths.isAbsolute(path);
    if(!from||outside(from)||outside(directory))throw new Error("The move must stay inside this tree's root.");
    const oldName=posix.basename(from),name=newName??oldName;
    if(!name||name==="."||name===".."||/[\\/\0]/.test(name))throw new Error("Enter a file name without path separators");
    const to=directory?`${directory}/${name}`:name;
    if(to===from)return;
    if(directory===from||directory.startsWith(from+"/"))throw new Error("A folder cannot be moved inside itself.");
    if(this.state.surfaces.some(surface=>surface.page==="database"&&surface.workspaceId===workspace.id&&surface.rootDir===root&&surface.path&&(surface.path===from||surface.path.startsWith(from+"/"))))throw new Error("Close the database tab before moving or renaming its file.");
    // Older daemons allow replacing a destination. Check the authoritative
    // listing, not our potentially stale navigation cache, before requesting it.
    const listing=Schema.decodeUnknownSync(DirectorySchema)(expectVariant(await this.run(this.client.request("Files",{ListDir:{path:directory}},root)),"DirListing"));
    if(listing.entries.some(entry=>paths===win32?entry.name.toLowerCase()===name.toLowerCase():entry.name===name))throw new Error("A file or folder with that name already exists there.");
    if(epoch!==this.epoch||this.disposed)return;
    expectVariant(await this.run(this.client.request("Files",{Rename:{from,to}},root)),"Renamed");
    if(this.disposed)return;
    this.directoryVersions.set(root,(this.directoryVersions.get(root)||0)+1);
    const remap=(path:string)=>path===from?to:path.startsWith(from+"/")?to+path.slice(from.length):path;
    const changed:Surface[]=[];
    const surfaces=this.state.surfaces.map(surface=>{
      if(surface.kind!=="file"||surface.document||surface.workspaceId!==workspace.id||(surface.rootDir||workspace.root_dir)!==root||!surface.path)return surface;
      const path=remap(surface.path);if(path===surface.path)return surface;
      const next={...surface,path,title:posix.basename(path)};changed.push(next);return next;
    });
    this.update({surfaces});
    for(const surface of changed){
      const path=this.hostPath(surface),owner=this.editors.get(surface.id);
      if(owner)this.collaboration.rebind(owner,path,workspace);
      for(const view of this.editorViews.values())if(view.surfaceId===surface.id){view.editor.call("path",{path});if(owner)this.collaboration.attachView(view.editor,owner);}
      for(const view of this.markdownViews.values())if(view.surfaceId===surface.id)view.markdown.call("path",{path});
      const pending=this.fileLoads.get(surface.id);if(pending)void pending.then(()=>this.loadFile(surface));
    }
    const navigationKey=this.workspaceKey(workspace);
    const current=epoch===this.epoch,source=current?this.state:this.workspaceNavigation.get(navigationKey);
    if(!source)return;
    const directories={...(notes?source.notesDirectories:source.directories)},expanded=new Set([...(notes?source.notesExpanded:source.expanded)].map(remap));
    for(const path of Object.keys(directories))if(path===from||path.startsWith(from+"/")){const next=remap(path);directories[next]={...directories[path],path:next};delete directories[path];}
    const oldParent=posix.dirname(from)==="."?"":posix.dirname(from),old=directories[oldParent];
    const movedEntry=old?.entries.find(entry=>entry.name===oldName)||{name,is_dir:Object.keys(directories).some(path=>path===to||path.startsWith(to+"/"))};
    if(old)directories[oldParent]={...old,entries:old.entries.filter(entry=>entry.name!==oldName)};
    directories[directory]={path:directory,entries:[...listing.entries.filter(entry=>oldParent!==directory||entry.name!==oldName),{...movedEntry,name}].sort((a,b)=>Number(b.is_dir)-Number(a.is_dir)||a.name.localeCompare(b.name))};
    let parent=directory;while(parent){expanded.add(parent);parent=posix.dirname(parent);if(parent===".")break;}expanded.add("");
    const patch=notes?{notesDirectories:directories,notesExpanded:expanded}:{directories,expanded};
    if(current){this.update(patch);await Promise.all([...new Set([oldParent,directory,...[...expanded].filter(path=>path===to||path.startsWith(to+"/"))])].map(path=>this.listDirectory(path,epoch,notes)));}
    else this.workspaceNavigation.set(navigationKey,{...source,...patch} as WorkspaceNavigation);
  };
  setWorkspaceSidebar = (value: WorkspaceSidebar | undefined) => {
    const id = this.state.activeWorkspace?.id;
    if (id) this.setUI(workspaceSidebarNavigation(this.state.ui, id, value));
  };
  openFile = async (path: string, rootOverride?: string, origin?: "files") => {
    const workspace = this.state.activeWorkspace;
    const root = rootOverride || workspace?.root_dir;
    if (!workspace || !root || this.closingWorkspaces.has(workspace.id)) return;
    if (isDatabasePath(path)) {
      const connection: DatabaseConnection = { id: `database:${workspace.id}:${root}:${path}`, workspaceId: workspace.id, rootDir: root, path, title: path.split(/[\\/]/).pop() || path };
      const connections = this.databaseConnections(workspace.id);
      const stored = isRecord(this.state.ui.databaseConnections) ? this.state.ui.databaseConnections : {};
      this.setUI({ databaseConnections: { ...stored, [workspace.id]: connections.some(item => item.id === connection.id) ? connections : [...connections, connection] }, ...workspaceSidebarNavigation(this.state.ui, workspace.id, "databases", origin) });
      this.selectDatabaseConnection(connection.id, workspace.id);
      const schema = this.getDatabaseSchema(connection);
      if (this.client.state.phase === "connected") {
        await schema.initialize();
        const table = schema.state.table || schema.state.objects[0]?.name;
        if (table && this.state.activeWorkspace?.id === workspace.id) this.openDatabaseTable(connection, table);
      }
      return;
    }
    const declaredRoot=this.state.workspaces.find(entry=>entry.id===workspace.id)?.root_dir||workspace.root_dir;
    const existing=this.state.surfaces.find(surface=>surface.kind==="file"&&!surface.document&&surface.workspaceId===workspace.id&&surface.path===path&&(surface.rootDir||declaredRoot)===root);
    const baseId=`file:${workspace.id}:${root !== declaredRoot ? root + ":" : ""}${path}`;
    // A host root picker can move the canonical root as well. Retain the old
    // surface identity/dirty bytes and qualify any colliding new-root buffer.
    const occupied=this.state.surfaces.some(surface=>surface.id===baseId);
    const id=existing?.id||(occupied?`file:${workspace.id}:${root}:${path}`:baseId);
    const surface: Surface = { id, kind: "file", title: path.split("/").pop() || path, path, rootDir: root, workspaceId: workspace.id };
    if (existing?.hidden) this.update({ surfaces: this.state.surfaces.map(current => current.id === id ? { ...current, hidden: false } : current) });
    this.addSurface(surface);
    return this.loadFile(surface);
  };
  loadGitEditor = async (root: string, path: string): Promise<NativeEditor> => {
    const workspace = this.state.activeWorkspace;
    if (!workspace || !root) throw new Error("Select a repository workspace first");
    const paths = hostPaths(root);
    const absolute = paths.resolve(root, path), relativePath = paths.relative(root, absolute);
    if (!relativePath || relativePath === ".." || relativePath.startsWith(".." + paths.sep) || paths.isAbsolute(relativePath))
      throw new Error("Git source is outside its repository");
    const review = this.state.surfaces.find(surface => surface.id === this.state.activeSurfaceId && surface.kind === "git");
    let surface = this.state.surfaces.find(surface => surface.kind === "file" && !surface.document && surface.workspaceId === workspace.id
      && paths.normalize(this.hostPath(surface)) === absolute);
    if (!surface) {
      const sourcePath = relativePath.replaceAll("\\", "/");
      const id = `file:${workspace.id}:${root}:${sourcePath}`;
      surface = { id, kind: "file", title: paths.basename(absolute), path: sourcePath, rootDir: root, workspaceId: workspace.id, hidden: true };
      this.update({ surfaces: [...this.state.surfaces, surface] });
    }
    await this.loadFile(surface);
    if (this.state.files[surface.id] === undefined || !this.state.surfaces.some(current => current.id === surface!.id))
      throw new Error("Could not load the Git source document");
    const owner = surface;
    if (review) this.update({ surfaces: this.state.surfaces.map(current => current.id === review.id
      ? { ...current, gitEditorIds: [...new Set([...(current.gitEditorIds || []), owner.id])] } : current) });
    const editor = this.getEditor(owner);
    editor.call("path", { path: absolute });
    return editor;
  };
  saveGitEditor = async (editor: NativeEditor) => {
    const id = [...this.editors].find(([, owner]) => owner === editor)?.[0]
      || [...this.editorViews.values()].find(view => view.editor === editor)?.surfaceId;
    const surface = this.state.surfaces.find(surface => surface.id === id && surface.kind === "file");
    if (!surface) throw new Error("The Git source document was closed");
    await this.saveFile(surface);
  };
  /** Successful host restore changed disk. Refresh only existing, unchanged
   * clean owners; neither a late reply nor Git may overwrite local edits. */
  refreshGitSource = async (root:string,path:string):Promise<void> => {
    const paths=hostPaths(root);
    const absolute=paths.resolve(root,path),relativePath=paths.relative(root,absolute);
    if(!root||!relativePath||relativePath===".."||relativePath.startsWith(".."+paths.sep)||paths.isAbsolute(relativePath))throw new Error("Git source is outside its repository");
    const owners=this.state.surfaces.filter(surface=>surface.kind==="file"&&!surface.document&&paths.normalize(this.hostPath(surface))===absolute&&!this.surfaceDirty(surface.id)).map(surface=>{
      const editor=this.editors.get(surface.id),views=[...this.editorViews.values()].filter(view=>view.surfaceId===surface.id).map(view=>view.editor);
      return{surface,editor,baseline:this.state.files[surface.id],versions:[...(editor?[editor]:[]),...views].map(editor=>({editor,revision:editor.frame.revision}))};
    });
    const first=owners[0];if(!first)return;
    const scope=first.surface.rootDir||this.state.workspaces.find(workspace=>workspace.id===first.surface.workspaceId)?.root_dir;if(!scope)return;
    const reply=await this.run(this.client.request("Files",{ReadFile:{path:first.surface.path!}},scope));
    const file=Schema.decodeUnknownSync(FileSchema)(expectVariant(reply,"FileContent"));
    if(this.disposed)return;
    if(file.bytes.length>2*1024*1024)throw new Error("The GPUIX editor currently supports files up to 2 MB.");
    const bytes=Uint8Array.from(file.bytes);if(bytes.subarray(0,8192).includes(0))throw new Error("Git restored a binary file; its text buffer was not replaced");
    const text=new TextDecoder().decode(bytes),files={...this.state.files};let changed=false;
    this.transaction(()=>{
      for(const owner of owners){
        const current=this.state.surfaces.find(surface=>surface.id===owner.surface.id);
        if(!current||current.path!==owner.surface.path||current.rootDir!==owner.surface.rootDir||this.surfaceDirty(current.id)||this.state.files[current.id]!==owner.baseline||this.editors.get(current.id)!==owner.editor||owner.versions.some(version=>version.editor.frame.revision!==version.revision))continue;
        if(owner.editor&&owner.editor.text()!==text){
          // A Git restore refreshes bytes, not the user's retained view. Bound
          // Yrs splices otherwise transform carets as though this were typing.
          const positions=owner.versions.map(({editor})=>({editor,...editor.frame.cursor}));
          owner.editor.call("replace-source",{text,historyBoundary:true});if(owner.editor.text()===text)owner.editor.call("saved",{revision:owner.editor.frame.revision});
          for(const position of positions)position.editor.call("goto",{line:position.line,col:position.col});
        }
        if (!owner.editor) delete this.recoveredViews[current.id];
        if(files[current.id]!==text){files[current.id]=text;changed=true;}
      }
      if(changed)this.update({files});
    });
  };
  isSurfaceModified = (id: string): boolean => this.surfaceDirty(id);
  private loadFile = (surface: Surface): Promise<void> => {
    const { id, path, rootDir: root } = surface;
    if (!path || !root || this.state.files[id] !== undefined) return Promise.resolve();
    const pending = this.fileLoads.get(id); if (pending) return pending;
    const closeEpoch=this.workspaceCloseEpochs.get(surface.workspaceId)||0;
    const task = Promise.resolve().then(async () => {
      try {
        const reply = await this.run(this.client.request("Files", { ReadFile: { path } }, root));
        const file = Schema.decodeUnknownSync(FileSchema)(expectVariant(reply, "FileContent"));
        // The surface's immutable workspace/root scopes this read. A tab switch
        // must not discard useful bytes, nor may a closed file be resurrected.
        if (this.disposed || closeEpoch!==(this.workspaceCloseEpochs.get(surface.workspaceId)||0) || !this.state.surfaces.some((s) => s.id === id && s.rootDir === root && s.path === path) || this.state.files[id] !== undefined) return;
        if (file.bytes.length > 2 * 1024 * 1024) throw new Error("The GPUIX editor currently supports files up to 2 MB.");
        const bytes = Uint8Array.from(file.bytes);
        if (bytes.subarray(0, 8192).includes(0)) throw new Error("This file is binary; the GPUIX binary viewer is not ported yet.");
        this.update({ files: { ...this.state.files, [id]: new TextDecoder().decode(bytes) } });
      } catch (error) { if (this.state.activeWorkspace?.id === surface.workspaceId) this.report(error); }
      finally { if(this.fileLoads.get(id)===task)this.fileLoads.delete(id); }
    });
    this.fileLoads.set(id, task); return task;
  };
  private addSurface(surface: Surface, split?: SplitAxis) {
    if(this.closingWorkspaces.has(surface.workspaceId))return;
    if(surface.kind!=="welcome")this.disposableWorkspaces.delete(surface.workspaceId);
    this.transaction(() => {
      this.update({ surfaces: this.state.surfaces.some((s) => s.id === surface.id) ? this.state.surfaces : [...this.state.surfaces, surface], ...(surface.workspaceId===this.state.activeWorkspace?.id?{activeSurfaceId:surface.id}:{}) });
      const layout = this.getLayout(surface.workspaceId);
      if (split) layout.split(split, surface);
      else if (layout.frame.leaves.some((leaf) => leaf.surfaceId === surface.id)) layout.focusSurface(surface.id);
      else layout.open(surface);
    });
  }
  selectSurface = (id: string, preserveLeaf = false) => {
    const surface = this.state.surfaces.find((s) => s.id === id);
    if (!surface) return;
    if (surface.page === "database") {
      const connection = this.databaseConnections(surface.workspaceId).find(connection => connection.path === surface.path && connection.rootDir === surface.rootDir);
      if (connection) { this.selectDatabaseConnection(connection.id, surface.workspaceId); if (surface.databaseTable) this.getDatabaseSchema(connection).highlightTable(surface.databaseTable); }
    }
    const changed = this.state.activeSurfaceId !== id;
    this.transaction(() => {
      this.update({ activeSurfaceId: id });
      if (!preserveLeaf) this.getLayout(surface.workspaceId).focusSurface(id);
    });
    const agent=agentSurface(surface);
    if (agent?.kind === "agent" && agent.sessionId) {
      const pending = this.historyRequests.get(agent.sessionId);
      // Layout focus publishes activeSurfaceId before PaneLayoutView calls
      // selectSurface. A restored chat still needs its first history request
      // even when that earlier publication makes `changed` false here.
      const needsHistory = !this.historyCursors.has(agent.sessionId)
        && (!pending || Date.now() - pending.sentAt >= 30000);
      if (changed || needsHistory) this.resumeThread(agent.sessionId);
    }
    if (surface.kind === "file" && this.state.files[id] === undefined) void this.loadFile(surface);
    if(surface.kind==="terminal"||surface.kind==="welcome")this.followMainTerminal();
  };
  retireTerminal=(sessionId:string)=>{
    if(this.disposed)return;
    this.transaction(()=>{
      const ended=isRecord(this.state.ui.unavailableTerminals)?this.state.ui.unavailableTerminals:{};
      const main={...(isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{})};
      const cwds={...(isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{})};
      for(const [workspace,id] of Object.entries(main))if(id===sessionId)delete main[workspace];
      delete cwds[sessionId];
      this.setUI({unavailableTerminals:{...ended,[sessionId]:true},mainTerminals:main,terminalCwds:cwds});
      for(const [surfaceId,owned]of Object.entries(this.homeTerminalOwners()))if(owned===sessionId)this.releaseHomeTerminal(surfaceId,false);
      for(const surface of this.state.surfaces.filter(surface=>surface.kind==="terminal"&&surface.sessionId===sessionId))this.closeSurface(surface.id);
    });
  };
  private surfaceDirty(id:string):boolean{
    const surface=this.state.surfaces.find(surface=>surface.id===id);
    const database = this.databases.get(id);
    if (database) return database.state.pending > 0 || database.state.applying;
    if(surface?.kind==="git"&&Array.isArray(surface.gitEditorIds))return surface.gitEditorIds.some(owner=>owner!==id&&this.surfaceDirty(owner));
    const editor = this.editors.get(id);
    return (editor ? editor.frame.dirty : recoveredEditorState(this.recoveredViews[id])?.dirty)===true||[...this.editorViews.values()].some(view=>view.surfaceId===id&&view.editor.frame.dirty)||[...this.markdownViews.values()].some(view=>view.surfaceId===id&&view.markdown.frame.dirty);
  }
  closeSurface = (id: string, discard = false) => {
    const editor = this.editors.get(id);
    if (this.databases.get(id)?.state.applying) { this.report(new Error("Wait for the database transaction to finish before closing.")); return; }
    if (!discard&&this.surfaceDirty(id)) { this.update({ error: this.databases.has(id) ? "Apply or discard the database changes before closing." : "Save this file before closing it (Ctrl+S)." }); return; }
    const closing = this.state.surfaces.find(surface => surface.id === id);
    if(closing?.kind==="welcome")this.releaseHomeTerminal(id);
    if (closing?.kind === "git") for (const ownerId of closing.gitEditorIds || []) {
      const owner = this.state.surfaces.find(surface => surface.id === ownerId);
      const usedByAnotherReview = this.state.surfaces.some(surface => surface.id !== id && surface.gitEditorIds?.includes(ownerId));
      const visible = [...this.layouts.values()].some(layout => layout.frame.leaves.some(leaf => leaf.surfaceId === ownerId));
      if (owner?.hidden && !usedByAnotherReview && !visible) this.closeSurface(ownerId, discard);
    }
    const targetLayout = closing?.workspaceId;
    this.databases.get(id)?.dispose(); this.databases.delete(id);
    if (targetLayout&&!this.closingWorkspaces.has(targetLayout)) this.getLayout(targetLayout).closeSurface(id);
    for (const [key, view] of this.markdownViews) if (view.surfaceId === id) { view.markdown.dispose(); this.markdownViews.delete(key); delete this.recoveredMarkdownViews[key]; }
    for (const [key, view] of this.editorViews) if (view.surfaceId === id) { this.collaboration.detach(view.editor);view.editor.dispose(); this.editorViews.delete(key); }
    if(editor)this.collaboration.detach(editor);
    delete this.recoveredViews[id];
    for(const key of Object.keys(this.recoveredViews))if(key.startsWith(`view:${id}@`))delete this.recoveredViews[key];
    for(const key of Object.keys(this.recoveredMarkdownViews))if(key.startsWith(`${id}@`))delete this.recoveredMarkdownViews[key];
    if(this.state.files[id]!==undefined){const files={...this.state.files};delete files[id];this.update({files});}
    this.fileLoads.delete(id);
    editor?.dispose(); this.editors.delete(id);
    const target = agentSurface(this.state.surfaces.find((s) => s.id === id));
    if (target?.kind === "terminal") {
      closeTerminalSession(this.client, target.sessionId!);
      const main={...(isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{})};
      const cwds={...(isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{})};
      const pending={...(isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{})};
      for(const [workspace,session]of Object.entries(main))if(session===target.sessionId)delete main[workspace];
      delete cwds[target.sessionId!];delete pending[target.id];
      this.setUI({mainTerminals:main,terminalCwds:cwds,pendingTerminalBindings:pending});
    }
    if (target?.kind === "agent" && target.sessionId&&!this.state.surfaces.map(agentSurface).some(surface=>surface?.id!==id&&surface?.kind==="agent"&&surface.sessionId===target.sessionId)) {this.familyStreams.deactivate(target.sessionId);this.sendAgent({ StopStream: { session_id: target.sessionId } });}
    // Closing a UI tab never kills a daemon-owned shell or deletes a chat.
    const surfaces = this.state.surfaces.filter((s) => s.id !== id);
    this.update({ surfaces, activeSurfaceId: this.state.activeSurfaceId === id ? surfaces.findLast((s) => s.workspaceId === this.state.activeWorkspace?.id && !s.hidden)?.id : this.state.activeSurfaceId });
    if(target?.kind==="agent"&&target.sessionId){
      const root=target.agentRootSessionId||agentFamilyRoot(this.state.ui,target.sessionId);
      if(!surfaces.map(agentSurface).some(surface=>surface?.kind==="agent"&&surface.sessionId&&(surface.agentRootSessionId||agentFamilyRoot(this.state.ui,surface.sessionId))===root)){
        this.closedTranscriptRoots.add(root);
        const history={...this.state.history};
        for(const session of new Set([root,target.sessionId,...Object.keys(history),...this.historyRequests.keys()])){
          if(session!==target.sessionId&&agentFamilyRoot(this.state.ui,session)!==root)continue;
          delete history[session];this.historyUpdates.forget(session);this.queuedHistory.delete(session);this.historyCursors.delete(session);this.completedHistory.delete(session);this.historyEdits.delete(session);
          // Drain an old in-flight read, but never publish it into a reopened tab.
          this.historyRevisions.set(session,(this.historyRevisions.get(session)||0)+1);
        }
        this.update({history});
      }
    }
    if (this.state.activeSurfaceId) this.selectSurface(this.state.activeSurfaceId);
    if(targetLayout&&!this.closingWorkspaces.has(targetLayout)&&targetLayout===this.state.activeWorkspace?.id&&!surfaces.some(surface=>surface.workspaceId===targetLayout&&!surface.hidden))this.ensureWelcome();
    if(closing)for(const listener of this.surfaceClosedListeners){try{listener(closing);}catch(error){this.report(error);}}
  };
  private observeEditor(surfaceId: string, editor: NativeEditor) {
    if (this.observedEditors.has(editor)) return;
    this.observedEditors.add(editor);
    let revision = editor.frame.revision, dirty = editor.frame.dirty, bound = editor.frame.collaborationBound;
    this.cleanups.push(editor.subscribe((frame) => {
      if (revision === frame.revision && dirty === frame.dirty && bound === frame.collaborationBound) return;
      revision = frame.revision; dirty = frame.dirty; bound = frame.collaborationBound;
      if (this.syncingDocuments.has(surfaceId)) return;
      this.syncingDocuments.add(surfaceId);
      try {
        const peers = [this.editors.get(surfaceId), ...[...this.editorViews.values()].filter((v) => v.surfaceId === surfaceId).map((v) => v.editor)];
        for (const peer of peers) if (peer && peer !== editor) peer.syncFrom(editor);
      } finally { this.syncingDocuments.delete(surfaceId); }
    }));
  }
  getEditor(surface: Surface, leaf?: number) {
    let editor = this.editors.get(surface.id);
    if (!editor) {
      const snapshot = this.recoveredViews[surface.id];
      editor = new NativeEditor(this.hostPath(surface), snapshot ? "" : this.state.files[surface.id] || "", this.config.vim);
      if (snapshot) try { editor.restore(snapshot as EditorSnapshot); }
      catch (error) { editor.dispose(); throw error; }
      this.editors.set(surface.id, editor);
    }
    this.observeEditor(surface.id, editor);
    if(!surface.document&&this.state.files[surface.id]!==undefined)this.collaboration.attach(editor,this.hostPath(surface),this.state.workspaces.find(workspace=>workspace.id===surface.workspaceId));
    if (leaf === undefined) return editor;
    const key = `${surface.id}@${leaf}`;
    let view = this.editorViews.get(key);
    if (!view) {
      const clone = editor.cloneView();
      const saved = this.recoveredViews["view:" + key];
      if (saved) try { clone.call("restore", { snapshot: saved }); clone.syncFrom(editor); } catch {}
      view = { surfaceId: surface.id, editor: clone }; this.editorViews.set(key, view); this.observeEditor(surface.id, clone);
    }
    this.collaboration.attachView(view.editor,editor);
    return view.editor;
  }
  getMarkdown(surface: Surface, leaf?: number) {
    const key = leaf === undefined ? surface.id : `${surface.id}@${leaf}`;
    let view = this.markdownViews.get(key);
    if (!view) {
      const markdown = new NativeMarkdown(this.getEditor(surface, leaf), this.config, this.hostPath(surface), async path => {
        const root = surface.rootDir || this.state.workspaces.find(w => w.id === surface.workspaceId)?.root_dir || "";
        const paths = hostPaths(root);
        const relativePath = paths.relative(root, path);
        if (root && relativePath !== ".." && !relativePath.startsWith(".." + paths.sep) && !paths.isAbsolute(relativePath)) {
          const reply = await this.run(this.client.request("Files", { ReadFile: { path: relativePath.replaceAll("\\", "/") } }, root));
          return Uint8Array.from(Schema.decodeUnknownSync(FileSchema)(expectVariant(reply, "FileContent")).bytes);
        }
        return new Uint8Array(await Bun.file(path).arrayBuffer());
      });
      if (this.recoveredMarkdownViews[key]) try { markdown.restore(this.recoveredMarkdownViews[key], true); } catch (error) { this.report(error); }
      view = { surfaceId: surface.id, markdown }; this.markdownViews.set(key, view);
    }
    return view.markdown;
  }
  hostPath(surface: Surface) {
    const root = surface.rootDir || this.state.workspaces.find((w) => w.id === surface.workspaceId)?.root_dir || "";
    const paths = hostPaths(root);
    return paths.resolve(root, surface.path || "");
  }
  private scopedEditorPath(path: string) {
    const workspace = this.state.activeWorkspace, root = workspace?.root_dir;
    if (!workspace || !root) throw new Error("Select a workspace first");
    const paths = hostPaths(root);
    const relativePath = paths.isAbsolute(path) ? paths.relative(root, path) : path;
    if (relativePath === ".." || relativePath.startsWith(".." + paths.sep) || paths.isAbsolute(relativePath)) throw new Error("This location is outside the current workspace");
    return { workspace, path: relativePath.replaceAll("\\", "/"), root };
  }
  openLocation = async (path: string, line = 0, col = 0) => {
    const target = this.scopedEditorPath(path);
    await this.openFile(target.path, target.root);
    if (this.state.activeWorkspace?.id !== target.workspace.id) return;
    const surface = this.state.surfaces.find((s) => s.workspaceId === target.workspace.id && s.path === target.path && s.rootDir === target.root);
    if (!surface || this.state.files[surface.id] === undefined) return;
    this.getEditor(surface, this.getLayout().frame.focusedLeaf || undefined).call("goto", { line, col });
  };
  applyEditorEdits = async (path: string, edits: readonly unknown[]) => {
    const target = this.scopedEditorPath(path), layout = this.getLayout(), beforeLeaf = layout.frame.focusedLeaf;
    let surface = this.state.surfaces.find((s) => s.workspaceId === target.workspace.id && s.path === target.path && s.rootDir === target.root);
    if (!surface) {
      await this.openFile(target.path, target.root);
      surface = this.state.surfaces.find((s) => s.workspaceId === target.workspace.id && s.path === target.path && s.rootDir === target.root);
    } else await this.loadFile(surface);
    if (!surface || this.state.files[surface.id] === undefined) throw new Error("Could not load the file for this edit");
    this.getEditor(surface).call("apply-edits", { edits });
    if (beforeLeaf != null && layout.frame.leaves.some((l) => l.leaf === beforeLeaf)) layout.focus(beforeLeaf);
  };
  saveFile = async (surface: Surface) => {
    surface = this.state.surfaces.find((current) => current.id === surface.id) || surface;
    const editor = this.editors.get(surface.id) || (this.recoveredViews[surface.id] ? this.getEditor(surface) : undefined);
    const workspace = this.state.workspaces.find((w) => w.id === surface.workspaceId);
    if (!editor || !surface.path || !workspace) return;
    const root = surface.rootDir || workspace.root_dir;
    const revision = editor.frame.revision;
    const text = editor.text();
    if (surface.document?.kind === "config") {
      if (!surface.document.writable) throw new Error("This daemon config document is read-only");
      const reply = expectVariant(await this.run(this.client.request("Config", { SaveConfigDocument: { content: text, expected_revision: surface.document.revision } })), "ConfigDocumentSaved");
      if (!isRecord(reply) || !isRecord(reply.document) || typeof reply.document.revision !== "string") throw new Error("Invalid saved config document reply");
      const savedDocument = reply.document;
      editor.call("saved", { revision });
      this.update({ files: { ...this.state.files, [surface.id]: text }, surfaces: this.state.surfaces.map((s) => s.id === surface.id ? { ...s, document: { kind: "config", revision: String(savedDocument.revision), writable: savedDocument.writable === true } } : s) });
      return;
    }
    if (!root) return;
    if(await this.collaboration.save(editor)){
      if(this.client.state.phase==="connected")this.client.send("Editor",{DidSave:{path:this.hostPath(surface)}},root);
      this.update({files:{...this.state.files,[surface.id]:editor.text()}});return;
    }
    expectVariant(await this.run(this.client.request("Files", { WriteFile: { path: surface.path, bytes: [...new TextEncoder().encode(text)] } }, root)), "FileWritten");
    editor.call("saved", { revision });
    if (this.client.state.phase === "connected") this.client.send("Editor", { DidSave: { path: this.hostPath(surface) } }, root);
    this.update({ files: { ...this.state.files, [surface.id]: text } });
  };
  /** Reuse the workspace Agent home for callers explicitly opening that page. */
  openAgentHome = () => {
    const workspace = this.state.activeWorkspace;
    if (!workspace) return;
    const existing = this.state.surfaces.find((s) => s.kind === "agent" && !s.sessionId && s.workspaceId === workspace.id);
    if (existing) { this.selectSurface(existing.id); return existing.id; }
    this.addSurface({ id: `agent-home:${workspace.id}`, kind: "agent", title: "Neoism", workspaceId: workspace.id });
  };
  /** Alt+A creates an independent local Agent tab; no server thread until send. */
  newAgentHome = () => {
    const workspace=this.state.activeWorkspace;if(!workspace)return;
    const surface:Surface={id:`agent-home:${workspace.id}:${crypto.randomUUID()}`,kind:"agent",title:"Neoism",workspaceId:workspace.id};
    this.addSurface(surface);return surface;
  };
  /** `adoptHome`: a conversation started from the Neoism page becomes that tab. */
  /** Opens (or focuses) the chat in the active workspace; returns its surface id. */
  openThread = (thread: Thread, adoptHome = false, familyRoot?:string, pointer=false): string | undefined => {
    const workspace = this.state.activeWorkspace;
    if (!workspace) return;
    const root=familyRoot||agentFamilyRoot(this.state.ui,thread.session_id);
    if(root!==thread.session_id){
      const parents=isRecord(this.state.ui.agentParentSessions)?this.state.ui.agentParentSessions:{};
      const roots=isRecord(this.state.ui.agentFamilyRoots)?this.state.ui.agentFamilyRoots:{};
      if(roots[thread.session_id]!==root)this.update({ui:{...this.state.ui,agentFamilyRoots:{...roots,[thread.session_id]:root},agentParentSessions:{...parents,[thread.session_id]:parents[thread.session_id]||root}}});
    }
    this.sessionWorkspaces.set(thread.session_id, workspace.id);
    // Reuse a tab only in this workspace: a chat already open in another
    // workspace tab is opened here too, never focused invisibly over there.
    const existing=this.state.surfaces.find(s=>{const agent=agentSurface(s);return agent?.kind==="agent"&&agent.sessionId&&s.workspaceId===workspace.id&&(s.agentRootSessionId||agentFamilyRoot(this.state.ui,agent.sessionId))===root;});
    if(existing){
      // Desktop catalog_activation_target: pointer on the viewed child goes
      // back to its family root; keyboard Enter keeps the selected transcript.
      const current=agentSurface(existing)!;
      const target=pointer&&current.sessionId===thread.session_id&&thread.session_id!==root?root:thread.session_id;
      const title=this.state.threads.find(thread=>thread.session_id===root)?.title||existing.title;
      if(current.sessionId!==target||existing.agentRootSessionId!==root||existing.kind==="welcome"&&existing.homeMode!=="agent"){const updated:Surface=existing.kind==="welcome"?{...existing,homeAgentSessionId:target,agentRootSessionId:root,homeMode:"agent"}:{...existing,sessionId:target,agentRootSessionId:root,title};this.update({surfaces:this.state.surfaces.map(s=>s.id===existing.id?updated:s)});this.getLayout(workspace.id).rename(updated);}
      this.selectSurface(existing.id);if(current.sessionId!==target)this.resumeThread(target);return existing.id;
    }
    const home = adoptHome ? this.state.surfaces.find((s) => s.id === this.state.activeSurfaceId && (s.kind === "agent" && !s.sessionId || s.kind==="welcome"&&s.homeMode==="agent") && s.workspaceId === workspace.id) : undefined;
    if (home) {
      const adopted:Surface = home.kind==="welcome"?{...home,homeAgentSessionId:thread.session_id,agentRootSessionId:root}:{ ...home, sessionId: thread.session_id, agentRootSessionId:root,title: thread.title };
      this.update({ surfaces: this.state.surfaces.map((s) => s.id === home.id ? adopted : s) });
      this.getLayout(workspace.id).rename(adopted);
      this.resumeThread(thread.session_id);
      return home.id;
    }
    const plain = `agent:${thread.session_id}`;
    const id = this.state.surfaces.some((s) => s.id === plain) ? `agent:${workspace.id}:${thread.session_id}` : plain;
    this.addSurface({ id, kind: "agent", title: thread.title, sessionId: thread.session_id, agentRootSessionId:root,workspaceId: workspace.id });
    this.resumeThread(thread.session_id);
    return id;
  };
  agentFamily = (sessionId:string) => agentFamily(this.state.ui,sessionId);
  private resumeThread(sessionId: string) {
    this.closedTranscriptRoots.delete(agentFamilyRoot(this.state.ui,sessionId));
    for(const stopped of this.familyStreams.activate(sessionId))this.sendAgent({StopStream:{session_id:stopped}});
    this.sendAgent({ SwitchThread: { session_id: sessionId } });
    this.sendAgent({ GetHistory: { session_id: sessionId, cursor: null, limit: 200 } });
    this.sendAgent({ ResumeStream: { session_id: sessionId } });
    this.preloadFamilyHistory();
  }
  private preloadFamilyHistory(){
    if(this.disposed||this.client.state.phase!=="connected")return;
    const now=Date.now();let available=4-[...this.historyRequests.values()].filter(request=>now-request.sentAt<30000).length;
    // Warm only open families, not the entire sidebar catalog. Reuse the head
    // cursor as the hydrated marker; live-only rows still need their baseline.
    const roots=new Set<string>();
    for(const source of this.state.surfaces){
      const surface=agentSurface(source)!;
      if(surface.kind!=="agent"||!surface.sessionId)continue;
      const family=this.agentFamily(surface.agentRootSessionId||surface.sessionId);
      if(roots.has(family.root)||this.closedTranscriptRoots.has(family.root))continue;
      roots.add(family.root);
      for(const row of family.roster){
        if(available<=0)return;
        if(this.historyCursors.has(row.id)||now-(this.historyRequests.get(row.id)?.sentAt??0)<30000)continue;
        this.sessionWorkspaces.set(row.id,surface.workspaceId);
        this.sendAgent({GetHistory:{session_id:row.id,cursor:null,limit:200}});available--;
      }
    }
  }
  private familyStreams=new FamilyStreamOwnership(id=>agentFamilyRoot(this.state.ui,id));
  private completedTranscript=new CompletedTranscriptRefresh();
  private completedHistory=new Set<string>();
  private completedHistoryTimer:ReturnType<typeof setTimeout>|undefined;
  private refreshCompletedHistory(sessionId:string){
    if(this.closedTranscriptRoots.has(agentFamilyRoot(this.state.ui,sessionId)))return;
    this.completedHistory.add(sessionId);if(this.completedHistoryTimer!==undefined)return;
    this.completedHistoryTimer=setTimeout(()=>{this.completedHistoryTimer=undefined;const sessions=[...this.completedHistory];this.completedHistory.clear();if(!this.disposed&&this.client.state.phase==="connected")for(const session_id of sessions)if(!this.closedTranscriptRoots.has(agentFamilyRoot(this.state.ui,session_id)))this.sendAgent({GetHistory:{session_id,cursor:null,limit:200}});},0);
  }
  private requestedAgentCatalogScopes=new Map<string,string|undefined>();
  private agentCatalogStream?:{workspaceId:string;directory?:string;abort:AbortController};
  private ensureAgentCatalogStream(workspaceId:string,directory?:string){
    // All-project discovery already covers narrower page reads in this window.
    if(this.requestedAgentCatalogScopes.has(workspaceId)&&this.requestedAgentCatalogScopes.get(workspaceId)===undefined)directory=undefined;
    this.requestedAgentCatalogScopes.set(workspaceId,directory);
    const previous=this.agentCatalogStream;
    if(previous?.workspaceId===workspaceId&&previous.directory===directory&&!previous.abort.signal.aborted)return;
    previous?.abort.abort();const stream={workspaceId,directory,abort:new AbortController()};this.agentCatalogStream=stream;
    void(async()=>{try{
      for await(const event of this.agentSdk.catalogEvents(workspaceId,{directory,signal:stream.abort.signal,onReset:()=>{if(this.agentCatalogStream===stream&&!this.disposed)this.enqueueReceive({family:"Agent",requestId:0,message:{CatalogReset:{directory:directory??null}}});}})){
        if(this.disposed||this.agentCatalogStream!==stream)break;
        const message=catalogAgentMessage(event);if(message)this.enqueueReceive({family:"Agent",requestId:0,message});
      }
    }catch(error){if(!this.disposed&&!stream.abort.signal.aborted&&this.agentCatalogStream===stream)this.report(error);}finally{if(this.agentCatalogStream===stream)this.agentCatalogStream=undefined;}})();
  }
  private catalogRead=(message:AgentClientMessage):boolean=>{
    const pair=variant(message),workspace=this.state.activeWorkspace;if(!pair||!workspace||!isRecord(pair[1]))return false;
    const[kind,body]=pair;if(!["ListThreads","ListSessionProjects","ListProjectSessions"].includes(kind))return false;
    const directory=typeof body.directory==="string"&&body.directory?body.directory:undefined;
    if(kind!=="ListProjectSessions")this.ensureAgentCatalogStream(workspace.id,directory);
    const emit=(message:unknown)=>{if(!this.disposed)this.enqueueReceive({family:"Agent",requestId:0,message});};
    void(async()=>{try{
      if(kind==="ListSessionProjects"){
        const result=await this.agentSdk.catalogProjects(workspace.id);
        emit({SessionProjects:{catalog_identity:body.catalog_identity,generation:body.generation,items:result.items,pinnedSessions:result.pinnedSessions}});
      }else{
        const page=await this.agentSdk.catalogSessions(workspace.id,{directory,roots:true,limit:kind==="ListProjectSessions"?5:typeof body.limit==="number"?body.limit:50,cursor:typeof body.cursor==="string"?body.cursor:undefined});
        const fields={threads:page.items.map(catalogThread),requested_cursor:body.cursor??null,next_cursor:page.cursor?.next??null};
        emit(kind==="ListThreads"?{ThreadList:fields}:{ProjectSessionPage:{...fields,catalog_identity:body.catalog_identity,directory,generation:body.generation}});
      }
    }catch(error){if(this.disposed)return;const fields={message:error instanceof Error?error.message:"Catalog unavailable"};
      emit(kind==="ListSessionProjects"?{SessionProjectsFailed:{...fields,catalog_identity:body.catalog_identity,generation:body.generation}}:kind==="ListProjectSessions"?{ProjectSessionPageFailed:{...fields,catalog_identity:body.catalog_identity,directory,requested_cursor:body.cursor??null,generation:body.generation}}:{ThreadListFailed:fields});
    }})();return true;
  };
  readonly historyCursors = new Map<string, string | null>();
  private historyRequests = new Map<string, { older: boolean; replace: boolean; revision: number; sentAt: number }>();
  private queuedHistory = new Map<string, {message: AgentClientMessage; replace: boolean}[]>();
  private historyRevisions = new Map<string,number>();
  private historyEdits = new Map<string,{base:EditableHistory;pending:HistoryEdit[];tail:Promise<void>}>();
  private publishHistoryEdit(sessionId:string,state:EditableHistory){
    const pending=(this.state.history[sessionId]||[]).filter(row=>this.optimisticPrompts.get(row.id)===sessionId&&!state.messages.some(message=>message.id===row.id));
    const messages=[...state.messages,...pending];
    this.update({history:{...this.state.history,[sessionId]:messages}});
    const message={HistoryChunk:{session_id:sessionId,messages,next_cursor:this.historyCursors.get(sessionId)??null}};
    for(const listener of this.agentEventListeners){try{listener(message,"replace");}catch(error){this.report(error);}}
  }
  editSessionHistory = (sessionId:string,action:HistoryEdit,mutate:()=>Promise<unknown>):Promise<void> => {
    this.flushHistoryUpdates();
    let edit=this.historyEdits.get(sessionId);
    if(!edit){edit={base:{messages:this.state.history[sessionId]||[],redo:[]},pending:[],tail:Promise.resolve()};this.historyEdits.set(sessionId,edit);}
    if(!edit.pending.length)edit.base={...edit.base,messages:this.state.history[sessionId]||[]};
    const current=edit;
    current.pending.push(action);
    this.historyRevisions.set(sessionId,(this.historyRevisions.get(sessionId)||0)+1);
    this.queuedHistory.delete(sessionId);
    this.publishHistoryEdit(sessionId,current.pending.reduce(stepHistoryEdit,current.base));
    // Project every keystroke immediately, but preserve server mutation order.
    const task=current.tail.catch(()=>{}).then(async()=>{
      if(this.disposed)return;
      try{await mutate();current.base=stepHistoryEdit(current.base,action);}
      finally{
        current.pending.shift();
        if(!this.disposed){
          this.publishHistoryEdit(sessionId,current.pending.reduce(stepHistoryEdit,current.base));
          if(!current.pending.length)this.refreshSessionHistory(sessionId);
        }
      }
    });
    current.tail=task;return task;
  };
  refreshSessionHistory = (sessionId: string) => {
    // Undo/redo invalidates both retained rows and queued pagination cursors.
    // An in-flight read still drains before the authoritative replacement.
    this.queuedHistory.delete(sessionId);
    this.sendAgent({GetHistory:{session_id:sessionId,cursor:null,limit:200}}, true);
  };
  private goalVersions = new Map<string, number>();
  private applySessionGoal(sessionId: string, info: Record<string, unknown>) {
    const version=isRecord(info.time)&&typeof info.time.updated==="number"?info.time.updated:0;
    // Include cleared goals in the watermark: a delayed initial read must not resurrect them.
    if(version<(this.goalVersions.get(sessionId)??0))return;
    const value=info.goal;
    const goal:SessionGoalView|null=isRecord(value)&&typeof value.text==="string"?{
      text:value.text,paused:value.paused===true,status:value.status==="complete"||value.status==="blocked"?value.status:"active",summary:typeof value.summary==="string"?value.summary:"",
    }:null;
    this.goalVersions.set(sessionId,version);
    this.update({goals:{...this.state.goals,[sessionId]:goal}});
  }
  refreshSessionGoal = async (sessionId: string) => {
    const info=await this.sdkForSession(sessionId).sessions.get(sessionId);
    if(!this.disposed)this.applySessionGoal(sessionId,info);
  };
  sendAgent = (message: AgentClientMessage, replaceHistory = false) => {
    if(this.catalogRead(message))return;
    // Legacy Agent envelopes carry the most recent request id, not the
    // request that produced a reply. Serialize history reads per session.
    const history = typeof message === "object" && "GetHistory" in message ? message.GetHistory : undefined;
    if (history) {
      const pending = this.historyRequests.get(history.session_id);
      if (pending && Date.now() - pending.sentAt < 30000) {
        const queue = this.queuedHistory.get(history.session_id) || [];
        if (!queue.some(item => item.replace === replaceHistory && typeof item.message === "object" && "GetHistory" in item.message && item.message.GetHistory.cursor === history.cursor)) queue.push({message,replace:replaceHistory});
        this.queuedHistory.set(history.session_id, queue);
        return;
      }
    }
    try {
      this.client.send("Agent", message);
      if (history) this.historyRequests.set(history.session_id, { older: !!history.cursor, replace: replaceHistory, revision:this.historyRevisions.get(history.session_id)||0, sentAt: Date.now() });
    } catch (error) { this.report(error); }
  };
  selection = (sessionId?: string): ProviderState => {
    const thread = sessionId ? this.state.threads.find((thread) => thread.session_id === sessionId) : undefined;
    const draft = this.state.drafts[this.state.activeWorkspace?.id || ""];
    const workspace=this.state.activeWorkspace,key=JSON.stringify([workspace?.id,workspace?.root_dir||""]);
    return { agent: "build", ...(!sessionId ? {...this.state.agentDefaults?.[key],...draft} : { agent: thread?.agent || "build", model: thread?.model }), ...(sessionId ? this.state.providerStates[sessionId] : {}) };
  };
  /** Accounts (profiles) connected for one provider, like the desktop's
   * `/v2/providers/{id}/connections`; one provider can hold several logins. */
  providerConnections = async (providerId: string) => {
    const workspace = this.state.activeWorkspace; if (!workspace) return [];
    const rows = await this.agentSdk.forWorkspace(workspace.id).catalog.providers.connections(providerId, workspace.id) as unknown;
    return (Array.isArray(rows) ? rows : []).filter(isRecord).flatMap((row) => typeof row.connectionId === "string" && typeof row.label === "string"
      ? [{ id: row.connectionId, label: row.label, authType: typeof row.authType === "string" ? row.authType : "", isDefault: row.isDefault === true }] : []);
  };
  setSelection = (field: "agent" | "model" | "thinking", value: string, sessionId?: string, connectionId?: string) => {
    const previous=this.selection(sessionId),selection={...previous,[field]:value};
    if(field==="model"){
      const provider=(model:string|null|undefined)=>{const slash=model?.indexOf("/")??-1;return model&&slash>0?model.slice(0,slash):undefined;};
      if(provider(previous.model)!==provider(value)){
        // A connection belongs to one provider. Changing providers must not
        // send that account with the newly selected provider's model.
        delete selection.connection_id;
        selection.provider_id=provider(value);
      }
      if(connectionId)selection.connection_id=connectionId;
    }
    if (!sessionId) {
      const id = this.state.activeWorkspace?.id; if (!id) return;
      this.update({ drafts: { ...this.state.drafts, [id]: selection } });
      return;
    }
    this.update({ providerStates: { ...this.state.providerStates, [sessionId]: selection } });
    const task = (this.selectionTasks.get(sessionId) || Promise.resolve()).then(async () => {
      // The original Agent command resolves the conversation's own scope.
      // A legacy chat shown by the master catalog need not belong to the
      // current UI workspace's HTTP tenant.
      if(field==="agent"){this.sendAgent({SetAgent:{session_id:sessionId,agent:value}});return;}
      await this.sdkForSession(sessionId).sessions.update(sessionId, { agent: selection.agent || undefined, model: sdkModel(selection) });
    }).catch(this.report);
    this.selectionTasks.set(sessionId, task);
    void task.finally(() => { if (this.selectionTasks.get(sessionId) === task) this.selectionTasks.delete(sessionId); });
  };
  private sdkForSession(sessionId: string) {
    const workspaceId = this.sessionWorkspaces.get(sessionId) || this.state.surfaces.find((s) => s.sessionId === sessionId)?.workspaceId || this.state.activeWorkspace?.id;
    if (!workspaceId) throw new Error("Select a workspace first");
    this.sessionWorkspaces.set(sessionId, workspaceId);
    return this.agentSdk.forSession(workspaceId,sessionId);
  }
  /** Plain SDK list does not create a second catalog SSE subscription. The
   * visible original sidebar owns its directory/all-projects live catalog. */
  private refreshThreadCatalog=async(workspace:Workspace,epoch:number)=>{
    try{
      const page=await this.run(Effect.tryPromise({try:()=>this.agentSdk.catalogSessions(workspace.id,{directory:workspace.root_dir||undefined,roots:true,limit:50}),catch:error=>error}));
      if(epoch!==this.epoch||this.state.activeWorkspace?.id!==workspace.id)return;
      const threads=Schema.decodeUnknownSync(Schema.Array(ThreadSchema))(page.items.map(sdkThread)).filter(thread=>thread.directory===workspace.root_dir);
      this.update({threads});
    }catch(error){if(epoch===this.epoch)this.report(error);}
  };
  private catalogFlights=new Map<string,Promise<void>>();
  private catalogFetchedAt=new Map<string,number>();
  private catalogScope="";
  private catalogGeneration=0;
  private catalogFailures=0;
  private catalogDirectory(){
    const sessionId=this.state.surfaces.find(surface=>surface.id===this.state.activeSurfaceId)?.sessionId;
    return this.state.threads.find(thread=>thread.session_id===sessionId)?.directory||this.state.activeWorkspace?.root_dir||undefined;
  }
  refreshCatalogs = (options:{maxAgeMs?:number}={}) => {
    const workspace=this.state.activeWorkspace;if(!workspace)return Promise.resolve();
    const directory=this.catalogDirectory(),key=JSON.stringify([this.epoch,workspace.id,directory]);
    const pending=this.catalogFlights.get(key);if(pending&&this.catalogScope===key)return pending;
    if(this.catalogScope===key&&options.maxAgeMs&&Date.now()-(this.catalogFetchedAt.get(key)||0)<options.maxAgeMs)return Promise.resolve();
    if(this.catalogScope!==key){
      this.catalogFailures=0;
      this.update({providers:[],providerCatalog:undefined,catalogRetryAfterMs:undefined});
    }
    this.catalogScope=key;
    const task=this.loadCatalogs(directory).then(current=>{if(current&&!this.state.catalogsError)this.catalogFetchedAt.set(key,Date.now());}).finally(()=>{if(this.catalogFlights.get(key)===task)this.catalogFlights.delete(key);});
    this.catalogFlights.set(key,task);return task;
  };
  private loadCatalogs = async (directory:string|undefined) => {
    const epoch = this.epoch, workspace = this.state.activeWorkspace;
    if (!workspace) return;
    const generation=++this.catalogGeneration,isCurrent=()=>epoch===this.epoch&&generation===this.catalogGeneration&&directory===this.catalogDirectory();
    const sdk = this.agentSdk.forWorkspace(workspace.id);
    // Provider accounts retain workspace scope. Project metadata follows the
    // actual cwd through daemon-verified desktop access, never the old root.
    const root=this.state.workspaces.find(entry=>entry.id===workspace.id)?.root_dir;
    const paths=hostPaths(root||"");
    const child=root&&directory?paths.relative(root,directory):undefined;
    const providerDirectory=child!==undefined&&child!==".."&&!child.startsWith(`..${paths.sep}`)&&!paths.isAbsolute(child)?directory:undefined;
    const projectSdk=this.agentSdk.forDirectory(workspace.id);
    const key=JSON.stringify([workspace.id,directory||""]);
    const hadReadyModel=!!this.state.agentDefaults?.[key]?.model;
    let config:unknown={},providers:unknown={},configReady=false,providersReady=false,providersFailed=false;
    const publishDefaults=()=>{
      if(!isCurrent())return;
      // Config is authoritative and arrives on its own critical path. Provider
      // discovery only supplies the original fallback when no model is set.
      if(!configReady)return;
      const resolved=coreCall<ProviderState>("agentcomposer.defaults",{config,providers});
      if(hadReadyModel&&!resolved.model&&(!providersReady||providersFailed))return;
      const selection:ProviderState={...(resolved.agent?{agent:resolved.agent}:{}),...(resolved.model?{model:resolved.model}:{}),...(resolved.thinking?{thinking:resolved.thinking}:{})};
      this.update({agentDefaults:{...this.state.agentDefaults,[key]:selection},agentDefaultsLoading:!resolved.model&&!providersReady});
    };
    this.update({ catalogsLoading: true, catalogsError: undefined,agentDefaultsLoading:!hadReadyModel });
    const catalog = async (load: (signal:AbortSignal) => Promise<Partial<AppState>>,failed?:()=>void) => {
      try {
        const patch = await this.run(Effect.timeout(
          Effect.tryPromise({ try: load, catch: (error) => error }),
          "20 seconds",
        ));
        if (isCurrent()) this.update(patch);
      } catch (error) { if (isCurrent()) {failed?.();this.update({ catalogsError: error instanceof Error ? error.message : "Catalog unavailable" });} }
    };
    await Promise.all([
      catalog(async(signal)=>{const result=await(await projectSdk).operations.request("v2.config.defaults",{query:{directory},signal});if(signal.aborted)return{};config=result;configReady=true;publishDefaults();return{};}),
      catalog(async (signal) => {
        const result = await sdk.operations.request("v2.providers.configured",{query:{directory:providerDirectory},signal});
        if(signal.aborted)return{};
        providers=result;providersReady=true;publishDefaults();
        const catalog = result.providers.map((p) => ({ id: p.id, name: p.name || p.id,
          models: Object.entries(p.models).map(([key, model]) => ({ id: model.id || key, name: model.name || model.id || key,
            aliases: [key, ...(isRecord(model.api) && typeof model.api.id === "string" ? [model.api.id] : [])],
            context_limit: isRecord(model.limit) && typeof model.limit.context === "number" && model.limit.context > 0 ? model.limit.context : undefined })) }));
        return { providers: Schema.decodeUnknownSync(Schema.Array(ProviderSchema))(catalog), providerCatalog: result };
      },()=>{providersReady=true;providersFailed=true;publishDefaults();}),
      catalog(async (signal) => ({ agents: Schema.decodeUnknownSync(Schema.Array(AgentSchema))((await(await projectSdk).operations.request("v2.agents.list",{query:{directory},signal})).filter((a) => !a.hidden && a.mode !== "subagent").map((a) => ({ ...a, description: a.description || "" }))) })),
      catalog(async (signal) => ({ skills: Schema.decodeUnknownSync(Schema.Array(SkillSchema))((await(await projectSdk).operations.request("v2.skills.list",{query:{directory},signal})).map((a) => ({ ...a, description: a.description || "" }))) })),
    ]);
    if (isCurrent()) {
      this.catalogFailures=this.state.catalogsError?Math.min(6,this.catalogFailures+1):0;
      this.update({catalogsLoading:false,agentDefaultsLoading:false,
        catalogRetryAfterMs:this.catalogFailures?Math.min(60_000,1000*2**this.catalogFailures):undefined});
    }
    return isCurrent();
  };
  private async createThread(selection: ProviderState, optimistic?:{sessionId:string;message:HistoryMessage}) {
    const workspace = this.state.activeWorkspace;
    if (!workspace) throw new Error("Select a workspace first");
    const sourceId=this.state.activeSurfaceId;
    const home=this.state.surfaces.find(surface=>surface.id===sourceId&&(surface.kind==="agent"&&!surface.sessionId||surface.kind==="welcome"&&surface.homeMode==="agent"&&!surface.homeAgentSessionId)&&surface.workspaceId===workspace.id);
    const homeDraftKey=home?this.composerDraftKey(undefined,home.id):undefined;
    const declaredRoot=this.state.workspaces.find(entry=>entry.id===workspace.id)?.root_dir;
    const paths=hostPaths(declaredRoot||""),relative=declaredRoot&&workspace.root_dir?paths.relative(declaredRoot,workspace.root_dir):undefined;
    const outsideWorkspace=relative!==undefined&&(relative===".."||relative.startsWith(`..${paths.sep}`)||paths.isAbsolute(relative));
    const session = await this.agentSdk.createSession(workspace.id,{
      directory: workspace.root_dir || undefined, agent: selection.agent || undefined, model: sdkModel(selection),
    },outsideWorkspace);
    const thread = Schema.decodeUnknownSync(ThreadSchema)({ session_id: session.id, directory: workspace.root_dir, title: this.agentTitles.get(session.id) || session.title || "New conversation" });
    this.sessionWorkspaces.set(thread.session_id, workspace.id);
    // Creating with defaults lets the Agent choose a concrete model. Retain
    // that canonical selection so later variant changes carry its model too.
    const ref=sdkModel(selection)||session.model;
    const resolved:ProviderState={...selection,agent:selection.agent||session.agent,
      model:selection.model||(ref?`${ref.providerId}/${ref.id}`:undefined),
      provider_id:selection.provider_id||ref?.providerId,
      thinking:selection.thinking||session.model?.variant,
      connection_id:selection.connection_id||session.model?.connectionId};
    this.update({ providerStates: { ...this.state.providerStates, [thread.session_id]: resolved } });
    if (this.state.activeWorkspace?.id === workspace.id) {
      this.update({ threads: [thread, ...this.state.threads.filter((t) => t.session_id !== thread.session_id)] });
    }
    // A slow create belongs to the home that submitted it. Opening another
    // Agent tab or workspace must never adopt or select that newer page.
    const current=home&&this.state.surfaces.find(surface=>surface.id===home.id);
    if(home&&current&&!agentSurface(current)?.sessionId){
      const adopted:Surface=current.kind==="welcome"?{...current,homeAgentSessionId:thread.session_id}:{...current,sessionId:thread.session_id,title:thread.title};
      this.transaction(()=>{
        const draft=homeDraftKey?this.state.composerDrafts[homeDraftKey]:undefined;
        const pendingAgentSessions={...this.state.pendingAgentSessions};delete pendingAgentSessions[home.id];
        // This is a newly created session, so its empty history is already
        // known (also for queue-only sends). Do not briefly show a cold chat.
        const history={...this.state.history,[thread.session_id]:this.state.history[thread.session_id]||[]};
        const streaming={...this.state.streaming};
        if(optimistic){history[thread.session_id]=[optimistic.message,...(history[thread.session_id]||[]).filter(row=>row.id!==optimistic.message.id)];delete history[optimistic.sessionId];this.optimisticPrompts.set(optimistic.message.id,thread.session_id);
          if(this.promptArrivals.get(optimistic.message.id)===optimistic.sessionId)this.promptArrivals.set(optimistic.message.id,thread.session_id);
          for(const listener of this.agentEventListeners){try{listener({LocalSessionAdopted:{from:optimistic.sessionId,to:thread.session_id}});}catch(error){this.report(error);}}
          if(this.promptRuntimeRoots.get(optimistic.sessionId)===optimistic.message.id){this.promptRuntimeRoots.delete(optimistic.sessionId);this.promptRuntimeRoots.set(thread.session_id,optimistic.message.id);}
          if(this.promptActivity.get(optimistic.message.id)===optimistic.sessionId){this.promptActivity.set(optimistic.message.id,thread.session_id);streaming[thread.session_id]=streaming[optimistic.sessionId];delete streaming[optimistic.sessionId];}}
        this.update({pendingAgentSessions,history,streaming,surfaces:this.state.surfaces.map(surface=>surface.id===home.id?adopted:surface),
          ...(draft?{composerDrafts:{...this.state.composerDrafts,[thread.session_id]:draft}}:{})});
        this.getLayout(workspace.id).rename(adopted);
      });
      if(this.state.activeSurfaceId===home.id)this.resumeThread(thread.session_id);
    }else if(!home&&this.state.activeWorkspace?.id===workspace.id&&this.state.activeSurfaceId===sourceId)this.openThread(thread);
    return thread;
  }
  newThread = async () => { try { await this.createThread(this.selection()); } catch (error) { this.report(error); } };
  private optimisticPrompts=new Map<string,string>();
  private promptArrivals=new Map<string,string>();
  /** Only unpainted live echoes can seed arrival motion. Taking is one-shot
   * across host remounts/engine eviction; ordinary history never enters here. */
  takePromptArrivals(sessionId:string):HistoryMessage[]{
    const messages=(this.state.history[sessionId]||[]).filter(message=>this.promptArrivals.get(message.id)===sessionId&&this.optimisticPrompts.get(message.id)===sessionId);
    for(const message of messages)this.promptArrivals.delete(message.id);
    return messages;
  }
  consumePromptArrival(messageId:string){this.promptArrivals.delete(messageId);}
  // Separate from echo reconciliation: a canonical User message can arrive
  // before the first streaming event. Keep its reserved row until that event.
  private promptActivity=new Map<string,string>();
  // A top-level turn started locally from Idle must not inherit a previous
  // execution's finished runtime, even after its own live busy has arrived.
  // Steers into existing busy runs deliberately never establish this fence.
  private promptRuntimeRoots=new Map<string,string>();
  // Attach hydration is ordered on the daemon socket, not against the HTTP
  // prompt request. Until live busy, its Idle can describe the *previous* turn.
  // Admission and HTTP busy reads prove acceptance, not that old WS hydration
  // has drained. Only live busy or a fresh idle read hands off this guard.
  private acceptedPromptRequests=new Set<string>();
  private deferredPromptIdle=new Set<string>();
  private promptStatusChecks=new Set<string>();
  private forgetPromptActivity(messageId:string){
    this.promptActivity.delete(messageId);
    this.acceptedPromptRequests.delete(messageId);
    this.deferredPromptIdle.delete(messageId);
  }
  private promptHydrationOwners(body:Record<string,unknown>):string[]{
    const snapshot=isRecord(body.snapshot)?body.snapshot:{};
    const branches=Array.isArray(snapshot.branches)?snapshot.branches:[];
    return [...this.promptActivity].filter(([,session])=>
      (body.session_id===session||snapshot.rootSessionId===session||branches.some(branch=>isRecord(branch)&&branch.sessionId===session))).map(([id])=>id);
  }
  private async reconcileDeferredPromptIdle(messageId:string,sessionId:string){
    if(this.promptStatusChecks.has(messageId)||!this.acceptedPromptRequests.has(messageId)||!this.deferredPromptIdle.delete(messageId)||this.promptActivity.get(messageId)!==sessionId)return;
    this.promptStatusChecks.add(messageId);
    // Do not replay an old Idle after HTTP succeeds: read post-admission state.
    // This also settles no-reply/instant completion when no busy edge was seen.
    try{
      const statuses=await this.sdkForSession(sessionId).sessions.status();
      if(this.disposed||this.promptActivity.get(messageId)!==sessionId)return;
      if(!statuses[sessionId]||statuses[sessionId].type==="idle")this.releasePromptActivity(messageId);
      // Busy may precede the live busy event on an independent transport.
      // Keep shielding against old WS snapshots until that lifecycle edge.
    }catch(error){
      if(this.disposed||this.promptActivity.get(messageId)!==sessionId)return;
      // A failed read is not an authoritative Idle. Preserve the guard; a
      // subsequent lifecycle edge/recheck or explicit session error settles it.
      this.report(error);
    }finally{
      this.promptStatusChecks.delete(messageId);
      // An Idle arriving while this read was in flight may be newer than its
      // captured busy result. Coalesce those edges into one fresh recheck.
      if(!this.disposed)void this.reconcileDeferredPromptIdle(messageId,sessionId);
    }
  }
  private publishPromptActivity(sessionId:string,state:string){
    this.update({streaming:{...this.state.streaming,[sessionId]:state}});
    for(const listener of this.agentEventListeners){try{listener({StreamingState:{session_id:sessionId,state,label:null}});}catch(error){this.report(error);}}
  }
  private releasePromptActivity(messageId:string){
    const sessionId=this.promptActivity.get(messageId);this.forgetPromptActivity(messageId);if(!sessionId)return;
    if(this.promptRuntimeRoots.get(sessionId)===messageId)this.promptRuntimeRoots.delete(sessionId);
    if(![...this.promptActivity.values()].includes(sessionId))this.publishPromptActivity(sessionId,"Idle");
  }
  private showPrompt(sessionId:string,message:HistoryMessage){
    // Adoption continues the original send, not a second user arrival.
    if(this.optimisticPrompts.get(message.id)===sessionId)return;
    if(!this.historyEdits.get(sessionId)?.pending.length)this.historyEdits.delete(sessionId);
    // Reserve Crafting/follow-bottom geometry before the bubble's first paint,
    // not on the later HTTP/SSE state. Existing busy turns remain server-owned.
    if(!this.state.streaming[sessionId]||this.state.streaming[sessionId]==="Idle"||[...this.promptActivity.values()].includes(sessionId)){
      if((!this.state.streaming[sessionId]||this.state.streaming[sessionId]==="Idle")&&this.agentFamily(sessionId).root===sessionId)this.promptRuntimeRoots.set(sessionId,message.id);
      this.promptActivity.set(message.id,sessionId);
      if(this.state.streaming[sessionId]!=="Generating")this.publishPromptActivity(sessionId,"Generating");
    }
    this.promptArrivals.set(message.id,sessionId);
    this.optimisticPrompts.set(message.id,sessionId);this.upsertMessage(sessionId,message);
    const event={MessageUpdated:{session_id:sessionId,message,local_echo:true}};
    for(const listener of this.agentEventListeners){try{listener(event);}catch(error){this.report(error);}}
  }
  private rollbackPrompt(messageId:string){
    this.promptArrivals.delete(messageId);
    const reservedSession=this.optimisticPrompts.get(messageId);
    if(reservedSession&&this.promptRuntimeRoots.get(reservedSession)===messageId)this.promptRuntimeRoots.delete(reservedSession);
    this.releasePromptActivity(messageId);
    const sessionId=this.optimisticPrompts.get(messageId);if(!sessionId)return;
    this.optimisticPrompts.delete(messageId);
    this.update({history:{...this.state.history,[sessionId]:(this.state.history[sessionId]||[]).filter(row=>row.id!==messageId)}});
    for(const listener of this.agentEventListeners){try{listener({PartRemoved:{session_id:sessionId,part_id:messageId}});}catch(error){this.report(error);}}
  }
  private promptMessage(request:ReturnType<typeof sdkPrompt>):HistoryMessage {
    return {id:request.messageId!,role:"User",kind:"User",text:(request.parts||[]).filter(part=>part.type==="text").map(part=>part.text).join("\n"),author:localPresenceIdentity(this.config).displayName,
      attachments:(request.parts||[]).filter(part=>part.type==="file").map(part=>({filename:part.filename||"image",url:part.url,mime:part.mime}))};
  }
  submitPrompt = async (sessionId: string, text: string, options: PromptOptions = {}) => {
    const attachments = [...(options.attachments || [])];
    if (!text.trim() && !attachments.length) return false;
    this.clearSessionError(sessionId);
    const selection=options.selection||this.selection(sessionId),request=sdkPrompt(text,attachments,selection,options.delivery||"steer",localPresenceIdentity(this.config).displayName);
    if(options.messageId)request.messageId=options.messageId;
    // Idle sends still paint before any await. A busy session may queue or
    // steer this prompt: wait for its canonical message instead of flashing
    // an optimistic bubble that queue admission would immediately remove.
    if(options.delivery!=="queue"&&(!this.state.streaming[sessionId]||this.state.streaming[sessionId]==="Idle"))
      this.transaction(()=>this.showPrompt(sessionId,this.promptMessage(request)));
    try {
      const sdk = this.sdkForSession(sessionId);
      // A new turn follows any queued undo/redo on the server, while its local
      // bubble is already visible above. Starting a turn invalidates redo.
      await this.historyEdits.get(sessionId)?.tail.catch(()=>{});
      this.historyEdits.delete(sessionId);
      await this.selectionTasks.get(sessionId);
      await sdk.sessions.prompt(sessionId,request);
      if(this.promptActivity.has(request.messageId!))this.acceptedPromptRequests.add(request.messageId!);
      void this.reconcileDeferredPromptIdle(request.messageId!,sessionId);
      return true;
    } catch (error) { this.rollbackPrompt(request.messageId!);this.reportSessionError(sessionId,error instanceof Error?error.message:"Agent request failed"); return false; }
  };
  /** Direct user shell submission: same authenticated workspace/session SDK as
   * prompts; the server owns permissions, cwd, cancellation and transcript. */
  submitShell = async (sessionId: string | undefined, command: string) => {
    if (!command.trim()) return false;
    let id = sessionId;
    try {
      if (!id) id = (await this.createThread(this.selection())).session_id;
      this.clearSessionError(id);
      const sdk = this.sdkForSession(id);
      await this.selectionTasks.get(id);
      await sdk.sessions.shell(id, command, {messageId:crypto.randomUUID()});
      return true;
    } catch (error) {
      if (id) this.reportSessionError(id, error instanceof Error ? error.message : "Shell request failed");
      else this.report(error);
      return false;
    }
  };
  submitNewPrompt = async (text: string, options: PromptOptions = {}) => {
    if(!text.trim()&&!options.attachments?.length)return false;
    const selection=this.selection(),sourceId=this.state.activeSurfaceId;
    const request=sdkPrompt(text,options.attachments||[],selection,options.delivery||"steer",localPresenceIdentity(this.config).displayName);
    const message=this.promptMessage(request),sessionId=`pending:${request.messageId}`;
    const home=this.state.surfaces.find(surface=>surface.id===sourceId&&(surface.kind==="agent"&&!surface.sessionId||surface.kind==="welcome"&&surface.homeMode==="agent"&&!surface.homeAgentSessionId));
    if(home&&this.state.pendingAgentSessions[home.id])return false;
    const optimistic=home&&options.delivery!=="queue"?{sessionId,message}:undefined;
    if(home&&optimistic)this.transaction(()=>{this.update({pendingAgentSessions:{...this.state.pendingAgentSessions,[home.id]:sessionId}});this.showPrompt(sessionId,message);});
    try {
      const thread=await this.createThread(selection,optimistic);
      if(home&&this.optimisticPrompts.get(message.id)===sessionId){this.rollbackPrompt(message.id);}
      return await this.submitPrompt(thread.session_id,text,{...options,messageId:message.id,selection:this.selection(thread.session_id)});
    }catch(error){this.rollbackPrompt(message.id);this.report(error);return false;}
    finally{if(home&&this.state.pendingAgentSessions[home.id]===sessionId){const pendingAgentSessions={...this.state.pendingAgentSessions};delete pendingAgentSessions[home.id];this.update({pendingAgentSessions});}}
  };
  answerPermission = async (permission: Permission, decision: "Yes" | "Always" | "No") => {
    const matches = (p: Permission) => p.sessionId === permission.sessionId && p.requestId === permission.requestId;
    if (this.state.permissions.find(matches)?.responding) return;
    this.update({ permissions: this.state.permissions.map((p) => matches(p) ? { ...p, responding: true, error: undefined } : p) });
    try {
      await this.sdkForSession(permission.sessionId).interactions.permissions.reply(permission.requestId, decision === "Always" ? "always" : decision === "Yes" ? "once" : "reject");
      this.update({ permissions: this.state.permissions.filter((p) => !matches(p)) });
    } catch (error) {
      this.update({ permissions: this.state.permissions.map((p) => matches(p) ? { ...p, responding: false, error: error instanceof Error ? error.message : "Could not send decision" } : p) });
    }
  };
  answerQuestion = (question: PendingQuestion, answers: string[][]) => this.replyQuestion(question, answers);
  rejectQuestion = (question: PendingQuestion) => this.replyQuestion(question);
  private async replyQuestion(question: PendingQuestion, answers?: string[][]) {
    const matches = (q: PendingQuestion) => q.sessionId === question.sessionId && q.requestId === question.requestId;
    if (this.state.questions.find(matches)?.responding) return;
    this.update({ questions: this.state.questions.map((q) => matches(q) ? { ...q, responding: true, error: undefined } : q) });
    try {
      const sdk = this.sdkForSession(question.sessionId);
      if (answers) await sdk.interactions.questions.reply(question.requestId, answers);
      else await sdk.interactions.questions.reject(question.requestId);
      this.update({ questions: this.state.questions.filter((q) => !matches(q)) });
    } catch (error) {
      this.update({ questions: this.state.questions.map((q) => matches(q) ? { ...q, responding: false, error: error instanceof Error ? error.message : "Could not send answer" } : q) });
    }
  }
  cancelPrompt = (sessionId: string) => { void this.sdkForSession(sessionId).sessions.abort(sessionId).catch(this.report); };
  clearQueue = (sessionId: string) => { void this.sdkForSession(sessionId).sessions.clearQueue(sessionId).catch(this.report); };
  private receiveQuestion(sessionId: string, request: unknown) {
    if (!isRecord(request) || typeof request.id !== "string" || !request.id || !Array.isArray(request.questions) || !request.questions.length) return;
    const previous = this.state.questions.find((q) => q.sessionId === sessionId && q.requestId === request.id);
    const question: PendingQuestion = { sessionId, requestId: request.id, request: { ...request, sessionID: sessionId }, responding: previous?.responding || false };
    this.update({ questions: previous ? this.state.questions.map((q) => q.sessionId === sessionId && q.requestId === request.id ? question : q) : [...this.state.questions, question] });
  }
  ensureWelcome = (options:{select?:boolean}={}) => {
    const workspace=this.state.activeWorkspace;if(!workspace)return;
    const layout=this.getLayout(workspace.id),prior=layout.frame.focusedLeaf;
    const existing=this.state.surfaces.find(surface=>surface.id===`welcome:${workspace.id}`&&surface.kind==="welcome")
      ||this.state.surfaces.find(surface=>surface.workspaceId===workspace.id&&surface.kind==="welcome");
    const id=existing?.id||`welcome:${workspace.id}`;
    const select=options.select!==false;
    this.transaction(()=>{
      if(!existing)this.addSurface({id,kind:"welcome",title:"Home",workspaceId:workspace.id});
      else if(select){layout.focusSurface(id);this.update({activeSurfaceId:id});}
      layout.firstTab(id);
      if(!select&&prior!=null)layout.focus(prior);
    });
    return existing||this.state.surfaces.find(surface=>surface.id===id);
  };
  newWelcome = () => {
    const workspace=this.state.activeWorkspace;if(!workspace)return;
    const surface:Surface={id:`welcome:${workspace.id}:${crypto.randomUUID()}`,kind:"welcome",title:"Home",workspaceId:workspace.id};
    this.addSurface(surface);return surface;
  };
  private bindPendingTerminals = async(workspaceId:string) => {
    const pending=isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{};
    for(const[surfaceId,value]of Object.entries(pending)){
      if(!isRecord(value)||value.workspaceId!==workspaceId||typeof value.sessionId!=="string")continue;
      if(this.state.activeWorkspace?.id!==workspaceId)return;
      if(this.disposed||!this.state.surfaces.some(surface=>surface.id===surfaceId&&surface.workspaceId===workspaceId&&surface.sessionId===value.sessionId))continue;
      try{
        await this.run(this.client.request("Workspace",{BindEditorSurface:{surface_id:surfaceId,session_id:value.sessionId,path:null}}));
      }catch(error){
        if(this.homeTerminalOwners()[surfaceId]===value.sessionId)this.releaseHomeTerminal(surfaceId);
        throw error;
      }
      const current=isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{};const next={...current};delete next[surfaceId];this.setUI({pendingTerminalBindings:next});
    }
  };
  newTerminal = async (split?: SplitAxis) => {
    const workspace = this.state.activeWorkspace;
    if (!workspace || this.closingWorkspaces.has(workspace.id)) return;
    const closeEpoch=this.workspaceCloseEpochs.get(workspace.id)||0;
    const closed=()=>this.disposed||this.closingWorkspaces.has(workspace.id)||closeEpoch!==(this.workspaceCloseEpochs.get(workspace.id)||0)||!this.windowWorkspaces().some(entry=>entry.id===workspace.id);
    try {
      const presentation = this.config.terminalBlockUi ? "blocks" : "grid";
      const result = expectVariant(await this.run(this.client.request("Pty", { CreatePty: { cwd: workspace.root_dir ?? null, cols: 100, rows: 30,presentation,
        ...(this.config.shell?.program?{shell:this.config.shell.program}:{}),...(this.config.shell?.args!==undefined?{shell_args:this.config.shell.args}:{}) } })), "PtyCreated");
      if (!isRecord(result) || typeof result.session_id !== "string") throw new Error("Invalid terminal session");
      if(closed()){await this.closeOwnedPty(result.session_id);return;}
      if(result.presentation!==presentation){
        // Only the fresh session returned to this create request is ours to
        // close. Existing attached shells are never changed or restarted.
        await this.run(this.client.request("Pty",{ClosePty:{session_id:result.session_id}}));
        throw new Error("The running daemon is too old for this terminal. Update or restart the workspace daemon, then reconnect.");
      }
      const surface:Surface={ id: `terminal:${result.session_id}`, kind: "terminal", title: "Terminal", sessionId: result.session_id, workspaceId: workspace.id,rootDir:workspace.root_dir||undefined };
      if(this.state.activeWorkspace?.id===workspace.id){
        await this.run(this.client.request("Workspace",{BindEditorSurface:{surface_id:surface.id,session_id:result.session_id,path:null}}));
      }else{
        const pending=isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{};
        this.setUI({pendingTerminalBindings:{...pending,[surface.id]:{workspaceId:workspace.id,sessionId:result.session_id}}});
      }
      if(closed()){await this.closeOwnedPty(result.session_id);return;}
      if(!this.mainTerminalSurface(workspace.id)){const main=isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{};this.setUI({mainTerminals:{...main,[workspace.id]:result.session_id}});}
      this.addSurface(surface,split);
      this.followMainTerminal();
    } catch (error) { this.report(error); }
  };
  openTerminal = (sessionId: string, workspaceId = this.state.activeWorkspace?.id) => {
    if (workspaceId) this.addSurface({ id: `terminal:${sessionId}`, kind: "terminal", title: "Terminal", sessionId, workspaceId });
  };
  private agentFamilyWatermarks=new Map<string,unknown>();
  private catalogResetSeen=false;
  private historyUpdates=new HistoryUpdates<HistoryMessage>();
  private incomingAgentEvents:WireEvent[]=[];
  private incomingAgentHead=0;
  private incomingAgentTimer?:ReturnType<typeof setTimeout>;
  private incomingAgentFrames=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  /** Socket request correlation stays synchronous in DaemonConnection. Agent
   * snapshots/deltas enter one FIFO here so a catalog baseline cannot run
   * hundreds of full Solid tree updates inside ws's message callback. */
  private enqueueReceive=(event:WireEvent)=>{
    if(this.disposed)return;
    if(event.family!=="Agent"){this.receive(event);return;}
    this.incomingAgentEvents.push(event);
    this.scheduleAgentEvents();
  };
  private scheduleAgentEvents(){
    if(this.incomingAgentTimer!==undefined||this.disposed)return;
    // Commit tokens with the other display work, avoiding a second full UI
    // transaction between animation frames. A hidden/occluded window may not
    // receive display callbacks; the deadline keeps its event FIFO draining.
    this.incomingAgentTimer=setTimeout(()=>this.flushAgentEvents(),16);
    this.incomingAgentFrames.request(()=>this.flushAgentEvents());
  }
  private flushAgentEvents(){
    clearTimeout(this.incomingAgentTimer);this.incomingAgentTimer=undefined;
    this.incomingAgentFrames.cancel();if(this.disposed)return;
    const started=performance.now();let count=0;
    const profileStarted=profileStart();
    this.transaction(()=>{
      while(this.incomingAgentHead<this.incomingAgentEvents.length&&count<64){
        const event=this.incomingAgentEvents[this.incomingAgentHead++]!;
        this.receive(event);count++;
        if(performance.now()-started>=4)break;
      }
      this.flushHistoryUpdates();
    });
    profileEnd("daemon.agentBatch",profileStarted);
    if(this.incomingAgentHead===this.incomingAgentEvents.length){this.incomingAgentEvents=[];this.incomingAgentHead=0;}
    else{if(this.incomingAgentHead>=1024){this.incomingAgentEvents=this.incomingAgentEvents.slice(this.incomingAgentHead);this.incomingAgentHead=0;}this.scheduleAgentEvents();}
  }
  private agentEventListeners = new Set<(message: unknown, historyKind?: "head" | "older" | "replace") => void>();
  async mutateSidebarSession(workspaceId:string,sessionId:string,mutation:{kind:"pin";pinned:boolean}|{kind:"rename";title:string}|{kind:"delete"}) {
    const sdk=this.agentSdk.forSession(workspaceId,sessionId);
    let message:unknown;
    if(mutation.kind==="pin"){await sdk.sessions.pin(sessionId,mutation.pinned);message={ThreadUpdated:{session_id:sessionId,pinned:mutation.pinned}};}
    else if(mutation.kind==="rename"){const title=mutation.title.trim();if(!title)throw new Error("Enter a chat title");await sdk.sessions.update(sessionId,{title});message={ThreadUpdated:{session_id:sessionId,title}};}
    else{await sdk.sessions.delete(sessionId);message={ThreadDeleted:{session_id:sessionId}};}
    if(!this.disposed)this.receive({family:"Agent",requestId:0,message});
  }
  private updateAgentTitle(sessionId:string,title:string){
    if(!title.trim())return;
    this.agentTitles.set(sessionId,title);
    const changed=this.state.surfaces.filter(surface=>surface.kind==="agent"&&(surface.agentRootSessionId||surface.sessionId)===sessionId&&surface.title!==title).map(surface=>({...surface,title}));
    const updates=new Map(changed.map(surface=>[surface.id,surface]));
    const changedThread=this.state.threads.some(thread=>thread.session_id===sessionId&&thread.title!==title);
    if(!changed.length&&!changedThread)return;
    this.transaction(()=>{
      this.update({...(changedThread?{threads:this.state.threads.map(thread=>thread.session_id===sessionId?{...thread,title}:thread)}:{}),...(changed.length?{surfaces:this.state.surfaces.map(surface=>updates.get(surface.id)||surface)}:{})});
      // Buffer-strip titles live in the canonical layout routes as well as
      // Surface metadata. Update both without replacing the tab or its pane.
      for(const surface of changed)this.getLayout(surface.workspaceId).rename(surface);
    });
  }
  /** Agent events for shared engines. Head history includes retained older
   * pages; continuations stay page-sized and carry their actual request kind. */
  subscribeAgentEvents = (listener: (message: unknown, historyKind?: "head" | "older" | "replace") => void) => {
    this.agentEventListeners.add(listener);
    return () => { this.agentEventListeners.delete(listener); };
  };
  private receive = (event: WireEvent) => {
    const pair = variant(event.message);
    if (!pair || !isRecord(pair[1])) return;
    const [tag, body] = pair as [string, Record<string, unknown>];
    if(event.family==="Agent"&&typeof body.session_id==="string"&&this.closedTranscriptRoots.has(agentFamilyRoot(this.state.ui,body.session_id))&&["MessageUpdated","ContentDelta","PartRemoved","ToolUseRequest","ToolUseResult"].includes(tag))return;
    if(tag!=="ContentDelta")this.flushHistoryUpdates();
    const runtime=tag==="RuntimeSnapshot"&&isRecord(body.snapshot)?body.snapshot:undefined;
    const runtimeRoot=runtime&&typeof runtime.rootSessionId==="string"?runtime.rootSessionId:undefined;
    const localRuntimeMessage=event.family==="Agent"&&runtimeRoot?this.promptRuntimeRoots.get(runtimeRoot):undefined;
    const execution=runtime&&isRecord(runtime.execution)?runtime.execution:undefined;
    const currentLocalRuntime=!!localRuntimeMessage&&execution?.rootMessageId===localRuntimeMessage;
    const staleLocalRuntime=!!localRuntimeMessage&&!currentLocalRuntime;
    const hydrationOwners=event.family==="Agent"&&(tag==="SessionIdle"||tag==="StreamingState"||tag==="RuntimeSnapshot")?this.promptHydrationOwners(body):[];
    const deferredHydration=staleLocalRuntime||hydrationOwners.length>0&&(tag==="SessionIdle"||tag==="StreamingState"&&body.state==="Idle"||tag==="RuntimeSnapshot"&&!currentLocalRuntime);
    if(deferredHydration){
      for(const id of hydrationOwners){this.deferredPromptIdle.add(id);const sessionId=this.promptActivity.get(id);if(sessionId)void this.reconcileDeferredPromptIdle(id,sessionId);}
      if(tag==="SessionIdle"||tag==="StreamingState")return;
      // Family metadata still enters the model below. Its pre-admission runtime
      // must not overwrite the timeline's local activity reservation.
    }
    if(event.family==="Agent"&&typeof body.session_id==="string"&&this.historyEdits.get(body.session_id)?.pending.length&&["MessageUpdated","ContentDelta","PartRemoved","MessageEnd"].includes(tag))return;
    if(event.family==="Agent"&&tag==="MessageUpdated"&&typeof body.session_id==="string"&&isRecord(body.message)&&body.message.kind!=="User"&&body.message.kind!=="System"){
      const sessionId=body.session_id,list=this.state.history[sessionId]||[],incoming=body.message;
      const index=list.findIndex(row=>row.id===incoming.id),lastUser=list.findLastIndex(row=>row.kind==="User");
      const created=typeof incoming.created_at==="number"&&incoming.created_at>0?incoming.created_at:list[index]?.created_at||0;
      const historical=index>=0&&index<lastUser;
      const boundary=lastUser>=0?list.slice(0,lastUser+1).findLast(row=>(row.created_at||0)>0)?.created_at:list.find(row=>(row.created_at||0)>0)?.created_at;
      const unanchored=index<0&&(created>0?created<(boundary||0):incoming.kind==="Tool"||incoming.kind==="Reasoning"||incoming.kind==="Subtask");
      if(historical||unanchored){
        // A snapshot is not a new execution. Missing old parts must get their
        // position from canonical history, not from their late arrival time.
        if(index<0)this.refreshCompletedHistory(sessionId);
        else{
          try{
            const message=Schema.decodeUnknownSync(HistoryMessageSchema)(incoming),messages=list.map((row,i)=>i===index?{...message,created_at:created}:row);
            this.update({history:{...this.state.history,[sessionId]:messages}});
            const snapshot={HistoryChunk:{session_id:sessionId,messages,next_cursor:this.historyCursors.get(sessionId)??null}};
            for(const listener of this.agentEventListeners){try{listener(snapshot,"head");}catch(error){this.report(error);}}
          }catch(error){this.report(error);}
        }
        return;
      }
    }
    if(event.family==="Agent"){
      const current=this.state.ui,snapshot=isRecord(body.snapshot)?body.snapshot:{};
      const key=tag==="CatalogFamily"||tag==="RuntimeSnapshot"?`${String(snapshot.rootSessionId||body.session_id||"")}:${tag}`:undefined;
      if(tag==="CatalogReset"){this.agentFamilyWatermarks.clear();this.catalogResetSeen=true;}
      const saved=!this.catalogResetSeen&&isRecord(current.agentFamilyVersions)?current.agentFamilyVersions:{};
      const ui=noteAgentFamily({...current,agentFamilyVersions:key?{[key]:this.agentFamilyWatermarks.get(key)??saved[key]}:{}},event.message);
      if(ui){
        if(key&&isRecord(ui.agentFamilyVersions)&&ui.agentFamilyVersions[key]!==undefined)this.agentFamilyWatermarks.set(key,ui.agentFamilyVersions[key]);
        const metadata=["agentParentSessions","agentFamilyRoots","agentFamilyRosters"] as const;
        const changed=metadata.some(field=>ui[field]!==current[field]&&(current[field]!==undefined||isRecord(ui[field])&&Object.keys(ui[field]).length>0));
        if(changed){this.update({ui:{...current,agentParentSessions:ui.agentParentSessions,agentFamilyRoots:ui.agentFamilyRoots,agentFamilyRosters:ui.agentFamilyRosters}});for(const stopped of this.familyStreams.reconcile())this.sendAgent({StopStream:{session_id:stopped}});this.preloadFamilyHistory();}
      }
      const completed=!deferredHydration&&this.completedTranscript.observe(event.message);if(completed)this.refreshCompletedHistory(completed);
      if(tag==="ThreadSwitched"&&typeof body.session_id==="string")for(const stopped of this.familyStreams.switched(body.session_id))this.sendAgent({StopStream:{session_id:stopped}});
    }
    // Admission permits fresh status reconciliation, but is not a WS drain
    // barrier. Live busy takes over the reservation, not its runtime identity.
    if(event.family==="Agent"&&tag==="SessionEvent"&&body.kind==="session.prompt.admitted"&&isRecord(body.properties)){
      const request=isRecord(body.properties.request)?body.properties.request:{};
      if(typeof request.messageId==="string"&&this.promptActivity.get(request.messageId)===body.session_id){
        this.acceptedPromptRequests.add(request.messageId);
        void this.reconcileDeferredPromptIdle(request.messageId,body.session_id as string);
      }
    }
    if(event.family==="Agent"&&typeof body.session_id==="string"&&(tag==="StreamingState"||tag==="SessionIdle"||tag==="QueueUpdate"&&typeof body.started_at==="number"||tag==="SessionEvent"&&body.kind==="session.error")){
      for(const[id,session]of this.promptActivity)if(session===body.session_id)this.forgetPromptActivity(id);
      if(tag==="SessionIdle"||tag==="StreamingState"&&body.state==="Idle"||tag==="SessionEvent"&&body.kind==="session.error")this.promptRuntimeRoots.delete(body.session_id);
    }
    if(currentLocalRuntime&&execution?.finished===true&&runtimeRoot){
      for(const[id,session]of this.promptActivity)if(session===runtimeRoot)this.forgetPromptActivity(id);
      this.promptRuntimeRoots.delete(runtimeRoot);
      this.update({streaming:{...this.state.streaming,[runtimeRoot]:"Idle"}});
    }
    if (event.family === "Agent" && tag !== "HistoryChunk"&&!deferredHydration) for (const listener of this.agentEventListeners) { try { listener(event.message); } catch (error) { this.report(error); } }
    try {
      if (event.family === "Workspace") {
        if (tag === "HostWorkspaceTree") {
          const tree = Schema.decodeUnknownSync(TreeSchema)(body);
          this.update({ workspaces: tree.workspaces, workspaceTabs: tree.tabs });
          const active=tree.workspaces.find(workspace=>workspace.id===this.state.activeWorkspace?.id);if(active&&this.presence.workspaceId()===active.id)this.presence.setWorkspace(active);
        } else if (tag === "HostWorkspaceUpserted" || tag === "WorkspaceControlChanged") {
          const workspace = Schema.decodeUnknownSync(WorkspaceSchema)(body.workspace);
          // The correlated create reply also reaches event subscribers. Publish
          // our own newly created workspace atomically with its Welcome layout.
          if(this.pendingWorkspaceCreations.has(workspace.id))return;
          this.update({ workspaces: this.state.workspaces.some((w) => w.id === workspace.id)
            ? this.state.workspaces.map((w) => w.id === workspace.id ? workspace : w)
            : [...this.state.workspaces, workspace] });
          if(workspace.id===this.presence.workspaceId())this.presence.setWorkspace(workspace);
        }
        return;
      }
      if(event.family==="Pty"){
        if(tag==="PtyClosed"&&typeof body.session_id==="string")this.retireTerminal(body.session_id);
        if(tag==="SessionProgram"&&typeof body.session_id==="string"&&typeof body.program==="string"){
          const program=body.program.split(/[\\/]/).pop()?.toLowerCase();
          const title=program==="claude"?"Claude":program==="codex"?"Codex":program==="opencode"?"OpenCode":"Terminal";
          const changed=this.state.surfaces.filter(surface=>surface.kind==="terminal"&&surface.sessionId===body.session_id&&surface.title!==title).map(surface=>({...surface,title}));
          if(changed.length){
            const updates=new Map(changed.map(surface=>[surface.id,surface]));
            this.update({surfaces:this.state.surfaces.map(surface=>updates.get(surface.id)||surface)});
            for(const surface of changed)this.getLayout(surface.workspaceId).rename(surface);
          }
        }
        if(tag==="SessionCwd"&&typeof body.session_id==="string"&&absoluteTerminalDirectory(body.cwd)){
          const previous=isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{};
          if(previous[body.session_id]!==body.cwd)this.setUI({terminalCwds:{...previous,[body.session_id]:body.cwd}});
          this.followMainTerminal();
        }
        return;
      }
      if (event.family !== "Agent") return;
      const sessionId = typeof body.session_id === "string" ? body.session_id : undefined;
      if (tag === "ThreadList") {
        const threads = Schema.decodeUnknownSync(Schema.Array(ThreadSchema))(body.threads);
        // Requests can finish after a switch; directory-scoped filtering is mandatory.
        const scoped = threads.filter((t) => t.directory === this.state.activeWorkspace?.root_dir);
        this.update({ threads: scoped });
        const workspaceId = this.state.activeWorkspace?.id;
        if (workspaceId && !this.seededWorkspaces.has(workspaceId)) {
          this.seededWorkspaces.add(workspaceId);
          // A catalog reply never chooses Agent as the workspace home.
          // The first actual Terminal route owns the original splash.
        }
      } else if (tag === "ThreadCreated" && sessionId) {
        const thread = Schema.decodeUnknownSync(ThreadSchema)({ ...body, title: body.title || "New conversation" });
        if (thread.directory === this.state.activeWorkspace?.root_dir) this.update({ threads: [thread, ...this.state.threads.filter((t) => t.session_id !== sessionId)] });
      } else if (tag === "HistoryChunk" && sessionId) {
        const request = this.historyRequests.get(sessionId);
        this.historyRequests.delete(sessionId);
        queueMicrotask(() => {
          if (this.disposed) return;
          const queue = this.queuedHistory.get(sessionId), next = queue?.shift();
          if (!queue?.length) this.queuedHistory.delete(sessionId);
          if (next) this.sendAgent(next.message, next.replace);
        });
        const edit=this.historyEdits.get(sessionId);
        if(this.closedTranscriptRoots.has(agentFamilyRoot(this.state.ui,sessionId))||(request&&request.revision!==(this.historyRevisions.get(sessionId)||0))||edit?.pending.length)return;
        let history = Schema.decodeUnknownSync(Schema.Array(HistoryMessageSchema))(body.messages);
        for(const message of history)if(message.kind==="User"){this.optimisticPrompts.delete(message.id);this.promptArrivals.delete(message.id);}
        const previous = this.state.history[sessionId] || [];
        const older = request?.older ?? false;
        if(!older&&!request?.replace){
          const live=new Map(previous.map(message=>[message.id,message]));
          history=history.map(incoming=>{
            const message=live.get(incoming.id);
            // A head read can finish after newer tokens reached the cache.
            return message&&["Assistant","Reasoning","Compaction"].includes(incoming.kind)&&["Assistant","Reasoning","Compaction"].includes(message.kind)&&typeof message.text==="string"&&message.text.startsWith(incoming.text||"")?{...incoming,text:message.text}:incoming;
          });
        }
        const nextCursor = request
          ? historyPageCursor(body.next_cursor, history, previous, older)
          : typeof body.next_cursor === "string" ? body.next_cursor : null;
        const initial = !this.historyCursors.has(sessionId);
        // Head refreshes must not forget the oldest retained page's cursor.
        if (older || initial || request?.replace) this.historyCursors.set(sessionId, nextCursor);
        const updates = new Map(history.map(message => [message.id, message]));
        const existing = new Set(previous.map(message => message.id));
        const added = history.filter(message => !existing.has(message.id));
        const retained = previous.map(message => updates.get(message.id) || message);
        const merged = request?.replace ? [...history,...previous.filter(row=>this.optimisticPrompts.get(row.id)===sessionId&&!updates.has(row.id))] : older ? [...added, ...retained] : mergeHistoryHead(previous,history,initial);
        if(edit&&request?.replace)edit.base={...edit.base,messages:merged};
        this.update({ history: { ...this.state.history, [sessionId]: merged } });
        // Recover a terminal failure after reconnect/reopen, but never revive
        // an older turn's error or a dismissed error on a later head refresh.
        if(initial&&!older&&!(sessionId in this.state.sessionErrors)){
          for(const message of [...merged].reverse()){
            if(message.kind==="User")break;
            if(message.kind==="System"&&message.title==="Agent error"&&message.text){
              this.update({sessionErrors:{...this.state.sessionErrors,[sessionId]:message.text}});break;
            }
          }
        }
        // Classify by the request that actually completed, not the pane's
        // loading flag: an older request can be queued behind a head refresh.
        const delivered = {HistoryChunk:{...body,messages:older ? history : merged,next_cursor:this.historyCursors.get(sessionId) ?? null}};
        for (const listener of this.agentEventListeners) { try { listener(delivered, request?.replace ? "replace" : older ? "older" : "head"); } catch (error) { this.report(error); } }
        this.preloadFamilyHistory();
      } else if (tag === "MessageUpdated" && sessionId) {
        const message=Schema.decodeUnknownSync(HistoryMessageSchema)(body.message);
        if(message.kind==="User"&&!this.state.history[sessionId]?.some(row=>row.id===message.id))this.historyEdits.delete(sessionId);
        const confirmed=message.kind==="User"&&this.optimisticPrompts.delete(message.id);
        if(message.kind==="User")this.promptArrivals.delete(message.id);
        this.upsertMessage(sessionId,message,confirmed);
      } else if (tag === "ContentDelta" && sessionId && typeof body.message_id === "string" && typeof body.text === "string") {
        const id = body.message_id;
        const previous = this.historyUpdates.get(sessionId,this.state.history[sessionId]||[],id);
        const tool = isRecord(body.kind) && isRecord(body.kind.Tool) && typeof body.kind.Tool.name === "string" ? body.kind.Tool.name : undefined;
        const kind = previous?.kind || (body.kind === "Thinking" || body.kind === "Reasoning" ? "Reasoning" : tool ? "Tool" : "Assistant");
        if(!this.state.history[sessionId])this.update({history:{...this.state.history,[sessionId]:[]}});
        this.historyUpdates.stage(sessionId,this.state.history[sessionId], { ...previous, id, role: "Assistant", kind, ...(tool ? { tool } : {}), text: (previous?.text || "") + body.text });
        if(!this.updateDepth)this.flushHistoryUpdates();
      } else if (tag === "ProviderState" && sessionId) {
        this.update({ providerStates: { ...this.state.providerStates, [sessionId]: Schema.decodeUnknownSync(ProviderStateSchema)(body) } });
      } else if (tag === "ThreadDeleted" && sessionId) {
        for(const surface of this.state.surfaces.filter(surface=>surface.kind==="agent"&&(surface.sessionId===sessionId||surface.agentRootSessionId===sessionId)))this.closeSurface(surface.id);
        this.update({surfaces:this.state.surfaces.map(surface=>surface.kind==="welcome"&&(surface.homeAgentSessionId===sessionId||surface.agentRootSessionId===sessionId)?{...surface,homeAgentSessionId:undefined,agentRootSessionId:undefined}:surface)});
        this.update({threads:this.state.threads.filter(thread=>thread.session_id!==sessionId)});
      } else if (tag === "ThreadUpdated" && sessionId) {
        if (typeof body.title === "string") this.updateAgentTitle(sessionId,body.title);
      } else if (tag === "SessionEvent" && sessionId && body.kind === "session.queue.updated" && isRecord(body.properties)) {
        const properties=body.properties,queue=isRecord(properties.queue)?properties.queue:{};
        if(typeof queue.count==="number")this.update({queues:{...this.state.queues,[sessionId]:{count:queue.count,preview:typeof queue.preview==="string"?queue.preview:undefined}}});
        // A starting primary can coexist with real waiting work; the aggregate count is not its disposition.
        if(properties.action==="enqueue"&&typeof properties.messageID==="string"&&queue.primaryMessageID!==properties.messageID&&typeof queue.count==="number"&&queue.count>0&&this.optimisticPrompts.get(properties.messageID)===sessionId)this.rollbackPrompt(properties.messageID);
      } else if (tag === "SessionEvent" && body.kind === "session.error" && isRecord(body.properties)) {
        const message=sessionErrorMessage(body.properties);
        if(message){if(sessionId)this.reportSessionError(sessionId,message,true);else this.report(new Error(message));}
      } else if (tag === "SessionEvent" && body.kind === "session.updated" && isRecord(body.properties) && isRecord(body.properties.info)) {
        const info=body.properties.info,id=typeof info.id==="string"?info.id:sessionId;
        if(id&&typeof info.title==="string")this.updateAgentTitle(id,info.title);
        if(id)this.applySessionGoal(id,info);
      } else if (tag === "QuestionAsked" && sessionId) this.receiveQuestion(sessionId, body.request);
      else if (tag === "QuestionsUpdated" && sessionId && Array.isArray(body.requests)) {
        const ids = new Set(body.requests.filter(isRecord).map((request) => request.id));
        this.update({ questions: this.state.questions.filter((q) => q.sessionId !== sessionId || ids.has(q.requestId)) });
        for (const request of body.requests) this.receiveQuestion(sessionId, request);
      } else if (tag === "QuestionRemoved" && sessionId && typeof body.request_id === "string") {
        this.update({ questions: this.state.questions.filter((q) => q.sessionId !== sessionId || q.requestId !== body.request_id) });
      } else if (tag === "QueueUpdate" && sessionId && typeof body.count === "number") {
        this.update({ queues: { ...this.state.queues, [sessionId]: { count: body.count, preview: typeof body.preview === "string" ? body.preview : undefined } } });
      } else if (tag === "UsageUpdate" && sessionId) {
        this.update({ usage: { ...this.state.usage, [sessionId]: Schema.decodeUnknownSync(UsageSchema)(body.usage) } });
      } else if (tag === "TodoUpdate" && sessionId) {
        this.update({ todos: { ...this.state.todos, [sessionId]: Schema.decodeUnknownSync(Schema.Array(TodoSchema))(body.todos) } });
      } else if (tag === "StreamingState" && sessionId && typeof body.state === "string") {
        if(body.state!=="Idle"&&[undefined,"Idle"].includes(this.state.streaming[sessionId]))this.clearSessionError(sessionId);
        this.update({ streaming: { ...this.state.streaming, [sessionId]: body.state } });
      } else if (tag === "SessionIdle" && sessionId) {
        this.update({ streaming: { ...this.state.streaming, [sessionId]: "Idle" } });
      } else if (tag === "ToolUseRequest" && sessionId && typeof body.request_id === "string") {
        const permission = { sessionId, requestId: body.request_id, tool: String(body.tool || "Tool"), title: String(body.title || body.tool || "Permission required") };
        this.update({ permissions: [...this.state.permissions.filter((p) => p.sessionId !== permission.sessionId || p.requestId !== permission.requestId), permission] });
      } else if (tag === "PermissionRemoved" && typeof body.request_id === "string") {
        this.update({ permissions: this.state.permissions.filter((p) => p.requestId !== body.request_id || !!sessionId && p.sessionId !== sessionId) });
      } else if (tag === "PermissionReplyFailed" && typeof body.request_id === "string") {
        this.update({ permissions: this.state.permissions.map((p) => p.requestId === body.request_id ? { ...p, responding: false, error: String(body.error || "Could not send decision") } : p) });
      } else if (tag === "Error" || tag === "ThreadListFailed") {
        const message=String(body.message || body.error || "Agent request failed");
        if(tag==="Error"&&sessionId)this.reportSessionError(sessionId,message);else this.update({error:message});
      }
    } catch { this.update({ error: `Unsupported ${tag} payload from daemon` }); }
  };
  private flushHistoryUpdates(){const history=this.historyUpdates.flush(this.state.history);if(history!==this.state.history)this.update({history});}
  private upsertMessage(sessionId: string, message: HistoryMessage, confirmed=false) {
    this.flushHistoryUpdates();
    // Confirmed echoes follow server event order, not each client's local send
    // order. Otherwise simultaneous users retain different transcript orders.
    const list = (this.state.history[sessionId] || []).filter(row=>!confirmed||row.id!==message.id);
    const index = list.findIndex((m) => m.id === message.id);
    if (index >= 0) list[index] = message; else list.push(message);
    this.update({ history: { ...this.state.history, [sessionId]: list } });
  }
}
