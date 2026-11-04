// Isolated real GPU window. Only the fixture daemon is contacted; never a live model.
import {mock} from "bun:test";
import assert from "node:assert/strict";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App as AutomationApp,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import {readFileSync} from "node:fs";
import * as native from "../../src/native-core";
import {installDisplayClock} from "../../src/animation/display-clock";
import {createAgentDaemon} from "./agent-daemon";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";

const actual=native.coreCall,trace:any[]=[];let stage="boot",started=performance.now(),lastComposer="";
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args:Record<string,unknown>={})=>{const value=actual<any>(op,args);const signature=JSON.stringify([args.home,args.state,value?.inputRect]);if(["agentcomposer.frame","agenttimeline.layout","agenttimeline.new","agenttimeline.adopt","agenttimeline.family"].includes(op)&&(op!=="agentcomposer.frame"||signature!==lastComposer)){trace.push({ms:performance.now()-started,stage,op,home:args.home,state:args.state,contentBottom:args.contentBottom,inputRect:value?.inputRect,id:args.id});if(op==="agentcomposer.frame")lastComposer=signature;}return value;}}));
const {App}=await import("../../src/ui/App");
const daemon=createAgentDaemon({deferPrompts:true}),config={...loadConfig(["--config","/nonexistent/home-realpaint"],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:process.argv.includes("--reduced"),cursorBlink:false,splashAutoAnimate:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const renderer=createRenderer();renderer.init({title:"Neoism isolated Home transition",appId:"ai.neoism.gpuix.home-transition-test",width:1100,height:760,titlebarTransparent:true,focus:false});
for(const name of ["CascadiaCode/CascadiaCodeNF-Regular.otf","PressStart2P/PressStart2P-Regular.ttf"])renderer.registerFont(readFileSync(new URL(`../../../../neoism-paint/src/font/resources/${name}`,import.meta.url)));
const clock=installDisplayClock(renderer),loop=startFrameLoop(renderer),root=render(()=><App model={model} config={config} visuals={visuals}/>,{renderer,onUncaughtError:console.error}),app=new AutomationApp(new InProcessBackend(liveRendererAsTest(renderer)));
const until=async(predicate:()=>boolean|Promise<boolean>)=>{const end=Date.now()+10000;while(!await predicate()){if(Date.now()>end)throw Error(`Realpaint timeout in ${stage}: ${model.state.error}`);await Bun.sleep(10);}};
const sample=async(label:string)=>{
 const clients=JSON.parse(await new Response(Bun.spawn(["hyprctl","-j","clients"],{stdout:"pipe",stderr:"pipe"}).stdout).text());
 const own=clients.find((window:any)=>window.class==="ai.neoism.gpuix.home-transition-test");if(!own)throw Error("Isolated GPU window not found");
 const prefix=(process.argv.includes("--after")?"after":"before")+(config.reducedMotion?"-reduced":"");
 const capture=Bun.spawn(["grim","-g",`${own.at[0]},${own.at[1]} ${own.size[0]}x${own.size[1]}`,`/tmp/neoism-home-transition/${prefix}-${label}.png`],{stdout:"pipe",stderr:"pipe"});
 if(await capture.exited)throw Error(await new Response(capture.stderr).text());
 const composer=await app.getByTestId("composer").bounds(),conversation=await app.getByTestId("conversation").count();console.log("PAINT",JSON.stringify({label,ms:performance.now()-started,composer,conversation,cached:visuals.agentComposer(model.state.activeSurfaceId!,config).frame?.inputRect}));
};
try{
 model.start();client.start();await until(()=>!model.state.loading&&!model.state.catalogsLoading&&model.state.connection.phase==="connected");model.newAgentHome();await app.getByTestId("composer").waitFor();await app.getByTestId("composer").fill("First Home send, isolated GPU test");await Bun.sleep(150);await sample("home");
 stage="send";started=performance.now();trace.length=0;lastComposer="";await app.getByTestId("composer").press("enter");
 for(const label of ["send-0","send-1","send-2"]){await sample(label);}
  await until(()=>daemon.creations.length===1);stage="canonical";daemon.creations.shift()!();await until(()=>daemon.prompts.length===1);
  if(process.argv.includes("--idle-before-admission")){
   daemon.push({SessionIdle:{session_id:"created-a"}});
   daemon.push({RuntimeSnapshot:{session_id:"created-a",snapshot:{rootSessionId:"created-a",familyRevision:1,branchesAuthoritative:true,
    execution:{executionId:"previous-run",rootSessionId:"created-a",rootMessageId:"previous-message",completedMs:1000,activeSegments:{},sessionActivities:{},revision:1,finished:true},branches:[]}}});
   await Bun.sleep(50);
  }
  await sample("canonical");
  stage="admitted";
  // The real status endpoint includes committed queue admissions before the
  // worker's busy event reaches the client on the separate event stream.
  daemon.statuses.set("created-a",{type:"busy"});daemon.prompts.shift()!();
  if(process.argv.includes("--idle-before-admission")){
   await Bun.sleep(30);
   daemon.push({RuntimeSnapshot:{session_id:"created-a",snapshot:{rootSessionId:"created-a",familyRevision:1,branchesAuthoritative:true,
    execution:{executionId:"previous-run",rootSessionId:"created-a",rootMessageId:"previous-message",completedMs:1000,activeSegments:{},sessionActivities:{},revision:1,finished:true},branches:[]}}});
   await Bun.sleep(30);
  }
  daemon.push({StreamingState:{session_id:"created-a",state:"Generating",label:null}});await sample("streaming-0");await Bun.sleep(60);await sample("streaming-1");
 console.log("TRACE",JSON.stringify(trace));console.log("REQUESTS",JSON.stringify(daemon.requests.filter(request=>["CreateThread","SubmitPrompt"].includes(request.tag))));
 if(process.argv.includes("--after")){
  const chat=trace.find(frame=>frame.op==="agentcomposer.frame"&&frame.home===false),layouts=trace.filter(frame=>frame.op==="agenttimeline.layout");
  assert.ok(chat&&layouts.length);assert.equal(layouts[0].contentBottom,chat.inputRect[1]);
  assert.ok(layouts.every(frame=>frame.contentBottom===chat.inputRect[1]));
  assert.ok(!trace.some(frame=>frame.op==="agentcomposer.frame"&&frame.home===true),"no empty Home frame during draft clear");
   assert.equal(trace.filter(frame=>frame.op==="agenttimeline.new").length,1);assert.equal(trace.filter(frame=>frame.op==="agenttimeline.adopt").length,1);
   if(process.argv.includes("--idle-before-admission"))assert.ok(!trace.some(frame=>frame.op==="agentcomposer.frame"&&frame.home===false&&frame.state==="Idle"),"old idle hydration cannot re-layout the submitting chat");
 }
}catch(error){console.error(error);console.log("TRACE",JSON.stringify(trace));process.exitCode=1;}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();loop.stop();clock.dispose();}
process.exit(process.exitCode||0);