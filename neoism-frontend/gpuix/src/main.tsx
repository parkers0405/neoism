import { readFileSync } from "node:fs";
import { Effect, ManagedRuntime } from "effect";
import { createRenderer, render } from "@gpuix/solid";
import { createSignal } from "solid-js";
import { startFrameLoop, type FrameLoop } from "@gpuix/native/runtime";
import type { GpuixRenderer } from "@gpuix/native";
import cascadia from "../../../neoism-paint/src/font/resources/CascadiaCode/CascadiaCodeNF-Regular.otf" with { type: "file" };
import pixel from "../../../neoism-paint/src/font/resources/PressStart2P/PressStart2P-Regular.ttf" with { type: "file" };
import { loadConfig, type AppConfig } from "./config";
import {appVersion} from "./design/generated";
import {ensureLocalDaemon} from "./daemon/local-startup";
import {createWindowKeyRouter} from "./commands/window-key-router";
import {FontZoomController} from "./commands/font-zoom";
import { expectVariant, isRecord } from "./daemon/protocol";
import { Daemon, DaemonConnection, daemonLayer } from "./daemon/client";
import { WorkspaceModel, type AppState } from "./model";
import { coreCall, type NativeEditor } from "./native-core";
import { App } from "./ui/App";
import {AppVisualState} from "./ui/AppVisualState";
import { AppGeneration } from "./ui/AppGeneration";
import { SessionStore, type SavedSession } from "./session-store";
import { installFrameMeter, notifyFrameActivity } from "./chrome/frame-meter";
import { installDisplayClock, type DisplayClock } from "./animation/display-clock";
import { installProfiling, profileInput, profileStart, profileEnd } from "./profiling";
import {prepareServerConnection,ServerHandshakeError,LocalServerConfig} from "./daemon/server-connection";
import {SelectedSshReconnect,preserveReconnectFocus} from "./daemon/ssh-reconnect";
import type {SshAttachment} from "./daemon/ssh-transport";
import type {ServerSelection} from "./menu-actions/ChromeActionsController";
import {localDaemon} from "./menu-actions/web-frontend";

if (process.argv.includes("--version") || process.argv.includes("-V")) {
  console.log(`Neoism ${appVersion}`);
  process.exit(0);
}

if (process.argv.includes("--help") || process.argv.includes("-h")) {
  console.log(`Neoism — native workspace client\n\nUsage: neoism [options]\n\n  --daemon URL       ws://host:7878/session or ws+unix:///path/neoism.sock:/session\n  --workspace ID     Open a particular existing daemon workspace\n  --config PATH      Read appearance from a Neoism JSONC config\n  --theme NAME       Override the configured theme\n  --font-size SIZE   Override the configured font size\n  --maximized        Start with a maximized window\n  --reduced-motion  Disable transitions\n\nCredentials: NEOISM_DAEMON_TOKEN (upgrade), NEOISM_PAIRING_TOKEN (Hello).\nDefault: reuse the local daemon or start one when missing.\nShortcuts: Alt+E files, Alt+C conversations, Alt+H details, Ctrl+P find, Alt+T terminal.`);
  process.exit(0);
}

export type {ServerSelection} from "./menu-actions/ChromeActionsController";
let sshAttachment:SshAttachment|undefined;
let releaseReconnectFocus:(()=>void)|undefined;
const sshReconnect=new SelectedSshReconnect(server=>connectServer(server,true),error=>session.model.report(error));
type HotState = { ssh?:SshAttachment; state: AppState; editors: Map<string, NativeEditor>; config?: AppConfig; recovery?: SavedSession };
const stateKey = Symbol.for("neoism/gpuix/state");
const stateGlobals = globalThis as typeof globalThis & { [stateKey]?: HotState };
const hotState = stateGlobals[stateKey]; delete stateGlobals[stateKey];
sshAttachment=hotState?.ssh;

