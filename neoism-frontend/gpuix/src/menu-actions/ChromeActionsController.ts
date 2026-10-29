import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import {ServerHandshakeError} from "../daemon/server-connection";
import {writeClipboard} from "../clipboard";
import {serverJob,serverDraftValues,serverFormOperation,type ServerDraft,type ServerEntry} from "./server-actions";
import type {SavedSystem,SshHost} from "../daemon/ssh-transport";
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

export interface ServerSelection { signal?:AbortSignal; id: string; endpoint: string; token?: string; pairingToken?: string; authToken?:string; workspaceId?:string; ssh?:{systemId?:string;target?:string;args?:readonly string[]}; temporary?:boolean;hosted?:boolean;invite?:string;name?:string }
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
  private serverDraft?:ServerDraft;
  private serverPending=false;
  private serverGeneration=0;
  private serverConnectionAbort?:AbortController;
  private systems:SavedSystem[]=[];
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
  close = () => { this.serverGeneration++;this.serverPending=false; this.serverConnectionAbort?.abort();this.serverConnectionAbort=undefined; this.overlayRequest?.abort(); this.directorySearch?.abort(); clearTimeout(this.directoryTimer); this.disposePanel(false); this.emit(); };
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
          if (this.options.config.collaborative || !localDaemon(this.options.config.endpoint)) throw new Error("Background discovery and image preview are unavailable for this remote host; the daemon does not expose its background catalog");
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
        case "OpenServers": case "ShowServers": this.openServers(); return;
        case "OpenSystems": case "ShowSystems": this.openSystems(); return;
        case "AddServer": case "CreateServer": this.beginServer(); return;
        case "AddSystem": this.create("system-form"); return;
        case "LeaveWorkspace": await this.options.onConnectServer?.({id:"local",endpoint:""});return;
        default: if (this.options.onRunAction) await this.options.onRunAction(action); else throw new Error(`This host cannot execute ${action}`);
      }
    } catch (error) { if (!this.disposed && !this.overlayRequest?.signal.aborted) this.options.model.report(error); }
  }
  private openServers(){
    const config=this.options.config,{entries}=coreCall<{entries:ServerEntry[]}>("server.list");
    const local=!(config.collaborative??!localDaemon(config.endpoint));
    this.create("servers",{entries:[{id:"local",name:"Local Server",address:local?publicEndpoint(config.endpoint):"This computer",local:true,status:local?this.options.model.client.state.phase:"closed",active:local},...entries.map(entry=>({...entry,active:config.serverId===entry.id||(!config.serverId&&entry.address===config.endpoint),status:config.serverId===entry.id?this.options.model.client.state.phase:entry.online===false?"closed":"unknown"}))]});
  }
  private openSystems(selected?:string){
    const result=coreCall<{systems:SavedSystem[];hosts:SshHost[]}>("ssh.systems.list");this.systems=result.systems;
    const entries=[{id:"local",name:"This computer",target:"Local system",local:true,enabled:true,connection:this.options.config.serverId?.startsWith("system:")?"disconnected":"connected"},...result.systems];
    if(selected){const entry=entries.find(entry=>entry.id===selected);if(!entry)throw new Error("System no longer exists");this.create("system-settings",{entries:[entry]});}else this.create("systems",{entries});
  }
  private beginServer(draft?:Partial<ServerDraft>){
    this.serverDraft={mode:"host",fast:false,directory:(this.options.config.collaborative??!localDaemon(this.options.config.endpoint))?process.env.HOME||"":this.options.model.state.activeWorkspace?.root_dir||process.env.HOME||"",address:"",name:"",password:"",...draft};this.create("server-form",{...this.serverDraft});
  }
  private async connect(server:ServerSelection){
    if(!this.options.onConnectServer)throw new Error("Server switching is unavailable in this host");
    this.serverConnectionAbort?.abort();const abort=new AbortController();this.serverConnectionAbort=abort;
    this.create("server-message",{title:"Connecting",body:server.name||"Connecting to the selected system…",busy:true});
    try{
      try{await this.options.onConnectServer({...server,signal:abort.signal});}
      catch(error){
        if(!abort.signal.aborted&&server.hosted&&!server.temporary&&error instanceof ServerHandshakeError&&["waiting","timeout"].includes(error.phase)){
          const ready=await serverJob("rehost",{serverId:server.id});if(!abort.signal.aborted)await this.options.onConnectServer({...ready,signal:abort.signal});
        }else throw error;
      }
      if(!this.disposed)this.close();
    }catch(error){if(abort.signal.aborted)return;if(!this.disposed)this.openServers();throw error;}
    finally{if(this.serverConnectionAbort===abort)this.serverConnectionAbort=undefined;}
  }
  private async serverAction(action:Record<string,any>):Promise<boolean>{
    switch(action.kind){
      case "servers":this.openServers();return true;
      case "systems":this.openSystems();return true;
      case "server-add":this.beginServer();return true;
      case "server-edit":{const entry=coreCall<ServerSelection>("server.edit",{serverId:action.id});this.beginServer({mode:"join",serverId:action.id,address:entry.endpoint,name:entry.name,password:entry.pairingToken||""});return true;}
      case "server-remove":case "system-remove":this.create("server-confirm",{serverId:action.id,system:action.kind==="system-remove",title:action.kind==="system-remove"?"Remove system?":"Remove server?",body:action.kind==="system-remove"?"Remove this saved SSH connection?":"Remove this saved connection? A server hosted here will stop and disconnect its clients."});return true;
      case "confirm-remove":{if(this.options.config.serverId===(action.system?`system:${action.id}`:action.id))await this.connect({id:"local",endpoint:""});if(action.system){coreCall("ssh.systems.remove",{systemId:action.id});if(!this.disposed)this.openSystems();}else{await serverJob("remove",{serverId:action.id});if(!this.disposed)this.openServers();}return true;}
      case "server-invite":{const entry=coreCall<ServerSelection>("server.resolve",{serverId:action.id});if(!entry.invite)throw new Error("This server has no Fast Server invite");await writeClipboard(entry.invite);return true;}
      case "server":await this.connect(action.id==="local"?{id:"local",endpoint:""}:coreCall<ServerSelection>("server.resolve",{serverId:action.id}));return true;
      case "system-add":this.create("system-form");return true;
      case "system-settings":this.openSystems(action.id);return true;
      case "system-connect":if(action.id==="local")await this.connect({id:"local",endpoint:""});else await this.connect({id:`system:${action.id}`,endpoint:"",ssh:{systemId:action.id}});return true;
      case "system-disconnect":if(this.options.config.serverId===`system:${action.id}`)await this.connect({id:"local",endpoint:""});else this.openSystems(action.id);return true;
      case "system-enabled":coreCall("ssh.systems.set-enabled",{systemId:action.id,enabled:!!action.enabled});if(action.enabled)await this.connect({id:`system:${action.id}`,endpoint:"",ssh:{systemId:action.id}});else if(this.options.config.serverId===`system:${action.id}`)await this.connect({id:"local",endpoint:""});else this.openSystems(action.id);return true;
      case "form-mode":{if(!this.serverDraft)return true;this.serverDraft=serverDraftValues(this.serverDraft,action.values||[]);this.serverDraft.mode=action.mode;this.serverDraft.fast=!!action.fast;this.create("server-form",{...this.serverDraft});return true;}
      case "form-submit":{
        if(this.serverPending)return true;
        if(action.system){const fields=Object.fromEntries(action.values||[]);coreCall("ssh.systems.add",{target:String(fields.system_target||""),name:String(fields.system_name||"")});this.openSystems();return true;}
        if(!this.serverDraft)return true;
        this.serverDraft=serverDraftValues(this.serverDraft,action.values||[]);const draft={...this.serverDraft},operation=serverFormOperation(draft);
        const generation=this.serverGeneration;
        this.serverPending=true;this.create("server-message",{title:draft.mode==="host"?"Starting server":"Joining server",body:draft.fast?"Preparing the Fast Server connection…":"Preparing the workspace connection…",busy:true});
        try{const selected=await serverJob(operation.action,operation.args);if(!this.disposed&&generation===this.serverGeneration)await this.connect(selected);}
        catch(error){if(this.disposed||generation!==this.serverGeneration)return true;this.beginServer(draft);throw error;}finally{if(generation===this.serverGeneration)this.serverPending=false;}return true;
      }
      default:return false;
    }
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
      default: if(await this.serverAction(action))return;
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
    if(await this.serverAction(target))return;
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
    const document = await this.configDocument(true); if (!document.writable) throw new Error("This daemon config document is read-only");
    const updates = target.kind === "theme" ? themeUpdates(document, target.name) : target.kind === "background" ? [
      ["ui.window.background-image-disabled", target.id === "none"],
      ["ui.window.background-image", target.id === "inherit" || target.id === "none" ? null : { path: target.path, opacity: target.opacity, blur: target.blur, fit: target.fit, "position-x": target.position_x, "position-y": target.position_y }],
    ] as [string, unknown][] : undefined;
    if (!updates) return;
    expectVariant(await this.request({ SaveConfigDocument: editConfigDocument(document, updates) }), "ConfigDocumentSaved"); this.close(); await this.options.reload();
  }
}
