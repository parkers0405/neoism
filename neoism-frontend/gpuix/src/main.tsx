import { readFileSync } from "node:fs";
import { Effect, ManagedRuntime } from "effect";
import { createRenderer, render } from "@gpuix/solid";
import { createSignal } from "solid-js";
import { startFrameLoop, type FrameLoop } from "@gpuix/native/runtime";
import type { GpuixRenderer } from "@gpuix/native";
import cascadia from "../../../sugarloaf/src/font/resources/CascadiaCode/CascadiaCodeNF-Regular.otf" with { type: "file" };
import pixel from "../../../sugarloaf/src/font/resources/PressStart2P/PressStart2P-Regular.ttf" with { type: "file" };
import { loadConfig, type AppConfig } from "./config";
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
import {localDaemon} from "./menu-actions/web-frontend";

if (process.argv.includes("--help")) {
  console.log(`Neoism GPUIX — native SolidJS frontend\n\nUsage: npm run start -- [options]\n\n  --daemon URL       ws://host:7878/session or ws+unix:///path/neoism.sock:/session\n  --workspace ID     Open a particular existing daemon workspace\n  --config PATH      Read appearance from a Neoism JSONC config\n  --theme NAME       Override the configured theme\n  --font-size SIZE   Override the configured font size\n  --maximized        Start with a maximized window\n  --reduced-motion  Disable transitions\n\nCredentials: NEOISM_DAEMON_TOKEN (upgrade), NEOISM_PAIRING_TOKEN (Hello).\nDefault: local Neoism Unix socket if present; otherwise 127.0.0.1:7878.\nShortcuts: Alt+E files, Alt+C conversations, Alt+H details, Ctrl+P find, Ctrl+T terminal.`);
  process.exit(0);
}

export interface ServerSelection { id: string; endpoint: string; pairingToken?: string; token?: string }
type HotState = { state: AppState; editors: Map<string, NativeEditor>; config?: AppConfig; recovery?: SavedSession };
const stateKey = Symbol.for("neoism/gpuix/state");
const stateGlobals = globalThis as typeof globalThis & { [stateKey]?: HotState };
const hotState = stateGlobals[stateKey]; delete stateGlobals[stateKey];

async function createSession(config: AppConfig, seed?: HotState) {
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
let session = await createSession(hotState?.config || loadConfig(), hotState);
const hostKey = Symbol.for("neoism/gpuix/native-host");
const globals = globalThis as typeof globalThis & { [hostKey]?: { renderer: GpuixRenderer; loop: FrameLoop; displayClock?: DisplayClock; shutdown?: () => Promise<void> } };
let host = globals[hostKey];
if (!host) {
  const renderer: GpuixRenderer = createRenderer((event) => { profileInput(event);notifyFrameActivity(renderer, event); });
  renderer.init({ title: "Neoism GPUIX", appName: "Neoism", appId: "ai.neoism.gpuix", width: 1440, height: 900,
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
host.shutdown = () => session.dispose();
installFrameMeter(host.renderer);
let transition = Promise.resolve();
function update(work: () => Promise<void>) {
  const next = transition.then(work); transition = next.catch(error => session.model.report(error)); return next;
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
      presenceDisplayName:localDaemon(previous.endpoint)?next.presenceDisplayName:previous.presenceDisplayName });
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
async function connectServer(server: ServerSelection) {
  return update(async () => {
    let next: AppConfig;
    if (server.id === "local") {
      const args: string[] = [];
      for (let i = 2; i < process.argv.length; i++) {
        if (["--daemon", "--workspace"].includes(process.argv[i])) { i++; continue; }
        args.push(process.argv[i]);
      }
      next = loadConfig(args, { ...process.env, NEOISM_GPUIX_DAEMON_URL: undefined, NEOISM_DAEMON_TOKEN: undefined, NEOISM_PAIRING_TOKEN: undefined });
    } else {
      // Saved registry credentials belong to Workspace.Hello. They never
      // become an upgrade URL token or implicit local daemon credential.
      next = { ...session.config, endpoint: server.endpoint, authToken: undefined, pairingToken: server.pairingToken ?? server.token, workspaceId: undefined };
    }
    root?.unmount(); await session.dispose(); session = await createSession(next); mount();
  });
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
const stop = () => { if (stopping) return; stopping = true; root?.unmount(); host!.loop.stop(); host!.displayClock?.dispose(); void session.dispose().finally(() => process.exit(0)); };
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
  stateGlobals[stateKey] = { state: session.model.state, editors: session.model.editors, config: session.config, recovery: session.model.captureSession() };
  session.checkpoint(); root?.unmount(); void session.dispose(true);
});
