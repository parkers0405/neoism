import {spawn} from "node:child_process";
import {existsSync,mkdirSync,openSync,closeSync} from "node:fs";
import {connect} from "node:net";
import {dirname,join,resolve} from "node:path";
import {homedir} from "node:os";
import {fileURLToPath} from "node:url";
import {setTimeout as delay} from "node:timers/promises";
import {loadConfig,type AppConfig} from "../config";

export function explicitDaemon(args:readonly string[],env:NodeJS.ProcessEnv){
 return !!env.NEOISM_GPUIX_DAEMON_URL||args.some(arg=>arg==="--daemon"||arg.startsWith("--daemon="));
}
export function localEndpoint(env:NodeJS.ProcessEnv=process.env){
 return `ws://${env.NEOISM_DAEMON_ADDR||"127.0.0.1:7878"}/session`;
}
export function reachable(endpoint:string,timeout=300):Promise<boolean>{
 return new Promise(resolve=>{
  let options:{path:string}|{host:string;port:number};
  try{
   if(endpoint.startsWith("ws+unix://"))options={path:endpoint.slice("ws+unix://".length).split(":/")[0]};
   else{const url=new URL(endpoint);options={host:url.hostname.replace(/^\[|\]$/g,""),port:Number(url.port|| (url.protocol==="wss:"?443:80))};}
  }catch{resolve(false);return;}
  const socket=connect(options);let settled=false;
  const finish=(ready:boolean)=>{if(settled)return;settled=true;socket.destroy();resolve(ready);};
  socket.setTimeout(timeout);socket.once("connect",()=>finish(true));socket.once("error",()=>finish(false));socket.once("timeout",()=>finish(false));
 });
}
export function daemonExecutable(env:NodeJS.ProcessEnv=process.env,executable=process.execPath){
 const suffix=process.platform==="win32"?".exe":"";
 const override=env.NEOISM_GPUIX_WORKSPACE_DAEMON;
 if(override){const path=resolve(override);if(!existsSync(path))throw new Error(`Configured workspace daemon is missing: ${path}`);return path;}
 const repo=fileURLToPath(new URL("../../../../",import.meta.url));
 const candidates=[join(dirname(executable),`neoism-workspace-daemon${suffix}`),
  join(repo,"neoism-frontend/gpuix/artifacts",`neoism-workspace-daemon${suffix}`),
  ...["activation","release","debug"].map(profile=>join(repo,"target",profile,`neoism-workspace-daemon${suffix}`))];
 const path=candidates.find(existsSync);
 if(!path)throw new Error("Workspace daemon is not built. Run make build from the repository root.");
 return path;
}
export interface LocalStartup {
 probe(endpoint:string):Promise<boolean>;
 launch():Promise<void>;
 refresh():AppConfig;
 sleep():Promise<unknown>;
 timeoutMs:number;
 now():number;
}
/** A remote selection never creates a local service. A local daemon persists
 * after the window exits so terminals and Agent sessions keep their owner. */
export async function ensureLocalDaemon(config:AppConfig,args:readonly string[]=process.argv,env:NodeJS.ProcessEnv=process.env,overrides:Partial<LocalStartup>={}):Promise<AppConfig>{
 if(explicitDaemon(args,env))return config;
 let startupExit:string|undefined;
 const hooks:LocalStartup={probe:reachable,refresh:()=>loadConfig(),sleep:()=>delay(100),timeoutMs:120000,now:Date.now,
  async launch(){
   const binary=daemonExecutable(env);
   const state=env.XDG_STATE_HOME||join(homedir(),".local/state");const folder=join(state,"neoism");mkdirSync(folder,{recursive:true});
   const log=join(folder,"daemon.log"),fd=openSync(log,"a",0o600);
   try{
    const child=spawn(binary,["--workspace",env.NEOISM_LAUNCH_DIRECTORY||process.cwd()],{env,detached:true,stdio:["ignore",fd,fd],windowsHide:true});
    await new Promise<void>((resolve,reject)=>{child.once("spawn",resolve);child.once("error",reject);});child.once("exit",(code,signal)=>{startupExit=`The local workspace daemon exited during startup (${signal||code}). See the daemon.log in XDG_STATE_HOME/neoism or ~/.local/state/neoism.`;});child.unref();
   }finally{closeSync(fd);}
  },...overrides};
 if(await hooks.probe(config.endpoint))return config;
 const tcp=localEndpoint(env);
 if(tcp!==config.endpoint&&await hooks.probe(tcp))return {...hooks.refresh(),endpoint:tcp};
 await hooks.launch();const deadline=hooks.now()+hooks.timeoutMs;
 do{
  const current=hooks.refresh();
  if(await hooks.probe(current.endpoint))return current;
  if(current.endpoint!==tcp&&await hooks.probe(tcp))return {...current,endpoint:tcp};
  if(startupExit)throw new Error(startupExit);
  await hooks.sleep();
 }while(hooks.now()<deadline);
 throw new Error("The local workspace daemon did not become ready. See ~/.local/state/neoism/daemon.log (or XDG_STATE_HOME/neoism/daemon.log).");
}
