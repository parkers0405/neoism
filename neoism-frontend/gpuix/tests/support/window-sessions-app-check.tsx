import assert from "node:assert/strict";
import {createSignal,Show} from "solid-js";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {WindowWorkspaceSessions} from "../../src/workspaces/window-sessions";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

function connection(remote=false){
 const daemon=createAgentDaemon({deferCreations:false});
 const config={...loadConfig(["--config","/nonexistent/window-sessions-check"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,collaborative:remote,serverId:remote?"remote":"local",reducedMotion:true,cursorBlink:false};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
 model.setUI({openWorkspaceIds:["a"]});model.start();client.start();
 return {daemon,client,model,config,visuals};
}
const local=connection(),remote=connection(true),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const [selected,setSelected]=createSignal(local);
const tabs=new WindowWorkspaceSessions(local,{activate:setSelected,local:async()=>local,dispose:async()=>{}});
async function until(predicate:()=>boolean){
 const deadline=Date.now()+5000;
 while(!predicate()){await host.pump(root);if(Date.now()>deadline)throw new Error("Window connection UI did not converge");await Bun.sleep(5);}
 await host.pump(root);
}
const click=async(id:string)=>{const node=host.find(id);assert.ok(node,`Missing ${id}`);root.dispatch({elementId:node.id,eventType:"click",button:0});await host.pump(root);};
try{
 await until(()=>!!local.model.state.activeWorkspace&&!local.model.state.loading&&!!remote.model.state.activeWorkspace&&!remote.model.state.loading);
 await local.model.openFile("README.md");
 const file=local.model.state.surfaces.find(surface=>surface.kind==="file")!;
 const editor=local.model.getEditor(file);editor.call("paste",{text:"unsaved local edit\n"});
 root.render(()=><Show when={selected()} keyed>{current=><App model={current.model} config={current.config} visuals={current.visuals} workspaceNavigation={tabs}/>}</Show>);
 await host.pump(root);const localTab=tabs.workspaces()[0];
 tabs.add(remote);tabs.select(remote);await host.pump(root);
 assert.ok(host.find(`workspace:${localTab.id}`),"Connecting must retain the original local workspace tab");
 const remoteTab=tabs.workspaces().find(workspace=>workspace.id!==localTab.id)!;
 assert.ok(host.find(`workspace:${remoteTab.id}`));
 await click("workspace-new");
 await until(()=>local.model.windowWorkspaces().length===2&&selected()===local);
 assert.equal(remote.daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length,0,"Plus must not create on the remote daemon");
 assert.equal(local.daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length,1);
 await click(`workspace:${localTab.id}`);
 await until(()=>local.model.state.activeWorkspace?.id==="a");
 assert.equal(local.model.getEditor(file),editor);assert.ok(editor.text().includes("unsaved local edit"));assert.equal(editor.frame.dirty,true);
 await click(`workspace:${remoteTab.id}`);await until(()=>selected()===remote);
 assert.ok(host.find(`workspace:${localTab.id}`));
 console.log("Actual App host switching, retained dirty editor, remote tab selection, and local plus routing passed.");
}finally{
 root.unmount();tabs.dispose();
 for(const current of [local,remote]){current.visuals.dispose();current.model.dispose();current.client.stop();current.daemon.stop();}
}
