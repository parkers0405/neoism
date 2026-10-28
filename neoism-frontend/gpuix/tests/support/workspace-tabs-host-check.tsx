import assert from "node:assert/strict";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createSignal} from "solid-js";
import {createRoot,type HostElement,type NativeRenderer,type EventPayload} from "@gpuix/solid";
import {WorkspaceTabs} from "../../src/ui/WorkspaceTabs";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {SessionStore} from "../../src/session-store";
import {createAgentDaemon} from "./agent-daemon";

// Actual Solid host, event map and mutation batching. Only native painting is
// replaced: this records native tree mutations and supplies explicit viewport
// geometry, so the test never constructs or opens a native desktop window.
interface Node {id:number;type:string;children:number[];props:Record<string,unknown>;style:Record<string,unknown>}
function recordingRenderer(){
 const nodes=new Map<number,Node>();let offset=0;
 const byTestId=(testId:string)=>[...nodes.values()].find(node=>node.props.testId===testId);
 const tabNodes=()=>{const scroll=byTestId("workspace-tab-scroll");return(scroll?.children||[]).map(id=>nodes.get(id)!).filter(node=>String(node.props.testId||"").startsWith("workspace:"));};
 const logicalBounds=(id:number)=>{const index=tabNodes().findIndex(tab=>tab.id===id);return{x:300+index*130-offset,y:30,width:130,height:30};};
 const bounds=(id:number):{x:number;y:number;width:number;height:number}|null=>{
  const node=nodes.get(id),testId=String(node?.props.testId||"");
  if(testId==="workspace-tabs"||testId==="workspace-tab-scroll")return{x:300,y:30,width:360,height:30};
  if(testId.startsWith("workspace:")){const logical=logicalBounds(id);return{...logical,x:logical.x+Number(node?.style.left||0)};}
  if(testId.startsWith("workspace-close:")){const parent=byTestId(`workspace:${testId.slice("workspace-close:".length)}`),tab=parent&&bounds(parent.id);return tab?{x:tab.x+100,y:36,width:18,height:18}:null;}
  if(testId==="workspace-drag-preview"){const parent=[...nodes.values()].find(parent=>parent.children.includes(id));return{x:Number(parent?.style.left||0)+Number(node?.style.left||0),y:Number(parent?.style.top||0),width:Number(node?.style.width||0),height:Number(node?.style.height||0)};}
  if(testId==="workspace-new")return{x:300+tabNodes().length*130-offset,y:30,width:30,height:30};
  return null;
 };
 const remove=(id:number,destroyed:number[])=>{const node=nodes.get(id);if(!node)return;for(const child of[...node.children])remove(child,destroyed);for(const parent of nodes.values())parent.children=parent.children.filter(child=>child!==id);nodes.delete(id);destroyed.push(id);};
 const renderer={
  setWindowKeyEvents(){},scrollIntoView(){},getElementBounds:bounds,getScrollOffset:()=>[-offset,0],
  applyBatch(json:string){
   const destroyed:number[]=[];
   for(const[op,id,a,b]of JSON.parse(json)){
    if(op==="createElement")nodes.set(id,{id,type:a,children:[],props:{},style:{}});
    else if(op==="destroyElement")remove(id,destroyed);
    else if(op==="setCustomProp")nodes.get(id)!.props[a]=b;
    else if(op==="setStyle")nodes.get(id)!.style=a;
    else if(op==="appendChild"||op==="insertBefore"){
     if(op==="insertBefore"&&a===b)continue;
     for(const node of nodes.values())node.children=node.children.filter(child=>child!==a);
     const parent=nodes.get(id)!;if(op==="appendChild")parent.children.push(a);else{const index=parent.children.indexOf(b);assert.ok(index>=0,"native insertBefore anchor must remain a sibling");parent.children.splice(index,0,a);}
    }
   }
   return destroyed;
  },
 };
 return{renderer:renderer as unknown as NativeRenderer,byTestId,tabNodes,bounds,logicalBounds,descendants(id:number):Node[]{const current=nodes.get(id);return current?[current,...current.children.flatMap(child=>this.descendants(child))]:[];},setOffset(value:number){offset=value;}};
}
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+3000;while(!predicate()){if(Date.now()>deadline)throw new Error("Workspace host state did not converge");await Bun.sleep(2);}};
const server=createAgentDaemon();server.threads.splice(0);server.workspaces.push({id:"c",host_id:"host",title:"C",root_dir:"/project/c"});
const config={...loadConfig(["--config","/nonexistent/workspace-tabs-host"],{}),endpoint:server.endpoint,authToken:undefined};
let client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
const host=recordingRenderer(),root=createRoot(host.renderer),registered=new Map<string,HostElement>();
const[state,setState]=createSignal(model.state);let release=()=>{},failure:unknown;
let activations=0,closes=0,creates=0,reorders=0;let closeTask:Promise<boolean>|undefined,createTask:ReturnType<WorkspaceModel["newWorkspace"]>|undefined;
let pointerFollowingSamples=0,intermediateSlideSamples=0;
const dispatch=(id:number,type:string,x:number,y:number)=>root.dispatch({elementId:id,eventType:type,x,y,button:0,pressedButton:0} satisfies EventPayload);
const node=(testId:string)=>{const node=host.byTestId(testId);assert.ok(node,`missing ${testId}`);return node;};
const box=(testId:string)=>{const value=host.bounds(node(testId).id);assert.ok(value,`missing bounds ${testId}`);return value;};
const order=()=>host.tabNodes().map(node=>String(node.props.testId).slice(10));
const membership=()=>model.windowWorkspaces().map(workspace=>workspace.id);
const settle=async()=>{await Promise.resolve();root.flush();};
const assertNonselectableText=(id:number)=>{const texts=host.descendants(id).filter(node=>node.type==="text"&&(node.props.testId!==undefined||node.children.length>0));assert.ok(texts.length>0);for(const text of texts){assert.equal(text.style.userSelect,"none","tab chrome cannot initiate native text selection");assert.equal(text.style.pointerEvents,"none","tab text forwards the full grab area to the tab host");}};
async function closeWithNativeBubbling(id:string){
 const close=node(`workspace-close:${id}`),tab=node(`workspace:${id}`),bounds=box(`workspace-close:${id}`),x=bounds.x+9,y=bounds.y+9;
 dispatch(close.id,"mouseDown",x,y);await settle();dispatch(tab.id,"mouseDown",x,y);await settle();
 dispatch(close.id,"click",x,y);
 // GPUI delivers child and parent native callbacks separately; the old
 // queueMicrotask close guard expired here and activated the closed tab.
 await settle();dispatch(tab.id,"click",x,y);await closeTask;await settle();
}
async function drag(id:string,targetId:string,side:"before"|"after"){
 const before=membership(),originalHosts=new Map([...registered].map(([id,host])=>[id,host])),ids=new Map(host.tabNodes().map(node=>[node.props.testId,node.id]));
 const tab=node(`workspace:${id}`),source=box(`workspace:${id}`),target=host.logicalBounds(node(`workspace:${targetId}`).id),x=Math.max(310,source.x+60),y=45,endX=target.x+(side==="before"?30:90),grab=x-source.x;
 dispatch(tab.id,"mouseDown",x,y);dispatch(tab.id,"mouseMove",x+(endX>=x?8:-8),y);await settle();
 const layer=node("workspace-drag-layer");dispatch(layer.id,"mouseMove",endX,y);await settle();assert.ok(Math.abs(box(`workspace:${id}`).x+grab-endX)<.01,"dragged stable host follows actual pointer");
 assertNonselectableText(node("workspace-drag-preview").id);
 assert.ok(Math.abs(box("workspace-drag-preview").x+grab-endX)<.01,"foreground painted preview follows the same pointer");assert.equal(node(`workspace:${id}`).style.opacity,0);pointerFollowingSamples++;
 dispatch(layer.id,"mouseUp",endX,y);await settle();
 dispatch(tab.id,"click",endX,y);await settle();
 const expected=before.filter(value=>value!==id);expected.splice(expected.indexOf(targetId)+(side==="after"?1:0),0,id);
 assert.deepEqual(membership(),expected);assert.deepEqual(order(),expected);
 for(const[id,original]of originalHosts)assert.equal(registered.get(id),original,"keyed drag must retain the actual Solid HostElement");
 for(const tab of host.tabNodes())assert.equal(tab.id,ids.get(tab.props.testId),"keyed drag must retain native element IDs");
 assert.equal(host.byTestId("workspace-drag-layer"),undefined);
 assert.equal(host.byTestId("workspace-drag-preview"),undefined);assert.equal(node(`workspace:${id}`).style.opacity,1);
 const moving=host.tabNodes().filter(node=>node.props.testId!==`workspace:${id}`&&Math.abs(Number(node.style.left||0))>1);assert.ok(moving.length,"neighbors begin at their prior painted positions after logical reorder");
 const start=new Map(moving.map(node=>[node.id,Math.abs(Number(node.style.left))]));await Bun.sleep(18);await settle();
 assert.ok(moving.some(tab=>{const offset=Math.abs(Number(host.byTestId(String(tab.props.testId))?.style.left||0));return offset>0&&offset<start.get(tab.id)!;}),"original spring exposes continuous intermediate neighbor offsets");intermediateSlideSamples++;
}
try{
 model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 await model.activateWorkspace(server.workspaces[1]!);await model.openFile("README.md");
 const dirtySurface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;model.getEditor(dirtySurface).call("paste",{text:"keep unsaved B "});
 await model.activateWorkspace(server.workspaces[2]!);await model.activateWorkspace(server.workspaces[0]!);
 setState(model.state);release=model.subscribe(()=>setState(model.state));
 root.render(()=><WorkspaceTabs config={config} workspaces={model.windowWorkspaces(state())} activeId={state().activeWorkspace?.id}
  onActivate={workspace=>{activations++;void model.activateWorkspace(workspace).catch(error=>failure=error);}}
  onClose={id=>{closes++;closeTask=model.closeWorkspace(false,id);}}
  onNew={()=>{creates++;createTask=model.newWorkspace();}}
  onReorder={(id,target,side)=>{reorders++;model.reorderWorkspace(id,target,side);}}
  registerHost={(id,value)=>{if(value)registered.set(id,value);else registered.delete(id);}} registerNewHost={()=>{}} onBounds={()=>{}} onScroll={()=>{}}/>);
 await settle();assert.deepEqual(order(),["a","b","c"]);
 assertNonselectableText(node("workspace-tab-scroll").id);
 const requestsBefore=server.workspaceRequests.length,activeBefore=model.state.activeWorkspace!.id;
 await closeWithNativeBubbling("b");assert.equal(closes,1);assert.equal(activations,0);assert.equal(model.state.activeWorkspace!.id,activeBefore);assert.deepEqual(order(),["a","b","c"]);assert.ok(model.state.error?.includes(":qall!"));assert.equal(model.getEditor(dirtySurface).frame.dirty,true);
 assert.equal(server.workspaceRequests.length,requestsBefore,"dirty inactive X never activates/subscribes");
 await model.saveFile(dirtySurface);await closeWithNativeBubbling("b");assert.equal(closes,2);assert.equal(activations,0);assert.deepEqual(order(),["a","c"]);assert.equal(model.state.activeWorkspace!.id,"a");assert.equal(server.workspaces.length,3);
 assert.equal(server.workspaceRequests.length,requestsBefore,"clean inactive X only hides membership");
 await drag("a","c","after");assert.equal(activations,0);assert.equal(reorders,1);assert.equal(server.workspaceRequests.length,requestsBefore);
 // Translate the painted hosts without onLayout, as horizontal scrolling
 // does in GPUIX. The component must query current bounds at final drop.
 host.setOffset(65);await drag("c","a","after");assert.equal(activations,0);assert.equal(reorders,2);
 const tab=node("workspace:a"),tabBox=box("workspace:a"),beforeCancel=membership();dispatch(tab.id,"mouseDown",tabBox.x+60,45);dispatch(tab.id,"mouseMove",tabBox.x+80,45);await settle();const layer=node("workspace-drag-layer");dispatch(layer.id,"mouseUp",tabBox.x+80,120);await settle();dispatch(tab.id,"click",tabBox.x+80,120);assert.deepEqual(membership(),beforeCancel);assert.equal(reorders,2);assert.equal(activations,0);
 const plus=node("workspace-new"),plusBounds=box("workspace-new");dispatch(plus.id,"mouseDown",plusBounds.x+15,45);dispatch(plus.id,"click",plusBounds.x+15,45);const created=await createTask;await settle();assert.ok(created);assert.equal(creates,1);assert.equal(server.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length,1);assert.equal(model.state.activeSurfaceId,`welcome:${created.id}`);assert.deepEqual(order(),membership());
 // Full disk decoding plus a new model, never a window relaunch.
 const directory=mkdtempSync(join(tmpdir(),"neoism-workspace-tabs-host-"));
 try{
  const store=new SessionStore(config,{NEOISM_GPUIX_STATE_PATH:join(directory,"state.json")});store.write(model.captureSession());const recovery=store.read()!;const expected=membership();
  release();model.dispose();client.stop();model=new WorkspaceModel(client=new DaemonConnection(config),config);model.restoreSession(recovery);model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);setState(model.state);release=model.subscribe(()=>setState(model.state));await settle();
  assert.deepEqual(membership(),expected);assert.deepEqual(order(),expected);assert.equal(model.state.activeWorkspace?.id,created.id);assert.equal(server.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").length,1);
 }finally{rmSync(directory,{recursive:true,force:true});}
 assert.equal(failure,undefined);assert.equal(server.ptyRequests.length,0);assert.equal(server.workspaceRequests.some(request=>request.tag==="CloseHostWorkspace"),false);
 console.log(JSON.stringify({actualSolidHost:true,windowsCreated:0,nonselectableTabChromeAndGhost:true,childParentClickMicrotaskGuard:true,dirtyInactiveCloseRejected:true,cleanInactiveCloseDoesNotActivate:true,retainedHostReorders:2,pointerFollowingSamples,intermediateSlideSamples,scrolledDropUsesCurrentGeometry:true,outsideDropCancels:true,backendCreates:1,oneBackendActionPerClick:true,persistedOrderRestored:true,ptyCreates:0}));
}finally{release();root.unmount();model.dispose();client.stop();server.stop();}
