import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
for(const fail of [false,true]){
 const daemon=createAgentDaemon({promptError:fail?"Rejected":undefined}),config={...loadConfig(["--config","/nonexistent/send-transition"],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:true,cursorBlink:false,splashAutoAnimate:false};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
 const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){if(Date.now()>end)throw Error("transition timed out");await Bun.sleep(5);await host.pump(root);}};
 try{
  model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);model.openAgentHome();root.render(()=><App model={model} config={config}/>);await host.pump(root);
  assert.ok(host.find("agent-wordmark"));const input=host.find("composer")!;
  root.dispatch({elementId:input.id,eventType:"change",value:"First prompt"});root.dispatch({elementId:input.id,eventType:"submit"});await host.pump(root);
  assert.equal(host.find("agent-wordmark"),undefined,"Send leaves home before waiting for session creation");
  assert.equal(host.find("composer")?.props.value,"","Composer clears before session creation");
  const pending=Object.values(model.state.pendingAgentSessions)[0]!;
  assert.ok(pending);assert.equal(model.state.history[pending]?.[0]?.text,"First prompt");
  assert.ok(host.find("conversation"),"Pending user message has a timeline before server ACK");
  await until(()=>daemon.creations.length===1);daemon.creations.shift()!();await until(()=>daemon.requests.some(request=>request.tag==="SubmitPrompt"));
  await Bun.sleep(50);await host.pump(root);
  if(fail){assert.ok(host.find("agent-wordmark"));assert.equal(host.find("composer")?.props.value,"First prompt");}
  else{
   assert.equal(host.find("agent-wordmark"),undefined,"Empty initial history must not flash home after send");
   daemon.push({HistoryChunk:{session_id:"created-a",messages:[],next_cursor:null}});await Bun.sleep(30);await host.pump(root);assert.equal(host.find("agent-wordmark"),undefined);
   assert.ok(host.find("conversation"));
  }
 }finally{root.unmount();model.dispose();client.stop();daemon.stop();}
}
console.log(JSON.stringify({noWindow:true,immediateTransition:true,noEmptyHistoryFlash:true,failedDraftRestored:true}));
