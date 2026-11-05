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

const daemon=createAgentDaemon({deferCreations:false});
const config={...loadConfig(["--config","/nonexistent/tree-root-app-check"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:false,cursorBlink:false,splashAutoAnimate:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),callbacks=new Set<()=>void>();
const restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>{callbacks.delete(callback);};});
model.setUI({openWorkspaceIds:["a","b"]});
const until=async(predicate:()=>boolean)=>{
  const deadline=Date.now()+5000;
  while(!predicate()){await host.pump(root);if(Date.now()>deadline)throw new Error("Actual App tree did not converge");await Bun.sleep(2);}
  await host.pump(root);
};
const row=()=>host.find("file:README.md")!;
const ink=()=>row().children.map(id=>host.nodes.get(id)!).find(node=>node.props["aria-label"]==="README.md")!;
const opacity=()=>Number(ink().style.opacity);
const display=async()=>{const pending=[...callbacks];callbacks.clear();for(const callback of pending)callback();await host.pump(root);};
const switchTo=async(id:string)=>{
  const tab=host.find(`workspace:${id}`);assert.ok(tab,"actual workspace tab is present");
  root.dispatch({elementId:tab.id,eventType:"click",button:0});
  await until(()=>model.state.activeWorkspace?.id===id&&!model.state.loading&&!!row());
};
try{
  model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
  root.render(()=><App model={model} config={config} visuals={visuals}/>);
  await until(()=>!!row());
  assert.ok(Number.isFinite(opacity()),"the hook alpha reaches the actual glyph image host");
  assert.equal(row().style.left,0);
  // Simulate an occluded display: no JS callbacks arrive until after the whole
  // sweep deadline. Reactivation must settle, not replay a fresh animation.
  await Bun.sleep(900);const focus=host.focused;await display();
  assert.equal(opacity(),1);assert.equal(ink().style.left,0);assert.equal(host.focused,focus);
  await model.listDirectory("");await host.pump(root);assert.equal(opacity(),1,"ordinary refresh is not activation");
  const key=JSON.stringify(["a",false,"/project/a"]),viewport=visuals.treeViewports.get(key)!;
  const saved={...viewport.frame};
  await switchTo("b");assert.ok(opacity()<1,"workspace B uses root-reveal instead of cached entry timestamps");
  assert.equal(ink().style.opacity,opacity());assert.ok(Number(ink().style.left)>0,"only ink is displaced");
  await Bun.sleep(900);await display();assert.equal(opacity(),1,"a parked live sweep expires on the first reactivation frame");
  await switchTo("a");assert.ok(opacity()<1,"cached A replays its root reveal on rapid B/A switching");
  await switchTo("b");await switchTo("a");assert.ok(opacity()<1,"rapid A/B/A replaces rather than queues reveals");
  assert.deepEqual(visuals.treeViewports.get(key)!.frame,saved,"activation reveal preserves A's native viewport");
  model.setUI({workspaceSidebars:{a:null,b:"files"}});await host.pump(root);
  assert.equal(host.find("file-tree"),undefined,"closing the panel unmounts its animation owner");
  root.unmount();assert.equal(callbacks.size,0,"unmounted App leaves no display work behind");
  console.log("Actual App workspace root sweep, refresh exclusion, parked-frame expiry, cached viewport and cleanup passed.");
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();restore();}