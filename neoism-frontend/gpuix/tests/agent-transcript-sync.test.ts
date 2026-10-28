import {expect,test} from "bun:test";
import {FamilyStreamOwnership,CompletedTranscriptRefresh} from "../src/agent/transcript-sync";
import {coreCall} from "../src/native-core";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {createAgentDaemon} from "./support/agent-daemon";
import type {PaintCommand} from "../src/ui/PaintLayer";

test("one family stream owns parent/child/sibling tokens, including late switch replies and newly discovered ancestry",()=>{
 const roots:Record<string,string>={root:"root",child:"root",sibling:"root"},owner=new FamilyStreamOwnership(id=>roots[id]||id);
 expect(owner.activate("root")).toEqual([]);expect(owner.activate("other")).toEqual([]);
 expect(owner.activate("child")).toEqual(["root"]);expect(owner.switched("root")).toEqual(["root"]);
 expect(owner.activate("sibling")).toEqual(["child"]);expect(owner.switched("other")).toEqual([]);
 expect(owner.activate("unknown")).toEqual([]);roots.unknown="root";expect(owner.reconcile()).toEqual(["sibling"]);
 expect(owner.activate("root")).toEqual(["unknown"]);expect(owner.switched("unknown")).toEqual(["unknown"]);
});
test("completed metadata refresh is distinct from incremental parts and repeated idle echoes",()=>{
 const sync=new CompletedTranscriptRefresh(),delta={ContentDelta:{session_id:"s",text:"never "}};
 expect(sync.observe(delta)).toBeUndefined();expect(sync.observe(delta)).toBeUndefined();
 expect(sync.observe({SessionIdle:{session_id:"s"}})).toBe("s");expect(sync.observe({SessionIdle:{session_id:"s"}})).toBeUndefined();
 const completed={SessionEvent:{session_id:"s",kind:"message.updated",properties:{info:{id:"m",role:"assistant",time:{completed:1000},finish:"stop"}}}};
 expect(sync.observe(completed)).toBe("s");expect(sync.observe(completed)).toBeUndefined();
 expect(sync.observe({SessionEvent:{...completed.SessionEvent,properties:{info:{...completed.SessionEvent.properties.info,id:"tool-step",finish:"tool-calls"}}}})).toBeUndefined();
});
test("closing a family owner tombstones late switch replies without reviving older members or stopping another family",()=>{
 const owner=new FamilyStreamOwnership(id=>id==="child"?"parent":id);
 owner.activate("parent");owner.activate("other");owner.deactivate("parent");
 expect(owner.switched("parent")).toEqual(["parent"]);expect(owner.reconcile()).toEqual([]);
 expect(owner.activate("child")).toEqual([]);expect(owner.switched("parent")).toEqual(["parent"]);expect(owner.switched("other")).toEqual([]);
 owner.deactivate("child");expect(owner.switched("child")).toEqual(["child"]);
 expect(owner.activate("parent")).toEqual([]);expect(owner.switched("parent")).toEqual([]);
});
const until=async(predicate:()=>boolean)=>{const end=Date.now()+4000;while(!predicate()){if(Date.now()>end)throw Error("Agent stream fixture timed out");await Bun.sleep(5);}};
test("real daemon envelopes keep family deltas once, format partial Markdown before idle, then paint original completed metrics",async()=>{
 const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/gpuix-response-sync"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const{id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"two",rootSessionId:"one",theme:config.themeName,scale:1,fontFamily:config.fontFamily});
 const event=(message:unknown)=>coreCall("agenttimeline.event",{id,message});const off=model.subscribeAgentEvents(event);
 try{
  model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
  daemon.push({CatalogFamily:{session_id:"one",snapshot:{directory:"/project/a",executionId:"run",familyRevision:1,branches:[{sessionId:"two",parentSessionId:"one",title:"Child",agent:"build",status:"outstanding",depth:1,permissionPending:false}]}}});await until(()=>model.agentFamily("two").root==="one");
  model.openThread(daemon.threads[0]);await until(()=>daemon.agentStreams.has("one"));model.openThread(daemon.threads[1]);await until(()=>daemon.agentStreams.has("two")&&!daemon.agentStreams.has("one"));
  expect(daemon.requests.filter(request=>request.tag==="StopStream").map(request=>request.body.session_id)).toContain("one");
  coreCall("agenttimeline.family",{id,rootSessionId:"one",sessionId:"two",roster:[{id:"two",parent:"one",title:"Child",status:"running",depth:1}]});
  let text="",pieces=0;const row=()=>({id:"answer",role:"Assistant",kind:"Assistant",text});
  for(const piece of ["# St","ream\n\n","Rust ","is ","**","fast","** and ","`safe","`.\n\n","- never ","never ","duplicate."]){
   // Model the real server: each subscription to any member carries this
   // entire family. A second visited-member stream would duplicate bytes.
   for(const member of daemon.agentStreams)if(["one","two"].includes(member))daemon.push({ContentDelta:{session_id:"two",message_id:"answer",parent_message_id:"message",kind:"Text",text:piece}});
   text+=piece;await until(()=>model.state.history.two?.find(message=>message.id==="answer")?.text===text);
   if(++pieces%3===0){daemon.push({MessageUpdated:{session_id:"two",parent_message_id:"message",message:row()}});await Bun.sleep(8);}
  }
  daemon.push({MessageUpdated:{session_id:"two",parent_message_id:"message",message:row()}});await Bun.sleep(650);
  const live=coreCall<{commands:PaintCommand[]}>("agenttimeline.frame",{id}),runs=live.commands.filter((command):command is Extract<PaintCommand,{kind:"text"}>=>command.kind==="text"),paint=runs.map(command=>command.text).join("");
  expect(model.state.streaming.two).not.toBe("Idle");expect(paint).toContain("Rust is fast and safe.");expect(paint).toContain("never never duplicate.");expect(paint).not.toContain("RustRust");expect(paint).not.toContain("**");expect(paint).not.toContain("`safe`");expect(runs.some(command=>command.text==="fast"&&command.bold)).toBe(true);
  const status="Build · GPT-6.1 Sol · 3m 38s · 32.0 tok/s";daemon.history.set("two",[{...row(),status}]);const historyBefore=daemon.requests.filter(request=>request.tag==="GetHistory").length;
  daemon.push({SessionIdle:{session_id:"two"}});daemon.push({SessionIdle:{session_id:"two"}});daemon.push({SessionEvent:{session_id:"two",kind:"message.updated",properties:{info:{id:"message",role:"assistant",time:{completed:219000},finish:"stop"}}}});
  await until(()=>model.state.history.two?.find(message=>message.id==="answer")?.status===status);await Bun.sleep(650);
  const finished=coreCall<{commands:PaintCommand[]}>("agenttimeline.frame",{id}),finishedText=finished.commands.filter(command=>command.kind==="text").map(command=>command.text).join("");expect(finishedText).toContain(status);expect(daemon.requests.filter(request=>request.tag==="GetHistory").length-historyBefore).toBe(1);
  expect(model.state.history.two?.find(message=>message.id==="answer")?.text).toBe(text);expect(daemon.ptyRequests).toEqual([]);expect(daemon.requests.some(request=>request.tag==="SubmitPrompt")).toBe(false);
  const tab=model.state.surfaces.find(surface=>surface.kind==="agent"&&surface.sessionId==="two")!;model.closeSurface(tab.id);await until(()=>!daemon.agentStreams.has("two"));
  // SwitchThread verifies over HTTP before binding: an old reply can arrive
  // after the tab was closed. It must be stopped again, not resurrected.
  daemon.agentStreams.add("two");daemon.push({ThreadSwitched:{session_id:"two"}});await until(()=>!daemon.agentStreams.has("two"));expect(model.state.surfaces.some(surface=>surface.id===tab.id)).toBe(false);
 }finally{off();model.dispose();client.stop();daemon.stop();coreCall("agenttimeline.dispose",{id});}
},10000);
