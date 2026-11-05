import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {createAgentDaemon} from "./support/agent-daemon";

const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Close state did not converge");await Bun.sleep(5);}};
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={}){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/close-workspace"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
  return {daemon,model,config,client,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("q closes exactly the focused split leaf and protects the last dirty owner until q!",async()=>{
 const {model,close}=await fixture();try{
  await model.openFile("README.md");const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
  const editor=model.getEditor(surface);editor.call("paste",{text:"dirty "});const layout=model.getLayout();layout.split("horizontal",surface);
  const closedLeaf=layout.frame.focusedLeaf!;expect(layout.frame.leaves.filter(leaf=>leaf.surfaceId===surface.id)).toHaveLength(2);
  expect(model.closeCurrentTab()).toBe(true);expect(layout.frame.leaves.some(leaf=>leaf.leaf===closedLeaf)).toBe(false);
  expect(layout.frame.leaves.filter(leaf=>leaf.surfaceId===surface.id)).toHaveLength(1);expect(editor.frame.dirty).toBe(true);expect(editor.text()).toStartWith("dirty ");
  const before=model.captureSession();expect(model.closeCurrentTab()).toBe(false);expect(model.state.error).toContain(":q!");expect(model.captureSession()).toEqual(before);
  expect(model.closeCurrentTab(true)).toBe(true);expect(model.state.surfaces.some(tab=>tab.id===surface.id)).toBe(false);
  expect(model.captureSession().editors[surface.id]).toBeUndefined();expect(model.state.files[surface.id]).toBeUndefined();
  await model.openFile("README.md");expect(model.getEditor(model.state.surfaces.find(tab=>tab.id===model.state.activeSurfaceId)!).text()).toStartWith("# Neoism");
 }finally{close();}
});
test("q closes Welcome, Agent, page and terminal tabs without deleting threads or killing shells",async()=>{
 const {daemon,model,close}=await fixture();try{
  const welcome=model.newWelcome()!;expect(model.closeCurrentTab()).toBe(true);expect(model.state.surfaces.some(surface=>surface.id===welcome.id)).toBe(false);
  const agent=model.newAgentHome()!;expect(model.closeCurrentTab()).toBe(true);expect(model.state.surfaces.some(surface=>surface.id===agent.id)).toBe(false);
  model.openPage("extensions");const page=model.state.activeSurfaceId;expect(model.closeCurrentTab()).toBe(true);expect(model.state.surfaces.some(surface=>surface.id===page)).toBe(false);
  await model.newTerminal();const terminal=model.state.activeSurfaceId;expect(model.closeCurrentTab()).toBe(true);expect(model.state.surfaces.some(surface=>surface.id===terminal)).toBe(false);
  expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);expect(daemon.ptyRequests.some(request=>request.tag==="ClosePty")).toBe(false);
  expect(model.state.activeWorkspace?.id).toBe("a");expect(model.state.workspaces).toHaveLength(2);expect(model.state.surfaces.some(surface=>surface.kind==="welcome")).toBe(true);
  expect(daemon.requests.some(request=>request.tag==="CreateThread")).toBe(false);
 }finally{close();}
});
test("qall closes target tabs and selects its neighbor without touching other dirty documents or conversation drafts",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.openFile("README.md");const file=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const editor=model.getEditor(file);editor.call("paste",{text:"keep dirty A "});
  await model.activateWorkspace(daemon.workspaces[1]!);model.setComposerDraft("ongoing-chat",{text:"Conversation draft survives",attachments:[]});await model.newTerminal();
  expect(await model.closeWorkspace()).toBe(true);
  expect(model.state.activeWorkspace?.id).toBe("a");expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a"]);
  expect(model.state.workspaces.map(workspace=>workspace.id)).toEqual(["a"]);expect(model.state.surfaces.filter(surface=>surface.workspaceId==="b")).toEqual([]);expect(model.layouts.has("b")).toBe(false);
  expect(model.state.composerDrafts["ongoing-chat"]?.text).toBe("Conversation draft survives");expect(editor.text()).toStartWith("keep dirty A ");expect(editor.frame.dirty).toBe(true);
  expect(daemon.ptyRequests.some(request=>request.tag==="ClosePty")).toBe(false);expect(daemon.workspaceRequests.some(request=>request.tag==="CloseHostWorkspace")).toBe(false);
  expect(daemon.workspaceRequests.filter(request=>request.tag==="CloseWindowWorkspace")).toHaveLength(1);
 }finally{close();}
});
test("workspace dirty guards reject unchanged; qall! discards only target documents, including inactive workspace X",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.openFile("README.md");const a=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;model.getEditor(a).call("paste",{text:"discard A "});
  await model.activateWorkspace(daemon.workspaces[1]!);await model.openFile("README.md");const b=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const bEditor=model.getEditor(b);bEditor.call("paste",{text:"preserve B "});
  const before=model.captureSession();expect(await model.closeWorkspace(false,"a")).toBe(false);expect(model.state.error).toContain(":qall!");expect(model.captureSession()).toEqual(before);
  expect(await model.closeWorkspace(true,"a")).toBe(true);expect(model.state.activeWorkspace?.id).toBe("b");expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["b"]);
  expect(model.state.surfaces.some(surface=>surface.id===a.id)).toBe(false);expect(model.captureSession().editors[a.id]).toBeUndefined();expect(model.getEditor(b)).toBe(bEditor);expect(bEditor.frame.dirty).toBe(true);expect(bEditor.text()).toStartWith("preserve B ");
  expect(model.state.workspaces).toHaveLength(1);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
