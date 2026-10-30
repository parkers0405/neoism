// Start graphics before evaluating the full workspace/UI module graph. The
// native init promise leaves JS free to load modules and hydrate the daemon.
import { GpuixRenderer } from "@gpuix/native";
import { createRendererState } from "@gpuix/native/host";
import { loadConfig } from "./config";
import { ensureLocalDaemon } from "./daemon/local-startup";
import { notifyFrameActivity } from "./chrome/frame-meter";
import { profileInput, profileStartup } from "./profiling";
import type { WindowBootstrap } from "./window-bootstrap";

if (!process.argv.some(arg => ["--help", "-h", "--version", "-V"].includes(arg))) {
  const config = await ensureLocalDaemon(loadConfig());
  // Keep the runtime's automation client/schema graph out of the graphics
  // kickoff. Use its public event router; main loads the full runtime next.
  const renderer = new GpuixRenderer((error,event) => {
    if(error){console.error("[neoism-gpuix] Native event error:",error);return;}
    try{if(createRendererState(renderer).dispatch(event)){profileInput(event);notifyFrameActivity(renderer,event);}}
    catch(error){console.error("[neoism-gpuix] Native event error:",error);}
  });
  profileStartup("window-init-started");
  const ready = (renderer as typeof renderer & {initAsync(options:Parameters<typeof renderer.init>[0]):Promise<void>}).initAsync({
    title: "Neoism", appName: "Neoism", appId: "ai.neoism.gpuix", width: 1440, height: 900,
    minWidth: 720, minHeight: 480, titlebarTransparent: true, windowBackground: "opaque",
  });
  // main consumes the rejection after module evaluation; attach a handler now
  // so a fast platform failure cannot become an unhandled rejection meanwhile.
  void ready.catch(() => {});
  (globalThis as typeof globalThis & { __neoismWindowBootstrap?: WindowBootstrap }).__neoismWindowBootstrap = { config, renderer, ready };
}
await import("./main");
