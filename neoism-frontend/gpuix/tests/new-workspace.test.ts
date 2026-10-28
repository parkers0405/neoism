import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel,type AppState} from "../src/model";
import type {SavedSession} from "../src/session-store";
import {createAgentDaemon} from "./support/agent-daemon";

const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Workspace creation did not converge");await Bun.sleep(5);}};
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={},recovery?:SavedSession,catalog?:ReturnType<typeof createAgentDaemon>["workspaces"]){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);
 if(catalog)daemon.workspaces.splice(0,daemon.workspaces.length,...catalog);
 const config={...loadConfig(["--config","/nonexistent/new-workspace"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);if(recovery)model.restoreSession(recovery);model.start();client.start();
 await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 return {daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("new workspace uses its current host/root and publishes a selected Welcome without creating a PTY",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  await model.openFile("README.md");const old=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
  const editor=model.getEditor(old);editor.call("paste",{text:"unsaved "});model.setComposerDraft("home:a",{text:"keep draft",attachments:[]});
  const frames:AppState[]=[];const unsubscribe=model.subscribe(()=>frames.push(model.state));
  const created=await model.newWorkspace();unsubscribe();expect(created).toBeDefined();const id=created!.id;
  expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  expect(daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace")).toEqual([{tag:"CreateHostWorkspace",body:{workspace_id:id,host_id:"host",root_dir:"/project/a",title:"a"}}]);
  const first=frames.find(frame=>frame.workspaces.some(workspace=>workspace.id===id))!;
  expect(first.activeWorkspace?.id).toBe(id);expect(first.activeSurfaceId).toBe(`welcome:${id}`);
  expect(first.surfaces.find(surface=>surface.id===first.activeSurfaceId)).toMatchObject({kind:"welcome",workspaceId:id,title:"Home"});
  expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a",id]);expect(model.state.workspaces.map(workspace=>workspace.id)).toEqual(["a","b",id]);
  expect(model.getLayout(id).frame.focusedSurfaceId).toBe(`welcome:${id}`);expect(model.state.creatingWorkspace).toBe(false);
  expect(editor.frame.dirty).toBe(true);expect(editor.text()).toStartWith("unsaved ");expect(model.state.surfaces).toContainEqual(old);expect(model.state.composerDrafts["home:a"]?.text).toBe("keep draft");
  expect(daemon.ptyRequests).toEqual([]);expect(daemon.workspaceRequests.some(request=>request.tag==="CloseHostWorkspace")).toBe(false);
  const recovery=model.captureSession();const restored=await fixture({},recovery,daemon.workspaces);
  try{
   expect(restored.model.state.activeWorkspace?.id).toBe(id);expect(restored.model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a",id]);
   expect(restored.model.state.activeSurfaceId).toBe(`welcome:${id}`);expect(restored.model.getEditor(old).text()).toStartWith("unsaved ");expect(restored.model.getEditor(old).frame.dirty).toBe(true);
   expect(restored.daemon.ptyRequests).toEqual([]);expect(restored.daemon.workspaceRequests.some(request=>request.tag==="CreateHostWorkspace")).toBe(false);
  }finally{restored.close();}
 }finally{close();}
});
test("rapid workspace clicks serialize distinct creates and retain both memberships",async()=>{
 const {daemon,model,close}=await fixture({workspaceCreateDelayMs:25});
 try{
  const first=model.newWorkspace(),second=model.newWorkspace();expect(model.state.creatingWorkspace).toBe(true);
  await until(()=>daemon.workspaceRequests.some(request=>request.tag==="CreateHostWorkspace"));
  expect(daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace")).toHaveLength(1);
  const a=await first;expect(model.state.creatingWorkspace).toBe(true);const b=await second;
  expect(a).toBeDefined();expect(b).toBeDefined();expect(a!.id).not.toBe(b!.id);
  expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a",a!.id,b!.id]);expect(model.state.activeWorkspace?.id).toBe(b!.id);
  expect(model.state.surfaces.filter(surface=>surface.workspaceId===a!.id)).toHaveLength(1);expect(model.state.surfaces.filter(surface=>surface.workspaceId===b!.id)).toHaveLength(1);
  expect(daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").map(request=>request.body.root_dir)).toEqual(["/project/a","/project/a"]);
  expect(daemon.ptyRequests).toEqual([]);expect(model.state.creatingWorkspace).toBe(false);
 }finally{close();}
});
test("failed workspace creation preserves the selected workspace, tabs, recovery, and later retries",async()=>{
 const options={workspaceCreateError:"Host is unavailable" as string|undefined};const {daemon,model,close}=await fixture(options);
 try{
  const before=model.captureSession(),catalog=model.state.workspaces;
  expect(await model.newWorkspace()).toBeUndefined();expect(model.state.error).toContain("Host is unavailable");
  expect(model.captureSession()).toEqual(before);expect(model.state.workspaces).toBe(catalog);expect(model.state.activeWorkspace?.id).toBe("a");expect(model.state.creatingWorkspace).toBe(false);
  options.workspaceCreateError=undefined;const created=await model.newWorkspace();expect(created).toBeDefined();expect(model.state.activeSurfaceId).toBe(`welcome:${created!.id}`);
  expect(daemon.workspaces).toHaveLength(3);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
