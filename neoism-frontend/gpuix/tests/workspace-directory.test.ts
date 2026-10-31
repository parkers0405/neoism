import {expect,test} from "bun:test";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {createAgentDaemon} from "./support/agent-daemon";
import {ChromeActionsController} from "../src/menu-actions/ChromeActionsController";
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Workspace directory did not converge");await Bun.sleep(2);}};
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={}){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/workspace-directory"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 return{daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("folder change without a terminal updates the host root and Explorer without creating a shell",async()=>{
 const f=await fixture();try{
  await f.model.changeWorkspaceDirectory("/selected","a");expect(f.model.state.activeWorkspace?.root_dir).toBe("/selected");expect(f.model.state.workspaces.find(workspace=>workspace.id==="a")?.root_dir).toBe("/selected");
  expect(f.daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot")).toEqual([{tag:"SetWorkspaceRoot",body:{workspace_id:"a",root_dir:"/selected"}}]);expect(f.daemon.ptyRequests).toHaveLength(0);
  expect(f.daemon.fileRequests.some(request=>request.tag==="ListDir")).toBe(true);
 }finally{f.close();}
});
test("first terminal host-confirmed actual cwd wins, secondary shells and dirty files remain independent",async()=>{
 const f=await fixture({terminalDirectoryResult:"/actual"});try{
  await f.model.newTerminal();const main=f.model.mainTerminalSurface()!;await f.model.newTerminal();const secondary=f.model.state.surfaces.find(surface=>surface.id===f.model.state.activeSurfaceId)!;
  await f.model.openFile("README.md");const file=f.model.state.surfaces.find(surface=>surface.id===f.model.state.activeSurfaceId)!,editor=f.model.getEditor(file);editor.call("paste",{text:"dirty "});const bytes=editor.text();
  f.model.selectSurface(main.id);await f.model.changeWorkspaceDirectory("/requested","a");
  expect(f.model.state.activeWorkspace?.root_dir).toBe("/actual");expect(f.model.state.ui.terminalCwds).toMatchObject({[main.sessionId!]:"/actual"});expect(f.model.getEditor(file).text()).toBe(bytes);expect(editor.frame.dirty).toBe(true);
  expect(f.daemon.ptyRequests.filter(request=>request.tag==="ChangeTerminalDirectory").map(request=>request.body.session_id)).toEqual([main.sessionId]);expect(secondary.sessionId).not.toBe(main.sessionId);
  expect(f.daemon.ptyRequests.some(request=>request.tag==="PtyInput"||request.tag==="ClosePty")).toBe(false);expect(f.daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(2);
 }finally{f.close();}
});
test("busy or unsupported terminal never changes workspace metadata",async()=>{
 for(const terminalDirectoryChange of ["failure","ack"] as const){const f=await fixture({terminalDirectoryChange});try{
  await f.model.newTerminal();await expect(f.model.changeWorkspaceDirectory("/requested","a")).rejects.toThrow();expect(f.model.state.activeWorkspace?.root_dir).toBe("/project/a");expect(f.daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);expect(f.daemon.ptyRequests.some(request=>request.tag==="PtyInput")).toBe(false);
 }finally{f.close();}}
},15000);
test("a delayed first-terminal confirmation updates only captured A while B remains active",async()=>{
 const f=await fixture({terminalDirectoryChange:"defer"});try{
  await f.model.newTerminal();const change=f.model.changeWorkspaceDirectory("/captured/a","a");await until(()=>f.daemon.directoryChanges.length===1);await f.model.activateWorkspace(f.model.state.workspaces.find(workspace=>workspace.id==="b")!);
  f.daemon.directoryChanges.shift()!();await change;expect(f.model.state.activeWorkspace?.id).toBe("b");expect(f.model.state.activeWorkspace?.root_dir).toBe("/project/b");expect(f.model.state.workspaces.find(workspace=>workspace.id==="a")?.root_dir).toBe("/captured/a");
  expect(f.daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot").map(request=>request.body.workspace_id)).toEqual(["a"]);
 }finally{f.close();}
});
test("project folder selection shares the controlled first-shell transition instead of publishing metadata early",async()=>{
 const f=await fixture({terminalDirectoryChange:"defer"});const controller=new ChromeActionsController({model:f.model,config:f.model.config,reload:()=>{}});try{
  await f.model.newTerminal();const main=f.model.mainTerminalSurface()!,change=controller.setActiveWorkspaceRoot("/project/selected");await until(()=>f.daemon.directoryChanges.length===1);
  expect(f.daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);expect(f.daemon.ptyRequests.find(request=>request.tag==="ChangeTerminalDirectory")?.body.session_id).toBe(main.sessionId);
  f.daemon.directoryChanges.shift()!();await change;expect(f.model.state.activeWorkspace?.root_dir).toBe("/project/selected");expect(f.daemon.ptyRequests.some(request=>request.tag==="PtyInput")).toBe(false);
 }finally{controller.dispose();f.close();}
});
test("the original directory palette uses host search results and drops a stale captured workspace before any write",async()=>{
 const f=await fixture({directoryEntries:{"/project/a":[{name:"selected",is_dir:true}]}}),controller=new ChromeActionsController({model:f.model,config:f.model.config,reload:()=>{}});try{
  await controller.open("OpenDirectoryPalette");controller.call("query",{query:"cd selected"});await until(()=>!!controller.frame?.commands.some(command=>command.kind==="text"&&command.text==="/project/a/selected"));controller.call("key",{key:"down"});expect(controller.frame?.target?.path).toBe("/project/a/selected");
  expect(f.daemon.searchRequests.some(request=>request.tag==="SearchDirectories"&&request.body.cwd==="/project/a")).toBe(true);
  await f.model.activateWorkspace(f.model.state.workspaces.find(workspace=>workspace.id==="b")!);controller.call("key",{key:"enter"});await until(()=>!controller.frame);
  expect(f.model.state.activeWorkspace?.id).toBe("b");expect(f.daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);expect(f.daemon.ptyRequests).toHaveLength(0);
  await f.model.activateWorkspace(f.model.state.workspaces.find(workspace=>workspace.id==="a")!);await controller.open("OpenDirectoryPalette");controller.call("query",{query:"cd selected"});expect(controller.frame?.target?.path).toBe("selected");controller.call("key",{key:"enter"});
  await until(()=>f.model.state.activeWorkspace?.root_dir==="/project/a/selected"&&!controller.frame);expect(f.daemon.ptyRequests).toHaveLength(0);
 }finally{controller.dispose();f.close();}
});
