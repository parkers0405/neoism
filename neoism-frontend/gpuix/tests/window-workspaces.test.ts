import {expect,test} from "bun:test";
import {getWindowWorkspaces,includeWindowWorkspace,openWorkspaceIds} from "../src/workspaces/window-tabs";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel,type Surface} from "../src/model";
import {NativeEditor} from "../src/native-core";
import {WorkspaceLayout} from "../src/layout/WorkspaceLayout";
import type {SavedSession} from "../src/session-store";
import {createAgentDaemon} from "./support/agent-daemon";
const catalog=[{id:"a",host_id:"host",title:"A",root_dir:"/same"},{id:"b",host_id:"host",title:"B",root_dir:"/same"},{id:"c",host_id:"host",title:"C",root_dir:"/other"}];
const until=async(test:()=>boolean)=>{const end=Date.now()+2500;while(!test()){if(Date.now()>end)throw new Error("Window workspace state did not converge");await Bun.sleep(5);}};
const saved=(ui:Record<string,unknown>={}):SavedSession=>({version:1,surfaces:[],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui});
async function fixture(recovery?:SavedSession){
 const daemon=createAgentDaemon();daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/window-workspaces"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);if(recovery)model.restoreSession(recovery);model.start();client.start();
 await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 return {daemon,client,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("window workspace order is independent from the full catalog and never deduplicates roots",()=>{
 const ui={openWorkspaceIds:["b","missing","a","b",false,"","   "]};
 expect(openWorkspaceIds(ui)).toEqual(["b","missing","a"]);
 expect(getWindowWorkspaces({workspaces:catalog,ui}).map(workspace=>workspace.id)).toEqual(["b","a"]);
 expect(getWindowWorkspaces({workspaces:[catalog[2]!,catalog[0]!,catalog[1]!],ui}).map(workspace=>workspace.id)).toEqual(["b","a"]);
 expect(getWindowWorkspaces({workspaces:catalog,ui:{}})).toEqual([]);
 expect(includeWindowWorkspace(ui,"c")).toEqual(["b","missing","a","c"]);expect(includeWindowWorkspace(ui,"b")).toEqual(["b","missing","a"]);
 expect(ui.openWorkspaceIds).toEqual(["b","missing","a","b",false,"","   "]);
});
test("legacy recovery migrates only the chosen startup workspace and retains hidden dirty state",async()=>{
 const surface:Surface={id:"file:b:hidden.rs",workspaceId:"b",kind:"file",title:"hidden.rs",path:"hidden.rs",rootDir:"/project/b"};
 const editor=new NativeEditor("hidden.rs","original",false);editor.call("paste",{text:"dirty "});
 const layout=new WorkspaceLayout();layout.open(surface);const layoutSnapshot=layout.snapshot();layout.dispose();
 const recovery={...saved({layouts:{b:layoutSnapshot},unrelated:{keep:true}}),activeWorkspaceId:"a",surfaces:[surface],editors:{[surface.id]:editor.snapshot()},drafts:{b:{agent:"plan",model:"other/model"}},composerDrafts:{"home:b":{text:"unsent private draft",attachments:[]}}};editor.dispose();
 const {daemon,model,close}=await fixture(recovery);
 try{
  expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a"]);expect(model.state.workspaces.map(workspace=>workspace.id)).toEqual(["a","b"]);
  expect(model.state.ui.openWorkspaceIds).toEqual(["a"]);expect(model.getEditor(surface).text()).toBe("dirty original");expect(model.getEditor(surface).frame.dirty).toBe(true);
  expect((model.state.ui.layouts as Record<string,unknown>).b).toEqual(layoutSnapshot);expect(model.state.drafts.b).toEqual(recovery.drafts.b);expect(model.state.composerDrafts["home:b"]).toEqual(recovery.composerDrafts["home:b"]);
  expect(daemon.ptyRequests).toEqual([]);expect(daemon.workspaceRequests.some(request=>["CreateWorkspace","CreateHostWorkspace","CloseHostWorkspace"].includes(request.tag))).toBe(false);
 }finally{close();}
});
test("saved membership restores its own order even when the active ID is absent",async()=>{
 const {model,close}=await fixture(saved({openWorkspaceIds:["b","a"]}));
 try{expect(model.state.activeWorkspace?.id).toBe("b");expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["b","a"]);expect(model.captureSession().ui.openWorkspaceIds).toEqual(["b","a"]);}finally{close();}
});
test("catalog broadcasts do not open extra tabs; explicit activation appends once and reconnect preserves membership",async()=>{
 const {daemon,client,model,close}=await fixture();
 try{
  const extra={id:"c",host_id:"host",title:"C",root_dir:"/project/c"};daemon.workspaces.push(extra);
  daemon.pushWorkspace({HostWorkspaceTree:{hosts:[{id:"host",label:"Host"}],workspaces:[extra,...daemon.workspaces.slice(0,2)].reverse(),tabs:[]}});
  await until(()=>model.state.workspaces.length===3);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a"]);
  await model.activateWorkspace(daemon.workspaces[1]!);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a","b"]);
  const membership=model.state.ui.openWorkspaceIds;await model.activateWorkspace(daemon.workspaces[1]!);expect(model.state.ui.openWorkspaceIds).toBe(membership);
  const subscriptions=daemon.workspaceRequests.filter(request=>request.tag==="SubscribeWorkspace").length;client.reconnect();
  await until(()=>client.state.phase==="connected"&&!model.state.loading&&daemon.workspaceRequests.filter(request=>request.tag==="SubscribeWorkspace").length>subscriptions);
  expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a","b"]);expect(model.state.workspaces).toHaveLength(3);
  expect(daemon.workspaceRequests.some(request=>["CreateWorkspace","CreateHostWorkspace","CloseHostWorkspace"].includes(request.tag))).toBe(false);
 }finally{close();}
});
