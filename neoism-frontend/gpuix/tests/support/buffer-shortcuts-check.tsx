import assert from "node:assert/strict";
import {mkdtempSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {createWindowKeyRouter} from "../../src/commands/window-key-router";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const directory=mkdtempSync(join(tmpdir(),"neoism-buffer-shortcuts-")),path=join(directory,"config.json");writeFileSync(path,"{}");process.argv.push("--config",path);
const daemon=createAgentDaemon({deferCreations:false});daemon.files.set("code.rs","fn retained() {}\nsecond\nthird\n");
const config={...loadConfig(["--config",path],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),keys=createWindowKeyRouter(),root=createRoot(host.renderer,{onKeyDown:keys.dispatch,keyboardFocusDim:false});
let sequence=0,stage="startup";
const until=async(predicate:()=>boolean)=>{const end=Date.now()+4000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw new Error("Buffer shortcut App did not settle");await Bun.sleep(3);}await host.pump(root);};
const press=async(key:string,modifiers={ctrl:true,shift:true,alt:false,cmd:false})=>{host.dispatchKey(root,host.focused,{key,keyChar:key.length===1?key:"",modifiers,keystrokeId:++sequence});await host.pump(root);};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);const home=model.state.activeSurfaceId!;await model.openFile("code.rs");const code=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const editor=model.getEditor(code);editor.call("paste",{text:"dirty "});const bytes=editor.text();await model.openFile("README.md");const markdown=model.state.activeSurfaceId!;const agent=model.newAgentHome()!;
 root.render(()=><App model={model} config={config} visuals={visuals} registerWindowKeys={keys.register}/>);await host.pump(root);model.selectSurface(home);await host.pump(root);
 const layout=model.getLayout(),order=layout.frame.panes[0].tabs.map(tab=>tab.surfaceId);
 for(const id of [code.id,markdown,agent.id,home]){stage=`cycling to ${id}`;await press("right");assert.equal(model.state.activeSurfaceId,id);}
 await press("ArrowLeft");assert.equal(model.state.activeSurfaceId,agent.id,"Left wraps to final buffer");
 assert.deepEqual(layout.frame.panes[0].tabs.map(tab=>tab.surfaceId),order,"Selecting does not reorder tabs");assert.equal(model.state.activeWorkspace?.id,"a");
 await press("right");assert.equal(model.state.activeSurfaceId,home);await press("up",{ctrl:false,shift:false,alt:true,cmd:false});assert.equal(host.focused,host.find(`pane-tab:${layout.frame.focusedLeaf}`)!.id);const before=model.state.activeSurfaceId;
 await press("right",{ctrl:false,shift:false,alt:true,cmd:false});assert.equal(model.state.activeSurfaceId,before,"Alt arrow still moves focus cursor without selecting");
 stage="cycling while strip focus parked";await press("right");assert.equal(model.state.activeSurfaceId,code.id);assert.equal(host.focused,host.find(`pane-tab:${layout.frame.focusedLeaf}`)!.id,"Cycling while parked on buffer strip retains focus on newly selected tab");
 await press("down",{ctrl:false,shift:false,alt:true,cmd:false});await until(()=>host.focused===host.find("code-editor")?.id);
 const active=model.state.activeSurfaceId;await press("up");await press("down");assert.equal(model.state.activeSurfaceId,active,"CtrlShift vertical arrows never cycle tabs");assert.equal(editor.text(),bytes);assert.ok(editor.frame.dirty);
 const split=model.newWelcome()!;layout.split("horizontal",split);const second=model.newWelcome()!;await host.pump(root);const other=layout.frame.panes.find(pane=>!pane.focused)!,otherLeaf=other.leaf,local=layout.frame.panes.find(pane=>pane.focused)!;assert.equal(local.tabs.length,2);
 await press("right");assert.equal(model.state.activeSurfaceId,split.id);assert.equal(layout.frame.panes.find(pane=>!pane.focused)!.leaf,otherLeaf);await press("right");assert.equal(model.state.activeSurfaceId,second.id);
 assert.equal(daemon.ptyRequests.length,0);assert.equal(daemon.requests.some(request=>request.tag==="SubmitPrompt"),false);
 console.log(JSON.stringify({noWindow:true,actualApp:true,allBufferTypesCycle:true,wrapsBothDirections:true,selectsWithoutReordering:true,altFocusUnchanged:true,stripFocusRetained:true,verticalEditingNotIntercepted:true,splitScoped:true,dirtyBytesPreserved:true,noPtyOrPrompt:true}));
}catch(error){console.error(JSON.stringify({stage,focused:host.focused,focusHost:[...host.nodes.values()].find(node=>node.id===host.focused)?.props,surface:model.state.activeSurfaceId,error:model.state.error}));throw error;}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();rmSync(directory,{recursive:true,force:true});}
