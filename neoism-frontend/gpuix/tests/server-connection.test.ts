import {expect,test} from "bun:test";
import {mkdtempSync,rmSync,readFileSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {prepareServerConnection,waitForConnection,ServerHandshakeError,LocalServerConfig} from "../src/daemon/server-connection";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {SessionStore,type SavedSession} from "../src/session-store";
function daemon(accepted:boolean|null=true){
 const upgrades:Array<string|null>=[],messages:Array<{family:string;message:any}>=[];
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request,server){upgrades.push(new URL(request.url).searchParams.get("token"));return server.upgrade(request)?undefined:new Response("",{status:400});},websocket:{message(socket,data){const [family,envelope]=Object.entries(JSON.parse(String(data)))[0]as[string,{request_id:number;message:any}];messages.push({family,message:envelope.message});if(family==="Workspace"&&envelope.message.Hello&&accepted!==null)socket.send(JSON.stringify({WorkspaceReply:{request_id:envelope.request_id,message:{HelloAck:{accepted,reason:accepted?null:"Fixture pairing denied"}}}}));}}});
 return{endpoint:`ws://127.0.0.1:${server.port}/session`,upgrades,messages,stop:()=>server.stop(true)};
}
const config=()=>loadConfig(["--config","/nonexistent/server-connection"],{});
test("selected server preflight uses that host's separate upgrade/Hello credentials without changing current config",async()=>{
 const local=daemon(),remote=daemon();const current={...config(),endpoint:local.endpoint,authToken:"old-local-upgrade",pairingToken:"old-local-pairing",workspaceId:"local-workspace",serverId:"local"};
 const original=JSON.stringify(current);Object.freeze(current);
 try{
  const prepared=await prepareServerConnection({id:"remote-server",endpoint:remote.endpoint,authToken:"remote-upgrade",pairingToken:"remote-pairing",workspaceId:"remote-workspace"},current,()=>{throw new Error("Remote selection must not resolve local config");});
  expect(prepared.config).toMatchObject({endpoint:remote.endpoint,authToken:"remote-upgrade",pairingToken:"remote-pairing",workspaceId:"remote-workspace",serverId:"remote-server",collaborative:true});
  expect(remote.upgrades).toEqual(["remote-upgrade"]);expect(remote.messages[0].message.Hello.token).toBe("remote-pairing");expect(local.messages).toEqual([]);expect(JSON.stringify(current)).toBe(original);
 }finally{remote.stop();local.stop();}
});
test("failed selected-server authentication preserves the active local connection and never creates/closes a terminal",async()=>{
 const local=daemon(),denied=daemon(false);const current={...config(),endpoint:local.endpoint,serverId:"local"},client=new DaemonConnection(current,undefined,()=>{});client.start();await waitForConnection(client);
 try{
  await expect(prepareServerConnection({id:"rejected",endpoint:denied.endpoint,pairingToken:"invalid"},current,()=>current)).rejects.toThrow("Fixture pairing denied");
  expect(client.state.phase).toBe("connected");client.send("Workspace",{Ping:{nonce:"still-here"}});
  const deadline=Date.now()+1000;while(!local.messages.some(entry=>entry.message.Ping)){if(Date.now()>deadline)throw new Error("Current connection was disturbed");await Bun.sleep(2);}
  expect([...local.messages,...denied.messages].some(entry=>entry.family==="Pty")).toBe(false);expect(current.endpoint).toBe(local.endpoint);
 }finally{client.stop();denied.stop();local.stop();}
});
test("returning Local Server restores retained local transport without inheriting remote credentials or identity",async()=>{
 const local=daemon();try{
  const restored={...config(),endpoint:local.endpoint,authToken:"local-upgrade",pairingToken:"local-pairing"};
  const prepared=await prepareServerConnection({id:"local",endpoint:"ws://ignored.invalid/session"},{...config(),endpoint:"ws://remote.invalid/session",authToken:"remote-upgrade",pairingToken:"remote-pairing",transportIdentity:"ssh:remote",collaborative:true},()=>restored);
  expect(prepared.config).toMatchObject({endpoint:local.endpoint,authToken:"local-upgrade",pairingToken:"local-pairing",serverId:"local",collaborative:false,transportIdentity:undefined});expect(local.messages[0].message.Hello.token).toBe("local-pairing");
 }finally{local.stop();}
});
test("SSH recovery survives a new tunnel port/token while remaining isolated from Local Server and other SSH hosts",()=>{
 const root=mkdtempSync(join(tmpdir(),"neoism-ssh-recovery-"));try{
  const env={XDG_STATE_HOME:root},first=new SessionStore({endpoint:"ws://127.0.0.1:54321/session",authToken:"old-remote-secret",transportIdentity:"ssh:host-a"},env),second=new SessionStore({endpoint:"ws://127.0.0.1:54322/session",authToken:"new-remote-secret",transportIdentity:"ssh:host-a"},env);
  const saved:SavedSession={version:1,surfaces:[{id:"terminal:retained",kind:"terminal",title:"Terminal",workspaceId:"remote-workspace",sessionId:"remote-pty"}],editors:{},providerStates:{},drafts:{},composerDrafts:{draft:{text:"unsent remote draft"}},ui:{openWorkspaceIds:["remote-workspace"]},activeWorkspaceId:"remote-workspace",activeSurfaceId:"terminal:retained"};
  first.write(saved);expect(second.path).toBe(first.path);expect(second.read()).toEqual(saved);
  expect(new SessionStore({endpoint:"ws://127.0.0.1:54322/session",authToken:"new-remote-secret",transportIdentity:"ssh:host-b"},env).read()).toBeUndefined();
  expect(new SessionStore({endpoint:"ws://127.0.0.1:54321/session",authToken:"old-remote-secret"},env).read()).toBeUndefined();
  const bytes=readFileSync(first.path,"utf8");expect(bytes).not.toContain("old-remote-secret");expect(bytes).not.toContain("new-remote-secret");
 }finally{rmSync(root,{recursive:true,force:true});}
});

