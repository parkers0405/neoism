// Start graphics before evaluating the full workspace/UI module graph. The
// native init promise leaves JS free to load modules and hydrate the daemon.
import { GpuixRenderer } from "@gpuix/native";
import { createRendererState } from "@gpuix/native/host";
import {prepareBundledFonts} from "./bundled-fonts";
import { loadConfig } from "./config";
import {coreCall} from "./native-core";
import { ensureLocalDaemon } from "./daemon/local-startup";
import { notifyFrameActivity } from "./chrome/frame-meter";
import { profileInput, profileStartup } from "./profiling";
import type { WindowBootstrap } from "./window-bootstrap";

if (!process.argv.some(arg => ["--help", "-h", "--version", "-V"].includes(arg))) {
  // Keep the runtime's automation client/schema graph out of the graphics
  // kickoff. Use its public event router; main loads the full runtime next.
  let bootstrap:WindowBootstrap;
  const renderer = new GpuixRenderer((error,event) => {
    if(error){console.error("[neoism-gpuix] Native event error:",error);return;}
    try{if(createRendererState(renderer).dispatch(event)){(bootstrap?.onInput||profileInput)(event);notifyFrameActivity(renderer,event);}}
    catch(error){console.error("[neoism-gpuix] Native event error:",error);}
  });
  prepareBundledFonts(renderer);
  if(process.platform!=="darwin")(renderer as typeof renderer & {setStartupRenderingReady(ready:boolean):void}).setStartupRenderingReady(false);
  profileStartup("window-init-started");
  const ready = (renderer as typeof renderer & {initAsync(options:Parameters<typeof renderer.init>[0]):Promise<void>}).initAsync({
    title: "Neoism", appName: "Neoism", appId: "ai.neoism.gpuix", width: 1440, height: 900,
    minWidth: 720, minHeight: 480, titlebarTransparent: true, windowBackground: "opaque",
  });
  // main consumes the rejection after module evaluation; attach a handler now
  // so a fast platform failure cannot become an unhandled rejection meanwhile.
  void ready.catch(() => {});
  // Graphics and service discovery are independent. A cold daemon must not
  // hold up window creation, and simultaneous launches can hydrate in parallel.
  const initialConfig=loadConfig();
  // Resolve the Rust painters' fonts while the UI module graph is loading.
  // Waiting until App construction makes its first chrome paint join these
  // font loaders synchronously, especially when several windows start at once.
  coreCall("paint.prewarm",{fontFamily:initialConfig.fontFamily,fontWeight:initialConfig.fontWeight,markdownFontFamily:initialConfig.markdownFontFamily,iconFontFamily:initialConfig.iconFontFamily});
  const config = ensureLocalDaemon(initialConfig);
  void config.catch(() => {});
  (globalThis as typeof globalThis & { __neoismWindowBootstrap?: WindowBootstrap }).__neoismWindowBootstrap = bootstrap = { config, renderer, ready };
}
await (await import("./main")).ready;
