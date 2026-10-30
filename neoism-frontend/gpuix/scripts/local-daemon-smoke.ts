// Real daemon readiness, credential discovery, and initial workspace; private state only.
import {mkdtempSync,mkdirSync,readFileSync,rmSync} from "node:fs";
import {join,resolve} from "node:path";
import {tmpdir} from "node:os";
import {Effect} from "effect";
import {loadConfig} from "../src/config";
import {ensureLocalDaemon} from "../src/daemon/local-startup";
import {DaemonConnection} from "../src/daemon/client";
const root=mkdtempSync(join(tmpdir(),"neoism-local-daemon-check-")),workspace=join(root,"workspace");
for(const path of [workspace,join(root,"runtime"),join(root,"temporary")])mkdirSync(path,{recursive:true});
const noRuntime=process.argv.includes("--without-xdg-runtime");
const agent=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>Response.json({workspaceId:"startup-test",healthy:true,version:"test",providerCredentialStore:"test"})});
Object.assign(process.env,{HOME:join(root,"home"),TMPDIR:join(root,"temporary"),XDG_STATE_HOME:join(root,"state-home"),XDG_DATA_HOME:join(root,"data-home"),XDG_CONFIG_HOME:join(root,"config-home"),NEOISM_STATE_DIR:join(root,"state"),NEOISM_CONFIG_HOME:join(root,"config"),NEOISM_CONFIG_DIR:join(root,"config"),NEOISM_DAEMON_DATA_DIR:join(root,"auth"),NEOISM_AGENT_STATE_DIR:join(root,"agent"),NEOISM_NOTES_HOME:join(root,"notes"),NEOISM_AGENT_SERVER:agent.url.origin,NEOISM_REQUIRE_AUTH:"0",NEOISM_DAEMON_ADDR:"127.0.0.1:0"});
for(const name of ["NEOISM_DAEMON_SOCKET","NEOISM_DAEMON_TOKEN","NEOISM_GPUIX_DAEMON_URL","NEOISM_PAIRING_TOKEN"])delete process.env[name];
if(noRuntime)delete process.env.XDG_RUNTIME_DIR;else process.env.XDG_RUNTIME_DIR=join(root,"runtime");
const binary=resolve(process.env.NEOISM_GPUIX_WORKSPACE_DAEMON||"dist/neoism-workspace-daemon");
let child:ReturnType<typeof Bun.spawn>|undefined,client:DaemonConnection|undefined;
try{
 let starts=0;
 const config=await ensureLocalDaemon(loadConfig([],process.env,{}),[],process.env,{
  launch:async()=>{starts++;child=Bun.spawn([binary,"--workspace",workspace],{env:process.env,stdin:"ignore",stdout:"ignore",stderr:"ignore"});},
  refresh:()=>loadConfig([],process.env,{}),timeoutMs:20000,
 });
 if(starts!==1||!config.endpoint.startsWith("ws+unix://"))throw new Error("The real daemon did not publish its local socket");
 const token=noRuntime?join(process.env.TMPDIR!,`neoism-${process.getuid!()}`,"daemon-token"):join(process.env.XDG_RUNTIME_DIR!,"neoism/daemon-token");
 if(!config.authToken||config.authToken!==readFileSync(token,"utf8").trim())throw new Error("Local upgrade credential discovery does not match the real daemon");
 client=new DaemonConnection(config);client.start();
 const handshakeDeadline=Date.now()+8000;
 while(client.state.phase!=="connected"&&Date.now()<handshakeDeadline)await Bun.sleep(20);
 if(client.state.phase!=="connected")throw new Error(`Local handshake failed: ${client.state.phase}`);
 const result=await Effect.runPromise(client.request("Workspace","RequestHostWorkspaceTree"));
 if(!JSON.stringify(result).includes(workspace))throw new Error("The daemon did not own the requested initial workspace");
 await ensureLocalDaemon(config,[],process.env,{launch:async()=>{throw new Error("Running daemon was restarted");}});
 console.log(JSON.stringify({realDaemon:true,initialWorkspace:true,credentialDiscovered:true,reused:true,withoutXdgRuntime:noRuntime}));
}finally{
 client?.stop();if(child){child.kill("SIGTERM");const timeout=setTimeout(()=>child?.kill("SIGKILL"),5000);await child.exited;clearTimeout(timeout);}agent.stop(true);rmSync(root,{recursive:true,force:true});
}
