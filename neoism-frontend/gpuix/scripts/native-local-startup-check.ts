// Exercise the installed application's default startup and daemon lifetime.
import {existsSync,mkdtempSync,mkdirSync,readFileSync,writeFileSync,rmSync} from "node:fs";
import {join,resolve} from "node:path";
import {tmpdir} from "node:os";
import {launchNative} from "../tests/support/native-launch";
import {reachable} from "../src/daemon/local-startup";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {AgentSdk} from "../src/daemon/sdk";
import {Effect} from "effect";

const directory=mkdtempSync(join(tmpdir(),"neoism-installed-startup-"));
const binary=resolve(process.env.NEOISM_GPUIX_TEST_BINARY||"dist/neoism");
const daemon=resolve(process.env.NEOISM_GPUIX_WORKSPACE_DAEMON||"dist/neoism-workspace-daemon");
const wrapper=join(directory,"daemon"),pidPath=join(directory,"daemon.pid");
writeFileSync(wrapper,'#!/bin/sh\nprintf "%s" "$$" > "$NEOISM_TEST_DAEMON_PID"\nexec "$NEOISM_TEST_REAL_DAEMON" "$@"\n',{mode:0o700});
const paths=["home","runtime","temporary","workspace","config"];
for(const path of paths)mkdirSync(join(directory,path),{recursive:true});
const port=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>new Response()});
const agentUrl=port.url.origin;port.stop(true);
const env:NodeJS.ProcessEnv={...process.env,HOME:join(directory,"home"),
 XDG_RUNTIME_DIR:join(directory,"runtime"),TMPDIR:join(directory,"temporary"),
 XDG_STATE_HOME:join(directory,"state-home"),XDG_DATA_HOME:join(directory,"data-home"),XDG_CONFIG_HOME:join(directory,"config-home"),
 NEOISM_STATE_DIR:join(directory,"state"),NEOISM_CONFIG_HOME:join(directory,"config"),NEOISM_CONFIG_DIR:join(directory,"config"),
 NEOISM_DAEMON_DATA_DIR:join(directory,"auth"),NEOISM_AGENT_STATE_DIR:join(directory,"agent"),NEOISM_NOTES_HOME:join(directory,"notes"),
 NEOISM_AGENT_SERVER:agentUrl,NEOISM_AGENT_DISABLE_MODELS_FETCH:"true",NEOISM_REQUIRE_AUTH:"0",NEOISM_DAEMON_ADDR:"127.0.0.1:0",
 NEOISM_GPUIX_STATE_PATH:join(directory,"session.json"),NEOISM_GPUIX_WORKSPACE_DAEMON:wrapper,
 NEOISM_LAUNCH_DIRECTORY:join(directory,"workspace"),NEOISM_TEST_REAL_DAEMON:daemon,NEOISM_TEST_DAEMON_PID:pidPath};
// Keep the existing compositor address while isolating daemon runtime state.
if(process.env.WAYLAND_DISPLAY&&!process.env.WAYLAND_DISPLAY.startsWith("/")&&process.env.XDG_RUNTIME_DIR)
 env.WAYLAND_DISPLAY=join(process.env.XDG_RUNTIME_DIR,process.env.WAYLAND_DISPLAY);
 // launchNative merges the parent environment; undefined prevents credentials
 // from the running desktop leaking back into this isolated daemon fixture.
 for(const key of ["NEOISM_DAEMON_SOCKET","NEOISM_DAEMON_TOKEN","NEOISM_GPUIX_DAEMON_URL","NEOISM_PAIRING_TOKEN","NEOISM_AGENT_TOKEN","NEOISM_AGENT_AUTH_CONFIG","NEOISM_WEB_ROOT","NEOISM_AGENT_GUI_ROOT"])env[key]=undefined;
