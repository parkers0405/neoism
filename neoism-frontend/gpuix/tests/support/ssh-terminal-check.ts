import {fileURLToPath} from "node:url";
import assert from "node:assert/strict";
import {writeFileSync,chmodSync,readFileSync,existsSync} from "node:fs";
import {join,delimiter} from "node:path";
import {createAgentDaemon} from "./agent-daemon";
import {attachSsh,prepareSshWorkspace,type SshAttachment} from "../../src/daemon/ssh-transport";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {terminalSession,disposeTerminalSessions} from "../../src/terminal/sessions";
import {coreCall} from "../../src/native-core";
import {SelectedSshReconnect,preserveReconnectFocus} from "../../src/daemon/ssh-reconnect";
const directory=process.env.NEOISM_SSH_FIXTURE_DIR;
if(!directory||process.env.NEOISM_CONFIG_HOME!==directory||!process.env.PATH?.split(delimiter).includes(directory))throw new Error("SSH fixture isolation must be configured before process startup");
const local=createAgentDaemon(),remote=createAgentDaemon();
const until=async(predicate:()=>boolean)=>{const end=Date.now()+5000;while(!predicate()){if(Date.now()>end)throw new Error("Remote terminal fixture did not settle");await Bun.sleep(5);}};
const script=`#!/usr/bin/env node
const net=require('node:net'),fs=require('node:fs');const args=process.argv.slice(2),index=args.indexOf('-L');
fs.appendFileSync(${JSON.stringify(join(directory,"calls.jsonl"))},JSON.stringify(args)+'\\n');
if(index<0){const finish=()=>{process.stdout.write('fixture-remote-daemon-credential\\n');process.exit(0);};if(fs.existsSync(${JSON.stringify(join(directory,"slow-credential"))})){fs.writeFileSync(${JSON.stringify(join(directory,"credential-started"))},'ready');setTimeout(finish,500);}else finish();}else{
fs.appendFileSync(${JSON.stringify(join(directory,"control-pids"))},String(process.pid)+'\\n');
const port=Number(args[index+1].split(':')[0]);
net.createServer(socket=>{const upstream=net.connect(${new URL(remote.endpoint).port},'127.0.0.1');socket.on('error',()=>upstream.destroy());upstream.on('error',()=>socket.destroy());socket.pipe(upstream);upstream.pipe(socket);}).listen(port,'127.0.0.1');}
`;
if(process.platform==="win32"||process.env.NEOISM_TEST_NATIVE_SSH_FIXTURE==="1"){
 writeFileSync(join(directory,"remote-port"),new URL(remote.endpoint).port);
 const source=fileURLToPath(new URL("./ssh-fixture.rs",import.meta.url));
 const build=Bun.spawn(["rustc","--edition=2021",source,"-o",join(directory,process.platform==="win32"?"ssh.exe":"ssh")],{stdout:"pipe",stderr:"pipe"});
 const [status,error]=await Promise.all([build.exited,new Response(build.stderr).text()]);assert.equal(status,0,error);
}else{writeFileSync(join(directory,"ssh"),script);chmodSync(join(directory,"ssh"),0o700);}
let attachment:SshAttachment|undefined,client:DaemonConnection|undefined,model:WorkspaceModel|undefined;
try{
 const saved=coreCall<{system:{id:string;enabled:boolean}}>("ssh.systems.add",{name:"Fixture remote",target:"tester@fixture.invalid",args:["-p","2222"]}).system;assert.equal(saved.enabled,false);
 coreCall("ssh.systems.set-enabled",{systemId:saved.id,enabled:true});assert.equal(coreCall<{system:{enabled:boolean}}>("ssh.systems.get",{systemId:saved.id}).system.enabled,true);
 assert.throws(()=>coreCall("ssh.systems.add",{name:"Unsafe",target:"host",args:["-o","ProxyCommand=anything"]}));
 attachment=await attachSsh({systemId:saved.id});assert.ok(attachment.alive());
 const config={...loadConfig(["--config","/nonexistent/ssh-terminal"],{}),endpoint:attachment.endpoint,authToken:attachment.authToken,pairingToken:undefined,workspaceId:attachment.workspaceId,remote:true,serverId:attachment.identity,transportIdentity:attachment.identity};
 client=new DaemonConnection(config,undefined,()=>{});
 const workspace=await prepareSshWorkspace(client,attachment);assert.equal(workspace.root_dir,"/home/fixture-remote");
 model=new WorkspaceModel(client,config);model.start();await until(()=>model!.state.activeWorkspace?.id===attachment!.workspaceId&&!model!.state.loading);
 assert.equal(remote.workspaceRequests.find(entry=>entry.tag==="CreateWorkspace")!.body.root_dir,"~");assert.equal(local.workspaceRequests.length,0);
 await model.newTerminal();const surface=model.state.surfaces.find(entry=>entry.id===model!.state.activeSurfaceId)!;assert.equal(surface.kind,"terminal");
 const session=terminalSession(client,surface.sessionId!,config.theme,model.report);assert.equal(session.buffer.backend,"ghostty");
 await until(()=>remote.ptyRequests.some(entry=>entry.tag==="AttachPty")&&session.buffer.nextOffset===2);
 remote.pushPty({SessionCwd:{session_id:surface.sessionId,cwd:"/srv/private-ssh-cwd"}});await until(()=>model!.state.activeWorkspace?.root_dir==="/srv/private-ssh-cwd");
 assert.equal(remote.workspaceRequests.some(entry=>entry.tag==="SetWorkspaceRoot"),false,"Quick SSH cwd-follow remains private to this frontend");
 session.send(new TextEncoder().encode("pwd\r"));client.send("Pty",{Resize:{session_id:surface.sessionId!,cols:90,rows:24}});
 await until(()=>remote.ptyRequests.some(entry=>entry.tag==="PtyInput"));
 remote.pushPty({PtyOutput:{session_id:surface.sessionId,offset:2,bytes:[...new TextEncoder().encode("\r\nREMOTE-SYSTEM /home/fixture-remote\r\n$ ")]}});
 await until(()=>session.buffer.frame().lines.flat().some(run=>run.text.includes("REMOTE-SYSTEM")));
 assert.equal(await session.changeDirectory("/srv/remote-project"),"/srv/remote-project");
 const tags=remote.ptyRequests.map(entry=>entry.tag);for(const tag of ["CreatePty","AttachPty","PtyInput","Resize","ChangeTerminalDirectory"])assert.ok(tags.includes(tag),tag);
 assert.equal(remote.ptyRequests.find(entry=>entry.tag==="CreatePty")!.body.cwd,"/home/fixture-remote");assert.equal(remote.ptyRequests.find(entry=>entry.tag==="CreatePty")!.body.presentation,"grid");
 const created=remote.ptyRequests.filter(entry=>entry.tag==="CreatePty").length;
 client.reconnect();await until(()=>client!.state.phase==="connected"&&remote.ptyRequests.filter(entry=>entry.tag==="AttachPty").length>=3);
 assert.equal(remote.ptyRequests.filter(entry=>entry.tag==="CreatePty").length,created);assert.equal(local.ptyRequests.length,0);
 // A dead SSH control process needs a new tunnel, not another WebSocket dial
 // to the old port. The selected transport resumes the exact daemon PTY.
 const previousEndpoint=attachment.endpoint,previousIdentity=attachment.identity;
 const reconnect=new SelectedSshReconnect(async selection=>{
  const renewed=await attachSsh(selection.ssh!,{signal:selection.signal});
  const candidateConfig={...config,endpoint:renewed.endpoint,authToken:renewed.authToken,workspaceId:model!.state.activeWorkspace!.id};
  const candidateClient=new DaemonConnection(candidateConfig,undefined,()=>{});let candidateModel:WorkspaceModel|undefined;
  try{
   await prepareSshWorkspace(candidateClient,renewed,{signal:selection.signal});
   const snapshot=model!.captureSession();candidateModel=new WorkspaceModel(candidateClient,candidateConfig);candidateModel.restoreSession(snapshot);
   const release=preserveReconnectFocus(candidateModel,{workspaceId:snapshot.activeWorkspaceId,surfaceId:snapshot.activeSurfaceId});candidateModel.start();
   await until(()=>!candidateModel!.state.loading&&candidateModel!.state.activeSurfaceId===surface.id);release();
   model!.dispose();client!.stop();attachment!.dispose();model=candidateModel;client=candidateClient;attachment=renewed;reconnect.select(selection,renewed);
  }catch(error){candidateModel?.dispose();candidateClient.stop();renewed.dispose();throw error;}
 },error=>{throw error;},{automatic:false});
 try{
  reconnect.select({id:"fixture-system",endpoint:attachment.endpoint,ssh:{systemId:saved.id}},attachment);
  const control=Number(readFileSync(join(directory,"control-pids"),"utf8").trim().split("\n")[0]);process.kill(control,"SIGTERM");await until(()=>!attachment!.alive());await reconnect.check();
  assert.notEqual(attachment.endpoint,previousEndpoint);assert.equal(attachment.identity,previousIdentity);assert.equal(model.state.activeSurfaceId,surface.id);
  const resumed=terminalSession(client,surface.sessionId!,config.theme,model.report);await until(()=>resumed.buffer.nextOffset===2);
  assert.equal(remote.ptyRequests.filter(entry=>entry.tag==="CreatePty").length,created);assert.equal(local.ptyRequests.length,0);
 }finally{reconnect.dispose();}
 model.dispose();model=undefined;client.stop();client=undefined;
 attachment.dispose();attachment.dispose();assert.equal(attachment.alive(),false);
 assert.equal(remote.ptyRequests.filter(entry=>entry.tag==="ClosePty").length,0,"Leaving the frontend does not terminate daemon-owned remote shells");
 const calls=readFileSync(join(directory,"calls.jsonl"),"utf8").trim().split("\n").map(line=>JSON.parse(line)as string[]);
 assert.ok(calls.some(args=>args.includes('-L')&&args.some(arg=>arg.includes('127.0.0.1:17878'))));assert.ok(calls.some(args=>args.some(arg=>arg.includes('runtime_token='))));
 writeFileSync(join(directory,"slow-credential"),"fixture");
 const pending=coreCall<{id:number}>("ssh.start",{target:"tester@fixture.invalid",args:[]});await until(()=>existsSync(join(directory,"credential-started")));coreCall("ssh.close",{id:pending.id});
 assert.deepEqual(coreCall("ssh.shutdown"),{closed:true});
 for(const pid of readFileSync(join(directory,"control-pids"),"utf8").trim().split("\n").map(Number))assert.throws(()=>process.kill(pid,0),{code:"ESRCH"},"Closing the app must reap even a cancelled bootstrap's SSH control process");
 coreCall("ssh.systems.remove",{systemId:saved.id});assert.equal(coreCall<{systems:unknown[]}>("ssh.systems.list").systems.length,0);
 console.log(JSON.stringify({noWindow:true,originalSshTransport:true,remoteHomeWorkspace:true,ghosttyTerminal:true,remoteCreateAttachInputResizeDirectory:true,noLocalPtyOperations:true,reconnectRetainsRemoteRoute:true,tunnelDisposed:true,savedSystemRegistry:true,shutdownReapsCancelledTunnel:true,selectedTunnelReconnectResumesPty:true}));
}finally{model?.dispose();if(client){disposeTerminalSessions(client);client.stop();}attachment?.dispose();coreCall("ssh.shutdown");remote.stop();local.stop();}
