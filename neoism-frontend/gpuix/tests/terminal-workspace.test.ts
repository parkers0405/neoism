import { expect,test } from "bun:test";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";
import { createAgentDaemon } from "./support/agent-daemon";
import { absoluteTerminalDirectory,followsTerminalDirectory,terminalDirectoryFollowsWorkspace } from "../src/terminal/workspace";

async function until(predicate:()=>boolean){const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Terminal workspace did not converge");await Bun.sleep(5);}}
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={},createTerminal=true){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/terminal-workspace-test"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
 // Welcome is UI-only. Terminal-specific tests explicitly request their PTY.
 expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(0);
 if(createTerminal)await model.newTerminal();
 return {daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("first grid terminal binds durably and its cwd follows only this frontend's Explorer",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  const main=model.mainTerminalSurface()!;expect(main.sessionId).toBe("fixture-pty");
  expect(model.state.activeSurfaceId).toBe(main.id);
  expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);
  expect(daemon.ptyRequests.find(request=>request.tag==="CreatePty")?.body).toMatchObject({cwd:"/project/a",presentation:"grid"});
  expect(daemon.workspaceRequests.some(request=>request.tag==="BindEditorSurface"&&request.body.session_id===main.sessionId&&request.body.path===null)).toBe(true);
  daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/project/a/subdirectory"}});
  await until(()=>model.state.activeWorkspace?.root_dir==="/project/a/subdirectory"&&!model.state.loading);
  expect(model.state.workspaces.find(workspace=>workspace.id==="a")?.root_dir).toBe("/project/a");
  expect(daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);
  expect(daemon.ptyRequests.some(request=>request.tag==="PtyInput")).toBe(false);
  const saved=model.captureSession();expect(saved.ui.terminalWorkspaceRoots).toEqual({a:{declaredRoot:"/project/a",root:"/project/a/subdirectory"}});
 }finally{close();}
});
test("secondary terminal cwd stays local and root follows its observed cwd only when selected",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  const main=model.mainTerminalSurface()!;await model.newTerminal("horizontal");const secondary=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
  expect(secondary.sessionId).not.toBe(main.sessionId);expect(model.mainTerminalSurface()?.id).toBe(main.id);
  daemon.pushPty({SessionCwd:{session_id:secondary.sessionId,cwd:"/tmp/secondary"}});daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/project/a/next"}});await Bun.sleep(25);
  expect(model.state.activeWorkspace?.root_dir).toBe("/project/a");
  model.selectSurface(main.id);await until(()=>model.state.activeWorkspace?.root_dir==="/project/a/next");
  const canonical=model.state.workspaces[0];await model.activateWorkspace(model.state.workspaces[1]);await model.activateWorkspace(canonical);
  expect(model.state.activeWorkspace?.root_dir).toBe("/project/a/next");expect(model.state.workspaces[0].root_dir).toBe("/project/a");
 }finally{close();}
});
test("root terminal cd opens a same-named file as a separate scoped buffer and preserves prior dirty bytes",async()=>{
 const {daemon,model,close}=await fixture({scopedFiles:{"/project/a":{"same.rs":"alpha"},"/project/b":{"same.rs":"beta"}}});
 try{
  const main=model.mainTerminalSurface()!;await model.openFile("same.rs");
  const first=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!,editor=model.getEditor(first);editor.call("paste",{text:"unsaved "});expect(editor.frame.dirty).toBe(true);
  model.selectSurface(main.id);daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/project/b"}});await until(()=>model.state.activeWorkspace?.root_dir==="/project/b");
  await model.openFile("same.rs");const second=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
  expect(second.id).not.toBe(first.id);expect(second.rootDir).toBe("/project/b");expect(model.getEditor(second).text()).toBe("beta");
  expect(editor.text()).toBe("unsaved alpha");expect(editor.frame.dirty).toBe(true);expect(first.rootDir).toBe("/project/a");
  await model.openFile("same.rs","/project/a");expect(model.state.activeSurfaceId).toBe(first.id);expect(model.getEditor(first)).toBe(editor);
 }finally{close();}
});
test("canonical root changes also keep same-named existing buffers isolated",async()=>{
 const {model,close}=await fixture({scopedFiles:{"/project/a":{"same.rs":"alpha"},"/project/b":{"same.rs":"beta"}}});
 try{
  await model.openFile("same.rs");const first=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!,editor=model.getEditor(first);editor.call("paste",{text:"dirty "});
  const workspace={...model.state.workspaces[0],root_dir:"/project/b"};model.update({workspaces:[workspace,...model.state.workspaces.slice(1)]});await model.activateWorkspace(workspace);await model.openFile("same.rs");
  const second=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
  expect(second.id).not.toBe(first.id);expect(model.getEditor(second).text()).toBe("beta");expect(editor.text()).toBe("dirty alpha");expect(editor.frame.dirty).toBe(true);
 }finally{close();}
});
test("joined host cwd never rebases Explorer and invalid cwd observations are ignored",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  const main=model.mainTerminalSurface()!;
  const remote={...model.state.workspaces[0],host_kind:"tailscale"};model.update({workspaces:[remote,...model.state.workspaces.slice(1)]});await model.activateWorkspace(remote);
  daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/home/remote"}});await Bun.sleep(25);expect(model.state.activeWorkspace?.root_dir).toBe("/project/a");
  expect(followsTerminalDirectory({...remote,host_kind:"local"},main,main,false)).toBe(false);
  for(const value of ["relative/path","",null,"/path\0invalid"])expect(absoluteTerminalDirectory(value)).toBe(false);
  expect(absoluteTerminalDirectory("C:\\Users\\example")).toBe(true);
 }finally{close();}
});
test("legacy daemon cannot silently create a hidden-prompt grid shell or close an existing session",async()=>{
 const {daemon,model,close}=await fixture({gridPresentation:false},false);
 try{
  const welcome=model.state.surfaces.find(surface=>surface.kind==="welcome")!;expect(welcome).toBeDefined();expect(welcome.title).toBe("Home");
  model.openTerminal("existing-session");await model.newTerminal();
  expect(model.state.error).toContain("running daemon");expect(model.state.error).toContain("restart");expect(model.state.surfaces.filter(surface=>surface.kind==="terminal").map(surface=>surface.sessionId)).toEqual(["existing-session"]);
  expect(model.state.surfaces.find(surface=>surface.id===welcome.id)).toBe(welcome);
  expect(daemon.ptyRequests.filter(request=>request.tag==="ClosePty").map(request=>request.body.session_id)).toEqual(["fixture-pty"]);
  expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);
 }finally{close();}
});

