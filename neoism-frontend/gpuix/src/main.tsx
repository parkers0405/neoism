import {WindowWorkspaceSessions,localWorkspaceSession} from "./workspaces/window-sessions";
import * as Effect from "effect/Effect";
import { createRenderer, render } from "@gpuix/solid";
import { createSignal } from "solid-js";
import { startFrameLoop, enableAutomation, type FrameLoop } from "@gpuix/native/runtime";
import type { GpuixRenderer } from "@gpuix/native";
import {prepareBundledFonts} from "./bundled-fonts";
import { loadConfig, type AppConfig } from "./config";
import {appVersion} from "./design/generated";
import {ensureLocalDaemon} from "./daemon/local-startup";
import {createWindowKeyRouter} from "./commands/window-key-router";
import {FontZoomController} from "./commands/font-zoom";
import { expectVariant, isRecord } from "./daemon/protocol";
import { DaemonConnection } from "./daemon/client";
import { WorkspaceModel, type AppState } from "./model";
import { coreCall, type NativeEditor } from "./native-core";
import { App } from "./ui/App";
import {AppVisualState} from "./ui/AppVisualState";
import { AppGeneration } from "./ui/AppGeneration";
import { SessionStore, type SavedSession } from "./session-store";
import { installFrameMeter, notifyFrameActivity } from "./chrome/frame-meter";
import { installDisplayClock, type DisplayClock } from "./animation/display-clock";
import { installProfiling, profileInput, profileStart, profileEnd, profileStartup } from "./profiling";
import {prepareServerConnection,ServerHandshakeError,LocalServerConfig} from "./daemon/server-connection";
import {SelectedSshReconnect,preserveReconnectFocus} from "./daemon/ssh-reconnect";
import type {SshAttachment} from "./daemon/ssh-transport";
import type {ServerSelection} from "./menu-actions/ChromeActionsController";
import {localDaemon} from "./menu-actions/web-frontend";
import type { WindowBootstrap } from "./window-bootstrap";

export type {ServerSelection} from "./menu-actions/ChromeActionsController";