const endpoint=`ws+unix://${env.XDG_RUNTIME_DIR}/neoism.sock:/session`;
let app:Awaited<ReturnType<typeof launchNative>>|undefined,pid:number|undefined;
try{
 const launch=()=>launchNative({command:binary,args:[],cwd:join(directory,"workspace"),env});
 app=await launch();await app.getByTestId("splash-wordmark").waitFor({timeoutMs:120000});
 await Bun.sleep(500);
 if(await app.getByTestId("app-notification").count())throw new Error("Fresh default startup reported a service error");
 if(existsSync(join(directory,"notes/Default")))throw new Error("Browsing eagerly created the notes vault");
 const health=await fetch(`${agentUrl}/v2/health`).then(response=>response.json()) as {healthy?:boolean};
  if(!health.healthy)throw new Error("The daemon's real embedded Agent is unavailable");
 for(const path of ["/","/index.html","/agent-gui/"]){
  const response=await fetch(`${agentUrl}${path}`,{signal:AbortSignal.timeout(5000)});
  if(response.status!==404)throw new Error(`Native Agent listener exposed browser path ${path}: ${response.status}`);
 }
  const config=loadConfig([],env),connection=new DaemonConnection(config);
  if(!config.authToken)throw new Error("Shell smoke did not discover the desktop daemon credential");
 const sdk=new AgentSdk(config,((input,init)=>fetch(input,{...init,signal:AbortSignal.any([AbortSignal.timeout(15000),...(init?.signal?[init.signal]:[])])})) as typeof fetch);
 try{
  connection.start();const deadline=Date.now()+10000;
  while(connection.state.phase!=="connected"&&Date.now()<deadline)await Bun.sleep(20);
  if(connection.state.phase!=="connected")throw new Error("Shell smoke daemon handshake failed");
  const reply=await Effect.runPromise(connection.request("Workspace",{ResolveInitialWorkspace:{preferred_host_id:connection.state.connectedHostId||null}}));
   const workspace=(reply as {InitialWorkspaceResolved:{workspace:{id:string}}}).InitialWorkspaceResolved.workspace;
   const session=await sdk.createSession(workspace.id,{directory:join(directory,"workspace"),permission:[{permission:"bash",pattern:"*",action:"allow"}]});
  // Probe and execute through the same authenticated proxy as the composer.
  // Direct Agent HTTP would miss daemon allowlist rejections of GET /shell.
  const output=await sdk.forSession(workspace.id,session.id).sessions.shell(session.id,"echo neoism-direct-shell-check",{model:{providerId:"missing-provider",modelId:"not-a-model"}});
  if(!output.parts?.some(part=>part.type==="tool"&&part.tool==="bash"&&part.state.status==="completed"&&part.state.output.includes("neoism-direct-shell-check")))throw new Error("Packaged shell did not produce a real completed command block through the daemon");
 }finally{sdk.dispose();connection.stop();}
 pid=Number(readFileSync(pidPath,"utf8"));if(!Number.isSafeInteger(pid)||pid<=1)throw new Error("Missing owned daemon PID");
 if(!await reachable(endpoint))throw new Error("Default launch did not start its private daemon");
 await app.close();app=undefined;
 if(!await reachable(endpoint))throw new Error("Closing the frontend stopped the daemon");
 app=await launch();await app.getByTestId("splash-wordmark").waitFor({timeoutMs:30000});
 if(Number(readFileSync(pidPath,"utf8"))!==pid)throw new Error("Relaunch replaced the running daemon");
 await app.close();app=undefined;
 if(!await reachable(endpoint))throw new Error("Relaunch lost daemon ownership");
 console.log(JSON.stringify({installedDefaultStartup:true,realEmbeddedAgent:true,directShell:true,browserUiDisabled:true,daemonSurvivesWindow:true,relaunchReusesDaemon:true,uncreatedNotesStayEmpty:true}));
}finally{
 await app?.close();
 if(pid===undefined){try{pid=Number(readFileSync(pidPath,"utf8"));}catch{}}
 if(pid&&Number.isSafeInteger(pid)&&pid>1){
  try{process.kill(pid,"SIGTERM");}catch{}
  const deadline=Date.now()+5000;while(await reachable(endpoint)&&Date.now()<deadline)await Bun.sleep(50);
  try{process.kill(-pid,"SIGKILL");}catch{}
 }
 rmSync(directory,{recursive:true,force:true});
}
