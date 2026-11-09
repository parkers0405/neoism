// Real layout/paint and fake protocol events: no provider accounts or model calls.
import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App as Automation,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import * as native from "../src/native-core";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {installDisplayClock} from "../src/animation/display-clock";
import {AppVisualState} from "../src/ui/AppVisualState";
import {createAgentDaemon} from "../tests/support/agent-daemon";

const realCore=native.coreCall;
let timelineId=0;
mock.module("../src/native-core",()=>({...native,coreCall:(op:string,args?:Record<string,unknown>)=>{const result=realCore<any>(op,args);if(op==="agenttimeline.new")timelineId=result.id;return result;}}));
const {App}=await import("../src/ui/App");
const daemon=createAgentDaemon({deferPrompts:true}),config={...loadConfig(["--config","/nonexistent/native-home-agent"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,terminalBlockUi:true,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const renderer=createRenderer();prepareBundledFonts(renderer);
renderer.init({title:"Neoism Home Agent paint verification",appId:"ai.neoism.gpuix.home-agent-check",width:1280,height:850,focus:false});
const clock=installDisplayClock(renderer,console.error),loop=startFrameLoop(renderer);
const root=render(()=><App model={model} config={config} visuals={visuals}/>,{renderer});
const app=new Automation(new InProcessBackend(liveRendererAsTest(renderer)));
let stage="startup",failed=false;
const until=async(check:()=>boolean|Promise<boolean>)=>{const deadline=Date.now()+12000;while(!await check()){if(Date.now()>deadline)throw Error(`Native Home Agent check timed out: ${stage}; ${model.state.error||""}`);await Bun.sleep(25);}};
const frame=()=>realCore<{commands:Array<{kind:string;text?:string;rect?:number[]}>;hasConversation:boolean;reason?:string}>("agenttimeline.frame",{id:timelineId,includeCommands:true,compactPaint:false});
// Animated activity glyphs paint their shadow/extrusion passes separately.
const text=()=>frame().commands.filter(command=>command.kind==="text").map(command=>command.text||"").join("").replace(/(.)\1{2}/g,"$1");
try{
  model.start();client.start();
  await until(()=>!!model.state.activeWorkspace&&!model.state.loading&&!!model.mainTerminalSurface()?.sessionId);
  const home=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!,pty=home.sessionId!;
  const prompt="\x1b]7;file:///project/a\x07\x1b]133;A\x07$ \x1b]133;B\x07";
  daemon.pushPty({PtyOutput:{session_id:pty,offset:2,bytes:[...new TextEncoder().encode(prompt)]}});
  await app.getByTestId("terminal-command").waitFor();model.setHomeMode(home.id,"agent");await app.getByTestId("home-agent-settings").waitFor();
  stage="optimistic user bubble before server admission";
  await app.getByTestId("terminal-command").fill("Show my user bubble immediately");await app.getByTestId("terminal-command").press("enter");
  await until(()=>daemon.creations.length===1&&timelineId>0);
  const conversation=await app.getByTestId("conversation").bounds(),history=await app.getByTestId("home-agent-history").bounds();
  assert.ok(conversation.height>100&&conversation.width>100,`Conversation collapsed inside the terminal: ${JSON.stringify({conversation,history})}`);
  assert.ok(Math.abs(conversation.height-history.height)<2,"Transcript must fill the space above the terminal input");
  await until(()=>text().includes("Show my user bubble immediately"));
  stage="initial Crafting activity";
  await until(()=>text().includes("Crafting"));
  daemon.creations.shift()!();
  await until(()=>model.state.surfaces.find(surface=>surface.id===home.id)?.homeAgentSessionId==="created-a"&&daemon.prompts.length===1);
  stage="streaming Markdown and reasoning";
  const answer={id:"home-live-answer",role:"Assistant",kind:"Assistant",text:"# Streaming answer\n\nA **visible** reply."};
  daemon.history.set("created-a",[...model.state.history["created-a"]||[],answer]);
  daemon.pushSubscribed({MessageUpdated:{session_id:"created-a",message:answer}});
  daemon.pushSubscribed({ContentDelta:{session_id:"created-a",message_id:answer.id,kind:"Text",text:" More streamed words."}});
  await until(()=>text().includes("More streamed words."));
  for(const [state,label] of [["Thinking","Pondering"],["Working","Tinkering"],["Compacting","Compacting"],["Generating","Crafting"]]){
    stage=`native activity ${state}`;daemon.pushSubscribed({StreamingState:{session_id:"created-a",state,label:null}});await until(()=>text().includes(label));
  }
  stage="tool and subagent call cards";
  daemon.pushSubscribed({MessageUpdated:{session_id:"created-a",message:{id:"home-task",role:"Assistant",kind:"Tool",tool:"task",status:"running",created_at:Date.now(),agent_kind:"codex",title:"Task(@codex \u00b7 Review Home input)",text:"Delegated workspace task",detail:"task_id: home-child\nstatus: running"}}});
  await until(()=>text().includes("Review Home input"));
  stage="subagents working activity";
  daemon.push({CatalogFamily:{session_id:"created-a",snapshot:{directory:"/project/a",executionId:"home-native-check",familyRevision:1,branches:[{sessionId:"home-child",parentSessionId:"created-a",agent:"codex",title:"Review Home input",status:"outstanding",depth:1}]}}});
  await until(()=>model.agentFamily("created-a").roster.some(row=>row.id==="home-child"));
  daemon.pushSubscribed({StreamingState:{session_id:"created-a",state:"Working",label:null}});
  await until(()=>text().includes("Sub-agents working"));
  assert.equal(await app.getByTestId("agent-input-card").count(),0);assert.equal(await app.getByTestId("home-mode-bar").count(),0);
  assert.equal(model.state.error,undefined);
  console.log(JSON.stringify({actualNativeApp:true,positiveTranscriptBounds:true,optimisticBubble:true,streamingMarkdown:true,activityStates:true,toolCards:true,noModelCalls:true}));
}catch(error){failed=true;console.error(stage,error);if(timelineId)console.error("Native timeline",{text:text(),hasConversation:frame().hasConversation,reason:frame().reason,streaming:model.state.streaming,history:Object.fromEntries(Object.entries(model.state.history).map(([id,messages])=>[id,messages.map(message=>({id:message.id,kind:message.kind,title:message.title,tool:message.tool}))]))});}
finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();loop.stop();clock.dispose();}
process.exit(failed?1:0);