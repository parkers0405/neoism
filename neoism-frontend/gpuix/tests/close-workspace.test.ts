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
 return {daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
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
test("qall hides only the current workspace and selects its adjacent tab, retaining other dirty state and PTYs",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.openFile("README.md");const file=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const editor=model.getEditor(file);editor.call("paste",{text:"keep dirty A "});
  await model.activateWorkspace(daemon.workspaces[1]!);model.setComposerDraft("home:b",{text:"B draft survives hiding",attachments:[]});await model.newTerminal();
  const before=model.captureSession();expect(await model.closeWorkspace()).toBe(true);
  expect(model.state.activeWorkspace?.id).toBe("a");expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a"]);
  expect(model.state.workspaces.map(workspace=>workspace.id)).toEqual(["a","b"]);expect(model.state.surfaces.filter(surface=>surface.workspaceId==="b")).toEqual(before.surfaces.filter(surface=>surface.workspaceId==="b"));
  expect(model.state.composerDrafts["home:b"]?.text).toBe("B draft survives hiding");expect(editor.text()).toStartWith("keep dirty A ");expect(editor.frame.dirty).toBe(true);
  expect(daemon.ptyRequests.some(request=>request.tag==="ClosePty")).toBe(false);expect(daemon.workspaceRequests.some(request=>request.tag==="CloseHostWorkspace")).toBe(false);
  await model.activateWorkspace(daemon.workspaces[1]!);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["a","b"]);
 }finally{close();}
});
test("workspace dirty guards reject unchanged; qall! discards only target documents, including inactive workspace X",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.openFile("README.md");const a=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;model.getEditor(a).call("paste",{text:"discard A "});
  await model.activateWorkspace(daemon.workspaces[1]!);await model.openFile("README.md");const b=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const bEditor=model.getEditor(b);bEditor.call("paste",{text:"preserve B "});
  const before=model.captureSession();expect(await model.closeWorkspace(false,"a")).toBe(false);expect(model.state.error).toContain(":qall!");expect(model.captureSession()).toEqual(before);
  expect(await model.closeWorkspace(true,"a")).toBe(true);expect(model.state.activeWorkspace?.id).toBe("b");expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["b"]);
  expect(model.state.surfaces.some(surface=>surface.id===a.id)).toBe(false);expect(model.captureSession().editors[a.id]).toBeUndefined();expect(model.getEditor(b)).toBe(bEditor);expect(bEditor.frame.dirty).toBe(true);expect(bEditor.text()).toStartWith("preserve B ");
  expect(model.state.workspaces).toHaveLength(2);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
test("closing the last workspace creates a fresh Welcome first, while creation failure preserves the old window",async()=>{
 const options={workspaceCreateError:"Cannot create replacement" as string|undefined};const {daemon,model,close}=await fixture(options);try{
  const before=model.captureSession();expect(await model.closeWorkspace()).toBe(false);expect(model.captureSession()).toEqual(before);expect(model.state.activeWorkspace?.id).toBe("a");
  options.workspaceCreateError=undefined;const published:string[][]=[];const unsubscribe=model.subscribe(()=>published.push(model.windowWorkspaces().map(workspace=>workspace.id)));
  expect(await model.closeWorkspace()).toBe(true);unsubscribe();const replacement=model.state.activeWorkspace!;
  expect(replacement.id).not.toBe("a");expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual([replacement.id]);expect(published.every(ids=>ids.length>0)).toBe(true);
  expect(model.state.activeSurfaceId).toBe(`welcome:${replacement.id}`);expect(model.state.workspaces.map(workspace=>workspace.id)).toEqual(["a","b",replacement.id]);
  expect(model.state.surfaces.some(surface=>surface.workspaceId==="a")).toBe(true);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
test("workspace close queues behind creation and cannot remove or select its new neighbor out of order",async()=>{
 const {daemon,model,close}=await fixture({workspaceCreateDelayMs:25});try{
  const creation=model.newWorkspace(),closing=model.closeWorkspace(false,"a");const created=await creation;expect(await closing).toBe(true);
  expect(model.state.activeWorkspace?.id).toBe(created!.id);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual([created!.id]);expect(daemon.workspaces).toHaveLength(3);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
