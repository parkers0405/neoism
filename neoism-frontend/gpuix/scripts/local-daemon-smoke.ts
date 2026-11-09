// Real daemon readiness, credential discovery, and initial workspace; private state only.
import {mkdtempSync,mkdirSync,readFileSync,rmSync} from "node:fs";
import {join,resolve} from "node:path";
import {tmpdir} from "node:os";
import {Effect} from "effect";
import {loadConfig} from "../src/config";
import {ensureLocalDaemon,localDaemonArgs,reachable} from "../src/daemon/local-startup";
import {DaemonConnection} from "../src/daemon/client";
const root=mkdtempSync(join(tmpdir(),"neoism-local-daemon-check-")),workspace=join(root,"workspace");
for(const path of [workspace,join(root,"runtime"),join(root,"temporary")])mkdirSync(path,{recursive:true});
const noRuntime=process.argv.includes("--without-xdg-runtime");
const unavailableAgent=process.argv.includes("--unavailable-agent");
process.env.NEOISM_GPUIX_PRIVATE_DAEMON="1";
const agent=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>unavailableAgent?new Response("Unavailable",{status:503}):Response.json({workspaceId:"startup-test",healthy:true,version:"test",providerCredentialStore:"test"})});
Object.assign(process.env,{HOME:join(root,"home"),TMPDIR:join(root,"temporary"),XDG_STATE_HOME:join(root,"state-home"),XDG_DATA_HOME:join(root,"data-home"),XDG_CONFIG_HOME:join(root,"config-home"),NEOISM_STATE_DIR:join(root,"state"),NEOISM_CONFIG_HOME:join(root,"config"),NEOISM_CONFIG_DIR:join(root,"config"),NEOISM_DAEMON_DATA_DIR:join(root,"auth"),NEOISM_AGENT_STATE_DIR:join(root,"agent"),NEOISM_NOTES_HOME:join(root,"notes"),NEOISM_AGENT_SERVER:agent.url.origin,NEOISM_REQUIRE_AUTH:"0",NEOISM_DAEMON_ADDR:"127.0.0.1:0"});
for(const name of ["NEOISM_DAEMON_SOCKET","NEOISM_DAEMON_TOKEN","NEOISM_GPUIX_DAEMON_URL","NEOISM_PAIRING_TOKEN","NEOISM_WEB_ROOT","NEOISM_AGENT_GUI_ROOT"])delete process.env[name];
if(unavailableAgent){delete process.env.NEOISM_GPUIX_PRIVATE_DAEMON;delete process.env.NEOISM_DAEMON_ADDR;}
if(noRuntime)delete process.env.XDG_RUNTIME_DIR;else process.env.XDG_RUNTIME_DIR=join(root,"runtime");
const binary=resolve(process.env.NEOISM_GPUIX_WORKSPACE_DAEMON||"dist/neoism-workspace-daemon");
let child:ReturnType<typeof Bun.spawn>|undefined,client:DaemonConnection|undefined;
try{
 let starts=0;
 const launch=async()=>{starts++;child=Bun.spawn([binary,...localDaemonArgs(workspace,process.env),...(unavailableAgent?["--addr","127.0.0.1:0"]:[])],{detached:process.platform!=="win32",env:process.env,stdin:"ignore",stdout:"ignore",stderr:"ignore"});};
 // A shared-mode check must never reuse the user's real default-port daemon.
 const startup={launch,refresh:()=>loadConfig([],process.env,{}),timeoutMs:20000,probe:(endpoint:string)=>unavailableAgent&&endpoint==="ws://127.0.0.1:7878/session"?Promise.resolve(false):reachable(endpoint)};
 const config=await ensureLocalDaemon(loadConfig([],process.env,{}),[],process.env,startup);
 if(starts!==1||(!unavailableAgent&&!config.endpoint.startsWith("ws+unix://")))throw new Error("The real daemon did not publish its local socket");
 const socket=config.endpoint.startsWith("ws+unix://")?config.endpoint.slice("ws+unix://".length).split(":/")[0]:noRuntime?join(process.env.TMPDIR!,`neoism-${process.getuid!()}.sock`):join(process.env.XDG_RUNTIME_DIR!,"neoism.sock");
 for(const path of ["/","/index.html","/agent-gui/","/web/app/manifest.json"]){
  const response=await fetch(`http://localhost${path}`,{unix:socket,signal:AbortSignal.timeout(5000)});
  if(response.status!==404)throw new Error(`Native daemon exposed browser path ${path}: ${response.status}`);
 }
 if(!(await fetch("http://localhost/health",{unix:socket,signal:AbortSignal.timeout(5000)})).ok)throw new Error("Native daemon health endpoint is unavailable");
 const token=noRuntime?join(process.env.TMPDIR!,`neoism-${process.getuid!()}`,"daemon-token"):join(process.env.XDG_RUNTIME_DIR!,"neoism/daemon-token");
 if(!config.authToken||config.authToken!==readFileSync(token,"utf8").trim())throw new Error("Local upgrade credential discovery does not match the real daemon");
 client=new DaemonConnection(config,undefined,undefined,{profileStart:()=>0,profileEnd:()=>{},prepareConnection:signal=>ensureLocalDaemon(loadConfig([],process.env,{}),[],process.env,{...startup,signal})});client.start();
 const handshakeDeadline=Date.now()+8000;
 while(client.state.phase!=="connected"&&Date.now()<handshakeDeadline)await Bun.sleep(20);
 if(client.state.phase!=="connected")throw new Error(`Local handshake failed: ${client.state.phase}`);
 const result=await Effect.runPromise(client.request("Workspace","RequestHostWorkspaceTree"));
 if(!JSON.stringify(result).includes(workspace))throw new Error("The daemon did not own the requested initial workspace");
 await ensureLocalDaemon(config,[],process.env,{launch:async()=>{throw new Error("Running daemon was restarted");}});
 if(unavailableAgent){
  const expectedStarts=starts+1;
  child!.kill("SIGKILL");await child!.exited;
  const recoveryDeadline=Date.now()+20000;
  while((starts<expectedStarts||client.state.phase!=="connected")&&Date.now()<recoveryDeadline)await Bun.sleep(20);
  if(starts!==expectedStarts||client.state.phase!=="connected")throw new Error("Client did not recover automatically after daemon termination");
  const recovered=await Effect.runPromise(client.request("Workspace","RequestHostWorkspaceTree"));
  if(!JSON.stringify(recovered).includes(workspace))throw new Error("Recovered daemon lost its initial workspace");
 }
 console.log(JSON.stringify({realDaemon:true,initialWorkspace:true,credentialDiscovered:true,reused:true,browserUisDisabled:true,withoutXdgRuntime:noRuntime,unavailableAgent,restarted:unavailableAgent}));
}finally{
 client?.stop();if(child){child.kill("SIGTERM");const timeout=setTimeout(()=>child?.kill("SIGKILL"),5000);await child.exited;clearTimeout(timeout);}agent.stop(true);rmSync(root,{recursive:true,force:true});
}