async function createSession(config: AppConfig, seed?: HotState) {
  coreCall("paint.prewarm",{fontFamily:config.fontFamily,fontWeight:config.fontWeight,markdownFontFamily:config.markdownFontFamily});
  for (const warning of config.warnings) console.error(`[neoism-gpuix] ${warning}`);
  const client = new DaemonConnection(config), model = new WorkspaceModel(client, config, seed),visuals=new AppVisualState();
  const store = new SessionStore(config);
  if (!seed) { const recovery = store.read(); if (recovery) model.restoreSession(recovery); }
  let savedDependencies: unknown[] | undefined;
  const checkpoint = () => { const start=profileStart();try {
    const dependencies = model.sessionDependencies();
    if (savedDependencies?.length === dependencies.length && dependencies.every((value,index) => Object.is(value,savedDependencies![index]))) return;
    store.write(model.captureSession()); savedDependencies = dependencies;
  } catch (error) { console.error("[neoism-gpuix] Recovery checkpoint failed:", error instanceof Error ? error.message : "write failed"); }finally{profileEnd("recovery.checkpoint",start);} };
  const timer = setInterval(checkpoint, 2000); timer.unref();
  const runtime = ManagedRuntime.make(daemonLayer(client));
  model.start(); await runtime.runPromise(Daemon);
  let disposed = false;
  return { config, client, model, visuals,checkpoint, async dispose(preserveEditors = false) {
    if (disposed) return; disposed = true; clearInterval(timer); checkpoint(); visuals.dispose();model.dispose(preserveEditors); await runtime.dispose();
  } };
}
let session = await createSession(hotState?.config || await ensureLocalDaemon(loadConfig()), hotState);
const localServerConfig=new LocalServerConfig(session.config);
const hostKey = Symbol.for("neoism/gpuix/native-host");
const globals = globalThis as typeof globalThis & { [hostKey]?: { renderer: GpuixRenderer; loop: FrameLoop; displayClock?: DisplayClock; shutdown?: () => Promise<void> } };
let host = globals[hostKey];
if (!host) {
  const renderer: GpuixRenderer = createRenderer((event) => { profileInput(event);notifyFrameActivity(renderer, event); });
  renderer.init({ title: "Neoism", appName: "Neoism", appId: "ai.neoism.gpuix", width: 1440, height: 900,
    minWidth: 720, minHeight: 480, titlebarTransparent: true, windowBackground: "opaque" });
  for (const path of [cascadia, pixel]) renderer.registerFont(readFileSync(path));
  if (process.argv.includes("--maximized")) renderer.zoomWindow();
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
const generationOf = () => ({ model: session.model, config: { ...session.config } });
const [appGeneration, setAppGeneration] = createSignal(generationOf());
host.shutdown = async () => {sshReconnect.dispose();releaseReconnectFocus?.();try{await session.dispose();}finally{try{sshAttachment?.dispose();}finally{try{coreCall("ssh.shutdown");}finally{coreCall("server.shutdown");}}}};
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
    const prepared=await prepareServerConnection(server,session.config,()=>localServerConfig.resolve(()=>{
      const args:string[]=[];
      for(let i=2;i<process.argv.length;i++){if(["--daemon","--workspace"].includes(process.argv[i])){i++;continue;}args.push(process.argv[i]);}
      return loadConfig(args,{...process.env,NEOISM_GPUIX_DAEMON_URL:undefined,NEOISM_DAEMON_TOKEN:undefined,NEOISM_PAIRING_TOKEN:undefined});
    }),{signal:server.signal});
    if(server.signal?.aborted){prepared.ssh?.dispose();return;}
    session.checkpoint();
    const focus={workspaceId:session.model.state.activeWorkspace?.id,surfaceId:session.model.state.activeSurfaceId};
    if(reconnecting&&prepared.config.transportIdentity===session.config.transportIdentity)prepared.config.workspaceId=focus.workspaceId;
    const previousSsh=sshAttachment;
    // Construct recovery/native state before tearing down the working UI.
    // An invalid recovery file or failed initialization leaves it usable.
    let replacement:Awaited<ReturnType<typeof createSession>>;
    try{replacement=await createSession(prepared.config);}catch(error){prepared.ssh?.dispose();throw error;}
    if(server.signal?.aborted){await replacement.dispose();prepared.ssh?.dispose();return;}
    releaseReconnectFocus?.();releaseReconnectFocus=reconnecting?preserveReconnectFocus(replacement.model,focus):undefined;
    root?.unmount();await session.dispose();session=replacement;sshAttachment=prepared.ssh;sshReconnect.select(server,prepared.ssh);mount();previousSsh?.dispose();
  },!reconnecting);}finally{resume();}
}

const windowKeys=createWindowKeyRouter();
function mount() {
  setAppGeneration(generationOf());
  root = render(() => <AppGeneration value={appGeneration()} background={appGeneration().config.background} backgroundColor={appGeneration().config.theme.bg}>{current => <App model={current.model} config={current.config} visuals={session.visuals} registerWindowKeys={windowKeys.register} reloadConfig={reloadConfig} onZoomFont={delta=>fontZoom.change(delta)} onConnectServer={connectServer} />}</AppGeneration>, {
    renderer: host!.renderer,
    keyboardFocusDim: false,
    onKeyDown:windowKeys.dispatch,
    onUncaughtError: error => console.error("[neoism-gpuix] Render error:", error),
  });
}
mount();
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
  session.checkpoint(); root?.unmount(); void session.dispose(true);
});
