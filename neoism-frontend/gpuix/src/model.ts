import { Effect, Schema } from "effect";
import type { AppConfig } from "./config";
import { DaemonConnection } from "./daemon/client";
import { NativeEditor, coreCall } from "./native-core";
import { NativeMarkdown } from "./markdown/NativeMarkdown";
import type { SavedSession } from "./session-store";
import { WorkspaceLayout, type LayoutSnapshot, type SplitAxis, type FocusDirection } from "./layout/WorkspaceLayout";
import { relative, isAbsolute, posix, win32 } from "node:path";
import { closeTerminalSession, disposeTerminalSessions,onTerminalUnavailable,terminalSession } from "./terminal/sessions";
import { absoluteTerminalDirectory,mainTerminalSurface,followsTerminalDirectory } from "./terminal/workspace";
import { localDaemon } from "./menu-actions/web-frontend";
import {getWindowWorkspaces,includeWindowWorkspace,openWorkspaceIds,reorderWindowWorkspace} from "./workspaces/window-tabs";
import { AgentSdk, sdkModel, sdkPrompt,sdkThread } from "./daemon/sdk";
import {localPresenceIdentity} from "./agent/local-presence";
import {catalogThread} from "./agent/catalog-session";
import {catalogAgentMessage} from "./agent/catalog-event";
import {FamilyStreamOwnership,CompletedTranscriptRefresh} from "./agent/transcript-sync";
import {agentFamily,agentFamilyRoot,noteAgentFamily} from "./agent/family";
import type { GitReviewDescriptor } from "./git/GitReviewController";
import { DirectorySchema, FileSchema, HistoryMessageSchema, ThreadSchema, TreeSchema, WorkspaceSchema,
  UsageSchema, TodoSchema, ProviderSchema, AgentSchema, SkillSchema, ProviderStateSchema, expectVariant, isRecord, variant,
  type ConnectionState, type Directory, type HistoryMessage, type Thread, type Todo, type Usage,
  type Provider, type ProviderState, type AgentInfo, type SkillInfo, type Attachment,
  type WireEvent, type Workspace, type WorkspaceTab, type AgentClientMessage } from "./daemon/protocol";

