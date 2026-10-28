import assert from "node:assert/strict";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createSignal} from "solid-js";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppGeneration} from "../../src/ui/AppGeneration";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {SessionStore,type SavedSession} from "../../src/session-store";
import {loadConfig} from "../../src/config";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const daemon=createAgentDaemon({deferCreations:false});
daemon.workspaces.push({id:"c",host_id:"host",title:"C",root_dir:"/project/c"},{id:"d",host_id:"host",title:"D",root_dir:"/project/d"});
const config={...loadConfig(["--config","/nonexistent/workspace-sidebar"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const directory=mkdtempSync(join(tmpdir(),"neoism-sidebar-"));
const store=new SessionStore(config,{NEOISM_GPUIX_STATE_PATH:join(directory,"recovery.json")});
function mount(saved?:SavedSession){
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
 if(saved)model.restoreSession(saved);else model.setUI({openWorkspaceIds:["a","b","c","d"],sidebar:"files"});
 const [generation,setGeneration]=createSignal({config});
 const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+5000;while(!predicate()){await host.pump(root);if(Date.now()>deadline)throw new Error("Workspace sidebar App fixture did not settle");await Bun.sleep(3);}await host.pump(root);};
 const ready=async()=>{model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);root.render(()=><AppGeneration value={generation()}>{value=><App model={model} config={value.config} visuals={visuals}/>}</AppGeneration>);await host.pump(root);};
 const activate=async(id:string)=>{await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id===id)!);await host.pump(root);};
 const pick=async(name:string)=>{const button=host.find(`toggle-${name}`);assert.ok(button);root.dispatch({elementId:button.id,eventType:"mouseDown",button:0});await host.pump(root);};
 const expectView=(view:"files"|"notes"|"conversations"|null)=>{
  assert.equal(!!host.find("sidebar"),view!==null);
  assert.equal(!!host.find("file-tree"),view==="files");assert.equal(!!host.find("notes-tree"),view==="notes");assert.equal(!!host.find("conversation-list"),view==="conversations");
 };
 return {client,model,visuals,host,root,ready,activate,pick,expectView,async zoom(){setGeneration({config:{...config,fontSize:20,scale:20/14}});await host.pump(root);},dispose(){root.unmount();visuals.dispose();model.dispose();client.stop();}};
}
let app=mount();
try{
 await app.ready();app.expectView("files");
 // Exercise the actual workspace + button, recording every published batch.
 const originalTree=app.host.find("file-tree")!.id,firstFrames:number[]=[];
 const apply=app.host.renderer.applyBatch.bind(app.host.renderer);let watching=true;
 app.host.renderer.applyBatch=json=>{const result=apply(json);if(watching&&app.model.state.activeWorkspace?.id!=="a")firstFrames.push([...app.host.nodes.values()].filter(node=>String(node.props.testId||"").startsWith("file:")).length);return result;};
 app.root.dispatch({elementId:app.host.find("workspace-new")!.id,eventType:"click",button:0});
 const deadline=Date.now()+5000;
 while(app.model.state.activeWorkspace?.id==="a"||app.model.state.creatingWorkspace){await app.host.pump(app.root);if(Date.now()>deadline)throw new Error("Workspace + did not settle");await Bun.sleep(3);}
 await app.host.pump(app.root);watching=false;
 assert.ok(firstFrames.length>0);assert.ok(firstFrames.every(count=>count>0),"Every workspace + frame must already contain file rows, without an empty/loading flash");assert.equal(app.host.find("file-tree")!.id,originalTree,"Same-panel creation keeps the tree host mounted");
 const created=app.model.state.activeWorkspace!.id;await app.activate("a");await app.model.closeWorkspace(false,created);

 await app.activate("b");await app.pick("notes");app.expectView("notes");
 await app.activate("c");await app.pick("conversations");app.expectView("conversations");
 await app.activate("d");await app.pick("files");app.expectView(null);
 const expected={a:"files",b:"notes",c:"conversations",d:null};
 assert.deepEqual(app.model.state.ui.workspaceSidebars,expected);
 for(const[id,view]of Object.entries(expected)){await app.activate(id);app.expectView(view as "files"|"notes"|"conversations"|null);}
 // Closing a workspace hides membership, not its own sidebar preference.
 assert.equal(await app.model.closeWorkspace(false,"b"),true);assert.ok(!app.model.windowWorkspaces().some(workspace=>workspace.id==="b"));
 await app.activate("b");app.expectView("notes");
 await app.zoom();app.expectView("notes");
 for(const[id,view]of Object.entries(expected)){await app.activate(id);app.expectView(view as "files"|"notes"|"conversations"|null);}
 await app.activate("a");store.write(app.model.captureSession());const saved=store.read()!;assert.deepEqual(saved.ui.workspaceSidebars,expected);
 app.dispose();app=mount(saved);await app.ready();
 for(const[id,view]of Object.entries(expected)){await app.activate(id);app.expectView(view as "files"|"notes"|"conversations"|null);}
 assert.equal(daemon.ptyRequests.length,0);assert.equal(daemon.requests.filter(request=>request.tag==="SubmitPrompt").length,0);
 console.log(JSON.stringify({actualApp:true,noWindow:true,workspacePlusNoEmptyFrame:true,independentFourStates:true,closedWorkspaceRetained:true,fontGenerationRetained:true,coldRecoveryRetained:true,noPtyOrPrompt:true}));
}finally{app.dispose();daemon.stop();rmSync(directory,{recursive:true,force:true});}
