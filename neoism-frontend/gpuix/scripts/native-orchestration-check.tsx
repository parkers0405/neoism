// Real App and native renderer; isolated protocol fixture, no provider accounts.
import assert from "node:assert/strict";
import {mkdtempSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App as Automation,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import {App} from "../src/ui/App";
import {AppVisualState} from "../src/ui/AppVisualState";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {installDisplayClock} from "../src/animation/display-clock";
import {createAgentDaemon} from "../tests/support/agent-daemon";

const directory=mkdtempSync(join(tmpdir(),"neoism-orchestration-ui-"));
const daemon=createAgentDaemon({deferCreations:false});
const config={...loadConfig(["--config",join(directory,"unused-config.json")],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const agents=["opencode","claude","codex","general"] as const;
const descriptions=["Implement parser","Review parser","Test parser","Integrate changes"];
const parent=daemon.threads[0]!;
parent.title="Agent orchestration verification";
const created=Date.now();
const task=(agent:string,index:number,status:string)=>({id:`task-${agent}`,role:"Assistant",kind:"Tool",tool:"task",status,created_at:created+index+2,
 agent_kind:agent==="general"?"neoism":agent,title:`Task(@${agent} · ${descriptions[index]})`,text:"Delegated workspace task",
 detail:`task_id: child-${agent}\nstatus: ${status}\n${status==="error"?"Fixture failure for error-state verification":"Provider task result"}`});
const messages=(statuses:string[])=>[
 {id:"request",role:"User",kind:"User",created_at:created,text:"Delegate parser work and integrate the results."},
 {id:"plan",role:"Assistant",kind:"Assistant",created_at:created+1,text:"Neoism coordinates the work. Each task keeps its own runtime and provider conversation."},
 ...agents.map((agent,index)=>task(agent,index,statuses[index]!)),
];
daemon.history.set(parent.session_id,messages(agents.map(()=>"running")).slice(0,2));
for(const agent of agents)daemon.history.set(`child-${agent}`,[{id:`answer-${agent}`,role:"Assistant",kind:"Assistant",text:`${agent} child transcript. This is an isolated UI fixture, not a live model response.`}]);
const renderer=createRenderer();prepareBundledFonts(renderer);
console.error("Native orchestration: initializing renderer");
renderer.init({title:"Neoism orchestration verification",appId:"ai.neoism.gpuix.orchestration-check",width:1280,height:850});
const clock=installDisplayClock(renderer,console.error),loop=startFrameLoop(renderer);
const root=render(()=><App model={model} config={config} visuals={visuals}/>,{renderer});
const app=new Automation(new InProcessBackend(liveRendererAsTest(renderer)));
const until=async(check:()=>boolean|Promise<boolean>)=>{const deadline=Date.now()+12000;while(!await check()){if(Date.now()>deadline)throw Error(`Native orchestration check timed out: ${stage}`);await Bun.sleep(20);}};
const compositor=async(args:string[])=>{const process=Bun.spawn(args,{stdout:"pipe",stderr:"pipe"}),output=await new Response(process.stdout).text();if(await process.exited)throw Error(`Compositor command failed: ${args[0]}`);return output;};
const capture=async(name:string)=>{
 const path=join(directory,name);await Bun.sleep(250);
 const windows=JSON.parse(await compositor(["hyprctl","clients","-j"])) as Array<{pid:number;at:number[];size:number[]}>;
 const window=windows.find(window=>window.pid===process.pid);
 if(!window)throw Error("Native orchestration window is not mapped");
 await compositor(["grim","-g",`${window.at[0]},${window.at[1]} ${window.size[0]}x${window.size[1]}`,path]);
 return path;
};
let stage="startup",failed=false;
try{
 model.start();client.start();
 console.error("Native orchestration: connecting isolated fixture");
 await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
 model.setUI({sidebar:"conversations",openWorkspaceIds:["a"]});
 model.openThread(parent);model.setComposerDraft(parent.session_id,{text:"Parent draft is preserved",attachments:[]});
 await app.getByTestId("conversation").waitFor();
 await until(()=>!!model.state.history[parent.session_id]?.some(message=>message.id==="plan"));
 if(!await app.getByTestId("conversation-list").count())await app.getByTestId("toggle-conversations").click();
 await app.getByTestId("conversation-list").waitFor();
 const family=(statuses:string[],revision:number)=>({CatalogFamily:{session_id:parent.session_id,snapshot:{directory:"/project/a",executionId:"orchestration-ui",familyRevision:revision,
  branches:agents.map((agent,index)=>({sessionId:`child-${agent}`,parentSessionId:parent.session_id,agent,title:descriptions[index],status:statuses[index],depth:1}))}}});
 daemon.push(family(agents.map(()=>"outstanding"),1));
 await until(()=>model.agentFamily("child-codex").root===parent.session_id);
 daemon.history.set(parent.session_id,messages(agents.map(()=>"running")));
 for(const [index,agent] of agents.entries())daemon.push({MessageUpdated:{session_id:parent.session_id,message:task(agent,index,"running")}});
 await until(()=>!!model.state.history[parent.session_id]?.some(message=>message.id==="task-general"));
 stage="running screenshot";
 console.error("Native orchestration: capturing live tasks");
 const running=await capture("running.png");
 const surfaceCount=model.state.surfaces.filter(surface=>surface.kind==="agent").length;
 for(const agent of agents){
  stage=`open ${agent} child`;
  console.error(`Native orchestration: navigating ${agent}`);
  model.openThread({session_id:`child-${agent}`,title:`${agent} child`,directory:"/project/a"},false,parent.session_id);
  await until(()=>model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId===`child-${agent}`);
  await app.getByTestId("agent-family-navigation:previous").waitFor();
  assert.equal(await app.getByTestId("composer").count(),0);
  assert.equal(model.state.surfaces.filter(surface=>surface.kind==="agent").length,surfaceCount);
  while(model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId!==parent.session_id){
   const before=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId;
   await app.getByTestId("agent-family-navigation:previous").click();
   await until(()=>model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId!==before);
  }
  await app.getByTestId("composer").waitFor();
  assert.equal(model.state.composerDrafts[parent.session_id]?.text,"Parent draft is preserved");
 }
 stage="completed and failed tasks";
 const statuses=["completed","completed","error","completed"];
 daemon.history.set(parent.session_id,messages(statuses));
 daemon.push({HistoryChunk:{session_id:parent.session_id,messages:messages(statuses),next_cursor:null}});
 for(const [index,agent] of agents.entries())daemon.push({ToolUseResult:{session_id:parent.session_id,tool_use_id:`task-${agent}`,tool:"task",status:statuses[index]==="error"?"Failed":"Completed",output:task(agent,index,statuses[index]!).detail,error:statuses[index]==="error"?"Fixture failure for error-state verification":null}});
 daemon.push(family(["completed","completed","failed","completed"],2));
 const terminal=await capture("completed-and-error.png");
 const finalMessage={id:"final",role:"Assistant",kind:"Assistant",created_at:created+10,text:"Verification complete. Three tasks succeeded; the Codex fixture intentionally exercised the error state."};
 daemon.history.set(parent.session_id,[...messages(statuses),finalMessage]);
 daemon.push({MessageUpdated:{session_id:parent.session_id,message:finalMessage}});
 daemon.push({StreamingState:{session_id:parent.session_id,state:"Idle",label:null}});
 await Bun.sleep(7500);
 const settled=await capture("after-8s.png");
 assert.equal(daemon.requests.some(request=>["CreateThread","SubmitPrompt"].includes(request.tag)),false);
 console.log(JSON.stringify({actualNativeApp:true,fakeProviders:true,providers:agents,nativeChildNavigation:true,parentDraftPreserved:true,singleFamilyTab:true,noModelCalls:true,fixtureOnlyPtyRequests:daemon.ptyRequests.length,screenshots:[running,terminal,settled]}));
}catch(error){failed=true;console.error(stage,{directory},error);await capture("failure.png").catch(()=>{});}
finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();loop.stop();clock.dispose();}
process.exit(failed?1:0);