export type Surface = {
  id: string; title: string; workspaceId: string;
  kind: "file" | "agent" | "terminal" | "page" | "welcome" | "git";
  page?: "extensions"; document?: { kind: "config"; revision: string; writable: boolean };
  path?: string; rootDir?: string; sessionId?: string; agentRootSessionId?: string;
  git?: GitReviewDescriptor; gitReveal?: number; gitEditorIds?: string[];
  /** Canonical source owner for inline Git editing, without a second visible tab. */
  hidden?: boolean;
};
export interface PendingQuestion { sessionId: string; requestId: string; request: Record<string, unknown>; responding: boolean; error?: string }
export interface PromptOptions { attachments?: readonly Attachment[]; delivery?: "steer" | "queue"; selection?: ProviderState }
export interface Permission { sessionId: string; requestId: string; tool: string; title: string; responding?: boolean; error?: string }
export interface ComposerDraft { text: string; attachments: Array<Attachment & { token: string; name: string }> }
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
  usage: Readonly<Record<string, Usage>>;
  todos: Readonly<Record<string, readonly Todo[]>>;
  streaming: Readonly<Record<string, string>>;
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
  loading: boolean;
  creatingWorkspace?: boolean;
  error?: string;
}
type WorkspaceNavigation = Pick<AppState, "directories" | "expanded" | "notesDirectories" | "notesExpanded" | "threads">;
export class WorkspaceModel {
  private listeners = new Set<() => void>();
  private cleanups: Array<() => void> = [];
  private abort = new AbortController();
  private epoch = 0;
  private disposed = false;
  private seededWorkspaces = new Set<string>();
  private startupWelcomePending = true;
  private workspaceNavigation = new Map<string, WorkspaceNavigation>();
  private fileLoads = new Map<string, Promise<void>>();
  private directoryLoads = new Map<string, Promise<void>>();
  private editorSnapshots = new WeakMap<NativeEditor, { frame: NativeEditor["frame"]; snapshot: ReturnType<NativeEditor["snapshot"]> }>();
  private updateDepth = 0;
  private updatePending = false;
  readonly editors: Map<string, NativeEditor>;
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
  private workspaceDirectoryChanges = new Set<string>();
  state: AppState = {
    ui: {}, composerDrafts: {}, connection: { phase: "closed", attempt: 0 }, workspaces: [], workspaceTabs: [],
    surfaces: [], notesDirectories: {}, notesExpanded: new Set([""]), directories: {}, expanded: new Set([""]), files: {}, threads: [],
    history: {}, usage: {}, todos: {}, streaming: {}, permissions: [], questions: [], providers: [], agents: [], skills: [], providerStates: {}, drafts: {}, queues: {}, catalogsLoading: false, loading: false,
  };
  constructor(readonly client: DaemonConnection, readonly config: AppConfig, seed?: { state: AppState; editors: Map<string, NativeEditor>; recovery?: SavedSession }) {
    this.agentSdk = new AgentSdk(config);
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
    return surface?.kind==="agent"&&!surface.sessionId&&surface.id!==`agent-home:${surface.workspaceId}`?`home:${surface.id}`:`home:${surface?.workspaceId||this.state.activeWorkspace?.id||""}`;
  };
  setComposerDraft = (key: string, draft: ComposerDraft) => {
    if (JSON.stringify(this.state.composerDrafts[key]) === JSON.stringify(draft)) return;
    this.update({ composerDrafts: { ...this.state.composerDrafts, [key]: draft } });
  };
  private snapshotEditor(editor: NativeEditor) {
    let saved = this.editorSnapshots.get(editor);
    if (!saved || saved.frame !== editor.frame) {
      saved = { frame: editor.frame, snapshot: editor.snapshot() }; this.editorSnapshots.set(editor, saved);
    }
    return saved.snapshot;
  }
  captureSession(): SavedSession {
    return { version: 1, activeWorkspaceId: this.state.activeWorkspace?.id, activeSurfaceId: this.state.activeSurfaceId,
      surfaces: [...this.state.surfaces], ui: { ...this.state.ui,agentFamilyVersions:{...(isRecord(this.state.ui.agentFamilyVersions)?this.state.ui.agentFamilyVersions:{}),...Object.fromEntries(this.agentFamilyWatermarks)}, markdownViews: { ...this.recoveredMarkdownViews, ...Object.fromEntries([...this.markdownViews].map(([key, view]) => [key, view.markdown.snapshot()])) } }, providerStates: this.state.providerStates, drafts: this.state.drafts,
      composerDrafts: { ...this.state.composerDrafts }, editors: { ...this.recoveredViews, ...Object.fromEntries([...this.editors].map(([id, editor]) => [id, this.snapshotEditor(editor)]).concat([...this.editorViews].map(([id, view]) => ["view:" + id, this.snapshotEditor(view.editor)]))) } };
  }
  restoreSession(session: SavedSession) {
    this.agentFamilyWatermarks.clear();
    this.startupWelcomePending = true;
    this.recoveredViews = session.editors;
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
    for (const surface of session.surfaces) {
      if (surface.kind !== "file" || !session.editors[surface.id]) continue;
      const editor = new NativeEditor(surface.path!, "", this.config.vim);
      try { editor.call("restore", { snapshot: session.editors[surface.id] }); this.editors.set(surface.id, editor); files[surface.id] = editor.text(); }
      catch { editor.dispose(); }
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
      if (connection.phase === "connected") void this.hydrate();
      else this.epoch++;
    }));
  }
  dispose(preserveEditors = false) { clearTimeout(this.completedHistoryTimer);this.completedHistory.clear();this.agentCatalogStream?.abort.abort();this.agentCatalogStream=undefined; clearTimeout(this.incomingAgentTimer);this.incomingAgentTimer=undefined;this.incomingAgentEvents=[];this.incomingAgentHead=0; for (const view of this.markdownViews.values()) view.markdown.dispose(); this.markdownViews.clear(); disposeTerminalSessions(this.client); this.disposed = true; this.epoch++; this.abort.abort(); this.agentSdk.dispose(); this.cleanups.forEach((cleanup) => cleanup()); this.listeners.clear(); for (const layout of this.layouts.values()) layout.dispose(); this.layouts.clear(); for (const view of this.editorViews.values()) view.editor.dispose(); this.editorViews.clear(); if (!preserveEditors) { for (const editor of this.editors.values()) editor.dispose(); this.editors.clear(); } }
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
    if (!layout.frame.leaves.length) for (const surface of this.state.surfaces.filter((s) => s.workspaceId === workspaceId && !s.hidden)) layout.open(surface);
    this.cleanups.push(layout.subscribe((frame) => {
      const layouts = isRecord(this.state.ui.layouts) ? this.state.ui.layouts : {};
      const patch: Partial<AppState> = { ui: { ...this.state.ui, layouts: { ...layouts, [workspaceId]: layout!.snapshot() } } };
      if (this.state.activeWorkspace?.id === workspaceId) patch.activeSurfaceId = frame.focusedSurfaceId;
      this.update(patch);
    }));
    return layout;
  }
  focusPane = (direction: FocusDirection) => { const frame = this.getLayout().focusDirection(direction); if (frame.focusedSurfaceId) this.selectSurface(frame.focusedSurfaceId, true); };
  splitSurface = (axis: SplitAxis) => { void this.newTerminal(axis); };
  openPage = (page: "extensions") => {
    const workspace = this.state.activeWorkspace; if (!workspace) throw new Error("Select a workspace first");
    this.addSurface({ id: `page:${workspace.id}:${page}`, kind: "page", page, title: "Extensions", workspaceId: workspace.id });
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
    if(!discard&&this.surfaceDirty(id)&&layout.frame.leaves.filter(entry=>entry.surfaceId===id).length<2){this.report(new Error("Unsaved changes. Save this file or use :q! to discard it."));return false;}
    this.transaction(()=>{
      const key=`${id}@${leaf}`,view=this.editorViews.get(key);this.markdownViews.get(key)?.markdown.dispose();this.markdownViews.delete(key);delete this.recoveredMarkdownViews[key];view?.editor.dispose();this.editorViews.delete(key);delete this.recoveredViews[`view:${key}`];
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
  report = (error: unknown) => {
    // Also to stderr: UI toasts vanish, and a field report needs the stack.
    if (process.env.NODE_ENV !== "test") console.error("[neoism-gpuix] reported:", error instanceof Error ? error.stack || error.message : error);
    if (!this.disposed) this.update({ error: error instanceof Error ? error.message : "Operation failed" });
  };
  private async run<A, E>(effect: Effect.Effect<A, E>): Promise<A> {
    return Effect.runPromise(effect, { signal: this.abort.signal });
  }
  private async hydrate() {
    const epoch = ++this.epoch;
    this.update({ loading: true, error: undefined });
    try {
      const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await this.run(this.client.request("Workspace", "RequestHostWorkspaceTree")), "HostWorkspaceTree"));
      if (epoch !== this.epoch) return;
      this.update({ workspaces: tree.workspaces, workspaceTabs: tree.tabs });
      const preferred = this.config.workspaceId || this.state.activeWorkspace?.id || this.restoredWorkspaceId;
      const rememberedWindow=openWorkspaceIds(this.state.ui);
      const opened=this.windowWorkspaces();
      let workspace = !this.config.workspaceId&&rememberedWindow!==undefined
        ? opened.find(workspace=>workspace.id===preferred)||opened[0]
        : tree.workspaces.find((w) => w.id === preferred);
      if (!workspace) {
        const reply = expectVariant(await this.run(this.client.request("Workspace", { ResolveInitialWorkspace: { preferred_host_id: null } })), "InitialWorkspaceResolved");
        if (isRecord(reply)) workspace = Schema.decodeUnknownSync(WorkspaceSchema)(reply.workspace);
      }
      if (epoch !== this.epoch || !workspace) return;
      await this.activateWorkspace(workspace, true);
    } catch (error) { if (epoch === this.epoch) this.report(error); }
    finally { if (epoch === this.epoch) this.update({ loading: false }); }
  }
  windowWorkspaces=(state:Pick<AppState,"workspaces"|"ui">=this.state)=>getWindowWorkspaces(state);
  reorderWorkspace = (dragId:string,targetId:string,position:"before"|"after"="before"):boolean => {
    const visible=this.windowWorkspaces();if(!visible.some(workspace=>workspace.id===dragId)||!visible.some(workspace=>workspace.id===targetId))return false;
    const ids=reorderWindowWorkspace(this.state.ui,dragId,targetId,position),previous=openWorkspaceIds(this.state.ui)||[];
    if(ids.every((id,index)=>id===previous[index]))return false;
    this.setUI({openWorkspaceIds:ids});return true;
  };
  newWorkspace = (): Promise<Workspace | undefined> => {
    const parent=this.state.activeWorkspace;
    if(!parent){this.report(new Error("Select a host workspace first"));return Promise.resolve(undefined);}
    const id=crypto.randomUUID();
    this.pendingWorkspaceCreations.add(id);this.update({creatingWorkspace:true});
    const task=this.workspaceCreations.then(()=>this.createWorkspace(parent,id));
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
      const dirty=this.state.surfaces.filter(surface=>surface.workspaceId===workspaceId&&this.surfaceDirty(surface.id));
      if(!discard&&dirty.length){this.report(new Error("Unsaved changes. Save this workspace or use :qall! to discard it."));return false;}
      if(this.state.activeWorkspace?.id===workspaceId){
        const adjacent=opened[index+1]||opened[index-1];
        if(adjacent)await this.activateWorkspace(adjacent);
        else if(!await this.createWorkspace(this.workspaceView(workspace)))return false;
      }
      if(this.disposed)return false;
      this.transaction(()=>{
        if(discard)for(const surface of dirty)this.closeSurface(surface.id,true);
        this.setUI({openWorkspaceIds:(openWorkspaceIds(this.state.ui)||[]).filter(id=>id!==workspaceId)});
      });
      return true;
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
    return mainTerminalSurface(workspace,this.state.surfaces.filter(surface=>!ended[surface.sessionId||""]),remembered);
  };
  /** Explicit folder selection: change the existing first shell through the
   * host's controlled channel, then publish only its confirmed actual cwd. */
  changeWorkspaceDirectory=async(path:string,workspaceId=this.state.activeWorkspace?.id):Promise<void>=>{
    const captured=this.state.workspaces.find(workspace=>workspace.id===workspaceId);
    if(!captured)throw new Error("Select a workspace first");
    if(!absoluteTerminalDirectory(path)||/[\x00-\x1f\x7f]/.test(path))throw new Error("Choose an absolute directory without control characters");
    if(this.workspaceDirectoryChanges.has(captured.id))throw new Error("This workspace is still waiting for its directory change");
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
      if(terminal?.sessionId){const cwds=isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{};this.setUI({terminalCwds:{...cwds,[terminal.sessionId]:actual}});}
    });
    if(this.state.activeWorkspace?.id===captured.id)await this.activateWorkspace(workspace);
    }finally{this.workspaceDirectoryChanges.delete(captured.id);}
  };
  private workspaceView(workspace:Workspace):Workspace{
    const roots=isRecord(this.state.ui.terminalWorkspaceRoots)?this.state.ui.terminalWorkspaceRoots:{},entry=roots[workspace.id];
    return localDaemon(this.config.endpoint)&&(!workspace.host_kind||workspace.host_kind==="local")&&isRecord(entry)&&entry.declaredRoot===(workspace.root_dir||null)&&absoluteTerminalDirectory(entry.root)?{...workspace,root_dir:entry.root}:workspace;
  }
  private followMainTerminal(){
    const workspace=this.state.activeWorkspace,surface=this.state.surfaces.find(surface=>surface.id===this.state.activeSurfaceId);
    if(!workspace||!followsTerminalDirectory(workspace,surface,this.mainTerminalSurface(),localDaemon(this.config.endpoint)))return;
    const cwds=isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{},cwd=cwds[surface!.sessionId!];
    if(!absoluteTerminalDirectory(cwd)||cwd===workspace.root_dir)return;
    const declared=this.state.workspaces.find(entry=>entry.id===workspace.id)||workspace;
    const roots=isRecord(this.state.ui.terminalWorkspaceRoots)?this.state.ui.terminalWorkspaceRoots:{};
    this.setUI({terminalWorkspaceRoots:{...roots,[workspace.id]:{declaredRoot:declared.root_dir||null,root:cwd}}});
    // Refresh only this frontend's Explorer/catalog scope. Never publish a
    // host SetWorkspaceRoot or send a cd command into any running shell.
    void this.activateWorkspace(declared);
  }
  activateWorkspace = async (workspace: Workspace, force = false) => {
    workspace=this.workspaceView(this.state.workspaces.find(entry=>entry.id===workspace.id)||workspace);
    const previous = this.state.activeWorkspace, key = this.workspaceKey(workspace);
    if (!force && previous && this.workspaceKey(previous) === key) {
      this.setUI({openWorkspaceIds:includeWindowWorkspace(this.state.ui,workspace.id)});
      return;
    }
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
    const subscribed = this.run(this.client.request("Workspace", { SubscribeWorkspace: { workspace_id: workspace.id } }));
    const listing = this.listDirectory("", epoch);
    if (workspace.notes_vault_dir) void this.listDirectory("", epoch, true);
    for (const pane of this.getLayout(workspace.id).frame.panes) {
      const surface = this.state.surfaces.find((s) => s.id === pane.surfaceId);
      if (surface?.kind === "file") void this.loadFile(surface);
    }
    try {
      await subscribed;
      if (epoch !== this.epoch) return;
      for (const pane of this.getLayout(workspace.id).frame.panes) {
        const surface = this.state.surfaces.find((s) => s.id === pane.surfaceId);
        if (surface?.kind === "agent" && surface.sessionId) this.resumeThread(surface.sessionId);
      }
      await this.bindPendingTerminals(workspace.id);
      if(epoch!==this.epoch)return;
      this.ensureWelcome({select:!this.state.activeSurfaceId});
      await listing;
      this.followMainTerminal();
    } catch (error) { if (epoch === this.epoch) this.report(error); }
    finally { if (epoch === this.epoch) this.update({ loading: false }); }
  };
  listDirectory = (path: string, epoch = this.epoch, notes = false): Promise<void> => {
    const root = notes ? this.state.activeWorkspace?.notes_vault_dir : this.state.activeWorkspace?.root_dir;
    if (!root) return Promise.resolve();
    const key = JSON.stringify([epoch, root, path, notes]), pending = this.directoryLoads.get(key);
    if (pending) return pending;
    const task = (async () => {
      try {
        const reply = await this.run(this.client.request("Files", { ListDir: { path } }, root));
        const directory = Schema.decodeUnknownSync(DirectorySchema)(expectVariant(reply, "DirListing"));
        if (epoch !== this.epoch) return;
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
  toggleDirectory = (path: string, notes = false) => {
    const expanded = new Set(notes ? this.state.notesExpanded : this.state.expanded);
    if (expanded.has(path)) expanded.delete(path);
    else { expanded.add(path); if (!(notes ? this.state.notesDirectories : this.state.directories)[path]) void this.listDirectory(path, this.epoch, notes); }
    this.update(notes ? { notesExpanded: expanded } : { expanded });
  };
  openFile = async (path: string, rootOverride?: string) => {
    const workspace = this.state.activeWorkspace;
    const root = rootOverride || workspace?.root_dir;
    if (!workspace || !root) return;
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
    const paths = /^[a-z]:[\\/]/i.test(root) ? win32 : posix;
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
    const paths=/^[a-z]:[\\/]/i.test(root)||root.startsWith("\\\\")?win32:posix;
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
        if(owner.editor&&owner.editor.text()!==text){owner.editor.call("replace-source",{text});if(owner.editor.text()===text)owner.editor.call("saved",{revision:owner.editor.frame.revision});}
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
    const task = (async () => {
      try {
        const reply = await this.run(this.client.request("Files", { ReadFile: { path } }, root));
        const file = Schema.decodeUnknownSync(FileSchema)(expectVariant(reply, "FileContent"));
        // The surface's immutable workspace/root scopes this read. A tab switch
        // must not discard useful bytes, nor may a closed file be resurrected.
        if (this.disposed || !this.state.surfaces.some((s) => s.id === id && s.rootDir === root && s.path === path) || this.state.files[id] !== undefined) return;
        if (file.bytes.length > 2 * 1024 * 1024) throw new Error("The GPUIX editor currently supports files up to 2 MB.");
        const bytes = Uint8Array.from(file.bytes);
        if (bytes.subarray(0, 8192).includes(0)) throw new Error("This file is binary; the GPUIX binary viewer is not ported yet.");
        this.update({ files: { ...this.state.files, [id]: new TextDecoder().decode(bytes) } });
      } catch (error) { if (this.state.activeWorkspace?.id === surface.workspaceId) this.report(error); }
      finally { this.fileLoads.delete(id); }
    })();
    this.fileLoads.set(id, task); return task;
  };
  private addSurface(surface: Surface, split?: SplitAxis) {
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
    const changed = this.state.activeSurfaceId !== id;
    this.transaction(() => {
      this.update({ activeSurfaceId: id });
      if (!preserveLeaf) this.getLayout(surface.workspaceId).focusSurface(id);
    });
    if (surface.kind === "agent" && changed && surface.sessionId) this.resumeThread(surface.sessionId);
    if (surface.kind === "file" && this.state.files[id] === undefined) void this.loadFile(surface);
    if(surface.kind==="terminal")this.followMainTerminal();
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
      for(const surface of this.state.surfaces.filter(surface=>surface.kind==="terminal"&&surface.sessionId===sessionId))this.closeSurface(surface.id);
    });
  };
  private surfaceDirty(id:string):boolean{
    const surface=this.state.surfaces.find(surface=>surface.id===id);
    if(surface?.kind==="git"&&Array.isArray(surface.gitEditorIds))return surface.gitEditorIds.some(owner=>owner!==id&&this.surfaceDirty(owner));
    return this.editors.get(id)?.frame.dirty===true||[...this.editorViews.values()].some(view=>view.surfaceId===id&&view.editor.frame.dirty)||[...this.markdownViews.values()].some(view=>view.surfaceId===id&&view.markdown.frame.dirty);
  }
  closeSurface = (id: string, discard = false) => {
    const editor = this.editors.get(id);
    if (!discard&&this.surfaceDirty(id)) { this.update({ error: "Save this file before closing it (Ctrl+S)." }); return; }
    const closing = this.state.surfaces.find(surface => surface.id === id);
    if (closing?.kind === "git") for (const ownerId of closing.gitEditorIds || []) {
      const owner = this.state.surfaces.find(surface => surface.id === ownerId);
      const usedByAnotherReview = this.state.surfaces.some(surface => surface.id !== id && surface.gitEditorIds?.includes(ownerId));
      const visible = [...this.layouts.values()].some(layout => layout.frame.leaves.some(leaf => leaf.surfaceId === ownerId));
      if (owner?.hidden && !usedByAnotherReview && !visible) this.closeSurface(ownerId, discard);
    }
    const targetLayout = closing?.workspaceId;
    if (targetLayout) this.getLayout(targetLayout).closeSurface(id);
    for (const [key, view] of this.markdownViews) if (view.surfaceId === id) { view.markdown.dispose(); this.markdownViews.delete(key); delete this.recoveredMarkdownViews[key]; }
    for (const [key, view] of this.editorViews) if (view.surfaceId === id) { view.editor.dispose(); this.editorViews.delete(key); }
    delete this.recoveredViews[id];
    for(const key of Object.keys(this.recoveredViews))if(key.startsWith(`view:${id}@`))delete this.recoveredViews[key];
    for(const key of Object.keys(this.recoveredMarkdownViews))if(key.startsWith(`${id}@`))delete this.recoveredMarkdownViews[key];
    if(discard){const files={...this.state.files};delete files[id];this.update({files});}
    editor?.dispose(); this.editors.delete(id);
    const target = this.state.surfaces.find((s) => s.id === id);
    if (target?.kind === "terminal") {
      closeTerminalSession(this.client, target.sessionId!);
      const main={...(isRecord(this.state.ui.mainTerminals)?this.state.ui.mainTerminals:{})};
      const cwds={...(isRecord(this.state.ui.terminalCwds)?this.state.ui.terminalCwds:{})};
      const pending={...(isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{})};
      for(const [workspace,session]of Object.entries(main))if(session===target.sessionId)delete main[workspace];
      delete cwds[target.sessionId!];delete pending[target.id];
      this.setUI({mainTerminals:main,terminalCwds:cwds,pendingTerminalBindings:pending});
    }
    if (target?.kind === "agent" && target.sessionId&&!this.state.surfaces.some(surface=>surface.id!==id&&surface.kind==="agent"&&surface.sessionId===target.sessionId)) {this.familyStreams.deactivate(target.sessionId);this.sendAgent({ StopStream: { session_id: target.sessionId } });}
    // Closing a UI tab never kills a daemon-owned shell or deletes a chat.
    const surfaces = this.state.surfaces.filter((s) => s.id !== id);
    this.update({ surfaces, activeSurfaceId: this.state.activeSurfaceId === id ? surfaces.findLast((s) => s.workspaceId === this.state.activeWorkspace?.id && !s.hidden)?.id : this.state.activeSurfaceId });
    if (this.state.activeSurfaceId) this.selectSurface(this.state.activeSurfaceId);
    if(targetLayout===this.state.activeWorkspace?.id&&!surfaces.some(surface=>surface.workspaceId===targetLayout&&!surface.hidden))this.ensureWelcome();
  };
  private observeEditor(surfaceId: string, editor: NativeEditor) {
    if (this.observedEditors.has(editor)) return;
    this.observedEditors.add(editor);
    let revision = editor.frame.revision, dirty = editor.frame.dirty;
    this.cleanups.push(editor.subscribe((frame) => {
      if (revision === frame.revision && dirty === frame.dirty) return;
      revision = frame.revision; dirty = frame.dirty;
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
    if (!editor) { editor = new NativeEditor(this.hostPath(surface), this.state.files[surface.id] || "", this.config.vim); this.editors.set(surface.id, editor); }
    this.observeEditor(surface.id, editor);
    if (leaf === undefined) return editor;
    const key = `${surface.id}@${leaf}`;
    let view = this.editorViews.get(key);
    if (!view) {
      const clone = editor.cloneView();
      const saved = this.recoveredViews["view:" + key];
      if (saved) try { clone.call("restore", { snapshot: saved }); clone.syncFrom(editor); } catch {}
      view = { surfaceId: surface.id, editor: clone }; this.editorViews.set(key, view); this.observeEditor(surface.id, clone);
    }
    return view.editor;
  }
  getMarkdown(surface: Surface, leaf?: number) {
    const key = leaf === undefined ? surface.id : `${surface.id}@${leaf}`;
    let view = this.markdownViews.get(key);
    if (!view) {
      const markdown = new NativeMarkdown(this.getEditor(surface, leaf), this.config, this.hostPath(surface), async path => {
        const root = surface.rootDir || this.state.workspaces.find(w => w.id === surface.workspaceId)?.root_dir || "";
        const paths = /^[a-z]:[\\/]/i.test(root) ? win32 : posix;
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
    const paths = /^[a-z]:[\\/]/i.test(root) ? win32 : posix;
    return paths.resolve(root, surface.path || "");
  }
  private scopedEditorPath(path: string) {
    const workspace = this.state.activeWorkspace, root = workspace?.root_dir;
    if (!workspace || !root) throw new Error("Select a workspace first");
    const paths = /^[a-z]:[\\/]/i.test(root) ? win32 : posix;
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
    const editor = this.editors.get(surface.id);
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
    const existing=this.state.surfaces.find(s=>s.kind==="agent"&&s.sessionId&&s.workspaceId===workspace.id&&(s.agentRootSessionId||agentFamilyRoot(this.state.ui,s.sessionId))===root);
    if(existing){
      // Desktop catalog_activation_target: pointer on the viewed child goes
      // back to its family root; keyboard Enter keeps the selected transcript.
      const target=pointer&&existing.sessionId===thread.session_id&&thread.session_id!==root?root:thread.session_id;
      const title=this.state.threads.find(thread=>thread.session_id===root)?.title||existing.title;
      if(existing.sessionId!==target||existing.agentRootSessionId!==root){const updated={...existing,sessionId:target,agentRootSessionId:root,title};this.update({surfaces:this.state.surfaces.map(s=>s.id===existing.id?updated:s)});this.getLayout(workspace.id).rename(updated);}
      this.selectSurface(existing.id);if(existing.sessionId!==target)this.resumeThread(target);return existing.id;
    }
    const home = adoptHome ? this.state.surfaces.find((s) => s.id === this.state.activeSurfaceId && s.kind === "agent" && !s.sessionId && s.workspaceId === workspace.id) : undefined;
    if (home) {
      const adopted = { ...home, sessionId: thread.session_id, agentRootSessionId:root,title: thread.title };
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
    for(const stopped of this.familyStreams.activate(sessionId))this.sendAgent({StopStream:{session_id:stopped}});
    this.sendAgent({ SwitchThread: { session_id: sessionId } });
    this.sendAgent({ GetHistory: { session_id: sessionId, cursor: null, limit: 200 } });
    this.sendAgent({ ResumeStream: { session_id: sessionId } });
  }
  private familyStreams=new FamilyStreamOwnership(id=>agentFamilyRoot(this.state.ui,id));
  private completedTranscript=new CompletedTranscriptRefresh();
  private completedHistory=new Set<string>();
  private completedHistoryTimer:ReturnType<typeof setTimeout>|undefined;
  private refreshCompletedHistory(sessionId:string){
    this.completedHistory.add(sessionId);if(this.completedHistoryTimer!==undefined)return;
    this.completedHistoryTimer=setTimeout(()=>{this.completedHistoryTimer=undefined;const sessions=[...this.completedHistory];this.completedHistory.clear();if(!this.disposed&&this.client.state.phase==="connected")for(const session_id of sessions)this.sendAgent({GetHistory:{session_id,cursor:null,limit:200}});},0);
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
  sendAgent = (message: AgentClientMessage) => {
    if(this.catalogRead(message))return;
    try { this.client.send("Agent", message); } catch (error) { this.report(error); }
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
      await this.sdkForSession(sessionId).sessions.update(sessionId, { agent: selection.agent || undefined, model: sdkModel(selection) });
    }).catch(this.report);
    this.selectionTasks.set(sessionId, task);
    void task.finally(() => { if (this.selectionTasks.get(sessionId) === task) this.selectionTasks.delete(sessionId); });
  };
  private sdkForSession(sessionId: string) {
    const workspaceId = this.sessionWorkspaces.get(sessionId) || this.state.surfaces.find((s) => s.sessionId === sessionId)?.workspaceId || this.state.activeWorkspace?.id;
    if (!workspaceId) throw new Error("Select a workspace first");
    this.sessionWorkspaces.set(sessionId, workspaceId);
    return this.agentSdk.forWorkspace(workspaceId);
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
  refreshCatalogs = async () => {
    const epoch = this.epoch, workspace = this.state.activeWorkspace;
    if (!workspace) return;
    const sdk = this.agentSdk.forWorkspace(workspace.id), directory = workspace.root_dir || undefined;
    const key=JSON.stringify([workspace.id,directory||""]);
    const hadReadyModel=!!this.state.agentDefaults?.[key]?.model;
    let config:unknown={},providers:unknown={},configReady=false,providersReady=false;
    const publishDefaults=()=>{
      if(epoch!==this.epoch)return;
      // Config is authoritative and arrives on its own critical path. Provider
      // discovery only supplies the original fallback when no model is set.
      if(!configReady)return;
      const resolved=coreCall<ProviderState>("agentcomposer.defaults",{config,providers});
      if(hadReadyModel&&!resolved.model&&!providersReady)return;
      const selection:ProviderState={...(resolved.agent?{agent:resolved.agent}:{}),...(resolved.model?{model:resolved.model}:{}),...(resolved.thinking?{thinking:resolved.thinking}:{})};
      this.update({agentDefaults:{...this.state.agentDefaults,[key]:selection},agentDefaultsLoading:!resolved.model&&!providersReady});
    };
    this.update({ catalogsLoading: true, catalogsError: undefined,agentDefaultsLoading:!hadReadyModel });
    const catalog = async (load: () => Promise<Partial<AppState>>,failed?:()=>void) => {
      try {
        const patch = await this.run(Effect.tryPromise({ try: load, catch: (error) => error }));
        if (epoch === this.epoch) this.update(patch);
      } catch (error) { if (epoch === this.epoch) {failed?.();this.update({ catalogsError: error instanceof Error ? error.message : "Catalog unavailable" });} }
    };
    await Promise.all([
      catalog(async()=>{config=await sdk.config.defaults(directory);configReady=true;publishDefaults();return{};},()=>{configReady=true;publishDefaults();}),
      catalog(async () => {
        const result = await sdk.catalog.providers.configured(directory);
        providers=result;providersReady=true;publishDefaults();
        const catalog = result.providers.map((p) => ({ id: p.id, name: p.name || p.id,
          models: Object.entries(p.models).map(([key, model]) => ({ id: model.id || key, name: model.name || model.id || key,
            context_limit: isRecord(model.limit) && typeof model.limit.context === "number" && model.limit.context > 0 ? model.limit.context : undefined })) }));
        return { providers: Schema.decodeUnknownSync(Schema.Array(ProviderSchema))(catalog), providerCatalog: result };
      },()=>{providersReady=true;publishDefaults();}),
      catalog(async () => ({ agents: Schema.decodeUnknownSync(Schema.Array(AgentSchema))((await sdk.catalog.agents.list(directory)).filter((a) => !a.hidden).map((a) => ({ ...a, description: a.description || "" }))) })),
      catalog(async () => ({ skills: Schema.decodeUnknownSync(Schema.Array(SkillSchema))((await sdk.catalog.skills.list(directory)).map((a) => ({ ...a, description: a.description || "" }))) })),
    ]);
    if (epoch === this.epoch) this.update({ catalogsLoading: false,agentDefaultsLoading:false });
  };
  private async createThread(selection: ProviderState) {
    const workspace = this.state.activeWorkspace;
    if (!workspace) throw new Error("Select a workspace first");
    const sourceId=this.state.activeSurfaceId;
    const home=this.state.surfaces.find(surface=>surface.id===sourceId&&surface.kind==="agent"&&!surface.sessionId&&surface.workspaceId===workspace.id);
    const homeDraftKey=home?this.composerDraftKey(undefined,home.id):undefined;
    const session = await this.agentSdk.forWorkspace(workspace.id).sessions.create({
      directory: workspace.root_dir || undefined, agent: selection.agent || undefined, model: sdkModel(selection),
    });
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
    if(home&&this.state.surfaces.some(surface=>surface.id===home.id&&!surface.sessionId)){
      const adopted={...home,sessionId:thread.session_id,title:thread.title};
      this.transaction(()=>{
        const draft=homeDraftKey?this.state.composerDrafts[homeDraftKey]:undefined;
        this.update({surfaces:this.state.surfaces.map(surface=>surface.id===home.id?adopted:surface),
          ...(draft?{composerDrafts:{...this.state.composerDrafts,[thread.session_id]:draft}}:{})});
        this.getLayout(workspace.id).rename(adopted);
      });
      if(this.state.activeSurfaceId===home.id)this.resumeThread(thread.session_id);
    }else if(!home&&this.state.activeWorkspace?.id===workspace.id&&this.state.activeSurfaceId===sourceId)this.openThread(thread);
    return thread;
  }
  newThread = async () => { try { await this.createThread(this.selection()); } catch (error) { this.report(error); } };
  submitPrompt = async (sessionId: string, text: string, options: PromptOptions = {}) => {
    const attachments = [...(options.attachments || [])];
    if (!text.trim() && !attachments.length) return false;
    try {
      const selection = options.selection || this.selection(sessionId);
      const sdk = this.sdkForSession(sessionId);
      await this.selectionTasks.get(sessionId);
      await sdk.sessions.prompt(sessionId, sdkPrompt(text, attachments, selection, options.delivery || "steer",localPresenceIdentity(this.config).displayName));
      return true;
    } catch (error) { this.report(error); return false; }
  };
  submitNewPrompt = async (text: string, options: PromptOptions = {}) => {
    try { const selection = this.selection(); const thread = await this.createThread(selection); return this.submitPrompt(thread.session_id, text, { ...options, selection:this.selection(thread.session_id) }); }
    catch (error) { this.report(error); return false; }
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
    const id=`welcome:${workspace.id}`,layout=this.getLayout(workspace.id),prior=layout.frame.focusedLeaf;
    const existing=this.state.surfaces.find(surface=>surface.id===id&&surface.kind==="welcome");
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
      await this.run(this.client.request("Workspace",{BindEditorSurface:{surface_id:surfaceId,session_id:value.sessionId,path:null}}));
      const current=isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{};const next={...current};delete next[surfaceId];this.setUI({pendingTerminalBindings:next});
    }
  };
  newTerminal = async (split?: SplitAxis) => {
    const workspace = this.state.activeWorkspace;
    if (!workspace) return;
    try {
      const result = expectVariant(await this.run(this.client.request("Pty", { CreatePty: { cwd: workspace.root_dir ?? null, cols: 100, rows: 30,presentation:"grid" } })), "PtyCreated");
      if (!isRecord(result) || typeof result.session_id !== "string") throw new Error("Invalid terminal session");
      if(result.presentation!=="grid"){
        // Only the fresh session returned to this create request is ours to
        // close. Existing attached shells are never changed or restarted.
        await this.run(this.client.request("Pty",{ClosePty:{session_id:result.session_id}}));
        throw new Error("This daemon does not support native-grid shell prompts. An updated daemon is required to create this terminal.");
      }
      const surface:Surface={ id: `terminal:${result.session_id}`, kind: "terminal", title: "Terminal", sessionId: result.session_id, workspaceId: workspace.id,rootDir:workspace.root_dir||undefined };
      if(this.state.activeWorkspace?.id===workspace.id){
        await this.run(this.client.request("Workspace",{BindEditorSurface:{surface_id:surface.id,session_id:result.session_id,path:null}}));
      }else{
        const pending=isRecord(this.state.ui.pendingTerminalBindings)?this.state.ui.pendingTerminalBindings:{};
        this.setUI({pendingTerminalBindings:{...pending,[surface.id]:{workspaceId:workspace.id,sessionId:result.session_id}}});
      }
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
  private incomingAgentEvents:WireEvent[]=[];
  private incomingAgentHead=0;
  private incomingAgentTimer?:ReturnType<typeof setTimeout>;
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
    // A macrotask gives the socket/parser a chance to drain between batches.
    // queueMicrotask would keep UI work ahead of the next network turn.
    this.incomingAgentTimer=setTimeout(()=>this.flushAgentEvents(),0);
  }
  private flushAgentEvents(){
    this.incomingAgentTimer=undefined;if(this.disposed)return;
    const started=performance.now();let count=0;
    this.transaction(()=>{
      while(this.incomingAgentHead<this.incomingAgentEvents.length&&count<64){
        const event=this.incomingAgentEvents[this.incomingAgentHead++]!;
        this.receive(event);count++;
        if(performance.now()-started>=4)break;
      }
    });
    if(this.incomingAgentHead===this.incomingAgentEvents.length){this.incomingAgentEvents=[];this.incomingAgentHead=0;}
    else{if(this.incomingAgentHead>=1024){this.incomingAgentEvents=this.incomingAgentEvents.slice(this.incomingAgentHead);this.incomingAgentHead=0;}this.scheduleAgentEvents();}
  }
  private agentEventListeners = new Set<(message: unknown) => void>();
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
  /** Raw daemon Agent-family messages, untouched, for engines that run the
   * shared Rust dispatcher (the original agent timeline). */
  subscribeAgentEvents = (listener: (message: unknown) => void) => {
    this.agentEventListeners.add(listener);
    return () => { this.agentEventListeners.delete(listener); };
  };
  private receive = (event: WireEvent) => {
    const pair = variant(event.message);
    if (!pair || !isRecord(pair[1])) return;
    const [tag, body] = pair as [string, Record<string, unknown>];
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
        if(changed){this.update({ui:{...current,agentParentSessions:ui.agentParentSessions,agentFamilyRoots:ui.agentFamilyRoots,agentFamilyRosters:ui.agentFamilyRosters}});for(const stopped of this.familyStreams.reconcile())this.sendAgent({StopStream:{session_id:stopped}});}
      }
      const completed=this.completedTranscript.observe(event.message);if(completed)this.refreshCompletedHistory(completed);
      if(tag==="ThreadSwitched"&&typeof body.session_id==="string")for(const stopped of this.familyStreams.switched(body.session_id))this.sendAgent({StopStream:{session_id:stopped}});
    }
    if (event.family === "Agent") for (const listener of this.agentEventListeners) { try { listener(event.message); } catch (error) { this.report(error); } }
    try {
      if (event.family === "Workspace") {
        if (tag === "HostWorkspaceTree") {
          const tree = Schema.decodeUnknownSync(TreeSchema)(body);
          this.update({ workspaces: tree.workspaces, workspaceTabs: tree.tabs });
        } else if (tag === "HostWorkspaceUpserted" || tag === "WorkspaceControlChanged") {
          const workspace = Schema.decodeUnknownSync(WorkspaceSchema)(body.workspace);
          // The correlated create reply also reaches event subscribers. Publish
          // our own newly created workspace atomically with its Welcome layout.
          if(this.pendingWorkspaceCreations.has(workspace.id))return;
          this.update({ workspaces: this.state.workspaces.some((w) => w.id === workspace.id)
            ? this.state.workspaces.map((w) => w.id === workspace.id ? workspace : w)
            : [...this.state.workspaces, workspace] });
        }
        return;
      }
      if(event.family==="Pty"){
        if(tag==="PtyClosed"&&typeof body.session_id==="string")this.retireTerminal(body.session_id);
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
        const history = Schema.decodeUnknownSync(Schema.Array(HistoryMessageSchema))(body.messages);
        const merged = new Map(this.state.history[sessionId]?.map((message) => [message.id, message]));
        for (const message of history) merged.set(message.id, message);
        this.update({ history: { ...this.state.history, [sessionId]: [...merged.values()] } });
      } else if (tag === "MessageUpdated" && sessionId) {
        this.upsertMessage(sessionId, Schema.decodeUnknownSync(HistoryMessageSchema)(body.message));
      } else if (tag === "ContentDelta" && sessionId && typeof body.message_id === "string" && typeof body.text === "string") {
        const id = body.message_id;
        const previous = this.state.history[sessionId]?.find((m) => m.id === id);
        const tool = isRecord(body.kind) && isRecord(body.kind.Tool) && typeof body.kind.Tool.name === "string" ? body.kind.Tool.name : undefined;
        const kind = previous?.kind || (body.kind === "Thinking" || body.kind === "Reasoning" ? "Reasoning" : tool ? "Tool" : "Assistant");
        this.upsertMessage(sessionId, { ...previous, id, role: "Assistant", kind, ...(tool ? { tool } : {}), text: (previous?.text || "") + body.text });
      } else if (tag === "ProviderState" && sessionId) {
        this.update({ providerStates: { ...this.state.providerStates, [sessionId]: Schema.decodeUnknownSync(ProviderStateSchema)(body) } });
      } else if (tag === "ThreadUpdated" && sessionId) {
        if (typeof body.title === "string") this.updateAgentTitle(sessionId,body.title);
      } else if (tag === "SessionEvent" && body.kind === "session.updated" && isRecord(body.properties) && isRecord(body.properties.info)) {
        const info=body.properties.info,id=typeof info.id==="string"?info.id:sessionId;
        if(id&&typeof info.title==="string")this.updateAgentTitle(id,info.title);
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
        this.update({ error: String(body.message || body.error || "Agent request failed") });
      }
    } catch { this.update({ error: `Unsupported ${tag} payload from daemon` }); }
  };
  private upsertMessage(sessionId: string, message: HistoryMessage) {
    const list = [...(this.state.history[sessionId] || [])];
    const index = list.findIndex((m) => m.id === message.id);
    if (index >= 0) list[index] = message; else list.push(message);
    this.update({ history: { ...this.state.history, [sessionId]: list } });
  }
}