test("server preflight cancellation stops only its probe and reports a cancellation phase",async()=>{
 const local=daemon(),remote=daemon(null),current={...config(),endpoint:local.endpoint,serverId:"local"},client=new DaemonConnection(current,undefined,()=>{});client.start();await waitForConnection(client);
 try{
  const abort=new AbortController();const pending=prepareServerConnection({id:"remote",endpoint:remote.endpoint},current,()=>current,{signal:abort.signal});
  const deadline=Date.now()+1000;while(!remote.messages.length){if(Date.now()>deadline)throw new Error("Probe never reached selected server");await Bun.sleep(2);}
  abort.abort(new Error("Modal closed"));await expect(pending).rejects.toMatchObject({name:"ServerHandshakeError",phase:"cancelled",message:"Modal closed"});expect(client.state.phase).toBe("connected");expect(local.messages.some(entry=>entry.family==="Pty")).toBe(false);
 }finally{client.stop();remote.stop();local.stop();}
});
test("handshake failures distinguish timeout from rejected credentials and dispose synchronous status listeners",async()=>{
 const remote=daemon(null),client=new DaemonConnection({...config(),endpoint:remote.endpoint},undefined,()=>{});
 try{await expect(waitForConnection(client,15)).rejects.toMatchObject({name:"ServerHandshakeError",phase:"timeout"});}finally{client.stop();remote.stop();}
 let starts=0,listeners=0;
 const rejected={state:{phase:"auth-rejected"},subscribeStatus(fn:(state:{phase:string;reason:string})=>void){listeners++;fn({phase:"auth-rejected",reason:"Wrong password"});return()=>{listeners--;};},start(){starts++;}} as unknown as DaemonConnection;
 try{await waitForConnection(rejected);throw new Error("Expected rejection");}catch(error){expect(error).toBeInstanceOf(ServerHandshakeError);expect((error as ServerHandshakeError).phase).toBe("auth-rejected");}
 expect(starts).toBe(0);expect(listeners).toBe(0);
 const abort=new AbortController();abort.abort();await expect(waitForConnection(rejected,100,abort.signal)).rejects.toMatchObject({phase:"cancelled"});expect(listeners).toBe(0);
});

test("Local Server retains its authenticated TCP descriptor across remote/SSH visits without inheriting their tokens",()=>{
 const original={...config(),endpoint:"ws://127.0.0.1:7878/session",authToken:"original-local-upgrade",pairingToken:"original-local-pairing",workspaceId:"local-root"};
 const local=new LocalServerConfig(original);local.observe({...original,endpoint:"ws://127.0.0.1:54321/session",serverId:"saved-system",transportIdentity:"ssh:fixture",collaborative:true,authToken:"remote-secret",pairingToken:undefined});
 const restored=local.resolve(()=>{throw new Error("Authenticated local transport must not be rebuilt from stripped defaults");});expect(restored).toMatchObject({endpoint:original.endpoint,authToken:original.authToken,pairingToken:original.pairingToken,workspaceId:"local-root"});expect(restored).not.toBe(original);
 const initialRemote=new LocalServerConfig({...original,endpoint:"wss://remote.invalid/session",collaborative:true});let fallbacks=0;expect(initialRemote.resolve(()=>{fallbacks++;return original;})).toBe(original);expect(fallbacks).toBe(1);
});
