import {spawn} from "node:child_process";
import {existsSync,mkdirSync,openSync,closeSync} from "node:fs";
import {connect} from "node:net";
import {dirname,join,resolve} from "node:path";
import {homedir} from "node:os";
import {fileURLToPath} from "node:url";
import {setTimeout as delay} from "node:timers/promises";
import {discoveredLocalDaemon,loadConfig,type AppConfig} from "../config";
import {startupDirectory} from "./startup-directory";
import {daemonExecutable} from "../../../daemon-executable";
import WebSocket from "ws";
import {localDaemon} from "../menu-actions/web-frontend";
export {daemonExecutable};

export function explicitDaemon(args:readonly string[],env:NodeJS.ProcessEnv){
 return !!env.NEOISM_GPUIX_DAEMON_URL||args.some(arg=>arg==="--daemon"||arg.startsWith("--daemon="));
}
export function localEndpoint(env:NodeJS.ProcessEnv=process.env){
 return `ws://${env.NEOISM_DAEMON_ADDR||"127.0.0.1:7878"}/session`;
}
/** Only an explicitly private preview may sacrifice its work on owner exit.
 * Never infer ownership from a PID, /tmp path, or an unavailable endpoint.
 * Unix detached spawn creates the dedicated group required by watch_parent.
 * Windows needs a kill-on-close job before enabling the same tree guarantee. */
