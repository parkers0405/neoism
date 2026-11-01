import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const daemon=createAgentDaemon({deferCreations:false}),config={...loadConfig(["--config","/nonexistent/workspace-focus"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const frames=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{frames.add(callback);return()=>frames.delete(callback);});
model.setUI({openWorkspaceIds:["a","b"],workspaceSidebars:{a:"files",b:"files"}});
let stage="initial composer";
const pulse=async()=>{for(const frame of [...frames]){frames.delete(frame);frame();}await host.pump(root);};
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await pulse();if(Date.now()>end)throw Error(`Focus did not restore: ${JSON.stringify({stage,workspace:model.state.activeWorkspace?.id,focused:host.focused,composer:host.find("composer")?.id,saved:[...visuals.workspaceFocus]})}`);await Bun.sleep(4);}await pulse();};
const clickWorkspace=async(id:string)=>{
 const tab=host.find(`workspace:${id}`)!;
 // Native pointer focus happens before the click handler switches workspaces.
 host.renderer.focusElement!(tab.id);await pulse();
 root.dispatch({elementId:tab.id,eventType:"click",button:0});await pulse();
};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);model.newAgentHome();
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await until(()=>!!host.find("composer")&&host.focused===host.find("composer")?.id);
 root.dispatch({elementId:host.focused!,eventType:"change",value:"A unfinished draft"});await pulse();
 stage="new B";await clickWorkspace("b");model.newAgentHome();await until(()=>!!host.find("composer")&&host.focused===host.find("composer")?.id);
 root.dispatch({elementId:host.focused!,eventType:"change",value:"B unfinished draft"});await pulse();
 stage="B sidebar to A composer";
 // Returning to A must restore its input even when B was left in the tree.
 host.renderer.focusElement!(host.find("file-tree")!.id);await until(()=>visuals.workspaceFocus.get("b")?.zone==="sidebar");
 await clickWorkspace("a");await until(()=>host.focused===host.find("composer")?.id);
 assert.equal(host.find("composer")!.props.value,"A unfinished draft");
 stage="A composer to B sidebar";await clickWorkspace("b");await until(()=>host.focused===host.find("file-tree")?.id);
 assert.equal(host.find("composer")!.props.value,"B unfinished draft");
 // Clicking the input must update logical focus as well as native focus.
 host.renderer.focusElement!(host.find("composer")!.id);await until(()=>visuals.workspaceFocus.get("b")?.zone==="content");
 for(const id of ["a","b","a"]){await clickWorkspace(id);await until(()=>host.focused===host.find("composer")?.id);}
 assert.equal(daemon.requests.filter(request=>request.tag==="SubmitPrompt").length,0);
 console.log("Workspace input/sidebar focus restores independently and drafts survive");
}finally{root.unmount();restore();visuals.dispose();model.dispose();client.stop();daemon.stop();}
