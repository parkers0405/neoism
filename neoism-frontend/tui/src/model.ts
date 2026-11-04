import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { DaemonConnection } from "@neoism/service-client";
import { DirectorySchema, FileSchema, TreeSchema, expectVariant, isRecord, variant, type ConnectionState, type Directory, type Workspace } from "@neoism/service-client/protocol";
import { hostPaths } from "../../gpuix/src/workspaces/host-paths";
import type { TuiConfig } from "./config";
import { TerminalEditor } from "./editor";
import { TuiDocuments } from "./documents";
import { TuiLsp, TuiLspError, type LspMutationResult } from "./lsp";
import type { DiagnosticItem } from "../../../neoism-protocol/typescript/types";
import { palettes } from "../../gpuix/src/design/generated";
import { persistTheme, persistConfigValue, watchSharedConfig } from "./config";
import type { GitStatus } from "./chrome";

export interface EditorBuffer { id: string; path: string; absolutePath: string; root: string; editor: TerminalEditor; saving: boolean; diagnostics?: DiagnosticItem[]; language?: string; lspStatus?: string; lspName?: string }
interface WorkspaceView { directories: TuiState["directories"]; expanded: ReadonlySet<string>; buffers: readonly EditorBuffer[]; activePath?: string }
export type Confirmation = { kind: "close"; buffer: EditorBuffer } | { kind: "quit" };
export interface TreeRow { path: string; name: string; directory: boolean; depth: number }
export interface TuiState {
  connection: ConnectionState;
  workspaces: readonly Workspace[];
  activeWorkspace?: Workspace;
  directories: Readonly<Record<string, Directory>>;
  expanded: ReadonlySet<string>;
  buffers: readonly EditorBuffer[];
  activePath?: string;
  loading: boolean;
  error?: string;
  notice?: string;
  confirmation?: Confirmation;
  quit?: boolean;
  git?: GitStatus;
}

export function treeRows(directories: TuiState["directories"], expanded: ReadonlySet<string>, root: string): TreeRow[] {
  const rows: TreeRow[] = [];
  const path = hostPaths(root);
  function walk(parent: string, depth: number) {
    for (const entry of directories[parent]?.entries ?? []) {
      const key = path.join(parent, entry.name);
      rows.push({ path: key, name: entry.name, directory: entry.is_dir, depth });
      if (entry.is_dir && expanded.has(key)) walk(key, depth + 1);
    }
  }
  walk("", 0);
  return rows;
}

export class TuiModel {
  readonly client: DaemonConnection;
  readonly documents: TuiDocuments;
  readonly lsp: TuiLsp;
  state: TuiState = {
    connection: { phase: "closed", attempt: 0 }, workspaces: [], directories: {}, expanded: new Set(), buffers: [], loading: false,
  };
  private listeners = new Set<() => void>();
  private cleanups: Array<() => void> = [];
  private generation = 0;
  private fileRequest = 0;
  private stopped = false;
  private abort = new AbortController();
  private directoryLoads = new Map<string, Promise<void>>();
  private views = new Map<string, WorkspaceView>();
  private pool = new Map<string, EditorBuffer>();
  private opens = new Map<string, Promise<EditorBuffer | undefined>>();
  private bufferListeners = new Set<(buffer: EditorBuffer, closed?: boolean) => void>();
  private lspTexts = new Map<string, string>();
  private gitWatch?: { token: string; root: string };
  private directorySelected = false;