test("closing the last workspace leaves a local empty window without creating a replacement",async()=>{
  const {daemon,model,close}=await fixture({workspaceCreateError:"Must not create replacement"});try{
   expect(await model.closeWorkspace()).toBe(true);
   expect(model.windowWorkspaces()).toEqual([]);expect(model.state.activeWorkspace).toBeUndefined();expect(model.state.activeSurfaceId).toBeUndefined();
   expect(model.state.workspaces.map(workspace=>workspace.id)).toEqual(["b"]);
   expect(model.state.surfaces.some(surface=>surface.workspaceId==="a")).toBe(false);expect(daemon.ptyRequests).toEqual([]);
   expect(daemon.workspaceRequests.some(request=>request.tag==="CreateHostWorkspace")).toBe(false);
   for(let i=0;i<5;i++)expect(await model.closeWorkspace()).toBe(false);
   expect(daemon.workspaces).toHaveLength(1);
 }finally{close();}
});
test("repeated create and close of unused same-root workspaces does not grow the daemon catalog",async()=>{
 const {daemon,model,close}=await fixture();try{
  for(let i=0;i<8;i++){
   const created=await model.newWorkspace();expect(created?.root_dir).toBe("/project/a");expect(daemon.workspaces).toHaveLength(3);
   expect(await model.closeWorkspace()).toBe(true);expect(model.state.activeWorkspace?.id).toBe("a");
   expect(daemon.workspaces).toHaveLength(2);expect(model.state.surfaces.some(surface=>surface.workspaceId===created!.id)).toBe(false);
   expect((model.captureSession().ui.layouts as Record<string,unknown>)[created!.id]).toBeUndefined();
  }
  const first=await model.newWorkspace(),second=await model.newWorkspace();
  expect(first!.id).not.toBe(second!.id);expect(first!.root_dir).toBe(second!.root_dir);
  expect(model.windowWorkspaces()).toHaveLength(3);
 }finally{close();}
});
test("closing a newly-created last workspace prunes it without a replacement, then plus works",async()=>{
 const {daemon,model,close}=await fixture();try{
  const created=await model.newWorkspace();await model.closeWorkspace(false,"a");
  expect(await model.closeWorkspace()).toBe(true);expect(model.state.activeWorkspace).toBeUndefined();
  expect(daemon.workspaces.some(workspace=>workspace.id===created!.id)).toBe(false);
  const next=await model.newWorkspace();expect(next).toBeDefined();expect(model.windowWorkspaces()).toHaveLength(1);
 }finally{close();}
});
test("empty-window recovery stays empty and does not resolve or recreate a workspace",async()=>{
 const {daemon,model,config,close}=await fixture();let restored:WorkspaceModel|undefined,client:DaemonConnection|undefined;try{
  await model.closeWorkspace();const recovery=model.captureSession(),before=daemon.workspaceRequests.length;
  client=new DaemonConnection(config);restored=new WorkspaceModel(client,config);restored.restoreSession(recovery);restored.start();client.start();
  await until(()=>restored!.state.connection.phase==="connected"&&!restored!.state.loading);
  expect(restored.windowWorkspaces()).toEqual([]);expect(restored.state.activeWorkspace).toBeUndefined();
  expect(daemon.workspaceRequests.slice(before).some(request=>["ResolveInitialWorkspace","CreateHostWorkspace"].includes(request.tag))).toBe(false);
  expect(await restored.newWorkspace()).toBeDefined();
 }finally{restored?.dispose();client?.stop();close();}
});
test("used workspaces close, while unsent local drafts require explicit discard",async()=>{
 const {daemon,model,close}=await fixture();try{
  const used=await model.newWorkspace();await model.openFile("README.md");model.closeCurrentTab();
   expect(await model.closeWorkspace()).toBe(true);expect(daemon.workspaces.some(workspace=>workspace.id===used!.id)).toBe(false);
  const draft=await model.newWorkspace();model.setComposerDraft(`home:${draft!.id}`,{text:"Keep unsent",attachments:[]});
   expect(await model.closeWorkspace()).toBe(false);expect(daemon.workspaces.some(workspace=>workspace.id===draft!.id)).toBe(true);
   expect(model.state.composerDrafts[`home:${draft!.id}`]?.text).toBe("Keep unsent");
   expect(await model.closeWorkspace(true)).toBe(true);expect(daemon.workspaces.some(workspace=>workspace.id===draft!.id)).toBe(false);
   expect(model.state.composerDrafts[`home:${draft!.id}`]).toBeUndefined();
 }finally{close();}
});
test("another window protects its workspace while this window releases its own tabs",async()=>{
 const {daemon,model,config,close}=await fixture();let second:WorkspaceModel|undefined,client:DaemonConnection|undefined;try{
  const created=await model.newWorkspace();
  client=new DaemonConnection(config);second=new WorkspaceModel(client,config);second.restoreSession(model.captureSession());second.start();client.start();
  await until(()=>!!second!.state.activeWorkspace&&!second!.state.loading);
  await second.activateWorkspace(daemon.workspaces[1]!);
  expect(await model.closeWorkspace()).toBe(true);
  expect(daemon.workspaces.some(workspace=>workspace.id===created!.id)).toBe(true);
   expect(second.windowWorkspaces().some(workspace=>workspace.id===created!.id)).toBe(true);
   expect(model.state.surfaces.some(surface=>surface.workspaceId===created!.id)).toBe(false);
  expect(second.state.activeWorkspace?.id).toBe("b");
 }finally{second?.dispose();client?.stop();close();}
});
test("workspace close queues behind creation and cannot remove or select its new neighbor out of order",async()=>{
 const {daemon,model,close}=await fixture({workspaceCreateDelayMs:25});try{
  const creation=model.newWorkspace(),closing=model.closeWorkspace(false,"a");const created=await creation;expect(await closing).toBe(true);
   expect(model.state.activeWorkspace?.id).toBe(created!.id);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual([created!.id]);expect(daemon.workspaces).toHaveLength(2);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
test("a legacy daemon cannot silently pretend to close workspace resources",async()=>{
 const {daemon,model,close}=await fixture();try{
  model.state={...model.state,connection:{...model.state.connection,workspaceLifecycle:false}};
  const before=daemon.workspaceRequests.length,created=await model.newWorkspace();
   expect(await model.closeWorkspace()).toBe(false);
  expect(daemon.workspaceRequests.slice(before).some(request=>request.tag==="SetWindowWorkspaces")).toBe(false);
  expect(daemon.workspaces.some(workspace=>workspace.id===created!.id)).toBe(true);
  expect(model.state.error).toContain("Update and restart the daemon");
 }finally{close();}
});
test("plus from an empty catalog resolves exactly one default workspace",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.closeWorkspace();daemon.workspaces.splice(0);model.state={...model.state,workspaces:[]};
  const before=daemon.workspaceRequests.length,created=await model.newWorkspace();
  expect(created).toBeDefined();expect(daemon.workspaces).toHaveLength(1);expect(model.windowWorkspaces()).toHaveLength(1);
  expect(daemon.workspaceRequests.slice(before).filter(request=>request.tag==="ResolveInitialWorkspace")).toHaveLength(1);
  expect(daemon.workspaceRequests.slice(before).some(request=>request.tag==="CreateHostWorkspace")).toBe(false);
 }finally{close();}
});

test("a rejected host close keeps the workspace, documents and tabs usable",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.openFile("README.md");const surface=model.state.surfaces.find(surface=>surface.kind==="file")!,editor=model.getEditor(surface);
  await model.newTerminal();const before=model.captureSession(),released:string[]=[];model.onWorkspaceClosed(id=>released.push(id));
  daemon.workspaceClose.error="Close refused";
  expect(await model.closeWorkspace()).toBe(false);expect(model.state.error).toContain("Close refused");
  expect(model.state.activeWorkspace?.id).toBe("a");expect(model.captureSession().surfaces).toEqual(before.surfaces);expect(model.getEditor(surface)).toBe(editor);
  expect(released).toEqual([]);expect(daemon.ptyRequests.some(request=>request.tag==="ClosePty")).toBe(false);
 }finally{close();}
});

