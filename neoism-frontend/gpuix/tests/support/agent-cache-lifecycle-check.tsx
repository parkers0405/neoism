import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import type {WorkspaceModel,Surface} from "../../src/model";

// Observe the real core boundary; never substitute native engine behavior.
const original=await import("../../src/native-core"),call=original.coreCall;
const calls:Array<{op:string;args:Record<string,unknown>;result:any}>=[];
mock.module("../../src/native-core",()=>({...original,coreCall:(op:string,args:Record<string,unknown>={})=>{
 const result=call(op,args);calls.push({op,args,result});return result;
}}));
const {AgentSidePanelView}=await import("../../src/agent/AgentSidePanelView");
const {AgentTimelineView}=await import("../../src/agent/AgentTimelineView");
const callbacks=new Set<()=>void>();
const restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
const config={...loadConfig(["--config","/nonexistent/cache-lifecycle"],{NEOISM_GPUIX_DAEMON_URL:"ws://127.0.0.1:1/session"}),reducedMotion:true};
try{
 for(const kind of ["agenttimeline","agentsidepanel"]){
  const wire=new Set<(message:unknown)=>void>(),surfaceClosed=new Set<(surface:Surface)=>void>(),workspaceClosed=new Set<(id:string)=>void>(),dispose=new Set<()=>void>();
  const updates=new Set<()=>void>();
  const surfaces=["a","b"].map(workspaceId=>({id:`tab-${workspaceId}`,kind:"agent",workspaceId,sessionId:"same-session",title:"Chat"} as Surface));
  const state={surfaces,threads:[],history:{"same-session":[{id:"message",role:"Assistant",kind:"Assistant",text:"Native lifecycle raster leases"}]},streaming:{},pendingAgentSessions:{},ui:{},activeWorkspace:{id:"a"}};
  const model={state,historyCursors:new Map(),subscribeAgentEvents(fn:(message:unknown)=>void){wire.add(fn);return()=>wire.delete(fn);},
   onSurfaceClosed(fn:(surface:Surface)=>void){surfaceClosed.add(fn);return()=>surfaceClosed.delete(fn);},
   onWorkspaceClosed(fn:(id:string)=>void){workspaceClosed.add(fn);return()=>workspaceClosed.delete(fn);},
   onDispose(fn:()=>void){dispose.add(fn);return()=>dispose.delete(fn);},subscribe(fn:()=>void){updates.add(fn);return()=>updates.delete(fn);},
   agentFamily:()=>({root:"same-session",roster:[]}),takePromptArrivals:()=>[],sendAgent(){},report(error:unknown){throw error;},
  } as unknown as WorkspaceModel;
  const errors:unknown[]=[];
  const mount=async(workspaceId:string)=>{
   state.activeWorkspace={id:workspaceId};
   const host=createRecordingAppRenderer((_,box)=>({...box,width:400,height:600}));
   const root=createRoot(host.renderer,{keyboardFocusDim:false});
   let puts=0,drops=0;
   const images=host.renderer as unknown as {putPaintImage:()=>void;dropPaintImage:()=>void};
   images.putPaintImage=()=>{puts++;};images.dropPaintImage=()=>{drops++;};
   root.render(()=>kind==="agenttimeline"
    ?<AgentTimelineView model={model} config={config} sessionId="same-session" workspaceId={workspaceId} surfaceId={`tab-${workspaceId}`} report={error=>errors.push(error)} openFile={()=>{}}/>
    :<AgentSidePanelView model={model} config={config} cacheKey={workspaceId} report={error=>errors.push(error)} onAction={()=>{}}/>);
   await host.pump(root);
   const scheduled=[...callbacks];callbacks.clear();for(const fn of scheduled)fn();await host.pump(root);
   return {root,host,get puts(){return puts;},get drops(){return drops;}};
  };
  const created=()=>calls.filter(c=>c.op===`${kind}.new`);
  const disposed=(id:number)=>calls.filter(c=>c.op===`${kind}.dispose`&&c.args.id===id).length;
  calls.length=0;
  const a=await mount("a"),aId=created().at(-1)!.result.id;
  a.root.unmount();
  assert.equal(disposed(aId),0,"switch/unmount caches an open workspace");
  const b=await mount("b"),bId=created().at(-1)!.result.id;
  assert.notEqual(aId,bId,"same session in another workspace owns a distinct engine");
  assert.equal(wire.size,2);
  const again=await mount("a");assert.equal(created().length,2,"hidden workspace reuses its native engine");
  const view=again.host.find(kind==="agenttimeline"?"conversation":"conversation-list")!;
  again.root.dispatch({elementId:view.id,eventType:"mouseMove",x:320,y:120});
  const pending=callbacks.size;assert.ok(pending>0);
  // A close callback may precede Solid's unmount flush. Stop wire and scheduled
  // work immediately, while retaining resources displayed by the old view.
  if(kind==="agenttimeline")for(const fn of surfaceClosed)fn(surfaces[0]!);
  else for(const fn of workspaceClosed)fn("a");
  assert.equal(wire.size,1);assert.equal(disposed(aId),0);
  assert.equal(callbacks.size,pending-1,"close immediately cancels the old view's scheduled paint");
  const before=calls.length;
  for(const fn of wire)fn({StreamingState:{session_id:"same-session",state:"Generating",label:null}});
  assert.ok(calls.slice(before).every(c=>c.args.id!==aId),"closed engine receives no wire work");
  const replacement=await mount("a"),newId=created().at(-1)!.result.id;
  assert.notEqual(newId,aId);
  again.root.unmount();await Promise.resolve();
  assert.equal(disposed(aId),1);assert.equal(disposed(newId),0,"old cleanup cannot destroy same-ID replacement");
  assert.throws(()=>call(`${kind}.frame`,{id:aId}),/Unknown/);
  assert.doesNotThrow(()=>call(`${kind}.frame`,{id:newId}));
  assert.equal(disposed(bId),0);
  replacement.root.unmount();
  for(const fn of workspaceClosed)fn("a");
  for(const fn of workspaceClosed)fn("a");
  assert.equal(disposed(newId),1);assert.equal(wire.size,1);
  b.root.unmount();assert.equal(disposed(bId),0);
  for(const fn of workspaceClosed)fn("b");
  assert.equal(disposed(bId),1);assert.equal(wire.size,0);
  for(const fn of dispose)fn();
  assert.equal(disposed(aId),1);assert.equal(disposed(newId),1);assert.equal(disposed(bId),1);
  assert.deepEqual(errors,[]);
  assert.equal(callbacks.size,0,"close cancels pending animation work");
  assert.ok(a.puts+again.puts+b.puts>0,"real native frames acquired raster leases");
  assert.equal(a.puts,a.drops);assert.equal(again.puts,again.drops);assert.equal(b.puts,b.drops);assert.equal(replacement.puts,replacement.drops);
 }
 console.log("agent cache lifecycle passed");
}finally{restore();}