  constructor(readonly config: TuiConfig, client?: DaemonConnection) {
    this.client = client ?? new DaemonConnection({ ...config, clientName: "neoism-tui" }, undefined, () => {});
    this.documents = new TuiDocuments(this.client, this.changed, this.report);
    this.lsp = new TuiLsp(this.client);
    this.cleanups.push(this.lsp.subscribe(event => {
      const buffer = this.pool.get(event.documentId);
      if (!buffer) return;
      if (event.kind === "diagnostics") {
        buffer.diagnostics = event.items;
        buffer.editor.setDiagnostics(event.items);
      } else if (event.kind === "status") {
        const match = variant(event.message), body = match?.[1];
        if (isRecord(body)) {
          if (typeof body.filetype === "string") buffer.language = body.filetype;
          if (typeof body.state === "string") buffer.lspStatus = body.state;
          if (typeof body.name === "string") buffer.lspName = body.name;
          if (Array.isArray(body.servers)) {
            const servers = body.servers.filter(isRecord);
            buffer.lspStatus = servers.map(server => server.state).join(", ") || undefined;
            buffer.lspName = servers.map(server => server.name).join(", ") || undefined;
          }
        }
      } else if (event.error.code !== "stale") this.report(event.error);
      this.update({ buffers: [...this.state.buffers] });
    }));
    this.cleanups.push(this.client.subscribe(event => {
      if (event.family !== "Git") return;
      const match = variant(event.message), body = match?.[1];
      if (match?.[0] !== "RepoStatus" || !isRecord(body) || body.token !== this.gitWatch?.token || !isRecord(body.snapshot)) return;
      const snapshot = body.snapshot;
      if (snapshot.workspace_root !== this.state.activeWorkspace?.root_dir) return;
      if (!snapshot.repo_root) { this.update({ git: undefined }); return; }
      const files = Array.isArray(snapshot.files) ? snapshot.files.filter(isRecord) : [];
      this.update({ git: { branch: typeof snapshot.branch === "string" ? snapshot.branch : undefined,
        added: files.reduce((sum, file) => sum + (typeof file.additions === "number" ? file.additions : 0), 0),
        deleted: files.reduce((sum, file) => sum + (typeof file.deletions === "number" ? file.deletions : 0), 0) } });
    }));
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  subscribeBuffers = (listener: (buffer: EditorBuffer, closed?: boolean) => void) => { this.bufferListeners.add(listener); return () => { this.bufferListeners.delete(listener); }; };
  get activeBuffer() { return this.state.buffers.find(buffer => buffer.path === this.state.activePath); }
  get allBuffers() { return [...this.pool.values()]; }
  private viewKey(workspace = this.state.activeWorkspace) { return workspace ? JSON.stringify([workspace.id, workspace.root_dir]) : ""; }
  private remember() {
    if (this.state.activeWorkspace) this.views.set(this.viewKey(), { directories: this.state.directories, expanded: this.state.expanded, buffers: this.state.buffers, activePath: this.state.activePath });
  }
  private update(patch: Partial<TuiState>) {
    if (this.stopped) return;
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  clearError = () => this.update({ error: undefined });
  start() {
    if (this.started) return;
    this.started = true;
    this.cleanups.push(watchSharedConfig(this.config, next => {
      const vimChanged = this.config.vim !== next.vim;
      this.config.theme = next.theme; this.config.themeName = next.themeName;
      this.config.vim = next.vim; this.config.keybinds = next.keybinds;
      if (vimChanged) for (const buffer of this.pool.values()) buffer.editor.call("toggle-vim", { enabled: next.vim });
      this.update({ buffers: [...this.state.buffers] });
    }, this.report));
    if (this.config.offline) return;
    this.cleanups.push(this.client.subscribeStatus(connection => {
      this.update({ connection });
      if (connection.phase === "connected") { this.watchGit(); void this.refresh(); }
      else { this.generation++; this.update({ loading: false }); }
    }));
    this.client.start();
  }
  stop() {
    if (this.stopped) return;
    this.stopped = true; this.generation++; this.abort.abort();
    for (const cleanup of this.cleanups.splice(0)) cleanup();
    this.lsp.dispose(); this.documents.dispose();
    for (const buffer of this.pool.values()) buffer.editor.dispose();
    this.pool.clear(); this.client.stop(); this.listeners.clear(); this.bufferListeners.clear();
  }
  reconnect = () => { if (!this.config.offline) this.client.reconnect(); };
  private run<A, E>(request: Effect.Effect<A, E>) { return Effect.runPromise(request, { signal: this.abort.signal }); }
  report = (error: unknown) => { this.update({ error: error instanceof Error ? error.message : String(error) }); };
  changed = (editor: TerminalEditor) => {
    const buffer = [...this.pool.values()].find(buffer => buffer.editor === editor);
    if (!buffer) return;
    editor.refresh();
    if (this.lspTexts.get(buffer.id) !== editor.text) {
      this.lspTexts.set(buffer.id, editor.text);
      try { this.lsp.update({ id: buffer.id, path: buffer.absolutePath, root: buffer.root, text: editor.text }); }
      catch (error) { if (!(error instanceof TuiLspError && error.code === "closed")) this.report(error); }
    }
    this.update({ buffers: [...this.state.buffers] });
    for (const listener of this.bufferListeners) listener(buffer);
  };
  async refresh() {
    const generation = ++this.generation;
    this.update({ loading: true, error: undefined });
    try {
      const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await this.run(this.client.request("Workspace", "RequestHostWorkspaceTree")), "HostWorkspaceTree"));
      if (generation !== this.generation) return;
      const workspaces = tree.workspaces.filter(workspace => workspace.host_id === this.state.connection.connectedHostId);
      this.update({ workspaces });
      if (this.config.directory && !this.directorySelected) {
        const syntax = hostPaths(this.config.directory), root = syntax.normalize(this.config.directory);
        const workspace = workspaces.find(workspace => workspace.root_dir && syntax.normalize(workspace.root_dir) === root);
        this.directorySelected = true;
        try { if (workspace) await this.selectWorkspace(workspace); else await this.setWorkspaceDirectory(root, true); }
        catch (error) { this.directorySelected = false; throw error; }
        return;
      }
      const preferred = this.state.activeWorkspace?.id || this.config.workspaceId;
      const workspace = preferred ? workspaces.find(w => w.id === preferred) : workspaces[0];
      if (!workspace) throw new Error(preferred ? `Workspace '${preferred}' was not found.` : "No workspace owned by this daemon is available.");
      if (this.viewKey(workspace) === this.viewKey()) {
        this.update({ activeWorkspace: workspace });
        await this.listDirectory("");
      } else await this.selectWorkspace(workspace);
    } catch (error) { if (generation === this.generation) this.report(error); }
    finally { if (generation === this.generation) this.update({ loading: false }); }
  }
  async selectWorkspace(workspace: Workspace) {
    this.remember();
    this.generation++; this.fileRequest++;
    const view = this.views.get(this.viewKey(workspace));
    this.update({ activeWorkspace: workspace, directories: view?.directories || {}, expanded: view?.expanded || new Set(), buffers: view?.buffers || [], activePath: view?.activePath, error: undefined, loading: false });
    this.watchGit();
    if (!workspace.root_dir) { this.report(new Error("This workspace has no directory.")); return; }
    if (!this.state.directories[""]) await this.listDirectory("");
  }
  async listDirectory(path: string) {
    const root = this.state.activeWorkspace?.root_dir;
    if (!root) return;
    const generation = this.generation;
    const key = JSON.stringify([generation, path]);
    const pending = this.directoryLoads.get(key);
    if (pending) return pending;
    const load = (async () => {
      try {
        const reply = await this.run(this.client.request("Files", { ListDir: { path } }, root));
        const listing = Schema.decodeUnknownSync(DirectorySchema)(expectVariant(reply, "DirListing"));
        if (generation !== this.generation) return;
        const entries = [...listing.entries].sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || a.name.localeCompare(b.name));
        this.update({ directories: { ...this.state.directories, [path]: { ...listing, entries } } });
      } catch (error) { if (generation === this.generation) this.report(error); }
      finally { this.directoryLoads.delete(key); }
    })();
    this.directoryLoads.set(key, load);
    return load;
  }
  async revealDirectory(path: string) {
    const root = this.state.activeWorkspace?.root_dir;
    if (!root) return;
    const syntax = hostPaths(root), relative = syntax.relative(root, path);
    if (syntax.isAbsolute(relative) || relative === ".." || relative.startsWith(`..${syntax.sep}`)) { this.report(new Error("This directory is outside the active workspace.")); return; }
    const generation = this.generation;
    const expanded = new Set(this.state.expanded);
    let current = "";
    await this.listDirectory("");
    for (const part of relative.split(syntax.sep).filter(Boolean)) {
      if (generation !== this.generation) return;
      current = syntax.join(current, part); expanded.add(current);
      await this.listDirectory(current);
    }
    if (generation === this.generation) this.update({ expanded });
  }
  async activateRow(row: TreeRow) {
    if (!row.directory) return this.openFile(row.path);
    const expanded = new Set(this.state.expanded);
    if (expanded.has(row.path)) expanded.delete(row.path);
    else expanded.add(row.path);
    this.update({ expanded });
    if (expanded.has(row.path) && !this.state.directories[row.path]) await this.listDirectory(row.path);
  }
  async openFile(path: string, position?: { line: number; col: number }) {
    const workspace = this.state.activeWorkspace, root = workspace?.root_dir;
    if (!root || !workspace) return;
    const request = ++this.fileRequest, key = this.viewKey(workspace);
    const syntax = hostPaths(root), absolutePath = syntax.resolve(root, path), documentKey = JSON.stringify([root, absolutePath]);
    const relative = syntax.relative(root, absolutePath);
    this.update({ loading: true, error: undefined });
    let pending = this.opens.get(documentKey);
    if (!pending) {
      pending = (async () => {
        const existing = this.pool.get(documentKey);
        if (existing) return existing;
        const reply = await this.run(this.client.request("Files", { ReadFile: { path: relative } }, root));
        const file = Schema.decodeUnknownSync(FileSchema)(expectVariant(reply, "FileContent"));
        if (this.stopped) return;
        if (file.bytes.length > 2 * 1024 * 1024) throw new Error("Open is limited to text files under 2 MiB.");
        const bytes = Uint8Array.from(file.bytes);
        if (bytes.includes(0)) throw new Error("Binary files cannot be edited as text.");
        const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
        const editor = new TerminalEditor(absolutePath, text, this.config.vim);
        const buffer: EditorBuffer = { id: documentKey, path: relative, absolutePath, root, editor, saving: false };
        this.pool.set(documentKey, buffer);
        this.lsp.open({ id: buffer.id, path: absolutePath, root, text: editor.text });
        this.lspTexts.set(buffer.id, editor.text);
        this.documents.attach(editor);
        for (const listener of this.bufferListeners) listener(buffer);
        return buffer;
      })();
      this.opens.set(documentKey, pending);
    }
    try {
      const buffer = await pending;
      if (!buffer || this.stopped) return;
      if (this.viewKey() === key) {
        if (!this.state.buffers.includes(buffer)) this.update({ buffers: [...this.state.buffers, buffer] });
        if (request === this.fileRequest) {
          if (position) buffer.editor.goto(position.line, position.col);
          this.update({ activePath: buffer.path });
        }
      } else {
        const view = this.views.get(key) || { directories: {}, expanded: new Set<string>(), buffers: [] };
        this.views.set(key, { ...view, buffers: view.buffers.includes(buffer) ? view.buffers : [...view.buffers, buffer], activePath: buffer.path });
      }
      return buffer;
    } catch (error) { if (!this.stopped) this.report(error); }
    finally {
      if (this.opens.get(documentKey) === pending) this.opens.delete(documentKey);
      if (key === this.viewKey() && request === this.fileRequest) this.update({ loading: false });
    }
  }
  async createFile(path: string) {
    const root = this.state.activeWorkspace?.root_dir;
    if (!root) return;
    const syntax = hostPaths(root), relative = syntax.relative(root, syntax.resolve(root, path.trim()));
    if (!relative || syntax.isAbsolute(relative) || relative === ".." || relative.startsWith(`..${syntax.sep}`)) throw new Error("New files must be inside the active workspace.");
    const dir = syntax.dirname(relative) === "." ? "" : syntax.dirname(relative);
    expectVariant(await this.run(this.client.request("Files", { CreateFile: { dir, name: syntax.basename(relative) } }, root)), "FileCreated");
    await this.listDirectory(dir);
    return this.openFile(relative);
  }
  async setWorkspaceDirectory(root: string, create = false) {
    const current = this.state.activeWorkspace;
    if (!root.trim() || !hostPaths(root).isAbsolute(root)) throw new Error("Enter an absolute path on the daemon host.");
    const id = create || !current ? crypto.randomUUID() : current.id;
    await this.run(this.client.request("Workspace", create || !current ? { CreateWorkspace: { workspace_id: id, root_dir: root } } : { SetWorkspaceRoot: { workspace_id: id, root_dir: root } }));
    await this.refresh();
    const workspace = this.state.workspaces.find(workspace => workspace.id === id);
    if (workspace) await this.selectWorkspace(workspace);
  }
  showHome = () => { this.fileRequest++; this.update({ activePath: undefined, loading: false }); };
  selectBuffer = (path: string) => { this.fileRequest++; this.update({ activePath: path, loading: false }); };
  cycleBuffer(delta: number) {
    const paths = [undefined, ...this.state.buffers.map(buffer => buffer.path)];
    const next = paths[(paths.indexOf(this.state.activePath) + delta + paths.length) % paths.length];
    if (next) this.selectBuffer(next); else this.showHome();
  }
  cycleWorkspace(delta: number) {
    const workspaces = this.state.workspaces;
    if (!workspaces.length) return;
    const index = workspaces.findIndex(workspace => workspace.id === this.state.activeWorkspace?.id);
    void this.selectWorkspace(workspaces[(index + delta + workspaces.length) % workspaces.length]!);
  }
  key(key: Parameters<TerminalEditor["key"]>[0]) {
    const buffer = this.activeBuffer;
    if (!buffer) return;
    try { const frame = buffer.editor.key(key); this.afterInput(buffer); return frame; }
    catch (error) { this.report(error); }
  }
  paste(text: string) { const buffer = this.activeBuffer; if (buffer) { buffer.editor.paste(text); this.afterInput(buffer); } }
  callEditor(op: string, args: Record<string, unknown> = {}) {
    const buffer = this.activeBuffer;
    if (!buffer) return;
    try { const frame = buffer.editor.call(op, args); buffer.editor.follow(); this.afterInput(buffer); return frame; }
    catch (error) { this.report(error); }
  }
  private afterInput(buffer: EditorBuffer) {
    this.documents?.flush(buffer.editor);
    this.changed(buffer.editor);
  }
  async saveBuffer(buffer = this.activeBuffer) {
    if (!buffer) return;
    if (!this.documents) { this.report(new Error("Document synchronization is not available.")); return false; }
    buffer.saving = true; this.update({ buffers: [...this.state.buffers], error: undefined });
    try {
      await this.documents.save(buffer.editor);
      this.client.send("Editor", { DidSave: { path: buffer.absolutePath } }, buffer.root);
      this.update({ notice: `Saved ${buffer.path}` });
      return true;
    } catch (error) { this.report(error); return false; }
    finally { buffer.saving = false; this.changed(buffer.editor); }
  }
  async saveAll() { for (const buffer of this.pool.values()) if (buffer.editor.frame.dirty && !await this.saveBuffer(buffer)) return false; return true; }
  closeBuffer(buffer = this.activeBuffer, force = false) {
    if (!buffer) return;
    if (buffer.editor.frame.dirty && !force) { this.update({ confirmation: { kind: "close", buffer } }); return; }
    this.fileRequest++;
    const buffers = this.state.buffers.filter(current => current !== buffer);
    this.update({ buffers, activePath: this.state.activePath === buffer.path ? buffers.at(-1)?.path : this.state.activePath, confirmation: undefined });
    this.remember();
    if (![...this.views.values()].some(view => view.buffers.includes(buffer))) {
      this.documents?.detach(buffer.editor);
      this.lsp.close(buffer.id); this.lspTexts.delete(buffer.id);
      for (const listener of this.bufferListeners) listener(buffer, true);
      this.pool.delete(buffer.id); buffer.editor.dispose();
    }
  }
  requestQuit(force = false) {
    if (!force && [...this.pool.values()].some(buffer => buffer.editor.frame.dirty)) this.update({ confirmation: { kind: "quit" } });
    else this.update({ quit: true });
  }
  cancelConfirmation = () => this.update({ confirmation: undefined });
  async resolveConfirmation(save: boolean) {
    const confirmation = this.state.confirmation;
    if (!confirmation) return;
    if (confirmation.kind === "quit") {
      if (save && !await this.saveAll()) return;
      this.update({ confirmation: undefined, quit: true });
    } else {
      if (save && !await this.saveBuffer(confirmation.buffer)) return;
      this.closeBuffer(confirmation.buffer, true);
    }
  }
  private started = false;
  private watchGit() {
    const root = this.state.activeWorkspace?.root_dir;
    if (this.client.state.phase !== "connected" || !root) return;
    if (this.gitWatch) try { this.client.send("Git", { UnwatchStatus: { token: this.gitWatch.token } }, this.gitWatch.root); } catch {}
    this.gitWatch = { root, token: `tui-git:${crypto.randomUUID()}` };
    this.update({ git: undefined });
    try { this.client.send("Git", { WatchStatus: { token: this.gitWatch.token } }, root); } catch (error) { this.report(error); }
  }
  async applyMutation(result: LspMutationResult) {
    for (const file of result.edits) {
      const buffer = this.allBuffers.find(buffer => buffer.absolutePath === file.path);
      if (!buffer) throw new Error(`No owned editor for LSP edit: ${file.path}`);
      buffer.editor.call("apply-edits", { edits: file.edits.map(edit => ({ ...edit, text: edit.new_text })) });
      this.afterInput(buffer);
    }
    this.update({ notice: result.title || `LSP updated ${result.edits.length + result.appliedFiles.length} file(s)` });
  }
  async setTheme(name: string) {
    if (!Object.hasOwn(palettes, name)) return;
    try {
      persistTheme(this.config, name);
      this.config.themeName = name; this.config.theme = palettes[name as keyof typeof palettes];
      this.update({ notice: `Theme: ${name}` });
    } catch (error) { this.report(error); }
  }
  setVim(enabled: boolean) {
    try {
      persistConfigValue(this.config, ["editor", "vim-mode"], enabled);
      this.config.vim = enabled;
      for (const buffer of this.pool.values()) buffer.editor.call("toggle-vim", { enabled });
      this.update({ buffers: [...this.state.buffers] });
    } catch (error) { this.report(error); }
  }
}