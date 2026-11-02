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
const callbacks=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
const [visible,setVisible]=createSignal(true);
const tick=()=>{const pending=[...callbacks];callbacks.clear();for(const callback of pending)callback();root.flush();};
try{
 root.render(()=><Show when={visible()}><AnimatedWordmark config={config} hosts={hosts} startupKey="retained-home" testId="logo" style={{width:600,height:100}}/></Show>);root.flush();
 const layout=()=>root.dispatch({elementId:host.find("logo")!.id,eventType:"layout",x:100,y:40,width:600,height:100});layout();root.flush();
 for(const x of [100,340,520,100]){
  box={...box,x};
  root.dispatch({elementId:host.find("logo")!.id,eventType:"mouseMove",x:x+50,y:90});tick();
  assert.deepEqual(frames.at(-1)!.mouse,[50,50],"Hover must use painted sidebar translation even without a layout event");
 }
 setVisible(false);root.flush();box={...box,x:420};setVisible(true);root.flush();layout();root.flush();
 root.dispatch({elementId:host.find("logo")!.id,eventType:"mouseMove",x:470,y:90});tick();assert.deepEqual(frames.at(-1)!.mouse,[50,50]);
 const cached=visuals.agentComposer("home",config,{width:1200,height:700});
 const returned=visuals.agentComposer("home",config,{width:900,height:650});
 assert.equal(returned,cached);assert.deepEqual(returned.viewport,{width:900,height:650});
 assert.deepEqual(visuals.agentComposer("home",config,{width:0,height:0}).viewport,{width:900,height:650});
 console.log("wordmark hover follows painted sidebar offsets and returning homes use current pane size");
}finally{root.unmount();restore();hosts.dispose();visuals.dispose();}
