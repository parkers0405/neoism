import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { Effect, Schema } from "effect";
import {posix,win32} from "node:path";
import type { ConfigDocument, ConfigDescriptor } from "../../../web/src/workspace/types";
import type { AppConfig } from "../config";
import { publicEndpoint } from "../config";
import type { WorkspaceModel } from "../model";
import { coreCall } from "../native-core";
import { expectVariant, isRecord, TreeSchema } from "../daemon/protocol";
import type { PaintCommand } from "../ui/PaintLayer";
import { backgroundCatalog } from "./backgrounds";
import { editConfigDocument, themeUpdates } from "./config-document";
import { localDaemon, openExternalUrl, startWebFrontend } from "./web-frontend";

export interface ServerSelection { id: string; endpoint: string; token?: string; pairingToken?: string }
export interface MenuFrame {
  kind: string; active: boolean; animating: boolean; commands: PaintCommand[]; query?: string | null;
  rect?: number[] | null;
  image?: { path: string; rect: number[] } | null; target?: Record<string, any> | null; action?: Record<string, any> | null;
  snapshot?: unknown;
}
export interface ChromeActionsOptions {
  model: WorkspaceModel; config: AppConfig; reload: () => void | Promise<void>;
  onConnectServer?: (server: ServerSelection) => void | Promise<void>;
  onRunAction?: (action: string) => void | Promise<void>;
  supportedActions?: () => readonly string[];
  openUrl?: (url: string) => Promise<void>;
}
/** Host effects only; the existing Rust panels own painting, hit tests,
 * search/filtering, selection, scrolling, controls, and keyboard policy. */
