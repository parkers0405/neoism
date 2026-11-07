import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import * as native from "../../src/native-core";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {createAgentDaemon} from "./agent-daemon";

const actual=native.coreCall,calls:Array<{op:string;args:Record<string,unknown>;result:any}>=[];
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args:Record<string,unknown>={})=>{const result=actual<any>(op,args);calls.push({op,args,result});return result;}}));
const {App}=await import("../../src/ui/App");
const reject=process.argv.includes("--reject"),reduced=process.argv.includes("--reduced"),queue=process.argv.includes("--queue"),background=process.argv.includes("--background"),admission=process.argv.includes("--admission");
const daemon=createAgentDaemon({deferPrompts:true,promptError:reject?"Rejected first send":undefined});
const config={...loadConfig(["--config","/nonexistent/send-adoption"],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:reduced,cursorBlink:false,splashAutoAnimate:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
let watching=false,emptyChatBatches=0,missingDetailBatches=0;
const apply=host.renderer.applyBatch.bind(host.renderer);
host.renderer.applyBatch=json=>{const result=apply(json);if(watching&&host.find("agent-pane")?.props["data-home"]===false){if(!host.find("conversation"))emptyChatBatches++;if(!host.find("agent-details"))missingDetailBatches++;}return result;};
const until=async(predicate:()=>boolean)=>{const end=Date.now()+4000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw Error(`Send adoption did not converge: ${model.state.error}`);await Bun.sleep(3);}await host.pump(root);};
const echoCalls=()=>calls.filter(call=>call.op==="agenttimeline.event"&&(call.args.message as any)?.MessageUpdated?.local_echo);
try{
 model.start();client.start();await until(()=>!model.state.loading&&!model.state.catalogsLoading&&model.state.connection.phase==="connected");
  model.newAgentHome();const surface=model.state.activeSurfaceId!;
   // Exercise adoption with an explicitly opened rail; fresh chats default closed.
   model.setUI({workspaceSidebars:{[model.state.activeWorkspace!.id]:"conversations"},details:true});
  root.render(()=><App model={model} config={config} visuals={visuals}/>);await host.pump(root);
  assert.ok(host.find("conversation-list"));
 const composer=host.find("composer")!;assert.ok(host.find("agent-wordmark"));
 root.dispatch({elementId:composer.id,eventType:"change",value:"One first-send bubble"});await host.pump(root);
 const sendCallsStart=calls.length;
 watching=true;
 if(queue)root.dispatch({elementId:composer.id,eventType:"keyDown",key:"enter",modifiers:{alt:true,ctrl:false,shift:false,cmd:false}});
 else root.dispatch({elementId:composer.id,eventType:"submit"});
 await until(()=>daemon.creations.length===1);
 if(queue){
  assert.ok(host.find("agent-wordmark"),"explicit queue must not flash an empty chat while creating");
  assert.equal(host.find("conversation"),undefined);assert.equal(echoCalls().length,0);
  }else{
   assert.ok(host.find("conversation"));assert.equal(host.find("agent-wordmark"),undefined);
   assert.ok(host.find("agent-details"),"details appear with the first local bubble, not after server adoption");
  assert.equal(echoCalls().length,1,"mount after local echo must seed exactly one live arrival");
  assert.ok(!calls.slice(sendCallsStart).some(call=>call.op==="agentcomposer.frame"&&call.args.home===true),"draft clear and local echo must not publish an intermediate empty Home frame");
  const layoutIndex=calls.findIndex(call=>call.op==="agenttimeline.layout"),chatFrameIndex=calls.findIndex(call=>call.op==="agentcomposer.frame"&&call.args.home===false&&call.args.state==="Generating");
  assert.ok(chatFrameIndex>=0&&chatFrameIndex<layoutIndex,"derive chat/status geometry before the first transcript layout, not in a later Composer effect");
  const chatY=calls[chatFrameIndex].result.inputRect[1];
  assert.equal(calls[layoutIndex].args.contentBottom,chatY,"first bubble uses chat-bottom composer geometry, never the old Home center");
  assert.ok(calls.filter(call=>call.op==="agenttimeline.layout").every(call=>call.args.contentBottom===chatY),"no Home→chat correction layout after the first paint");
 }
 const conversation=host.find("conversation")?.id,engine=calls.find(call=>call.op==="agenttimeline.new")?.result.id;
 assert.equal(emptyChatBatches,0,"never publish home=false before local echo and timeline exist");
 let otherHome:string|undefined;
 if(background){
  model.newAgentHome();otherHome=model.state.activeSurfaceId!;await host.pump(root);
  root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:"Keep this other tab draft"});await host.pump(root);
 }
  daemon.creations.shift()!();await until(()=>daemon.prompts.length===1);
  assert.equal(model.state.error,undefined,"local adoption must not surface a native event decode error");
 if(queue){assert.ok(host.find("agent-wordmark"),"queue-only adoption stays on the known-empty home");assert.equal(calls.filter(call=>call.op==="agenttimeline.new").length,0);}
  if(!queue){
  if(!background)assert.equal(host.find("conversation")?.id,conversation,"canonical session adoption must not remount the host");
  assert.equal(calls.filter(call=>call.op==="agenttimeline.new").length,1,"only one native engine for pending and canonical session");
  assert.equal(calls.filter(call=>call.op==="agenttimeline.adopt"&&call.args.id===engine).length,1,`native adoption must succeed (rebuild dev core if stale): ${model.state.error}`);
  assert.equal(calls.filter(call=>call.op==="agenttimeline.dispose"&&call.args.id===engine).length,0);
   assert.equal(echoCalls().length,1,"canonical submit must not replay the local arrival");
  }
  if(admission){
   const messageID=daemon.requests.find(request=>request.tag==="SubmitPrompt")!.body.message_id;
   daemon.push({SessionEvent:{session_id:"created-a",kind:"session.prompt.admitted",properties:{request:{messageId:messageID}}}});
   // Durable admission is not a waiting prompt, even before the run is claimed.
   daemon.push({SessionEvent:{session_id:"created-a",kind:"session.queue.updated",properties:{action:"enqueue",messageID,queue:{count:0,primaryMessageID:messageID,running:false,worker:true}}}});
   daemon.push({QueueUpdate:{session_id:"created-a",count:0}});
   await until(()=>model.state.queues["created-a"]?.count===0);
   assert.equal(host.find("agent-wordmark"),undefined);
   assert.equal(host.find("conversation")?.id,conversation);
   assert.ok(host.find("agent-details"));
   assert.equal(model.state.streaming["created-a"],"Generating");
   assert.ok(!calls.slice(sendCallsStart).some(call=>call.op==="agentcomposer.frame"&&call.args.home===true),"admission cannot flash Home");
  }
 if(background){
  assert.equal(model.state.activeSurfaceId,otherHome);assert.equal(host.find("composer")?.props.value,"Keep this other tab draft");
  model.openThread({session_id:"created-a",title:"Created",directory:"/project/a"});await host.pump(root);
  assert.equal(echoCalls().length,1);assert.equal(calls.filter(call=>call.op==="agenttimeline.new").length,1);
 }
 daemon.prompts.shift()!();await until(()=>reject?!!model.state.sessionErrors["created-a"]:model.state.composerDrafts["created-a"]?.text==="");
 if(reject){
  await until(()=>host.find("composer")?.props.value==="One first-send bubble");
  assert.ok(host.find("agent-wordmark"));assert.equal(host.find("conversation"),undefined);
  assert.equal(model.state.streaming["created-a"],"Idle");
 }else if(!queue){
  const echo=model.state.history["created-a"].find(message=>message.kind==="User")!;
  daemon.push({MessageUpdated:{session_id:"created-a",message:echo}});
  daemon.push({StreamingState:{session_id:"created-a",state:"Generating",label:null}});
  await Bun.sleep(20);await host.pump(root);
  if(!background)assert.equal(host.find("conversation")?.id,conversation);assert.equal(echoCalls().length,1);
  // Remounting old history must not replay an already consumed local arrival.
  model.newAgentHome();await host.pump(root);model.openThread({session_id:"created-a",title:"Created",directory:"/project/a"});await host.pump(root);
  assert.equal(echoCalls().length,1);
 }
  assert.equal(emptyChatBatches,0);
  assert.equal(missingDetailBatches,0,"details and chat must enter together, not on a later server response");
 assert.equal(model.state.surfaces.find(entry=>entry.id===surface)?.sessionId,"created-a");
 console.log(JSON.stringify({noWindow:true,stableAdoption:!queue,oneShotArrival:!queue,noEmptyChatFrame:true,rejected:reject,queued:queue,reducedMotion:reduced}));
}finally{watching=false;root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}