import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import type {WorkspaceModel} from "../../src/model";

const original=await import("../../src/native-core"),call=original.coreCall;
const calls:Array<{op:string;args:Record<string,unknown>;result:any}>=[];
mock.module("../../src/native-core",()=>({...original,coreCall:(op:string,args:Record<string,unknown>={})=>{
 const result=call(op,args);calls.push({op,args,result});return result;
}}));
const {AgentSidePanelView}=await import("../../src/agent/AgentSidePanelView");
const callbacks=new Set<()=>void>();
const restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
try{
 for(const cacheKey of [undefined,"workspace"]){
  const listeners=new Set<(message:unknown)=>void>(),errors:unknown[]=[];
  const disposal=new Set<()=>void>();
  const model={state:{threads:[],activeWorkspace:{id:"workspace"}},subscribeAgentEvents(listener:(message:unknown)=>void){listeners.add(listener);return()=>listeners.delete(listener);},onWorkspaceClosed(){return()=>{};},onDispose(listener:()=>void){disposal.add(listener);return()=>disposal.delete(listener);},sendAgent(){}} as unknown as WorkspaceModel;
  const host=createRecordingAppRenderer((_,box)=>({...box,width:300,height:600})),root=createRoot(host.renderer,{keyboardFocusDim:false});
  try{
   root.render(()=><AgentSidePanelView model={model} config={loadConfig([],{})} directory="/proj/app" cacheKey={cacheKey} report={error=>errors.push(error)} onAction={()=>{}}/>);
   await host.pump(root);calls.length=0;
   // Large tool/history payloads must not even enter JSON/FFI for a catalog.
   for(const tag of ["HistoryChunk","MessageUpdated","PartRemoved","ContentDelta","ToolUseRequest","ToolUseResult","LocalSessionAdopted"])
    for(const listener of listeners)listener({[tag]:{session_id:"chat",text:"x".repeat(100_000)}});
   for(const listener of listeners)listener({SessionEvent:{session_id:"chat",kind:"message.part.delta",properties:{field:"raw",partType:"tool",delta:"x".repeat(100_000)}}});
   assert.equal(calls.length,0);assert.equal(errors.length,0);
   for(const listener of listeners)listener({ThreadList:{threads:[],requested_cursor:null,next_cursor:null}});
   assert.equal(calls.filter(call=>call.op==="agentsidepanel.event").length,1);
   const scheduled=[...callbacks];callbacks.clear();for(const callback of scheduled)callback();await host.pump(root);calls.length=0;
   const sidebar=host.find("conversation-list")!;
   for(let x=0;x<50;x++)root.dispatch({elementId:sidebar.id,eventType:"mouseMove",x:310+x,y:190});
   const moves=calls.filter(call=>call.op==="agentsidepanel.pointer");
   assert.equal(moves.length,50);assert.ok(moves.every(call=>call.args.deferFrame===true&&call.result===null));
   assert.equal(calls.filter(call=>call.op==="agentsidepanel.frame").length,0);
   assert.equal(callbacks.size,1);
   const next=[...callbacks];callbacks.clear();for(const callback of next)callback();await host.pump(root);
   const frames=calls.filter(call=>call.op==="agentsidepanel.frame");
   assert.equal(frames.length,1);assert.ok(frames[0]!.result.canvas);assert.equal(errors.length,0);
  }finally{root.unmount();callbacks.clear();}
  if(cacheKey){
   calls.length=0;
   for(const listener of listeners)listener({ContentDelta:{session_id:"chat",message_id:"part",delta:"x".repeat(100_000)}});
   assert.equal(calls.length,0);assert.equal(callbacks.size,0);
   for(const listener of listeners)listener({StreamingState:{session_id:"chat",state:"Generating",label:null}});
   assert.equal(calls.filter(call=>call.op==="agentsidepanel.event").length,1);assert.equal(callbacks.size,0);
  }
  for(const dispose of disposal)dispose();assert.equal(listeners.size,0);
 }
 console.log("sidebar traffic and frame coalescing passed");
}finally{restore();}