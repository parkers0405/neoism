import {expect,test} from "bun:test";
import {createServer} from "node:net";
import {existsSync,mkdirSync,mkdtempSync,readFileSync,realpathSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {loadConfig} from "../src/config";
import {ensureLocalDaemon,recoverLocalDaemon,reachable,daemonReady,localDaemonArgs} from "../src/daemon/local-startup";
const configuration=()=>loadConfig([],{},{});
test("parent ownership is explicit, private, and Unix-only; shared/headless work is not reaped",()=>{
 expect(localDaemonArgs("/project",{},123,"linux")).toEqual(["--workspace","/project","--auto-port","--auto-agent-port"]);
 expect(localDaemonArgs("/tmp/preview",{XDG_RUNTIME_DIR:"/tmp/preview"},123,"linux")).toEqual(["--workspace","/tmp/preview","--auto-port","--auto-agent-port"]);
 for(const platform of ["linux","darwin"] as const)
  expect(localDaemonArgs("/project",{NEOISM_GPUIX_PRIVATE_DAEMON:"1"},123,platform)).toEqual(["--workspace","/project","--temporary-parent-pid","123"]);
 expect(localDaemonArgs("/project",{NEOISM_GPUIX_PRIVATE_DAEMON:"1"},123,"win32")).toEqual(["--workspace","/project"]);
 expect(localDaemonArgs("/project",{NEOISM_DAEMON_ADDR:"127.0.0.1:9000",NEOISM_AGENT_SERVER:"http://127.0.0.1:9001"},123,"win32")).toEqual(["--workspace","/project"]);
});
test("an explicit remote daemon never probes or starts local services",async()=>{
 for(const [args,env] of [[["--daemon","ws://remote/session"],{}],[[],{NEOISM_GPUIX_DAEMON_URL:"ws://remote/session"}]] as const){
  const config=configuration();const result=await ensureLocalDaemon(config,args,env,{probe:async()=>{throw new Error("local probe");},launch:async()=>{throw new Error("local launch");}});
  expect(result).toBe(config);
 }
});
test("an available local daemon is reused without a restart",async()=>{
 const config=configuration();let starts=0;
 expect(await ensureLocalDaemon(config,[],{},{probe:async()=>true,ready:async()=>true,refresh:()=>config,launch:async()=>{starts++;}})).toBe(config);
 expect(starts).toBe(0);
});
test("startup refreshes the endpoint and newly published upgrade credential",async()=>{
 const config={...configuration(),endpoint:"ws://127.0.0.1:1234/session"};let started=false,now=0;
 const ready={...config,endpoint:"ws+unix:///tmp/fixture.sock:/session",authToken:"new-local-credential"};
 const result=await ensureLocalDaemon(config,[],{},{probe:async endpoint=>started&&endpoint===ready.endpoint,ready:async()=>true,launch:async()=>{started=true;},refresh:()=>ready,sleep:async()=>{now+=100;},now:()=>now,timeoutMs:1000});
 expect(result).toBe(ready);
});
test("a stale Unix socket can fall back to the existing local TCP daemon",async()=>{
 const config={...configuration(),endpoint:"ws+unix:///tmp/stale.sock:/session"};let starts=0;
 const result=await ensureLocalDaemon(config,[],{},{probe:async endpoint=>endpoint==="ws://127.0.0.1:7878/session",ready:async()=>true,refresh:()=>config,launch:async()=>{starts++;}});
 expect(result.endpoint).toBe("ws://127.0.0.1:7878/session");expect(starts).toBe(0);
});
test("failed startup terminates its wait and reports the diagnostic location",async()=>{
 let now=0,starts=0;
 await expect(ensureLocalDaemon(configuration(),[],{},{probe:async()=>false,launch:async()=>{starts++;},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:200})).rejects.toThrow("daemon.log");
 expect(starts).toBe(1);
});
test("an explicitly configured occupied port is not moved or competed with",async()=>{
 let now=0,starts=0,checks=0;
 const config=configuration();
 await expect(ensureLocalDaemon(config,[],{NEOISM_DAEMON_ADDR:"127.0.0.1:7878"},{refresh:()=>config,probe:async()=>true,ready:async()=>{checks++;return false;},launch:async()=>{starts++;},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:300})).rejects.toThrow("handshake");
 expect(starts).toBe(0);expect(checks).toBeGreaterThan(1);
});
test("a warming daemon is reused once its handshake succeeds",async()=>{
 let now=0,checks=0;
 const config=configuration();
 const result=await ensureLocalDaemon(config,[],{NEOISM_DAEMON_ADDR:"127.0.0.1:7878"},{refresh:()=>config,probe:async()=>true,ready:async()=>++checks===3,launch:async()=>{throw new Error("duplicate daemon");},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:1000});
 expect(result).toBe(config);expect(checks).toBe(3);
});
test("an unrelated default listener permits managed fallback and refreshes the selected endpoint",async()=>{
 let now=0,starts=0;
 const config={...configuration(),endpoint:"ws://127.0.0.1:7878/session"};
 const selected={...config,endpoint:"ws://127.0.0.1:54321/session"};
 const result=await ensureLocalDaemon(config,[],{},{probe:async()=>true,ready:async current=>current.endpoint===selected.endpoint,refresh:()=>starts?selected:config,launch:async()=>{starts++;},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:1000});
 expect(result.endpoint).toBe(selected.endpoint);expect(starts).toBe(1);
});
test("a published warming endpoint wins over a stale Unix socket without a competing launch",async()=>{
 if(process.platform==="win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-published-startup-"));mkdirSync(join(folder,"neoism"));
 const published="ws://127.0.0.1:54321/session",config={...configuration(),endpoint:`ws+unix://${folder}/stale.sock:/session`};
 writeFileSync(join(folder,"neoism","daemon-endpoint.json"),JSON.stringify({protocol:1,pid:process.pid,nonce:"owner",address:"127.0.0.1:54321"}));
 let now=0,checks=0;
 try{
  const result=await ensureLocalDaemon(config,[],{XDG_RUNTIME_DIR:folder},{refresh:()=>config,probe:async endpoint=>endpoint===published,ready:async()=>++checks===3,launch:async()=>{throw new Error("competing launch");},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:1000});
  expect(result.endpoint).toBe(published);expect(checks).toBe(3);
 }finally{rmSync(folder,{recursive:true,force:true});}
});
test("a daemon that disappears during warmup is started again",async()=>{
 let now=0,starts=0;
 const config={...configuration(),endpoint:"ws://127.0.0.1:7878/session"};
 await ensureLocalDaemon(config,[],{},{refresh:()=>config,probe:async()=>now===0||starts>0,ready:async()=>starts>0,launch:async()=>{starts++;},sleep:async()=>{now+=100;},now:()=>now,timeoutMs:1000});
 expect(starts).toBe(1);
});
test("readiness requires an authenticated HelloAck, not just a listening socket",async()=>{
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request,server){
  if(new URL(request.url).searchParams.get("token")!=="fresh")return new Response("Unauthorized",{status:401});
  return server.upgrade(request)?undefined:new Response("upgrade required",{status:400});
 },websocket:{message(socket,raw){
  const hello=JSON.parse(raw.toString()).Workspace?.message?.Hello;
  socket.send(JSON.stringify({WorkspaceReply:{request_id:0,message:{HelloAck:{accepted:hello?.token==="paired"}}}}));
 }}});
 const config={...configuration(),endpoint:`ws://127.0.0.1:${server.port}/session`,pairingToken:"paired"};
 try{
  expect(await daemonReady({...config,authToken:"stale"},1000)).toBe(false);
  expect(await daemonReady({...config,authToken:"fresh"},1000)).toBe(true);
 }finally{await server.stop(true);}
});
test("cancelled startup releases its wait and a subsequent attempt can recover",async()=>{
 const controller=new AbortController();let now=0;
 const config=configuration();
 await expect(ensureLocalDaemon(config,[],{NEOISM_DAEMON_ADDR:"127.0.0.1:7878"},{signal:controller.signal,refresh:()=>config,probe:async()=>true,ready:async()=>false,sleep:async()=>{now+=100;controller.abort(new Error("window closed"));},now:()=>now,timeoutMs:1000})).rejects.toThrow("window closed");
 expect(await ensureLocalDaemon(config,[],{},{refresh:()=>config,probe:async()=>true,ready:async()=>true})).toBe(config);
});
test("sessions share recovery without one closed session cancelling the other",async()=>{
 let upgrades=0;
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request,server){upgrades++;return server.upgrade(request)?undefined:new Response("",{status:400});},websocket:{message(socket){
  setTimeout(()=>socket.send(JSON.stringify({WorkspaceReply:{request_id:0,message:{HelloAck:{accepted:true}}}})),50);
 }}});
 const config={...configuration(),endpoint:`ws://127.0.0.1:${server.port}/session`},first=new AbortController(),second=new AbortController();
 try{
  const cancelled=recoverLocalDaemon(config,first.signal).catch(error=>error);
  const retained=recoverLocalDaemon(config,second.signal);
  first.abort(new Error("first window closed"));
  expect((await cancelled).message).toBe("first window closed");
  expect((await retained).endpoint).toBe(config.endpoint);expect(upgrades).toBe(1);
  await recoverLocalDaemon(config,second.signal);expect(upgrades).toBe(2);
 }finally{first.abort();second.abort();await server.stop(true);}
});
test("the readiness probe checks a real Unix listener and does not trust socket-file existence",async()=>{
 if(process.platform==="win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-startup-")),path=join(folder,"daemon.sock");
 const server=createServer(socket=>socket.end());
 try{
  await new Promise<void>((resolve,reject)=>{server.once("error",reject);server.listen(path,resolve);});
  expect(await reachable(`ws+unix://${path}:/session`)).toBe(true);
  await new Promise<void>(resolve=>server.close(()=>resolve()));
  expect(await reachable(`ws+unix://${path}:/session`)).toBe(false);
 }finally{server.close();rmSync(folder,{recursive:true,force:true});}
});
test("recovery adopts the new private Unix socket instead of retrying the initial port zero",async()=>{
 if(process.platform==="win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-private-recovery-")),path=join(folder,"daemon.sock");
 const keys=["NEOISM_DAEMON_SOCKET","NEOISM_DAEMON_ADDR","NEOISM_GPUIX_DAEMON_URL"] as const;
 const previous=keys.map(key=>process.env[key]);
 const controller=new AbortController();
 let server:ReturnType<typeof Bun.serve>|undefined;
 try{
  process.env.NEOISM_DAEMON_SOCKET=path;process.env.NEOISM_DAEMON_ADDR="127.0.0.1:0";delete process.env.NEOISM_GPUIX_DAEMON_URL;
  const config=loadConfig([],process.env,{});
  expect(config.endpoint).toBe("ws://127.0.0.1:0/session");
  server=Bun.serve({unix:path,fetch(request,server){return server.upgrade(request,{data:undefined})?undefined:new Response("",{status:400});},websocket:{message(socket){
   socket.send(JSON.stringify({WorkspaceReply:{request_id:0,message:{HelloAck:{accepted:true}}}}));
  }}});
  const result=await recoverLocalDaemon(config,controller.signal);
  expect(result.endpoint).toBe(`ws+unix://${path}:/session`);
 }finally{
  controller.abort();await server?.stop(true);
  keys.forEach((key,index)=>{if(previous[index]===undefined)delete process.env[key];else process.env[key]=previous[index];});
  rmSync(folder,{recursive:true,force:true});
 }
});
test("application readiness also works over the default Unix transport",async()=>{
 if(process.platform==="win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-startup-handshake-")),path=join(folder,"daemon.sock");
 const server=Bun.serve({unix:path,fetch(request,server){return server.upgrade(request)?undefined:new Response("",{status:400});},websocket:{message(socket){
  socket.send(JSON.stringify({WorkspaceReply:{request_id:0,message:{HelloAck:{accepted:true}}}}));
 }}});
 try{expect(await daemonReady({...configuration(),endpoint:`ws+unix://${path}:/session`},1000)).toBe(true);}
 finally{await server.stop(true);rmSync(folder,{recursive:true,force:true});}
});

