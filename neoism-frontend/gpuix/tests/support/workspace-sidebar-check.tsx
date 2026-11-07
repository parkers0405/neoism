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
import {withWorkspaceSidebar,type WorkspaceSidebar} from "../../src/ui/workspace-sidebar";
const daemon=createAgentDaemon({deferCreations:false});
daemon.workspaces.push({id:"c",host_id:"host",title:"C",root_dir:"/project/c"},{id:"d",host_id:"host",title:"D",root_dir:"/project/d"},{id:"e",host_id:"host",title:"E",root_dir:"/project/e"});
// This scenario exercises passive Welcome actions and forbids shell allocation.
const config={...loadConfig(["--config","/nonexistent/workspace-sidebar"],{}),terminalBlockUi:false,endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const directory=mkdtempSync(join(tmpdir(),"neoism-sidebar-"));
const store=new SessionStore(config,{NEOISM_GPUIX_STATE_PATH:join(directory,"recovery.json")});
function mount(saved?:SavedSession){
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
 if(saved)model.restoreSession(saved);else model.setUI({openWorkspaceIds:["a","b","c","d","e"],sidebar:"files"});
 const [generation,setGeneration]=createSignal({config});
 const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+5000;while(!predicate()){await host.pump(root);if(Date.now()>deadline)throw new Error("Workspace sidebar App fixture did not settle");await Bun.sleep(3);}await host.pump(root);};
 const ready=async()=>{model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);root.render(()=><AppGeneration value={generation()}>{value=><App model={model} config={value.config} visuals={visuals}/>}</AppGeneration>);await host.pump(root);};
 const activate=async(id:string)=>{await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id===id)!);await host.pump(root);};
 const pick=async(name:string)=>{const button=host.find(`toggle-${name}`);assert.ok(button);root.dispatch({elementId:button.id,eventType:"mouseDown",button:0});await host.pump(root);};
 const expectView=(view:WorkspaceSidebar|null)=>{
  assert.equal(!!host.find("sidebar"),view!==null);
   assert.equal(!!host.find("file-tree"),view==="files");assert.equal(!!host.find("notes-tree"),view==="notes");assert.equal(!!host.find("conversation-list"),view==="conversations");
   assert.equal(!!host.find("database-sidebar"),view==="databases");
 };
 return {client,model,visuals,host,root,ready,activate,pick,expectView,async zoom(){setGeneration({config:{...config,fontSize:20,scale:20/14}});await host.pump(root);},dispose(){root.unmount();visuals.dispose();model.dispose();client.stop();}};
}
let app=mount();
try{
 await app.ready();app.expectView("files");
 // Exercise the actual workspace + button, recording every published batch.
  const firstFrames:number[]=[];
 const apply=app.host.renderer.applyBatch.bind(app.host.renderer);let watching=true;
 app.host.renderer.applyBatch=json=>{const result=apply(json);if(watching&&app.model.state.activeWorkspace?.id!=="a")firstFrames.push([...app.host.nodes.values()].filter(node=>String(node.props.testId||"").startsWith("file:")).length);return result;};
 app.root.dispatch({elementId:app.host.find("workspace-new")!.id,eventType:"click",button:0});
 const deadline=Date.now()+5000;
 while(app.model.state.activeWorkspace?.id==="a"||app.model.state.creatingWorkspace){await app.host.pump(app.root);if(Date.now()>deadline)throw new Error("Workspace + did not settle");await Bun.sleep(3);}
 await app.host.pump(app.root);watching=false;
  assert.ok(firstFrames.length>0);assert.ok(firstFrames.every(count=>count===0),"New workspaces must not flash an open file tree");app.expectView(null);
  const created=app.model.state.activeWorkspace!.id;await app.activate("a");await app.model.closeWorkspace(false,created);

  // Enter through the actual file-tree row, not a direct Database menu action.
  app.model.update({directories:{...app.model.state.directories,"":{path:"",entries:[{name:"agent.sqlite3",is_dir:false}]}}});
  await app.host.pump(app.root);
  const databaseFile=app.host.find("file:agent.sqlite3");assert.ok(databaseFile);
  app.root.dispatch({elementId:databaseFile.id,eventType:"click",button:0});await app.host.pump(app.root);
  app.expectView("databases");assert.ok(app.host.find("database-back-to-files"));
  const connection=app.model.activeDatabaseConnection()!;assert.equal(connection.path,"agent.sqlite3");
  app.model.openDatabaseTable(connection,"fruit");await app.host.pump(app.root);
  const selectedDatabase=app.model.state.activeSurfaceId!;
  await app.activate("e");app.model.setWorkspaceSidebar("databases");await app.host.pump(app.root);
  assert.equal(app.host.find("database-back-to-files"),undefined,"Other workspace's direct Database entry has no Back button");
  await app.activate("a");assert.ok(app.host.find("database-back-to-files"),"Origin follows its workspace");
  const back=app.host.find("database-back-to-files")!;assert.equal(back.props["aria-label"],"Back to Files");
  app.root.dispatch({elementId:back.id,eventType:"keyDown",key:"enter"});await app.host.pump(app.root);
  app.expectView("files");assert.equal(app.model.state.activeSurfaceId,selectedDatabase,"Back preserves the selected database tab");
  app.model.setWorkspaceSidebar("databases");await app.host.pump(app.root);
  assert.equal(app.host.find("database-back-to-files"),undefined,"Direct Database navigation clears stale file-tree origin");
  app.model.setWorkspaceSidebar("files");app.model.closeSurface(selectedDatabase);await app.host.pump(app.root);

 await app.activate("b");await app.pick("notes");app.expectView("notes");
 await app.activate("c");await app.pick("conversations");app.expectView("conversations");
   await app.activate("d");app.expectView(null);await app.pick("files");app.expectView("files");await app.pick("files");app.expectView(null);
  await app.activate("e");app.model.setUI({workspaceSidebars:withWorkspaceSidebar(app.model.state.ui,"e","databases")});await app.host.pump(app.root);app.expectView("databases");
  assert.ok(app.host.find("database-browse"));assert.equal(app.host.find("database-tabs"),undefined,"Database tables use Neoism buffer tabs, not a nested tab strip");
  let expected:Record<string,WorkspaceSidebar|null>={a:"files",b:"notes",c:"conversations",d:null,e:"databases"};
 assert.deepEqual(app.model.state.ui.workspaceSidebars,expected);
  for(const[id,view]of Object.entries(expected)){await app.activate(id);app.expectView(view);}
  // Explicit close releases only this workspace's cached sidebar preference.
 assert.equal(await app.model.closeWorkspace(false,"b"),true);assert.ok(!app.model.windowWorkspaces().some(workspace=>workspace.id==="b"));
  assert.ok(!app.model.state.workspaces.some(workspace=>workspace.id==="b"));
  assert.ok([...app.visuals.trees.keys()].every(key=>JSON.parse(key)[0]!=="b"));
  assert.ok([...app.visuals.treeViewports.keys()].every(key=>JSON.parse(key)[0]!=="b"));
   expected={a:"files",c:"conversations",d:null,e:"databases"};
  await app.activate("a");app.expectView("files");
  await app.zoom();app.expectView("files");
  for(const[id,view]of Object.entries(expected)){await app.activate(id);app.expectView(view);}
 await app.activate("a");store.write(app.model.captureSession());const saved=store.read()!;assert.deepEqual(saved.ui.workspaceSidebars,expected);
 app.dispose();app=mount(saved);await app.ready();
  for(const[id,view]of Object.entries(expected)){await app.activate(id);app.expectView(view);}
  assert.equal(daemon.ptyRequests.length,0);assert.equal(daemon.requests.filter(request=>request.tag==="SubmitPrompt").length,0);
   const creates=daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length;
   const resolves=daemon.workspaceRequests.filter(request=>request.tag==="ResolveInitialWorkspace").length;
  for(const workspace of app.model.windowWorkspaces())assert.equal(await app.model.closeWorkspace(false,workspace.id),true);
  await app.host.pump(app.root);
  assert.equal(app.model.state.activeWorkspace,undefined);assert.ok(app.host.find("welcome-page"),"Last close paints a window-local Welcome");
  assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length,creates);
  const terminal=app.host.find("splash-action:new-terminal");assert.ok(terminal,"Empty-window Welcome offers a usable terminal action");
  app.root.dispatch({elementId:terminal.id,eventType:"click",button:0});
  const emptyDeadline=Date.now()+5000;
  while(!app.model.state.surfaces.some(surface=>surface.kind==="terminal"&&surface.workspaceId===app.model.state.activeWorkspace?.id)){
    await app.host.pump(app.root);if(Date.now()>emptyDeadline)throw new Error("Empty-window terminal action did not settle");await Bun.sleep(3);
  }
   assert.equal(app.model.windowWorkspaces().length,1);assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length,creates);
   assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="ResolveInitialWorkspace").length,resolves+1);
    console.log(JSON.stringify({actualApp:true,noWindow:true,workspacePlusStartsClosed:true,independentFiveStates:true,closedWorkspaceReleased:true,fontGenerationRetained:true,coldRecoveryRetained:true,databaseSidebarIntegrated:true,noPtyOrPromptDuringSwitches:true,localWelcomeNoReplacement:true,emptyWelcomeAction:true}));
}finally{app.dispose();daemon.stop();rmSync(directory,{recursive:true,force:true});}
