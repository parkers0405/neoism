import {expect,test} from "bun:test";
import {SelectedSshReconnect,preserveReconnectFocus} from "../src/daemon/ssh-reconnect";
import type {ServerSelection} from "../src/menu-actions/ChromeActionsController";
import {createAgentDaemon} from "./support/agent-daemon";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {loadConfig} from "../src/config";
import {terminalSession} from "../src/terminal/sessions";
const server:ServerSelection={id:"system:fixture",endpoint:"",ssh:{target:"tester@fixture.invalid",args:["-p","2222"]}};
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+3000;while(!predicate()){if(Date.now()>deadline)throw new Error("SSH reconnect fixture did not settle");await Bun.sleep(3);}};
test("selected SSH health check serializes attempts, retains exact route, and manual selection cancels the old attempt",async()=>{
 const calls:ServerSelection[]=[],errors:unknown[]=[];let healthy=true;
 const reconnect=new SelectedSshReconnect(async selection=>{calls.push(selection);await new Promise<void>((resolve,reject)=>selection.signal!.addEventListener("abort",()=>reject(new Error("cancelled")),{once:true}));},error=>errors.push(error),{automatic:false});
 try{
  reconnect.select(server,{alive:()=>healthy});await reconnect.check();expect(calls).toHaveLength(0);
  healthy=false;const pending=reconnect.check();await reconnect.check();await reconnect.check();expect(calls).toHaveLength(1);expect(calls[0].id).toBe(server.id);expect(calls[0].ssh).toEqual(server.ssh);expect(calls[0].signal?.aborted).toBe(false);
  const resume=reconnect.pause();expect(calls[0].signal?.aborted).toBe(true);await pending;await reconnect.check();expect(calls).toHaveLength(1);
  reconnect.select({id:"local",endpoint:"ws://127.0.0.1:7878/session"});resume();await reconnect.check();expect(calls).toHaveLength(1);expect(errors).toHaveLength(0);
 }finally{reconnect.dispose();}
});
test("SSH retries use bounded backoff without falling back locally or repeatedly reporting the same outage",async()=>{
 let now=0;const calls:ServerSelection[]=[],errors:unknown[]=[];
 const reconnect=new SelectedSshReconnect(async selection=>{calls.push(selection);throw new Error("fixture offline");},error=>errors.push(error),{automatic:false,now:()=>now});
 try{
  reconnect.select(server,{alive:()=>false});await reconnect.check();await reconnect.check();expect(calls).toHaveLength(1);
  now=499;await reconnect.check();expect(calls).toHaveLength(1);now=500;await reconnect.check();expect(calls).toHaveLength(2);
  now=1499;await reconnect.check();expect(calls).toHaveLength(2);now=1500;await reconnect.check();expect(calls).toHaveLength(3);
  expect(calls.every(selection=>selection.id===server.id&&selection.ssh?.target===server.ssh?.target)).toBe(true);expect(errors).toHaveLength(1);
  reconnect.dispose();now=30000;await reconnect.check();expect(calls).toHaveLength(3);
 }finally{reconnect.dispose();}
});
test("a renewed SSH model restores the exact workspace, terminal handle, dirty file and draft without allocating a replacement shell",async()=>{
 const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/ssh-reconnect"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,workspaceId:"b",transportIdentity:"ssh:fixture",serverId:"system:fixture"};
 const firstClient=new DaemonConnection(config),first=new WorkspaceModel(firstClient,config);let secondClient:DaemonConnection|undefined,second:WorkspaceModel|undefined,cleanup:undefined|(()=>void);
 try{
  first.start();firstClient.start();await until(()=>first.state.activeWorkspace?.id==="b"&&!first.state.loading);
  await first.openFile("README.md");const file=first.state.surfaces.find(surface=>surface.id===first.state.activeSurfaceId)!;first.getEditor(file).call("paste",{text:"unsaved SSH edit "});
  first.setComposerDraft("home:b",{text:"unsent SSH draft",attachments:[]});await first.newTerminal();const terminal=first.state.surfaces.find(surface=>surface.id===first.state.activeSurfaceId)!;expect(terminal.sessionId).toBe("fixture-pty");
  const saved=first.captureSession();secondClient=new DaemonConnection(config);second=new WorkspaceModel(secondClient,config);second.restoreSession(saved);cleanup=preserveReconnectFocus(second,{workspaceId:"b",surfaceId:terminal.id});second.start();secondClient.start();
  await until(()=>!second!.state.loading&&second!.state.activeWorkspace?.id==="b"&&second!.state.activeSurfaceId===terminal.id);
  expect(second.getEditor(file).text()).toStartWith("unsaved SSH edit ");expect(second.getEditor(file).frame.dirty).toBe(true);expect(second.state.composerDrafts["home:b"].text).toBe("unsent SSH draft");
  const session=terminalSession(secondClient,terminal.sessionId!,config.theme,second.report);await until(()=>session.buffer.nextOffset===2);
  expect(daemon.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(1);expect(daemon.ptyRequests.some(request=>request.tag==="AttachPty"&&request.body.session_id===terminal.sessionId)).toBe(true);expect(daemon.ptyRequests.some(request=>request.tag==="ClosePty")).toBe(false);
 }finally{cleanup?.();second?.dispose();secondClient?.stop();first.dispose();firstClient.stop();daemon.stop();}
});
