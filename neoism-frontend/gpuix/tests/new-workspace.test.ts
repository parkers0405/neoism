import {expect,test,spyOn} from "bun:test";
import {loadConfig,type AppConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel,type AppState} from "../src/model";
import type {SavedSession} from "../src/session-store";
import {createAgentDaemon} from "./support/agent-daemon";
import {WindowWorkspaceSessions} from "../src/workspaces/window-sessions";
import * as startupDirectory from "../src/daemon/startup-directory";
import {NativeEditor} from "../src/native-core";

const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Workspace creation did not converge");await Bun.sleep(5);}};
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={},recovery?:SavedSession,catalog?:ReturnType<typeof createAgentDaemon>["workspaces"],overrides:Partial<AppConfig>={}){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);
 if(catalog)daemon.workspaces.splice(0,daemon.workspaces.length,...catalog);
  const config={...loadConfig(["--config","/nonexistent/new-workspace"],{}),endpoint:daemon.endpoint,authToken:undefined,...overrides};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);if(recovery)model.restoreSession(recovery);model.start();client.start();
 await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 return {daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("new workspace uses its current host/root and publishes a selected Welcome without creating a PTY",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  await model.openFile("README.md");const old=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
  const editor=model.getEditor(old);editor.call("paste",{text:"unsaved "});model.setComposerDraft("home:a",{text:"keep draft",attachments:[]});
  const parentDirectories=model.state.directories,parentExpanded=model.state.expanded;
  const frames:AppState[]=[];const unsubscribe=model.subscribe(()=>frames.push(model.state));
  const created=await model.newWorkspace();unsubscribe();expect(created).toBeDefined();const id=created!.id;
  expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  expect(daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace")).toEqual([{tag:"CreateHostWorkspace",body:{workspace_id:id,host_id:"host",root_dir:"/project/a",title:"a"}}]);
  const first=frames.find(frame=>frame.workspaces.some(workspace=>workspace.id===id))!;
  expect(first.activeWorkspace?.id).toBe(id);expect(first.activeSurfaceId).toBe(`welcome:${id}`);
  expect(first.directories[""]).toEqual(parentDirectories[""]);expect(first.loading).toBe(false);
  expect(first.directories).not.toBe(parentDirectories);expect(first.expanded).not.toBe(parentExpanded);
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

test("the window plus inherits the effective terminal directory instead of its older catalog root",async()=>{
 const f=await fixture();
 const session={model:f.model,config:f.model.config};
 const tabs=new WindowWorkspaceSessions(session,{activate:()=>{},local:async()=>session,dispose:async()=>{}});
 try{
  await f.model.newTerminal();const main=f.model.mainTerminalSurface()!;
  f.daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/project/a/nested"}});
  await until(()=>f.model.state.activeWorkspace?.root_dir==="/project/a/nested"&&!f.model.state.loading);
  expect(f.model.state.workspaces[0].root_dir).toBe("/project/a");
  const created=await tabs.newWorkspace();
  expect(created?.root_dir).toBe("/project/a/nested");
  expect(f.daemon.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace").at(-1)?.body.root_dir).toBe("/project/a/nested");
  const home=f.model.ensureWelcome()!;await f.model.ensureHomeTerminal(home.id);
  expect(f.daemon.ptyRequests.filter(request=>request.tag==="CreatePty").at(-1)?.body.cwd).toBe("/project/a/nested");
 }finally{tabs.dispose();f.close();}
});

test("a later Home in a followed workspace also starts in its effective directory",async()=>{
 const f=await fixture();try{
  f.model.setUI({terminalWorkspaceRoots:{a:{declaredRoot:"/project/a",root:"/project/a/nested"}}});
  await f.model.activateWorkspace(f.model.state.workspaces[0]);
  const home=f.model.newWelcome()!;await f.model.ensureHomeTerminal(home.id);
  expect(f.daemon.ptyRequests.filter(request=>request.tag==="CreatePty").at(-1)?.body.cwd).toBe("/project/a/nested");
  expect(f.model.state.surfaces.find(surface=>surface.id===home.id)?.sessionId).toBeDefined();
 }finally{f.close();}
});

test("implicit macOS recovery replaces only the mistaken root tab and preserves its dirty data and shells",async()=>{
 const policy=spyOn(startupDirectory,"recoveredStartupDirectory").mockImplementation(root=>root==="/"?"/Users/person":undefined);
 const file={id:"dirty-file",kind:"file" as const,title:"dirty.rs",path:"dirty.rs",workspaceId:"a",rootDir:"/"};
 const editor=new NativeEditor("dirty.rs","original",false);editor.call("paste",{text:"unsaved "});
 const snapshot=editor.snapshot();editor.dispose();
 const recovery:SavedSession={version:1,activeWorkspaceId:"a",surfaces:[{id:"old-home",kind:"welcome",title:"Home",workspaceId:"a",sessionId:"old-pty",rootDir:"/"},file],editors:{[file.id]:snapshot},providerStates:{},drafts:{},composerDrafts:{"home:a":{text:"unsent",attachments:[]}},ui:{openWorkspaceIds:["a","b"],homeTerminalSessions:{"old-home":"old-pty"}}};
 const f=await fixture({},recovery,[{id:"a",host_id:"host",title:"Home",root_dir:"/"},{id:"b",host_id:"host",title:"Project",root_dir:"/project/b"}]);
 try{
  const id=f.model.state.activeWorkspace!.id;
  expect(id).not.toBe("a");expect(f.model.state.activeWorkspace?.root_dir).toBe("/Users/person");
  expect(f.model.windowWorkspaces().map(workspace=>workspace.id)).toEqual([id,"b"]);
  expect(f.model.state.workspaces.find(workspace=>workspace.id==="a")?.root_dir).toBe("/");
  expect(f.model.state.surfaces).toContainEqual(recovery.surfaces[0]);
  expect(f.model.getEditor(file).text()).toBe("unsaved original");expect(f.model.getEditor(file).frame.dirty).toBe(true);
  expect(f.model.state.composerDrafts["home:a"]?.text).toBe("unsent");
  const home=f.model.ensureWelcome()!;await f.model.ensureHomeTerminal(home.id);
  expect(f.daemon.ptyRequests.filter(request=>request.tag==="CreatePty").at(-1)?.body.cwd).toBe("/Users/person");
  expect((await f.model.newWorkspace())?.root_dir).toBe("/Users/person");
  expect(f.daemon.ptyRequests.some(request=>["ClosePty","PtyInput","ChangeTerminalDirectory"].includes(request.tag))).toBe(false);
  expect(f.daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);
 }finally{f.close();policy.mockRestore();}
});

for(const protection of ["explicit workspace","explicit folder","remote connection","followed directory"]){
 test(`startup recovery preserves an ${protection}`,async()=>{
  const policy=spyOn(startupDirectory,"recoveredStartupDirectory").mockImplementation(root=>root==="/"?"/Users/person":undefined);
  const recovery:SavedSession={version:1,activeWorkspaceId:"a",surfaces:[],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:["a"],
   ...(protection==="explicit folder"?{explicitWorkspaceRoots:{a:"/"}}:{}),
   ...(protection==="followed directory"?{terminalWorkspaceRoots:{a:{declaredRoot:"/",root:"/project/current"}}}:{})}};
  const overrides=protection==="explicit workspace"?{workspaceId:"a"}:protection==="remote connection"?{collaborative:true}:{};
  const f=await fixture({},recovery,[{id:"a",host_id:"host",title:"Home",root_dir:"/"}],overrides);
  try{
   expect(f.model.state.activeWorkspace?.id).toBe("a");
   expect(f.model.state.activeWorkspace?.root_dir).toBe(protection==="followed directory"?"/project/current":"/");
   expect(f.daemon.workspaceRequests.some(request=>request.tag==="CreateHostWorkspace")).toBe(false);
  }finally{f.close();policy.mockRestore();}
 });
}

test("the real window plus resolves locally instead of inheriting a stale foreign active workspace",async()=>{
 const f=await fixture();const session={model:f.model,config:f.model.config};
 const tabs=new WindowWorkspaceSessions(session,{activate:()=>{},local:async()=>session,dispose:async()=>{}});
 try{
  const foreign={id:"foreign",host_id:"other-host",title:"Foreign",root_dir:"/foreign"};
  f.model.update({workspaces:[foreign],activeWorkspace:foreign});
  const workspace=await tabs.newWorkspace();
  expect(workspace?.host_id).toBe("host");expect(workspace?.root_dir).toBe("/project/a");
  expect(f.daemon.workspaceRequests.some(request=>request.tag==="CreateHostWorkspace")).toBe(false);
 }finally{tabs.dispose();f.close();}
});
