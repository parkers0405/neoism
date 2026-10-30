// Exercise the installed application's default startup and daemon lifetime.
import {existsSync,mkdtempSync,mkdirSync,readFileSync,writeFileSync,rmSync} from "node:fs";
import {join,resolve} from "node:path";
import {tmpdir} from "node:os";
import {launchNative} from "../tests/support/native-launch";
import {reachable} from "../src/daemon/local-startup";

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
for(const key of ["NEOISM_DAEMON_SOCKET","NEOISM_DAEMON_TOKEN","NEOISM_GPUIX_DAEMON_URL","NEOISM_PAIRING_TOKEN","NEOISM_AGENT_TOKEN","NEOISM_AGENT_AUTH_CONFIG"])delete env[key];
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
 pid=Number(readFileSync(pidPath,"utf8"));if(!Number.isSafeInteger(pid)||pid<=1)throw new Error("Missing owned daemon PID");
 if(!await reachable(endpoint))throw new Error("Default launch did not start its private daemon");
 await app.close();app=undefined;
 if(!await reachable(endpoint))throw new Error("Closing the frontend stopped the daemon");
 app=await launch();await app.getByTestId("splash-wordmark").waitFor({timeoutMs:30000});
 if(Number(readFileSync(pidPath,"utf8"))!==pid)throw new Error("Relaunch replaced the running daemon");
 await app.close();app=undefined;
 if(!await reachable(endpoint))throw new Error("Relaunch lost daemon ownership");
 console.log(JSON.stringify({installedDefaultStartup:true,realEmbeddedAgent:true,daemonSurvivesWindow:true,relaunchReusesDaemon:true,uncreatedNotesStayEmpty:true}));
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
