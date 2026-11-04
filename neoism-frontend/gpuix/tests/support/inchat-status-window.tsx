// Isolated real GPU window: no live provider requests or user conversation data.
import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App as AutomationApp,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import * as native from "../../src/native-core";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {installDisplayClock} from "../../src/animation/display-clock";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {createAgentDaemon} from "./agent-daemon";

const original=native.coreCall,frames:any[]=[];let watch=false,phase="local",started=0;
const timing=process.argv.includes("--timing"),costs=new Map<string,{count:number;total:number;max:number}>();
let latestLayout:Record<string,unknown>|undefined;
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args:Record<string,unknown>={})=>{
 if(op==="agenttimeline.layout")latestLayout=args;
 const began=performance.now();
 const result=original<any>(op,op==="agenttimeline.frame"&&!timing?{...args,includeCommands:true,compactPaint:false}:args);
 if(watch){const elapsed=performance.now()-began,entry=costs.get(op)||{count:0,total:0,max:0};entry.count++;entry.total+=elapsed;entry.max=Math.max(entry.max,elapsed);costs.set(op,entry);}
 if(watch&&op==="agenttimeline.frame"){
  const commands=result.commands||[],blue=commands.filter((c:any)=>c.kind==="rect"&&c.color?.[2]===1&&Math.abs((c.color?.[1]||0)-122/255)<0.001&&c.radii?.[0]>0);
  const text=commands.filter((c:any)=>c.kind==="text"&&c.text?.includes("Followup bubble"));
  const glyphs=commands.filter((c:any)=>c.kind==="text"&&c.text?.length===1&&c.size===12);
  frames.push({phase,ms:Math.round(performance.now()-started),blue:blue.map((c:any)=>c.rect),text:text.map((c:any)=>({x:c.x,y:c.y,clip:c.clip})),status:glyphs.length?Math.min(...glyphs.map((c:any)=>c.y)):null});
 }
 return result;
}}));
const {App}=await import("../../src/ui/App");
const daemon=createAgentDaemon({deferPrompts:true}),short=process.argv.includes("--short");
daemon.history.set("one",[{id:"old-user",role:"User",kind:"User",text:"Earlier request"},{id:"old-answer",role:"Assistant",kind:"Assistant",text:short?"Earlier response":Array.from({length:16},(_,i)=>`Paragraph ${i}. Earlier response text with enough history to fill the chat.`).join("\n\n")}]);
const config={...loadConfig(["--config","/nonexistent/inchat-status"],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:process.argv.includes("--reduced"),cursorBlink:false,splashAutoAnimate:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const renderer=createRenderer();renderer.init({title:"Neoism in-chat status regression",appId:"ai.neoism.inchat-check",width:1000,height:700,focus:false});
const clock=installDisplayClock(renderer),loop=startFrameLoop(renderer),root=render(()=><App model={model} config={config} visuals={visuals}/>,{renderer});
const auto=new AutomationApp(new InProcessBackend(liveRendererAsTest(renderer)));
const until=async(label:string,predicate:()=>boolean)=>{const end=Date.now()+8000;while(!predicate()){if(Date.now()>end)throw Error(`Timed out ${label}: ${model.state.error}`);await Bun.sleep(10);}};
try{
 await Bun.sleep(150);
 const instances=JSON.parse(new TextDecoder().decode(Bun.spawnSync(["hyprctl","instances","-j"]).stdout));
 const instance=process.env.HYPRLAND_INSTANCE_SIGNATURE||instances[0]?.instance;
 if(instance){
  for(const[command,argument]of [["setfloating",`pid:${process.pid}`],["resizewindowpixel",`exact 1000 650,pid:${process.pid}`],["centerwindow",`pid:${process.pid}`]])Bun.spawnSync(["hyprctl","-i",instance,"dispatch",command,argument]);
  await Bun.sleep(150);
 }
 const capture=(path:string)=>{
  const clients=JSON.parse(new TextDecoder().decode(Bun.spawnSync(["hyprctl","-i",instance,"clients","-j"]).stdout));
  const window=clients.find((entry:any)=>entry.pid===process.pid);
  if(window){const result=Bun.spawnSync(["grim","-g",`${window.at[0]},${window.at[1]} ${window.size[0]}x${window.size[1]}`,path],{env:{...process.env,WAYLAND_DISPLAY:"wayland-1",XDG_RUNTIME_DIR:"/run/user/1000"}});assert.equal(result.exitCode,0);}
 };
 model.start();client.start();await until("connect",()=>model.state.connection.phase==="connected"&&!!model.state.activeWorkspace&&!model.state.loading&&!model.state.catalogsLoading);
 model.openThread(daemon.threads[0]);await until("history",()=>!!model.state.history.one);
 await until("full-size native layout",()=>Number(latestLayout?.contentBottom)>=400);await Bun.sleep(150);
 await auto.getByTestId("composer").fill("Followup bubble");await Bun.sleep(100);
 started=performance.now();watch=true;
 if(process.argv.includes("--direct"))void model.submitPrompt("one","Followup bubble");
 else await auto.getByTestId("composer").press("enter");
 await until("prompt",()=>daemon.prompts.length===1);
 await until("first send frame",()=>timing?frames.length>0:frames.some(frame=>frame.status!==null));
 if(process.argv.includes("--capture"))capture("/tmp/neoism-inchat-first.png");
 if(process.argv.includes("--idle-before-admission")){
  phase="idle-before-admission";
  daemon.push({SessionIdle:{session_id:"one"}});
  daemon.push({StreamingState:{session_id:"one",state:"Idle",label:null}});
  daemon.push({RuntimeSnapshot:{session_id:"one",snapshot:{rootSessionId:"one",familyRevision:1,branchesAuthoritative:true,
   execution:{executionId:"previous-run",rootSessionId:"one",rootMessageId:"previous-message",completedMs:1000,activeSegments:{},sessionActivities:{},revision:1,finished:true},branches:[]}}});
 }
 await Bun.sleep(280);
 phase="canonical";daemon.statuses.set("one",{type:"busy"});daemon.prompts.shift()!();
 if(process.argv.includes("--idle-before-admission")){
  await Bun.sleep(30);
  daemon.push({RuntimeSnapshot:{session_id:"one",snapshot:{rootSessionId:"one",familyRevision:1,branchesAuthoritative:true,
   execution:{executionId:"previous-run",rootSessionId:"one",rootMessageId:"previous-message",completedMs:1000,activeSegments:{},sessionActivities:{},revision:1,finished:true},branches:[]}}});
  await Bun.sleep(30);
 }
 const echo=model.state.history.one.find(row=>row.text==="Followup bubble");assert.ok(echo);
 daemon.push({MessageUpdated:{session_id:"one",message:echo}});daemon.push({StreamingState:{session_id:"one",state:"Generating",label:null}});
 await Bun.sleep(250);
 watch=false;
 const activityChanges=frames.filter((frame,index)=>!index||(frame.status!==null)!==(frames[index-1].status!==null)).map(frame=>({ms:frame.ms,phase:frame.phase,active:frame.status!==null,bubble:frame.blue.at(-1)}));
 console.log(JSON.stringify({short,reduced:config.reducedMotion,frames:process.argv.includes("--quiet")?frames.length:frames.slice(0,8),activityChanges,first:frames[0],canonical:frames.find(frame=>frame.phase==="canonical"),last:frames.at(-1),error:model.state.error},null,process.argv.includes("--quiet")?undefined:2));
 if(timing)console.log(JSON.stringify({costs:Object.fromEntries(costs),frameGaps:frames.slice(1).map((frame,index)=>frame.ms-frames[index].ms)},null,2));
 if(process.argv.includes("--capture"))capture("/tmp/neoism-inchat-status.png");
 if(process.argv.includes("--assert"))for(const frame of frames.filter(frame=>frame.status!==null)){
  assert.equal(frame.text.length,1,`status must never arrive without the sent bubble: ${JSON.stringify(frame)}`);
  const bubble=frame.blue.at(-1),clip=frame.text[0].clip;
  assert.ok(bubble&&clip&&bubble[1]+bubble[3]+8<=clip[1]+clip[3]+1,`bubble/tail may not slide through the status clipping boundary: ${JSON.stringify(frame)}`);
  assert.ok(bubble[1]+bubble[3]+8<frame.status,`bubble and status need disjoint space on their first joint frame: ${JSON.stringify(frame)}`);
 }
 if(process.argv.includes("--assert")&&process.argv.includes("--idle-before-admission")){
  const firstActive=frames.findIndex(frame=>frame.status!==null);
  assert.ok(firstActive>=0&&frames.slice(firstActive).every(frame=>frame.status!==null),"pre-admission idle hydration must not remove the already reserved status row");
 }
 if(process.argv.includes("--hold"))await Bun.sleep(20000);
}catch(error){console.error(error);process.exitCode=1;}
finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();loop.stop();clock.dispose();}
process.exit(process.exitCode||0);