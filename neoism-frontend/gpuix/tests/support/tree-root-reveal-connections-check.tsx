import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {App} from "../../src/ui/App";
import {AppGeneration} from "../../src/ui/AppGeneration";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {WindowWorkspaceSessions} from "../../src/workspaces/window-sessions";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer,type RecordedAppNode} from "./full-app-recording-renderer";

function connection(remote=false){
  const daemon=createAgentDaemon({deferCreations:false});
  const config={...loadConfig(["--config","/nonexistent/tree-reveal-connections"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,collaborative:remote,serverId:remote?"remote":"local",reducedMotion:false,cursorBlink:false,splashAutoAnimate:false};
  const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
  model.setUI({openWorkspaceIds:["a"]});model.start();client.start();
  return {daemon,client,model,config,visuals};
}
const local=connection(),remote=connection(true),host=createRecordingAppRenderer();
// Keep the reveal/promotion clock deterministic even on cold native glyph
// caches: synchronous raster work must not make a first-frame assertion flaky.
let now=0;const realNow=performance.now;performance.now=()=>now;
// Exercise AppGeneration's actual hidden-layout path, not its headless atomic
// fallback. A parked native frame counter promotes at the real 120ms cap.
Object.assign(host.renderer,{getRenderedFrameCount:()=>0});
const root=createRoot(host.renderer),callbacks=new Set<()=>void>(),callbackOrigins=new Map<()=>void,string|undefined>();
const restore=usePresentationScheduler(callback=>{callbacks.add(callback);callbackOrigins.set(callback,new Error("Scheduled presentation callback").stack);return()=>{callbacks.delete(callback);callbackOrigins.delete(callback);};});
let selected=local,workspaceActivation=0;
const snapshot=()=>({session:selected,config:{...selected.config},workspaceActivation});
const [generation,setGeneration]=createSignal(snapshot());
const tabs=new WindowWorkspaceSessions(local,{
  activate(next){selected=next;workspaceActivation++;root.flushSync(()=>setGeneration(snapshot()));},
  local:async()=>local,dispose:async()=>{},
});
tabs.add(remote);
async function until(predicate:()=>boolean){
  const deadline=Date.now()+5000;
  while(!predicate()){await host.pump(root);if(Date.now()>deadline)throw new Error("Connection reveal did not converge");await Bun.sleep(2);}
  await host.pump(root);
}
function descendant(parent:RecordedAppNode,testId:string):RecordedAppNode|undefined{
  if(parent.props.testId===testId)return parent;
  for(const id of parent.children){const node=host.nodes.get(id);if(node){const found=descendant(node,testId);if(found)return found;}}
}
const ink=(pending=false)=>{
  const container=host.find(pending?"app-generation-pending":"app-generation");assert.ok(container);
  const row=descendant(container,"file:README.md");assert.ok(row);
  const glyph=row.children.map(id=>host.nodes.get(id)!).find(node=>node.props["aria-label"]==="README.md");assert.ok(glyph);return glyph;
};
const expire=async()=>{
  now+=1000;const pending=[...callbacks];callbacks.clear();for(const callback of pending)callback();await host.pump(root);
  assert.equal(ink().style.opacity,1);
};
const promote=async()=>{
  now+=120;
  await until(()=>!host.find("app-generation-pending"));
  assert.equal(ink().style.opacity,0,"first presented frame starts at zero, not 120ms into the sweep");
  assert.ok(Number(ink().style.left)>0);
};
try{
  await until(()=>!!local.model.state.activeWorkspace&&!local.model.state.loading&&!!remote.model.state.activeWorkspace&&!remote.model.state.loading);
   await Promise.all([local.model.listDirectory(""),remote.model.listDirectory("")]);
   for(const connection of [local,remote])connection.model.setWorkspaceSidebar("files");
  root.render(()=><AppGeneration value={generation()}>{current=><App model={current.session.model} config={current.config} visuals={current.session.visuals} workspaceActivation={current.workspaceActivation} workspaceNavigation={tabs}/>}</AppGeneration>);
  await until(()=>!!host.find("file:README.md"));await expire();
  for(const next of [remote,local,remote]){
    tabs.select(next);assert.ok(host.find("app-generation-pending"));
    assert.equal(ink(true).style.opacity,1,"hidden generation must not spend the reveal clock");
    await promote();await expire();
  }
  const serial=workspaceActivation;
  // Config/font generation gets a new App but the same activation snapshot.
  root.flushSync(()=>setGeneration({...snapshot(),config:{...selected.config,scale:selected.config.scale*1.05}}));
  assert.equal(ink(true).style.opacity,1);
  now+=120;
  await until(()=>!host.find("app-generation-pending"));assert.equal(ink().style.opacity,1,"font/theme remount must not replay");
  assert.equal(workspaceActivation,serial);
  // Supersede an incoming activation with a config generation before it has
  // ever been presented. That hidden mount must not consume its identity.
  tabs.select(local);assert.equal(ink(true).style.opacity,1);
  root.flushSync(()=>setGeneration(snapshot()));assert.equal(ink(true).style.opacity,1);
  await promote();
  root.unmount();assert.equal(callbacks.size,0,[...callbacks].map(callback=>callbackOrigins.get(callback)).join("\n"));
  console.log("Cached connection A/B/A, hidden promotion clock, config no-replay and superseded-generation reveal passed.");
}finally{
  root.unmount();tabs.dispose();restore();performance.now=realNow;
  for(const current of [local,remote]){current.visuals.dispose();current.model.dispose();current.client.stop();current.daemon.stop();}
}