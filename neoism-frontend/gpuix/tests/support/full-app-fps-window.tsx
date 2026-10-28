import { installDisplayClock } from "../../src/animation/display-clock";
import { IntervalSamples, observeNativeWake } from "./display-timing";
import { createRenderer,render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { App as AutomationApp,InProcessBackend,liveRendererAsTest } from "@gpuix/native/automation";
import { readFileSync } from "node:fs";
import { createAgentDaemon } from "./agent-daemon";
import { DaemonConnection } from "../../src/daemon/client";
import { WorkspaceModel } from "../../src/model";
import { App } from "../../src/ui/App";
import { loadConfig } from "../../src/config";
import { installFrameMeter,notifyFrameActivity } from "../../src/chrome/frame-meter";
import { installProfiling,profileInput,profileStart,profileEnd } from "../../src/profiling";
const server=createAgentDaemon({deferCreations:false});
server.files.clear();
server.files.set("code-fps.rs",Array.from({length:5000},(_,i)=>`pub fn function_${i}(value: usize) -> String { format!("sample item ${i}: {}", value) }`).join("\n"));
for(let i=0;i<300;i++)server.files.set(`source-${String(i).padStart(3,"0")}.rs`,`pub const VALUE: usize = ${i};`);
const sampleOption=process.argv.indexOf("--sample-ms");
const sampleMs=sampleOption<0?6000:Number(process.argv[sampleOption+1]);
if(!Number.isFinite(sampleMs)||sampleMs<=0)throw new Error("--sample-ms requires a positive number");
const config={...loadConfig(["--config","/nonexistent/neoism-full-app-fps"],{}),endpoint:server.endpoint,authToken:undefined,vim:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
model.start();client.start();
const renderer=createRenderer(event=>{profileInput(event);notifyFrameActivity(renderer,event);});
renderer.init({title:"Neoism full application FPS measurement",appId:"ai.neoism.gpuix.full-app-fps",width:1440,height:900,titlebarTransparent:true});
for(const name of ["CascadiaCode/CascadiaCodeNF-Regular.otf","PressStart2P/PressStart2P-Regular.ttf"])renderer.registerFont(readFileSync(new URL(`../../../../sugarloaf/src/font/resources/${name}`,import.meta.url)));
installProfiling(renderer);installFrameMeter(renderer);
const wakeIntervals=observeNativeWake(renderer),displayClock=installDisplayClock(renderer,console.error);
const loop=startFrameLoop(renderer);
const ready=async()=>{const limit=Date.now()+10000;while(!model.state.activeWorkspace){if(Date.now()>limit)throw new Error("Workspace did not initialize");await Bun.sleep(10);}await model.openFile("code-fps.rs");};
const root=render(()=><App model={model} config={config}/>,{renderer,keyboardFocusDim:false,onUncaughtError:error=>console.error("FULL APP RENDER ERROR",error)});
const automation=new AutomationApp(new InProcessBackend(liveRendererAsTest(renderer)));
const until=async(test:()=>boolean)=>{const limit=Date.now()+10000;while(!test()){if(Date.now()>limit)throw new Error("Full App FPS fixture did not initialize");await Bun.sleep(10);}};
let input:ReturnType<typeof setInterval>|undefined;
try{
 if(!displayClock.active)throw new Error("Native display clock is unavailable; refusing to label fallback timer results as display-clock FPS");
 await ready();
 const host=automation.getByTestId("code-editor");try{await host.waitFor({timeoutMs:5000});}catch(error){console.error("FULL APP STATE",JSON.stringify({error:model.state.error,surfaces:model.state.surfaces,files:Object.keys(model.state.files),layout:model.getLayout().frame}));throw error;}await host.click();
 const activationIndex=process.argv.indexOf("--activation-wait-ms");
 if(activationIndex>=0){renderer.activateWindow();await Bun.sleep(Number(process.argv[activationIndex+1])||0);}
 const surface=model.state.surfaces.find(s=>s.id===model.state.activeSurfaceId)!;
 const editor=model.getEditor(surface,model.getLayout().frame.focusedLeaf||undefined);
 const viewportIntervals=new IntervalSamples();let previousScroll=editor.frame.scrollY;
 const unsubscribe=editor.subscribe(frame=>{if(frame.scrollY!==previousScroll){viewportIntervals.sample();previousScroll=frame.scrollY;}});
 for(const sidebar of (process.argv.includes("--sidebar-only")?[true]:[true,false])){
  if(!sidebar)await host.press("alt-e");
  await Bun.sleep(500);input=setInterval(()=>editor.queueScroll(-30),20);await Bun.sleep(400);
  renderer.resetDebugFrameOverlayStats();await Bun.sleep(80);
  const before=renderer.getDebugFrameOverlayStats(),start=performance.now(),scroll=editor.frame.scrollY,cpu=process.cpuUsage();
  wakeIntervals.reset();viewportIntervals.reset();
  await Bun.sleep(sampleMs);
  const elapsed=performance.now()-start,after=renderer.getDebugFrameOverlayStats(),used=process.cpuUsage(cpu);
  clearInterval(input);input=undefined;editor.cancelFrame();
  const checkpointStart=profileStart();model.captureSession();profileEnd("fixture.recovery.capture",checkpointStart);
  if(editor.frame.scrollY-scroll<100)throw new Error("Full App FPS fixture did not scroll through the document");
  console.log(JSON.stringify({fullApp:true,sidebar,displayClock:displayClock.active,elapsed,nativeFrames:after.frames-before.frames,nativeFps:(after.frames-before.frames)*1000/elapsed,
    nativeWakeDeliveryIntervals:wakeIntervals.summary(),nativeViewportPublishIntervals:viewportIntervals.summary(),intervalScope:"JS-observed native API callback delivery and viewport publication, not compositor presentation",scrollDistance:editor.frame.scrollY-scroll,cpuMs:(used.user+used.system)/1000,draw:after,window:renderer.getWindowSize(),surfaces:model.state.surfaces.length}));
 }
 unsubscribe();
}finally{clearInterval(input);root.unmount();model.dispose();client.stop();server.stop();loop.stop();displayClock.dispose();}
process.exit(0);