test("ended PTYs retire only their terminal views and stale main role, retaining dirty files",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  const old=model.mainTerminalSurface()!;
  await model.openFile("README.md");const file=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!,editor=model.getEditor(file);editor.call("paste",{text:"dirty "});
  daemon.pushPty({SessionCwd:{session_id:old.sessionId,cwd:"/tmp/old-terminal"}});await Bun.sleep(10);
  daemon.pushPty({PtyClosed:{session_id:old.sessionId,exit_code:0}});await until(()=>!model.state.surfaces.some(surface=>surface.id===old.id));
  expect(model.mainTerminalSurface()).toBeUndefined();expect(model.state.ui.mainTerminals).toEqual({});expect(model.state.ui.terminalCwds).toEqual({});
  expect(model.getEditor(file)).toBe(editor);expect(editor.frame.dirty).toBe(true);expect(editor.text()).toStartWith("dirty ");
  expect(daemon.ptyRequests.filter(request=>request.tag==="ClosePty")).toHaveLength(0);
  await model.newTerminal();expect(model.mainTerminalSurface()?.sessionId).toBe("fixture-pty-2");
  expect(model.captureSession().surfaces.some(surface=>surface.id===file.id)).toBe(true);
 }finally{close();}
});

test("SSH tunnel identity permits private home cwd follow without treating other loopback joins as local",()=>{
 const remote={id:"shared",host_id:"remote",title:"Shared",root_dir:"/srv/project"};
 const quick={...remote,id:"neoism-quick-ssh-home-0123456789abcdef"};
 const tunnel={endpoint:"ws://127.0.0.1:54321/session",serverId:"ssh:home",remote:true,transportIdentity:"ssh:home"};
 expect(terminalDirectoryFollowsWorkspace(quick,tunnel)).toBe(true);
 expect(terminalDirectoryFollowsWorkspace(remote,tunnel)).toBe(false);
 expect(terminalDirectoryFollowsWorkspace(remote,{endpoint:tunnel.endpoint,serverId:"joined",remote:true})).toBe(false);
 expect(terminalDirectoryFollowsWorkspace(remote,{endpoint:tunnel.endpoint,serverId:"local"})).toBe(true);
 expect(terminalDirectoryFollowsWorkspace({...quick,host_kind:"tailscale"},tunnel)).toBe(false);
});
