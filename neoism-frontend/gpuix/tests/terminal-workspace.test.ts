import { expect,test } from "bun:test";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";
import { createAgentDaemon } from "./support/agent-daemon";
import { absoluteTerminalDirectory,followsTerminalDirectory,terminalDirectoryFollowsWorkspace } from "../src/terminal/workspace";

async function until(predicate:()=>boolean){const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Terminal workspace did not converge");await Bun.sleep(5);}}
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={},createTerminal=true,values:Record<string,unknown>={terminal:{"block-ui":false}}){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/terminal-workspace-test"],{},values),endpoint:daemon.endpoint,authToken:undefined};
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
test("main terminal cd selects the linked notes vault or returns to Default, including Home",async()=>{
 for(const home of [false,true]){
  const {daemon,model,close}=await fixture({scopedFiles:{"/vaults/Default":{"default.md":"Default"},"/vaults/Linked":{"linked.md":"Linked"}}},!home,{terminal:{"block-ui":home}});
  try{
   if(home)await model.ensureHomeTerminal(model.ensureWelcome()!.id);
   const main=model.mainTerminalSurface()!;
   const declared={...model.state.workspaces[0],notes_vault_dir:"/vaults/Linked",linked_vault_dir:"/vaults/Linked"};
   Object.assign(daemon.workspaces[0],declared);
   model.update({workspaces:[declared,...model.state.workspaces.slice(1)]});await model.activateWorkspace(declared);
   await until(()=>model.state.notesDirectories[""]?.entries[0]?.name==="linked.md");
   daemon.notesVaults.resolve=root=>root.startsWith("/other-linked")?{notes_vault_dir:"/vaults/Linked",linked_vault_dir:"/vaults/Linked"}:{notes_vault_dir:"/vaults/Default",linked_vault_dir:null};
   for(const [cwd,vault,note] of [["/unlinked","Default","default.md"],["/other-linked/sub","Linked","linked.md"],["/unlinked-again","Default","default.md"],["/project/a","Linked","linked.md"]]){
    daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd}});
    await until(()=>model.state.activeWorkspace?.root_dir===cwd&&model.state.activeWorkspace.notes_vault_dir===`/vaults/${vault}`&&model.state.notesDirectories[""]?.entries[0]?.name===note);
    expect(model.state.activeWorkspace?.linked_vault_dir).toBe(vault==="Linked"?"/vaults/Linked":null);
   }
   expect(model.state.workspaces[0].root_dir).toBe("/project/a");
   expect(daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);
  }finally{close();}
 }
});
test("delayed notes resolution cannot restore a vault from an obsolete terminal cwd",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  const main=model.mainTerminalSurface()!;
  daemon.notesVaults.resolve=root=>({notes_vault_dir:root==="/slow-linked"?"/vaults/Linked":"/vaults/Default",linked_vault_dir:null});
  daemon.notesVaults.delay=100;
  daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/slow-linked"}});
  await until(()=>daemon.workspaceRequests.some(request=>request.tag==="ResolveNotesVault"&&request.body.root_dir==="/slow-linked"));
  daemon.notesVaults.delay=0;
  daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/unlinked"}});
  await until(()=>model.state.activeWorkspace?.root_dir==="/unlinked"&&model.state.activeWorkspace.notes_vault_dir==="/vaults/Default");
  await Bun.sleep(150);
  expect(model.state.activeWorkspace?.root_dir).toBe("/unlinked");
  expect(model.state.activeWorkspace?.notes_vault_dir).toBe("/vaults/Default");
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
test("first Home controls local working directory while later Home and terminal shells stay independent",async()=>{
  const {daemon,model,close}=await fixture({},false,{terminal:{"block-ui":true}});
  try{
    const home=model.ensureWelcome()!;await model.ensureHomeTerminal(home.id);
    const main=model.mainTerminalSurface()!;expect(main.id).toBe(home.id);
    daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/project/a/home-cwd"}});
    await until(()=>model.state.activeWorkspace?.root_dir==="/project/a/home-cwd"&&!model.state.loading);
    const second=model.newWelcome()!;await model.ensureHomeTerminal(second.id);
    const secondId=model.state.surfaces.find(surface=>surface.id===second.id)!.sessionId;
    daemon.pushPty({SessionCwd:{session_id:secondId,cwd:"/tmp/second-home"}});
    await model.newTerminal();const terminal=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
    expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty").at(-1)?.body.cwd).toBe("/project/a/home-cwd");
    daemon.pushPty({SessionCwd:{session_id:terminal.sessionId,cwd:"/tmp/secondary"}});
    daemon.pushPty({SessionCwd:{session_id:main.sessionId,cwd:"/project/a/next"}});await Bun.sleep(25);
    expect(model.state.activeWorkspace?.root_dir).toBe("/project/a/home-cwd");
    expect(model.mainTerminalSurface()?.id).toBe(home.id);
    model.selectSurface(home.id);await until(()=>model.state.activeWorkspace?.root_dir==="/project/a/next"&&!model.state.loading);
    expect(model.state.workspaces.find(workspace=>workspace.id==="a")?.root_dir).toBe("/project/a");
    expect(daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);
    expect(daemon.ptyRequests.some(request=>request.tag==="PtyInput")).toBe(false);
  }finally{close();}
});
test("turning blocks off restores classic first-terminal authority without killing Home's shell",async()=>{
  const {daemon,model,close}=await fixture({},false,{terminal:{"block-ui":true}});
  try{
    const home=model.ensureWelcome()!;await model.ensureHomeTerminal(home.id);
    const homeSession=model.mainTerminalSurface()!.sessionId;
    model.config.terminalBlockUi=false;
    expect(model.mainTerminalSurface()).toBeUndefined();
    await model.newTerminal();
    const terminal=model.mainTerminalSurface()!;
    expect(terminal.id).toBe(model.state.activeSurfaceId!);
    expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty").at(-1)?.body.presentation).toBe("grid");
    model.selectSurface(home.id);
    daemon.pushPty({SessionCwd:{session_id:homeSession,cwd:"/tmp/hidden-home"}});await Bun.sleep(25);
    expect(model.state.activeWorkspace?.root_dir).toBe("/project/a");
    expect(model.mainTerminalSurface()?.id).toBe(terminal.id);
    expect(daemon.ptyRequests.some(request=>request.tag==="ClosePty"&&request.body.session_id===homeSession)).toBe(false);
  }finally{close();}
});
test("Home does not steal an existing first terminal's directory role",async()=>{
  const {model,close}=await fixture();
  try{
    const first=model.mainTerminalSurface()!;const home=model.ensureWelcome()!;
    await model.ensureHomeTerminal(home.id);
    expect(model.mainTerminalSurface()?.id).toBe(first.id);
  }finally{close();}
});
test("choosing another directory authority transfers cwd follow without sending shell commands or creating tabs",async()=>{
  const {daemon,model,close}=await fixture({},false,{terminal:{"block-ui":true}});
  try{
    const home=model.ensureWelcome()!;await model.ensureHomeTerminal(home.id);
    const homeSession=model.mainTerminalSurface()!.sessionId;
    await model.newTerminal();const terminal=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
    daemon.pushPty({SessionCwd:{session_id:terminal.sessionId,cwd:"/tmp/chosen"}});await Bun.sleep(25);
    expect(model.state.activeWorkspace?.root_dir).toBe("/project/a");
    model.setMainTerminal(terminal.id);
    await until(()=>model.state.activeWorkspace?.root_dir==="/tmp/chosen"&&!model.state.loading);
    expect(model.mainTerminalSurface()?.id).toBe(terminal.id);
    expect(model.captureSession().ui.mainTerminals).toEqual({a:terminal.sessionId});
    model.selectSurface(home.id);
    daemon.pushPty({SessionCwd:{session_id:homeSession,cwd:"/tmp/home"}});await Bun.sleep(25);
    expect(model.state.activeWorkspace?.root_dir).toBe("/tmp/chosen");
    model.setMainTerminal(home.id);
    await until(()=>model.state.activeWorkspace?.root_dir==="/tmp/home"&&!model.state.loading);
    expect(model.mainTerminalSurface()?.id).toBe(home.id);
    model.setMainTerminal(home.id);expect(model.mainTerminalSurface()?.id).toBe(home.id);
    expect(model.state.surfaces.map(surface=>surface.id)).toEqual([home.id,terminal.id]);
    expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(2);
    expect(daemon.ptyRequests.some(request=>request.tag==="PtyInput"||request.tag==="ChangeTerminalDirectory")).toBe(false);
    expect(daemon.workspaceRequests.some(request=>request.tag==="SetWorkspaceRoot")).toBe(false);
  }finally{close();}
});
test("cwd refresh reuses a surviving Home instead of recreating the closed startup Home",async()=>{
  const {daemon,model,close}=await fixture({},false,{terminal:{"block-ui":true}});
  try{
    const startup=model.ensureWelcome()!,home=model.newWelcome()!;
    model.closeSurface(startup.id);await model.ensureHomeTerminal(home.id);
    expect(model.ensureWelcome({select:false})?.id).toBe(home.id);
    const sessionId=model.mainTerminalSurface()!.sessionId;
    const leaves=model.getLayout().frame.leaves.map(leaf=>leaf.leaf);
    for(const cwd of ["/project/a/sub","/project/a"]){
      daemon.pushPty({SessionCwd:{session_id:sessionId,cwd}});
      await until(()=>model.state.activeWorkspace?.root_dir===cwd&&!model.state.loading);
      expect(model.state.surfaces.filter(surface=>surface.kind==="welcome").map(surface=>surface.id)).toEqual([home.id]);
      expect(model.state.activeSurfaceId).toBe(home.id);
      expect(model.getLayout().frame.leaves.map(leaf=>leaf.leaf)).toEqual(leaves);
      expect(model.mainTerminalSurface()?.sessionId).toBe(sessionId);
    }
    expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);
  }finally{close();}
});
test("directory refresh does not reopen Home after the user closes it",async()=>{
  const {daemon,model,close}=await fixture();
  try{
    const terminal=model.mainTerminalSurface()!;
    model.closeSurface("welcome:a");
    daemon.pushPty({SessionCwd:{session_id:terminal.sessionId,cwd:"/project/a/sub"}});
    await until(()=>model.state.activeWorkspace?.root_dir==="/project/a/sub"&&!model.state.loading);
    expect(model.state.surfaces.map(surface=>surface.id)).toEqual([terminal.id]);
    expect(model.getLayout().frame.leaves.map(leaf=>leaf.surfaceId)).toEqual([terminal.id]);
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

test("new terminals forward configured shell and arguments; defaults remain host selected",async()=>{
 for(const shell of [undefined,{program:"/custom path/zsh",args:["--login"]},{program:"pwsh.exe",args:["-NoLogo"]},{program:"/bin/fish",args:[]}]){
   const {daemon,model,close}=await fixture({},false,{terminal:{shell,"block-ui":false}});
  try{
   await model.newTerminal();const request=daemon.ptyRequests.find(row=>row.tag==="CreatePty")!.body;
   expect(request.shell).toBe(shell?.program);expect(request.shell_args).toEqual(shell?.args);
   expect(request).toMatchObject({cwd:"/project/a",presentation:"grid"});
  }finally{close();}
 }
});

for(const source of ["home","code"] as const)test(`terminal split below ${source} uses the existing grid PTY and keeps its source pane`,async()=>{
 const {daemon,model,close}=await fixture({},false);
 try{
  if(source==="code")await model.openFile("README.md");
  const original=model.state.activeSurfaceId!,layout=model.getLayout("a");
  layout.layout(1000,700);
  await model.newTerminal("vertical");
  expect(layout.frame.panes).toHaveLength(2);
  expect(layout.frame.panes[0].surfaceId).toBe(original);
  expect(layout.frame.panes[1].surfaceId).toBe(model.state.activeSurfaceId);
  expect(layout.frame.dividers[0].axis).toBe("vertical");
  const terminal=model.state.activeSurfaceId;
  model.focusPane("up");expect(model.state.activeSurfaceId).toBe(original);
  model.focusPane("down");expect(model.state.activeSurfaceId).toBe(terminal);
  expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);
  expect(daemon.ptyRequests.find(request=>request.tag==="CreatePty")?.body).toMatchObject({presentation:"grid",cwd:"/project/a"});
 }finally{close();}
});