async function startApplication(){
profileStartup("modules-ready");
const bootstrapGlobal=globalThis as typeof globalThis & {__neoismWindowBootstrap?:WindowBootstrap};
const bootstrap=bootstrapGlobal.__neoismWindowBootstrap;
delete bootstrapGlobal.__neoismWindowBootstrap;
if(bootstrap)bootstrap.onInput=profileInput;

if (process.argv.includes("--version") || process.argv.includes("-V")) {
  console.log(`neoism ${appVersion}`);
  process.exit(0);
}

if (process.argv.includes("--help") || process.argv.includes("-h")) {
  console.log(`Neoism — native workspace client\n\nUsage: neoism [options]\n       neoism update [--nightly] [--force]\n\n  --daemon URL       ws://host:7878/session or ws+unix:///path/neoism.sock:/session\n  --workspace ID     Open a particular existing daemon workspace\n  --config PATH      Read appearance from a Neoism JSONC config\n  --theme NAME       Override the configured theme\n  --font-size SIZE   Override the configured font size\n  --maximized        Start with a maximized window\n  --reduced-motion  Disable transitions\n\nCredentials: NEOISM_DAEMON_TOKEN (upgrade), NEOISM_PAIRING_TOKEN (Hello).\nDefault: reuse the local daemon or start one when missing.\nShortcuts: Alt+E files, Alt+C conversations, Alt+H details, Ctrl+P find, Alt+T terminal.`);
  process.exit(0);
}

let sshAttachment:SshAttachment|undefined;
let releaseReconnectFocus:(()=>void)|undefined;
const sshReconnect=new SelectedSshReconnect(server=>connectServer(server,true),error=>session.model.report(error));
type HotState = { ssh?:SshAttachment; state: AppState; editors: Map<string, NativeEditor>; config?: AppConfig; recovery?: SavedSession };
const stateKey = Symbol.for("neoism/gpuix/state");
const stateGlobals = globalThis as typeof globalThis & { [stateKey]?: HotState };
const hotState = stateGlobals[stateKey]; delete stateGlobals[stateKey];
sshAttachment=hotState?.ssh;

async function createSession(config: AppConfig, seed?: HotState) {
  coreCall("paint.prewarm",{fontFamily:config.fontFamily,fontWeight:config.fontWeight,markdownFontFamily:config.markdownFontFamily,iconFontFamily:config.iconFontFamily});
  profileStartup("fonts-started");
  for (const warning of config.warnings) console.error(`[neoism-gpuix] ${warning}`);
  const client = new DaemonConnection(config), model = new WorkspaceModel(client, config, seed),visuals=new AppVisualState();
  profileStartup("model-created");
  const store = new SessionStore(config);
  if (!seed) { const recovery = store.read(); if (recovery) model.restoreSession(recovery); }
  profileStartup("recovery-loaded");
  let savedDependencies: unknown[] | undefined;
  let pendingCheckpoint: Promise<void> | undefined;
  const checkpoint = async () => {
    // Final shutdown waits for an in-flight write, then captures any edits made
    // during it. Filesystem latency must never park the render/input thread.
    while (pendingCheckpoint) await pendingCheckpoint;
    const start=profileStart();let write:Promise<void>|undefined;let dependencies:unknown[]|undefined;
    try {
    dependencies = model.sessionDependencies();
    if (savedDependencies?.length === dependencies.length && dependencies.every((value,index) => Object.is(value,savedDependencies![index]))) return;
    write = store.writeAsync(model.captureSession());
    } catch (error) { console.error("[neoism-gpuix] Recovery checkpoint failed:", error instanceof Error ? error.message : "write failed"); }
    finally { profileEnd("recovery.checkpoint",start); }
    if (write) {
      const pending=write.then(()=>{savedDependencies=dependencies;}).catch(error=>{
        console.error("[neoism-gpuix] Recovery checkpoint failed:",error instanceof Error?error.message:"write failed");
      });
      pendingCheckpoint=pending;
      await pending;
      if(pendingCheckpoint===pending)pendingCheckpoint=undefined;
    }
  };
  const timer = setInterval(()=>{void checkpoint();}, 2000); timer.unref();
  model.start(); client.start();
  let disposal: Promise<void> | undefined;
  return { config, client, model, visuals,checkpoint, ssh:undefined as SshAttachment|undefined, server:undefined as ServerSelection|undefined, dispose(preserveEditors = false) {
    return disposal ||= (async () => {
      clearInterval(timer); await checkpoint(); visuals.dispose();model.dispose(preserveEditors); client.stop();
    })();
  } };
}
const initialConfig = hotState?.config || await (bootstrap?.config || ensureLocalDaemon(loadConfig()));
profileStartup("config-ready");
let session = await createSession(initialConfig, hotState);
session.ssh=sshAttachment;
profileStartup("session-created");
const localServerConfig=new LocalServerConfig(session.config);
const hostKey = Symbol.for("neoism/gpuix/native-host");
const globals = globalThis as typeof globalThis & { [hostKey]?: { renderer: GpuixRenderer; loop: FrameLoop; displayClock?: DisplayClock; shutdown?: () => Promise<void> } };
let host = globals[hostKey];
let windowReady:Promise<void>=Promise.resolve();
if (!host) {
  const renderer: GpuixRenderer = bootstrap?.renderer || createRenderer((event) => { profileInput(event);notifyFrameActivity(renderer, event); });
  if(!bootstrap){
    prepareBundledFonts(renderer);
    if(process.platform!=="darwin")(renderer as typeof renderer & {setStartupRenderingReady(ready:boolean):void}).setStartupRenderingReady(false);
  }
  const ready = bootstrap?.ready || (renderer as GpuixRenderer & {initAsync(options:Parameters<GpuixRenderer["init"]>[0]):Promise<void>}).initAsync({ title: "Neoism", appName: "Neoism", appId: "ai.neoism.gpuix", width: 1440, height: 900,
    minWidth: 720, minHeight: 480, titlebarTransparent: true, windowBackground: "opaque" });
  // Build the retained UI while the native thread initializes graphics. Its
  // command queue and requested viewport already exist; first paint can use
  // the real root instead of drawing an empty window and mounting afterward.
  let initialized=false;
  const tick=renderer.tick.bind(renderer);
  renderer.tick=()=>initialized?tick():true;
  windowReady=ready.then(()=>{
    initialized=true;
    if(process.stdin&&!process.stdin.isTTY)enableAutomation(renderer);
    profileStartup("window-created");
    profileStartup("fonts-registered");
    if(process.argv.includes("--maximized"))renderer.zoomWindow();
  });
  void windowReady.catch(()=>{});
  if(process.platform==="darwin")await windowReady;
  installProfiling(renderer);
  installFrameMeter(renderer);
  const displayClock = installDisplayClock(renderer, error => console.error("[neoism-gpuix] Display frame error:", error));
  const loop = startFrameLoop(renderer, {
    onError: error => console.error("[neoism-gpuix] Frame error:", error),
    onTerminated: () => { void (globals[hostKey]?.shutdown?.() || session.dispose()).finally(() => process.exit(0)); },
  });
  if (!displayClock.active) {
    loop.stop(); displayClock.dispose(); await session.dispose();
    throw new Error("Native display frame callbacks are unavailable. Rebuild with npm run build:display.");
  }
  host = { renderer, loop, displayClock }; globals[hostKey] = host;
}
let root: ReturnType<typeof render> | undefined;
// Each App generation gets its own config snapshot: reloads update the
// session config in place, and the outgoing generation stays on screen until
// its replacement has settled (AppGeneration), so it must keep its own values.
const generationOf = () => ({ model: session.model, config: { ...session.config }, visuals:session.visuals });
const [appGeneration, setAppGeneration] = createSignal(generationOf());
function resolveLocalConfig(){
  return localServerConfig.resolve(()=>{
    const args:string[]=[];
    for(let i=2;i<process.argv.length;i++){if(["--daemon","--workspace"].includes(process.argv[i])){i++;continue;}args.push(process.argv[i]);}
    return loadConfig(args,{...process.env,NEOISM_GPUIX_DAEMON_URL:undefined,NEOISM_DAEMON_TOKEN:undefined,NEOISM_PAIRING_TOKEN:undefined});
  });
}
const windowSessions=new WindowWorkspaceSessions(session,{
  activate(next){
    session=next;sshAttachment=next.ssh;localServerConfig.observe(next.config);
    sshReconnect.select(next.server,next.ssh);
    root?.flushSync(()=>setAppGeneration(generationOf()));
  },
  local:async()=>createSession(await ensureLocalDaemon(resolveLocalConfig())),
  dispose:async previous=>{await previous.dispose();previous.ssh?.dispose();},
});
host.shutdown = async () => {
  sshReconnect.dispose();releaseReconnectFocus?.();windowSessions.dispose();
  try{await Promise.all(windowSessions.sessions().map(async current=>{try{await current.dispose();}finally{current.ssh?.dispose();}}));}
  finally{try{coreCall("ssh.shutdown");}finally{coreCall("server.shutdown");}}
};
installFrameMeter(host.renderer);
let transition = Promise.resolve();
function update(work: () => Promise<void>,reportErrors=true) {
  const next = transition.then(work); transition = next.catch(error => {if(reportErrors&&!(error instanceof ServerHandshakeError&&error.phase==="cancelled"))session.model.report(error);}); return next;
}
async function reloadConfig(fontSizeOverride?:number,expectedSession?:typeof session) {
  return update(async () => {
    if(expectedSession&&expectedSession!==session)return;
    const reply = expectVariant(await Effect.runPromise(session.client.request("Config", "GetConfig")), "Config");
    if (!isRecord(reply) || !isRecord(reply.value)) throw new Error("Invalid config reply");
    const next = loadConfig(process.argv.slice(2), process.env, reply.value);
    if(fontSizeOverride!==undefined){next.fontSize=fontSizeOverride;next.scale=fontSizeOverride/14;}
    const previous = session.config;
    const beforeVim = previous.vim;
    // Appearance follows the selected host's config; transport identity remains
    // scoped to the server the user explicitly selected.
    Object.assign(previous, next, { endpoint: previous.endpoint, authToken: previous.authToken, pairingToken: previous.pairingToken, workspaceId: previous.workspaceId,
      serverId:previous.serverId,collaborative:previous.collaborative,transportIdentity:previous.transportIdentity,
      presenceDisplayName:!previous.collaborative&&localDaemon(previous.endpoint)?next.presenceDisplayName:previous.presenceDisplayName });
    if (beforeVim !== previous.vim) session.model.setVimMode(previous.vim);
    // Rebuild components that capture font metrics without clearing the native
    // root first. Both generations commit through its existing mutation queue.
    root?.flushSync(() => setAppGeneration(generationOf()));
  });
}
let zoomSession=session;
const fontZoom=new FontZoomController(()=>session.config.fontSize,async size=>{
  zoomSession=session;
  const reply=await Effect.runPromise(zoomSession.client.request("Config",{SetSetting:{key:"appearance.fonts.size",value:size}}));
  expectVariant(reply,"SettingWritten");
},size=>{
  // Local, immediate: no config round trip before the new size shows. The
  // size is current synchronously so rapid presses accumulate correctly.
  Object.assign(session.config,{fontSize:size,scale:size/14});
  return update(async()=>{root?.flushSync(()=>setAppGeneration(generationOf()));});
});
async function connectServer(server: ServerSelection,reconnecting=false) {
  const resume=reconnecting?()=>{}:sshReconnect.pause();
  try{return await update(async () => {
    localServerConfig.observe(session.config);
    if(!reconnecting){
      const existing=windowSessions.sessions().find(current=>server.id==="local"?localWorkspaceSession(current):
        current.server?.id===server.id&&current.server.endpoint===server.endpoint&&JSON.stringify(current.server.ssh)===JSON.stringify(server.ssh));
      if(existing){
        const target=server.workspaceId&&existing.model.state.workspaces.find(workspace=>workspace.id===server.workspaceId);
        const activation=target?existing.model.activateWorkspace(target):Promise.resolve();
        windowSessions.select(existing);await activation;return;
      }
    }
    const previous=session;
    const prepared=await prepareServerConnection(server,session.config,resolveLocalConfig,{signal:server.signal});
    if(server.signal?.aborted){prepared.ssh?.dispose();return;}
    const focus={workspaceId:previous.model.state.activeWorkspace?.id,surfaceId:previous.model.state.activeSurfaceId};
    if(reconnecting){
      await previous.checkpoint();
      if(prepared.config.transportIdentity===previous.config.transportIdentity)prepared.config.workspaceId=focus.workspaceId;
    }
    let next:typeof session;
    try{next=await createSession(prepared.config);}catch(error){prepared.ssh?.dispose();throw error;}
    next.ssh=prepared.ssh;next.server={...server,signal:undefined};
    if(server.signal?.aborted){await next.dispose();next.ssh?.dispose();return;}
    if(reconnecting){
      releaseReconnectFocus?.();releaseReconnectFocus=preserveReconnectFocus(next.model,focus);
      windowSessions.replace(previous,next);
      await previous.dispose();previous.ssh?.dispose();
    }else{
      // Keep every existing connection/model alive. Opening another host adds
      // its workspace tabs instead of converting the selected local workspace.
      windowSessions.add(next);windowSessions.select(next);
    }
  },!reconnecting);}finally{resume();}
}

const windowKeys=createWindowKeyRouter();
function mount() {
  setAppGeneration(generationOf());
  root = render(() => <AppGeneration value={appGeneration()} background={appGeneration().config.background} backgroundColor={appGeneration().config.theme.bg}>{current => <App model={current.model} config={current.config} visuals={current.visuals} workspaceNavigation={windowSessions} registerWindowKeys={windowKeys.register} reloadConfig={reloadConfig} onZoomFont={delta=>fontZoom.change(delta)} onConnectServer={connectServer} />}</AppGeneration>, {
    renderer: host!.renderer,
    keyboardFocusDim: false,
    onKeyDown:windowKeys.dispatch,
    onUncaughtError: error => console.error("[neoism-gpuix] Render error:", error),
  });
}
profileStartup("app-mount-started");
mount();
root?.flushSync(()=>{});
if(process.platform!=="darwin")(host.renderer as GpuixRenderer & {setStartupRenderingReady(ready:boolean):void}).setStartupRenderingReady(true);
profileStartup("app-mounted");
await windowReady;
let stopping = false;
const stop = () => { if (stopping) return; stopping = true; root?.unmount(); host!.loop.stop(); host!.displayClock?.dispose(); void host!.shutdown!().finally(() => process.exit(0)); };
process.once("SIGINT", stop); process.once("SIGTERM", stop);
// `kill -USR2 <pid>` writes the live UI model (tabs, workspaces, history
// readiness) for diagnosing a state that cannot be reproduced from outside.
process.on("SIGUSR2", () => {
  const state = session.model.state;
  const dump = {
    timelines: coreCall("agenttimeline.diagnostics", {}),
    performance: {draw: host!.renderer.getDebugFrameOverlayStats(),
      presented: (host!.renderer as unknown as {getPresentedFrameSample?:()=>number[]|null}).getPresentedFrameSample?.()},
    connection: state.connection,
    agentDefaults: state.agentDefaults,
    agentDefaultsLoading: state.agentDefaultsLoading,
    catalogsError: state.catalogsError,
    activeWorkspace: state.activeWorkspace && { id: state.activeWorkspace.id, root: state.activeWorkspace.root_dir },
    openWorkspaceIds: state.ui.openWorkspaceIds, activeSurfaceId: state.activeSurfaceId, error: state.error,
    surfaces: state.surfaces.map(surface => ({ id: surface.id, kind: surface.kind, title: surface.title, workspaceId: surface.workspaceId, sessionId: surface.sessionId,
      history: surface.sessionId ? (state.history[surface.sessionId]?.length ?? "unknown") : undefined, streaming: surface.sessionId ? state.streaming[surface.sessionId] : undefined })),
    workspaces: state.workspaces.map(workspace => ({ id: workspace.id, title: workspace.title, root: workspace.root_dir })),
  };
  void Bun.write(`/tmp/neoism-gpuix-state-${process.pid}.json`, JSON.stringify(dump, null, 2));
});
if (import.meta.hot) import.meta.hot.dispose(() => {
  process.removeListener("SIGINT", stop); process.removeListener("SIGTERM", stop);
  stateGlobals[stateKey] = { ssh:sshAttachment,state: session.model.state, editors: session.model.editors, config: session.config, recovery: session.model.captureSession() };
  windowSessions.dispose();sshReconnect.dispose();releaseReconnectFocus?.();
  session.checkpoint(); root?.unmount(); void session.dispose(true);
  for(const current of windowSessions.sessions())if(current!==session)void current.dispose().finally(()=>current.ssh?.dispose());
});

}
export const ready=startApplication();
