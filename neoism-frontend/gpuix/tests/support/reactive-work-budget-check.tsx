import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import * as native from "../../src/native-core";
const calls=new Map<string,number>();
const original=native.coreCall;
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args?:Record<string,unknown>)=>{calls.set(op,(calls.get(op)||0)+1);return original(op,args);}}));
const {App}=await import("../../src/ui/App");
const {WorkspaceModel}=await import("../../src/model");
const {DaemonConnection}=await import("../../src/daemon/client");
const {loadConfig}=await import("../../src/config");
const {createAgentDaemon}=await import("./agent-daemon");
const {createRecordingAppRenderer}=await import("./full-app-recording-renderer");
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/reactive-budget"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false,splashAutoAnimate:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
try{
 model.start();client.start();
 const deadline=Date.now()+4000;while(!model.state.activeWorkspace||model.state.loading){if(Date.now()>deadline)throw Error("workspace timeout");await Bun.sleep(5);}
 model.openAgentHome();root.render(()=><App model={model} config={config}/>);await host.pump(root);
 assert.ok(host.find("composer"));assert.ok(host.find("file-tree"));
 calls.clear();
 for(let i=0;i<40;i++){model.setUI({unrelatedBudgetProbe:i});root.flush();}
 const unrelated=Object.fromEntries(calls);
 for(const op of ["tree.visible","tree.layout","agentcomposer.frame","agentcomposer.chip-frame","agentcomposer.chips-layout","composer.labels"])
  assert.equal(calls.get(op)||0,0,`${op} must not run on unrelated model updates: ${JSON.stringify(unrelated)}`);
 calls.clear();
 root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:"a real draft change"});root.flush();
 assert.ok((calls.get("agentcomposer.frame")||0)>0,"Draft changes must still redraw the composer");
 assert.equal(calls.get("tree.layout")||0,0,"Typing must not rebuild the file tree");
 assert.equal(model.state.composerDrafts[model.composerDraftKey(undefined,model.state.activeSurfaceId)]?.text,"a real draft change");
 console.log(JSON.stringify({noWindow:true,unrelatedUpdates:40,unrelated,draftRedraw:true}));
}finally{root.unmount();model.dispose();client.stop();daemon.stop();}
