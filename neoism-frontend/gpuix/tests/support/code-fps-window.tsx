import { installDisplayClock } from "../../src/animation/display-clock";
import { IntervalSamples, observeNativeWake } from "./display-timing";
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { App, InProcessBackend, liveRendererAsTest } from "@gpuix/native/automation";
import { readFileSync } from "node:fs";
import { NativeEditor } from "../../src/native-core";
import { EditorView } from "../../src/editor/EditorView";
import { loadConfig } from "../../src/config";
import { AnimationFrameScheduler } from "../../src/animation/frame-scheduler";

const option = (name:string,fallback:number) => {
  const index=process.argv.indexOf(name), value=index<0?fallback:Number(process.argv[index+1]);
  if(!Number.isFinite(value)||value<=0)throw new Error(`${name} requires a positive number`);
  return value;
};
const sampleMs=option("--sample-ms",2200),hz=option("--hz",240);
const renderer=createRenderer();
renderer.init({title:"Neoism code scroll FPS measurement",appId:"ai.neoism.gpuix.code-fps",width:1200,height:800});
renderer.registerFont(readFileSync(new URL("../../../../neoism-paint/src/font/resources/CascadiaCode/CascadiaCodeNF-Regular.otf",import.meta.url)));
const wakeIntervals=observeNativeWake(renderer),displayClock=installDisplayClock(renderer,console.error);
const loop=startFrameLoop(renderer);
const config={...loadConfig(["--config","/nonexistent/neoism-code-fps"],{}),vim:true,cursorBlink:false};
const source=Array.from({length:5000},(_,i)=>`pub fn function_${i}(value: usize) -> String { format!("sample item ${i}: {}", value) }`).join("\n");
const editor=new NativeEditor("code-fps.rs",source,true);
const viewportIntervals=new IntervalSamples();let previousScroll=editor.frame.scrollY;
const unsubscribeViewport=editor.subscribe(frame=>{if(frame.scrollY!==previousScroll){viewportIntervals.sample();previousScroll=frame.scrollY;}});
let calls=0,workMs=0,frames=0,last=0,intervals:number[]=[],batchMs=0,batches=0,batchBytes=0;
const call=editor.call.bind(editor);
editor.call=(op,args={})=>{
  const start=performance.now();if(last)intervals.push(start-last);last=start;
  const frame=call(op,args);calls++;if(op==="frame")frames++;workMs+=performance.now()-start;return frame;
};
const applyBatch=renderer.applyBatch.bind(renderer);
renderer.applyBatch=(json)=>{
  const start=performance.now(),result=applyBatch(json);
  batchMs+=performance.now()-start;batches++;batchBytes+=json.length;return result;
};
const root=render(()=>{
  return <div style={{display:"flex",width:"100%",height:"100%",backgroundColor:config.theme.bg}}>
    <EditorView editor={editor} config={config} active focusToken={0} prompt={()=>{}} save={async()=>{}} report={console.error} onState={()=>{}}/>
  </div>;
},{renderer,keyboardFocusDim:false});
const app=new App(new InProcessBackend(liveRendererAsTest(renderer)));
let input:ReturnType<typeof setInterval>|undefined;
try{
  if(!displayClock.active)throw new Error("Native display clock is unavailable; refusing to label fallback timer results as display-clock FPS");
  await Bun.sleep(300);await app.getByTestId("code-editor").click();
  for(const raster of [false,true]){
    editor.cancelFrame();
    const scheduling=editor as unknown as {animation:AnimationFrameScheduler};
    scheduling.animation.dispose();scheduling.animation=new AnimationFrameScheduler(1000/hz);
    editor.call("raster.configure",{enabled:raster,fontFamily:config.fontFamily,fontSize:config.fontSize,fontWeight:config.fontWeight,colors:{
      Plain:config.theme.fg,Keyword:config.theme.syn_keyword,Type:config.theme.syn_type,String:config.theme.syn_string,
      Number:config.theme.syn_number,Comment:config.theme.syn_comment,Function:config.theme.syn_func,Property:config.theme.syn_property,
      Constructor:config.theme.syn_constructor,Special:config.theme.syn_special,
    }});
    // Negative deltas move down from the initial top in the shared CodePane
    // policy. Positive deltas here would measure a clamped, idle document.
    input=setInterval(()=>editor.queueScroll(-30),20);
    await Bun.sleep(450);renderer.resetDebugFrameOverlayStats();await Bun.sleep(80);
    calls=0;frames=0;workMs=0;last=0;intervals=[];batchMs=0;batches=0;batchBytes=0;
    const before=renderer.getDebugFrameOverlayStats(),start=performance.now(),cpu=process.cpuUsage(),beforeScroll=editor.frame.scrollY;
    wakeIntervals.reset();viewportIntervals.reset();
    await Bun.sleep(sampleMs);
    const elapsed=performance.now()-start,after=renderer.getDebugFrameOverlayStats(),used=process.cpuUsage(cpu);
    clearInterval(input);input=undefined;editor.cancelFrame();intervals.sort((a,b)=>a-b);
    const scrollDistance=editor.frame.scrollY-beforeScroll;
    if(scrollDistance<100)throw new Error(`Code FPS fixture did not scroll: ${scrollDistance}px`);
    console.log(JSON.stringify({raster,requestedFallbackHz:hz,displayClock:displayClock.active,nativeWakeDeliveryIntervals:wakeIntervals.summary(),nativeViewportPublishIntervals:viewportIntervals.summary(),intervalScope:"JS-observed native API callback delivery; editor call intervals below are not presentation timestamps",elapsed,calls,animationFrames:frames,jsFps:calls*1000/elapsed,
      nativeFrames:after.frames-before.frames,nativeFps:(after.frames-before.frames)*1000/elapsed,
      callAvgMs:workMs/calls,callWorkMs:workMs,medianInterval:intervals[Math.floor(intervals.length/2)],p95Interval:intervals[Math.floor(intervals.length*.95)],
      batchMs,batches,batchBytes,cpuMs:(used.user+used.system)/1000,draw:after,window:renderer.getWindowSize(),scrollDistance}));
    await Bun.sleep(150);
  }
}finally{
  clearInterval(input);unsubscribeViewport();root.unmount();editor.dispose();loop.stop();displayClock.dispose();
}
process.exit(0);