test("a late terminal creation cannot resurrect a closed workspace or leak its newly owned PTY",async()=>{
 const {daemon,model,close}=await fixture();try{
  daemon.ptyCreations.defer=true;const creating=model.newTerminal();await until(()=>daemon.ptyCreations.pending.length===1);
  expect(await model.closeWorkspace()).toBe(true);daemon.ptyCreations.pending.shift()!();await creating;
  expect(daemon.ptyRequests.filter(request=>request.tag==="ClosePty").map(request=>request.body.session_id)).toEqual(["fixture-pty"]);
  expect(model.state.surfaces.some(surface=>surface.workspaceId==="a")).toBe(false);expect(model.layouts.has("a")).toBe(false);
 }finally{close();}
});

test("closing an agent workspace drops view history but preserves the running conversation and can reopen it",async()=>{
 const {daemon,model,close}=await fixture();try{
  const thread={session_id:"working",title:"Working",directory:"/project/a",agent:"build",model:"openai/base"};daemon.threads.push(thread);
  daemon.history.set(thread.session_id,[{id:"part",role:"Assistant",kind:"Assistant",text:"Retained on server"}]);
  model.openThread(thread);await until(()=>model.state.history.working?.length===1);
  daemon.push({StreamingState:{session_id:"working",state:"Generating",label:null}});await until(()=>model.state.streaming.working==="Generating");
  const after=daemon.requests.length;expect(await model.closeWorkspace()).toBe(true);
  await until(()=>!daemon.agentStreams.has("working"));expect(model.state.history.working).toBeUndefined();
  expect(daemon.requests.slice(after).some(request=>["Cancel","CancelInflight","ClearQueue","DeleteThread"].includes(request.tag))).toBe(false);
  expect(daemon.history.get("working")).toHaveLength(1);expect(daemon.threads).toContainEqual(thread);
  daemon.push({ContentDelta:{session_id:"working",message_id:"late",text:"background data",kind:"Text"}});
  daemon.push({StreamingState:{session_id:"working",state:"Thinking",label:null}});await until(()=>model.state.streaming.working==="Thinking");
  expect(model.state.history.working).toBeUndefined();
  await model.activateWorkspace(daemon.workspaces[0]!);model.openThread(thread);await until(()=>model.state.history.working?.length===1);
  expect(model.state.history.working![0]!.text).toBe("Retained on server");
 }finally{close();}
});
