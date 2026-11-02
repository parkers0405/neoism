import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/session-error-app"],{}),endpoint:daemon.endpoint,reducedMotion:true,cursorBlink:false,agentStars:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await host.pump(root);if(Date.now()>end)throw Error("Error footer App did not settle");await Bun.sleep(5);}await host.pump(root);};
const click=(id:string)=>root.dispatch({elementId:host.find(id)!.id,eventType:"click",button:0});
const message="OpenAI OAuth refresh failed: authorization has expired.\nRun /connect to sign in again.";
try{
  model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
  model.setUI({agentHints:false});model.openThread(daemon.threads[0]);
  root.render(()=><App model={model} config={config} visuals={visuals}/>);await until(()=>!!host.find("composer"));
  root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:"Keep this draft"});await host.pump(root);
  daemon.push({SessionEvent:{session_id:"one",kind:"session.error",properties:{error:{name:"ProviderAuthError",data:{message}}}}});
  await until(()=>!!host.find("agent-composer-error"));
  assert.equal(model.state.error,message,"Live error cards must reach the Rust timeline without a protocol error");
  assert.equal(host.find("agent-composer-error")!.props["aria-label"],message);
  assert.equal(host.find("agent-composer-error")!.style.borderColor,config.theme.red);
  for(const width of [240,310,800]){
    root.dispatch({elementId:host.find("agent-pane")!.id,eventType:"layout",x:0,y:0,width,height:600});await host.pump(root);
    assert.ok(Number(host.find("agent-composer-error")!.style.width)<=width,"Error footer stays within narrow panes");
    assert.ok(host.find("agent-error-details"),"Narrow errors retain their details action");
  }
  model.clearError();daemon.push({SessionIdle:{session_id:"one"}});await Bun.sleep(250);await host.pump(root);
  assert.ok(host.find("agent-composer-error"),"Toast expiry and idle must not hide the failure");
  assert.equal(host.find("composer")!.props.value,"Keep this draft");
  click("agent-error-details");await until(()=>!!host.find("agent-picker:connect-input"));
  assert.equal(host.find("agent-picker:connect-input")!.props["aria-label"],"Agent error");
  const options=[...host.nodes.values()].filter(node=>node.props.role==="option").map(node=>String(node.props["aria-label"]));
  assert.ok(options.some(label=>label.includes("Connect a provider")));assert.ok(options.some(label=>label.includes("Copy error")));assert.ok(options.some(label=>label.includes("Dismiss")));
  const key=(key:string)=>root.dispatch({elementId:host.find("agent-picker:connect-input")!.id,eventType:"keyDown",key});
  key("down");key("down");key("enter");await until(()=>!host.find("agent-composer-error"));
  assert.equal(model.state.sessionErrors.one,undefined);assert.equal(host.find("composer")!.props.value,"Keep this draft");
  daemon.push({SessionEvent:{session_id:"two",kind:"session.error",properties:{error:{message:"Another chat failed"}}}});await until(()=>!!model.state.sessionErrors.two);
  assert.equal(host.find("agent-composer-error"),undefined,"Another chat's error must not replace this footer");
  console.log("Persistent composer error, recovery actions, dismissal and conversation scoping verified");
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}