export class ChromeActionsController {
  frame?: MenuFrame;
  extensionFrame?: MenuFrame;
  private ids = new Map<boolean, number>();
  private animations = new Map<boolean, AnimationFrameScheduler>();
  private viewports = new Map<boolean, { width: number; height: number }>();
  private listeners = new Set<() => void>();
  private overlayRequest?: AbortController;
  private pageRequest?: AbortController;
  private directorySearch?: AbortController;
  private directoryTimer?: ReturnType<typeof setTimeout>;
  private directoryTarget?: { id: string; root: string };
  private disposed = false;
  constructor(readonly options: ChromeActionsOptions) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private emit() { for (const listener of this.listeners) listener(); }
  private request(message: Parameters<WorkspaceModel["client"]["request"]>[1], signal?: AbortSignal) {
    return Effect.runPromise(this.options.model.client.request("Config", message as never), signal ? { signal } : undefined);
  }
  private async configDocument(ensure = false) {
    const reply = expectVariant(await this.request(ensure ? "EnsureConfigDocument" : "GetConfigDocument"), "ConfigDocument");
    if (!isRecord(reply) || !isRecord(reply.document) || typeof reply.document.content !== "string" || typeof reply.document.revision !== "string" || typeof reply.document.display_path !== "string") throw new Error("Invalid daemon config document");
    return reply.document as unknown as ConfigDocument;
  }
  private create(kind: string, payload: Record<string, unknown> = {}, page = false) {
    if (this.disposed) return;
    this.disposePanel(page);
    const started = performance.now();
    const c = this.options.config, viewport = this.viewports.get(page) || { width: 1440, height: 900 };
    const result = coreCall<{ id: number; frame: MenuFrame }>("menu.new", { kind, scale: c.scale, theme: c.themeName, fontFamily: c.fontFamily, fontWeight: c.fontWeight, extraFamilies: [c.markdownFontFamily, c.iconFontFamily], ...viewport, ...payload });
    this.ids.set(page, result.id); this.setFrame(result.frame, page, started);
  }
  private setFrame(frame: MenuFrame, page: boolean, started = performance.now()) {
    if (page) this.extensionFrame = frame; else this.frame = frame;
    this.emit();
    let animation = this.animations.get(page);
    if (!animation) { animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS); this.animations.set(page, animation); }
    animation.cancel();
    if (frame.animating) animation.request(() => this.call("frame", {}, page), started);
  }
  private disposePanel(page: boolean) {
    this.animations.get(page)?.dispose(); this.animations.delete(page);
    const id = this.ids.get(page); if (id !== undefined) coreCall("menu.dispose", { id }); this.ids.delete(page);
    if (page) this.extensionFrame = undefined; else this.frame = undefined;
  }
  close = () => { this.overlayRequest?.abort(); this.directorySearch?.abort(); clearTimeout(this.directoryTimer); this.disposePanel(false); this.emit(); };
  dispose() { this.disposed = true; this.close(); this.pageRequest?.abort(); this.disposePanel(true); this.listeners.clear(); }
  layout(width: number, height: number, page = false) {
    const previous = this.viewports.get(page); if (previous?.width === width && previous.height === height) return;
    this.viewports.set(page, { width, height }); this.call("layout", { width, height }, page);
  }
  async open(action: string) {
    try {
      this.close(); const abort = new AbortController(); this.overlayRequest = abort;
      switch (action) {
        case "OpenAbout": this.create("about"); return;
        case "OpenSettings": {
          const [valuesReply, schemaReply] = await Promise.all([this.request("GetConfig", abort.signal), this.request("GetConfigSchema", abort.signal)]);
          const values = expectVariant(valuesReply, "Config"), schema = expectVariant(schemaReply, "ConfigSchema");
          if (!isRecord(values) || !isRecord(schema) || !Array.isArray(schema.descriptors)) throw new Error("Invalid daemon settings catalog");
          if (!abort.signal.aborted) { this.create("settings", { values: values.value, descriptors: (schema.descriptors as ConfigDescriptor[]).filter(descriptor => descriptor.path !== "editor.minimap"), snapshot: this.options.model.state.ui.chromeSnapshot }); this.options.model.setUI({ chromeSnapshot: null }); } return;
        }
        case "OpenThemes": case "OpenThemePicker": this.create("themes"); return;
        case "ConfigEditor": { this.options.model.openConfigDocument(await this.configDocument(true)); this.close(); return; }
        case "OpenBackgrounds": case "ListBackgrounds": {
          if (!localDaemon(this.options.config.endpoint)) throw new Error("Background discovery and image preview are unavailable for this remote host; the daemon does not expose its background catalog");
          const [reply, document] = await Promise.all([this.request("GetConfig", abort.signal), this.configDocument()]);
          const config = expectVariant(reply, "Config"); if (!isRecord(config)) throw new Error("Invalid daemon config");
          if (!abort.signal.aborted) this.create("backgrounds", backgroundCatalog(config.value, document.display_path)); return;
        }
        case "OpenExtensions": this.options.model.openPage("extensions"); await this.ensureExtensions(); return;
        case "StartWebServer": await startWebFrontend(this.options.config.endpoint, this.options.openUrl); return;
        case "OpenDirectoryPalette": {
          const workspace = this.options.model.state.activeWorkspace; if (!workspace?.root_dir) throw new Error("Select a workspace first");
          this.directoryTarget = { id: workspace.id, root: workspace.root_dir };
          this.create("directories", { workspaceId: workspace.id, root: workspace.root_dir, supportedActions: this.options.supportedActions?.() || [] }); this.searchDirectories(); return;
        }
        case "OpenWorkspaces": {
          const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await Effect.runPromise(this.options.model.client.request("Workspace", "RequestHostWorkspaceTree"), { signal: abort.signal }), "HostWorkspaceTree"));
          if (!abort.signal.aborted) this.create("workspaces", { entries: tree.workspaces.map((workspace) => ({ id: workspace.id, title: workspace.title, detail: workspace.root_dir || "", hostId: workspace.host_id, hostLabel: tree.hosts.find((host) => host.id === workspace.host_id)?.label || workspace.host_id, current: workspace.id === this.options.model.state.activeWorkspace?.id })) }); return;
        }
        case "OpenServers": case "ShowServers": {
          const config = this.options.config, stored = coreCall<{ entries: unknown[] }>("menu.servers", { endpoint: publicEndpoint(config.endpoint) });
          this.create("servers", { entries: [{ id: "local", name: "Local Server", address: publicEndpoint(config.endpoint), local: true, status: this.options.model.client.state.phase, active: !stored.entries.some((entry) => isRecord(entry) && entry.active === true) }, ...stored.entries.map((entry) => isRecord(entry) ? { ...entry, status: this.options.model.client.state.phase } : entry)] }); return;
        }
        default: if (this.options.onRunAction) await this.options.onRunAction(action); else throw new Error(`This host cannot execute ${action}`);
      }
    } catch (error) { if (!this.disposed && !this.overlayRequest?.signal.aborted) this.options.model.report(error); }
  }
  async ensureExtensions() {
    if (this.extensionFrame || this.pageRequest || this.disposed) return;
    const abort = new AbortController(); this.pageRequest = abort;
    try {
      const data = expectVariant(await this.request("ListExtensions", abort.signal), "Extensions");
      if (!isRecord(data) || !Array.isArray(data.entries)) throw new Error("Invalid daemon extensions inventory");
      if (!abort.signal.aborted) this.create("extensions", { entries: data.entries }, true);
    } catch (error) { if (!abort.signal.aborted) this.options.model.report(error); }
    finally { if (this.pageRequest === abort) this.pageRequest = undefined; }
  }
  call(op: string, args: Record<string, unknown> = {}, page = false) {
    const id = this.ids.get(page); if (id === undefined || this.disposed) return;
    try {
      const started = performance.now(), frame = coreCall<MenuFrame>(`menu.${op}`, { id, ...args }); this.setFrame(frame, page, started);
      if (!frame.active && !page) { this.close(); return; }
      if (frame.kind === "directories" && ["key", "text", "query"].includes(op)) this.searchDirectories();
      if (frame.action) void this.perform(frame.action, frame.target).catch(this.options.model.report);
    } catch (error) { this.options.model.report(error); }
  }
  private searchDirectories() {
    this.directorySearch?.abort(); clearTimeout(this.directoryTimer);
    const target = this.directoryTarget, fullQuery = this.frame?.query || ""; if (!target || !/^cd(?:\s|$)/.test(fullQuery)) return;
    const query = fullQuery.replace(/^cd\s*/, "");
    const abort = new AbortController(); this.directorySearch = abort;
    this.directoryTimer = setTimeout(() => {
      void Effect.runPromise(this.options.model.client.request("Search", { SearchDirectories: { req_id: 0, query, cwd: target.root } }, target.root), { signal: abort.signal }).then((reply) => {
        if (abort.signal.aborted || this.frame?.kind !== "directories") return;
        const data = expectVariant(reply, "SearchDirectoriesResult"); if (!isRecord(data) || !Array.isArray(data.hits)) throw new Error("Invalid directory search results");
        this.call("directories", { entries: data.hits.filter(isRecord).map((hit) => hit.path).filter((path) => typeof path === "string") });
      }).catch((error) => { if (!abort.signal.aborted) this.options.model.report(error); });
    }, 50);
  }
  private async perform(action: Record<string, any>, target?: Record<string, any> | null) {
    switch (action.kind) {
      case "setting": await this.request({ SetSetting: { key: action.key, value: action.value } }); this.options.model.setUI({ chromeAction: "OpenSettings", chromeSnapshot: this.frame?.snapshot }); await this.options.reload(); return;
      case "keybind": await this.request({ SetKeybind: { action: action.action, key: action.key, with: action.with } }); this.options.model.setUI({ chromeAction: "OpenSettings", chromeSnapshot: this.frame?.snapshot }); await this.options.reload(); return;
      case "config-document": { const document = await this.configDocument(true); this.options.model.openConfigDocument(document); this.close(); return; }
      case "url": await (this.options.openUrl || openExternalUrl)(action.url); return;
      case "run": if (action.action === "backgrounds:pick") await this.open("OpenBackgrounds"); else if (["OpenServers", "OpenWorkspaces", "OpenSystems"].includes(action.action)) await this.open(action.action); else if (this.options.onRunAction) await this.options.onRunAction(action.action); else throw new Error(`This host cannot execute ${action.action}`); return;
      case "accept": await this.accept(target); return;
    }
  }
  /** All explicit folder choices use the host-confirmed terminal transition;
   * implicit terminal cwd following remains a separate local navigation rule. */
  async setActiveWorkspaceRoot(path: string) {
    const workspace = this.options.model.state.activeWorkspace; if (!workspace) throw new Error("Select a workspace first");
    await this.options.model.changeWorkspaceDirectory(path, workspace.id);
  }
  private async accept(target?: Record<string, any> | null) {
    if (!target) return;
    if (target.kind === "run") { this.close(); if (this.options.onRunAction) await this.options.onRunAction(target.action); return; }
    if (target.kind === "workspace") { const workspace = this.options.model.state.workspaces.find((workspace) => workspace.id === target.id); if (!workspace) throw new Error("Workspace no longer exists"); await this.options.model.activateWorkspace(workspace); this.close(); return; }
    if (target.kind === "directory") {
      const captured = this.directoryTarget; if (!captured) return;
      const current = this.options.model.state.activeWorkspace;
      if (current?.id !== captured.id || current.root_dir !== captured.root) { this.close(); return; }
      const paths=/^[a-z]:[\\/]/i.test(captured.root)||captured.root.startsWith("\\\\")?win32:posix;
      if(typeof target.path!=="string")throw new Error("Invalid host directory selection");
      if(!paths.isAbsolute(target.path)&&/^~(?:[\\/]|$)/.test(target.path))throw new Error("Select a home directory result resolved by the host");
      await this.options.model.changeWorkspaceDirectory(paths.resolve(captured.root,target.path), captured.id);
      this.close(); return;
    }
    if (target.kind === "server") {
      if (!this.options.onConnectServer) throw new Error("Server switching is unavailable in this host");
      const server = target.id === "local" ? { id: "local", endpoint: this.options.config.endpoint } : coreCall<ServerSelection>("menu.server-connect", { serverId: target.id });
      this.close(); await this.options.onConnectServer(server); return;
    }
    if (target.kind === "server-add") throw new Error("The GPUIX saved-server editor is not ported yet");
    const document = await this.configDocument(true); if (!document.writable) throw new Error("This daemon config document is read-only");
    const updates = target.kind === "theme" ? themeUpdates(document, target.name) : target.kind === "background" ? [
      ["ui.window.background-image-disabled", target.id === "none"],
      ["ui.window.background-image", target.id === "inherit" || target.id === "none" ? null : { path: target.path, opacity: target.opacity, blur: target.blur, fit: target.fit, "position-x": target.position_x, "position-y": target.position_y }],
    ] as [string, unknown][] : undefined;
    if (!updates) return;
    expectVariant(await this.request({ SaveConfigDocument: editConfigDocument(document, updates) }), "ConfigDocumentSaved"); this.close(); await this.options.reload();
  }
}