test("a detached daemon that exits immediately reports startup failure instead of waiting for readiness", async () => {
 if(process.platform === "win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-startup-exit-"));
 const env={...process.env,NEOISM_GPUIX_DAEMON_URL:undefined,NEOISM_DAEMON_SOCKET:join(folder,"missing.sock"),NEOISM_DAEMON_ADDR:"127.0.0.1:0",NEOISM_GPUIX_WORKSPACE_DAEMON:Bun.which("false")!,XDG_STATE_HOME:folder};
 try {
  const config=loadConfig([],env,{});
  await expect(ensureLocalDaemon(config,[],env,{refresh:()=>config,timeoutMs:3000})).rejects.toThrow("exited during startup");
 } finally {rmSync(folder,{recursive:true,force:true});}
});

for(const privateDaemon of [false,true])test(`the ${privateDaemon?"private":"shared"} daemon receives its workspace, cwd and ownership policy`,async()=>{
 if(process.platform==="win32")return;
 const folder=mkdtempSync(join(tmpdir(),"neoism-startup-directory-")),project=join(folder,"project with spaces"),capture=join(folder,"launch.txt"),binary=join(folder,"daemon");
 mkdirSync(project);
  writeFileSync(binary,'#!/bin/sh\nprintf "%s\\n" "$PWD" "$NEOISM_WORKSPACE_ROOT" "$@" > "$NEOISM_STARTUP_CAPTURE"\n',{mode:0o700});
  const env={...process.env,NEOISM_DAEMON_ADDR:undefined,NEOISM_DAEMON_SOCKET:undefined,NEOISM_AGENT_SERVER:undefined,NEOISM_SERVER:undefined,NEOISM_GPUIX_PRIVATE_DAEMON:privateDaemon?"1":undefined,NEOISM_GPUIX_DAEMON_URL:undefined,NEOISM_GPUIX_WORKSPACE_DAEMON:binary,NEOISM_LAUNCH_DIRECTORY:project,NEOISM_STARTUP_CAPTURE:capture,XDG_STATE_HOME:folder};
 const config=configuration();
 try{
   await ensureLocalDaemon(config,[],env,{probe:async()=>existsSync(capture),ready:async()=>true,refresh:()=>config,timeoutMs:3000});
   const [cwd,root,...args]=readFileSync(capture,"utf8").trimEnd().split("\n");
   expect(realpathSync(cwd!)).toBe(realpathSync(project));
   expect(root).toBe(project);
   expect(args).toEqual(privateDaemon?["--workspace",project,"--temporary-parent-pid",String(process.pid)]:["--workspace",project,"--auto-port","--auto-agent-port"]);
 }finally{rmSync(folder,{recursive:true,force:true});}
});
