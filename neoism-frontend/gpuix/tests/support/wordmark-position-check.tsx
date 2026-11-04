import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import {createSignal,Show} from "solid-js";
import * as native from "../../src/native-core";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const frames:Record<string,unknown>[]=[];
const actual=native.coreCall;
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args:Record<string,unknown>)=>{if(op==="wordmark.frame")frames.push(args);return actual(op,args);}}));
const {AnimatedWordmark,WordmarkHosts}=await import("../../src/ui/AnimatedWordmark");
const {AppVisualState}=await import("../../src/ui/AppVisualState");
const config={...loadConfig(["--config","/nonexistent/wordmark-position"],{}),reducedMotion:false,splashAutoAnimate:false};
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),hosts=new WordmarkHosts(),visuals=new AppVisualState();
let box={x:100,y:40,width:600,height:100};host.renderer.getElementBounds=()=>box;
const requests:Array<{delay:number;kind?:string}>=[];
const callbacks=new Set<()=>void>(),restore=usePresentationScheduler((callback,delay,kind)=>{requests.push({delay,kind});callbacks.add(callback);return()=>callbacks.delete(callback);});
const [visible,setVisible]=createSignal(true);
const [kind,setKind]=createSignal<"agent"|"splash">("agent");
const tick=()=>{const pending=[...callbacks];callbacks.clear();for(const callback of pending)callback();root.flush();};
try{
 root.render(()=><Show when={visible()}><AnimatedWordmark config={config} kind={kind()} hosts={hosts} startupKey={`retained-${kind()}`} testId="logo" style={{width:600,height:100}}/></Show>);root.flush();
 const layout=()=>root.dispatch({elementId:host.find("logo")!.id,eventType:"layout",x:100,y:40,width:600,height:100});layout();root.flush();
 for(const x of [100,340,520,100]){
  box={...box,x};
  root.dispatch({elementId:host.find("logo")!.id,eventType:"mouseMove",x:x+50,y:90});tick();
  assert.deepEqual(frames.at(-1)!.mouse,[50,50],"Hover must use painted sidebar translation even without a layout event");
 }
 setVisible(false);root.flush();box={...box,x:420};setVisible(true);root.flush();layout();root.flush();
 root.dispatch({elementId:host.find("logo")!.id,eventType:"mouseMove",x:470,y:90});tick();assert.deepEqual(frames.at(-1)!.mouse,[50,50]);
 for(const surface of ["agent","splash"] as const){
  setVisible(false);root.flush();setKind(surface);setVisible(true);root.flush();layout();root.flush();
  for(let click=0;click<18;click++){
   const before=frames.length;
   const scheduled=requests.length;
   root.dispatch({elementId:host.find("logo")!.id,eventType:"click",x:470,y:90});root.flush();tick();
   assert.ok(frames.length>before,"click replay must produce a frame");
   assert.ok(frames.length-before<=3,"click replay must not recursively redraw");
   assert.ok(requests.slice(scheduled).some(request=>request.kind==="yield-frame"),"polygon replay must yield to input then follow native frames without an FPS cap");
  }
 }
 const cached=visuals.agentComposer("home",config,{width:1200,height:700});
 const returned=visuals.agentComposer("home",config,{width:900,height:650});
 assert.equal(returned,cached);assert.deepEqual(returned.viewport,{width:900,height:650});
 assert.deepEqual(visuals.agentComposer("home",config,{width:0,height:0}).viewport,{width:900,height:650});
 console.log("wordmark hover follows painted sidebar offsets and returning homes use current pane size");
}finally{root.unmount();restore();hosts.dispose();visuals.dispose();}