export function localDaemonArgs(directory:string,env:NodeJS.ProcessEnv,parentPid=process.pid,platform:NodeJS.Platform=process.platform):string[]{
 const args=["--workspace",directory];
 if(env.NEOISM_GPUIX_PRIVATE_DAEMON!=="1"){
  if(!env.NEOISM_DAEMON_ADDR&&!env.NEOISM_DAEMON_SOCKET)args.push("--auto-port");
  if(!env.NEOISM_AGENT_SERVER&&!env.NEOISM_SERVER)args.push("--auto-agent-port");
 }
 if(env.NEOISM_GPUIX_PRIVATE_DAEMON==="1"&&platform!=="win32")args.push("--temporary-parent-pid",String(parentPid));
 return args;
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
/** A listening port is not readiness: require the application handshake. */
export function daemonReady(config:AppConfig,timeout=5000,signal?:AbortSignal):Promise<boolean>{
 return new Promise(resolve=>{
  if(signal?.aborted){resolve(false);return;}
  let socket:WebSocket;
  try{
   const url=new URL(config.endpoint);if(config.authToken)url.searchParams.set("token",config.authToken);
   socket=new WebSocket(url.toString(),{maxPayload:16*1024*1024});
  }catch{resolve(false);return;}
  let settled=false;
  const finish=(ready:boolean)=>{if(settled)return;settled=true;clearTimeout(timer);signal?.removeEventListener("abort",abort);socket.terminate();resolve(ready);};
  const abort=()=>finish(false),timer=setTimeout(()=>finish(false),timeout);
  signal?.addEventListener("abort",abort,{once:true});
  socket.once("open",()=>socket.send(JSON.stringify({Workspace:{request_id:0,message:{Hello:{token:config.pairingToken??null,client_name:"neoism-local-readiness"}}}})));
  socket.on("message",data=>{try{const ack=JSON.parse(data.toString())?.WorkspaceReply?.message?.HelloAck;if(ack)finish(ack.accepted===true);}catch{finish(false);}});
  // terminate() during a failed upgrade may emit a second error (notably Bun).
  socket.on("error",()=>finish(false));socket.once("close",()=>finish(false));
 });
}
export interface LocalStartup {
 probe(endpoint:string):Promise<boolean>;
 ready(config:AppConfig):Promise<boolean>;
 launch():Promise<void>;
 refresh():AppConfig;
 sleep():Promise<unknown>;
 timeoutMs:number;
 now():number;
 signal?:AbortSignal;
}
/** A remote selection never creates a local service. Shared local daemons
 * persist for terminals/agents; explicitly private previews follow their owner. */
export async function ensureLocalDaemon(config:AppConfig,args:readonly string[]=process.argv,env:NodeJS.ProcessEnv=process.env,overrides:Partial<LocalStartup>={}):Promise<AppConfig>{
  if(explicitDaemon(args,env)||!localDaemon(config.endpoint))return config;
 let startupExit:string|undefined;
  const hooks:LocalStartup={probe:reachable,ready:current=>daemonReady(current,process.platform==="win32"?30000:5000,overrides.signal),refresh:()=>loadConfig(args,env),sleep:()=>delay(250,undefined,{signal:overrides.signal}),timeoutMs:process.platform==="win32"?240000:120000,now:Date.now,
  async launch(){
   const binary=daemonExecutable(env);
   const state=env.XDG_STATE_HOME||join(homedir(),".local/state");const folder=join(state,"neoism");mkdirSync(folder,{recursive:true});
   const log=join(folder,"daemon.log"),fd=openSync(log,"a",0o600);
   try{
    const directory=startupDirectory(env);
     // Windows detached processes get a new console; ignored stdio + unref
     // already lets the shared daemon outlive this window without that flag.
     const child=spawn(binary,localDaemonArgs(directory,env),{env:{...env,NEOISM_WORKSPACE_ROOT:directory},cwd:directory,detached:process.platform!=="win32",stdio:["ignore",fd,fd],windowsHide:true});
    await new Promise<void>((resolve,reject)=>{child.once("spawn",resolve);child.once("error",reject);});child.once("exit",(code,signal)=>{startupExit=`The local workspace daemon exited during startup (${signal||code}). See the daemon.log in XDG_STATE_HOME/neoism or ~/.local/state/neoism.`;});child.unref();
   }finally{closeSync(fd);}
  },...overrides};
  hooks.signal?.throwIfAborted();
  const tcp=localEndpoint(env);
  let launched=false;
  let current=config;
  const deadline=hooks.now()+hooks.timeoutMs;
  do{
   hooks.signal?.throwIfAborted();
   const primaryOccupied=await hooks.probe(current.endpoint);
   let occupied=primaryOccupied;
   if(occupied&&await hooks.ready(current))return current;
   const published=discoveredLocalDaemon(env);
   let managedListener=primaryOccupied&&(current.endpoint.startsWith("ws+unix://")||current.endpoint===published);
   if(published&&published!==current.endpoint&&await hooks.probe(published)){
    occupied=true;managedListener=true;
    const discovered={...current,endpoint:published};if(await hooks.ready(discovered))return discovered;
   }
   if(current.endpoint!==tcp&&published!==tcp&&await hooks.probe(tcp)){
    occupied=true;
    const fallback={...current,endpoint:tcp};if(await hooks.ready(fallback))return fallback;
   }
   if(startupExit)throw new Error(startupExit);
   // A published service may still be warming up. Unknown TCP owners must
   // not trap default startup: the daemon reserves an alternate port under
   // an OS-held ownership lock, without killing or competing for their port.
   const allowFallback=env.NEOISM_GPUIX_PRIVATE_DAEMON!=="1"&&!env.NEOISM_DAEMON_ADDR&&!env.NEOISM_DAEMON_SOCKET&&!managedListener;
   if((!occupied||allowFallback)&&!launched){await hooks.launch();launched=true;}
   await hooks.sleep();
   current=hooks.refresh();
 }while(hooks.now()<deadline);
   throw new Error("The local workspace daemon did not complete its handshake; recovery will retry. Explicit endpoint conflicts are not moved or killed. See ~/.local/state/neoism/daemon.log (or XDG_STATE_HOME/neoism/daemon.log).");
}

// Window sessions share a startup attempt instead of spawning competing daemons.
const recovering=new Map<string,{promise:Promise<AppConfig>;controller:AbortController;users:number}>();
export async function recoverLocalDaemon(config:AppConfig,signal:AbortSignal):Promise<Pick<AppConfig,"endpoint"|"authToken">>{
 signal.throwIfAborted();
 const key=config.endpoint;
 let entry=recovering.get(key);
 if(!entry){
  const env={...process.env,NEOISM_GPUIX_DAEMON_URL:undefined};
  const controller=new AbortController();
  const promise=ensureLocalDaemon(config,[],env,{refresh:()=>({...config,endpoint:discoveredLocalDaemon(env)||config.endpoint,authToken:loadConfig([],env).authToken??config.authToken}),signal:controller.signal});
  entry={promise,controller,users:0};recovering.set(key,entry);
 }
 const active=entry;active.users++;
 let abort=()=>{};
 try{
  const ready=await Promise.race([active.promise,new Promise<never>((_,reject)=>{
   abort=()=>reject(signal.reason);signal.addEventListener("abort",abort,{once:true});if(signal.aborted)abort();
  })]);
  signal.throwIfAborted();return {endpoint:ready.endpoint,authToken:ready.authToken};
 }finally{
  signal.removeEventListener("abort",abort);
  if(--active.users===0){active.controller.abort();if(recovering.get(key)===active)recovering.delete(key);}
 }
}
