import type {AppConfig} from "../config";
import type {ServerSelection} from "../menu-actions/ChromeActionsController";
import {DaemonConnection} from "./client";
import {localDaemon} from "../menu-actions/web-frontend";
import {attachSsh,prepareSshWorkspace,type SshAttachment} from "./ssh-transport";

export type ServerHandshakePhase="waiting"|"timeout"|"auth-rejected"|"cancelled";
export class ServerHandshakeError extends Error{
 constructor(readonly phase:ServerHandshakePhase,message:string){super(message);this.name="ServerHandshakeError";}
}
function cancellation(signal?:AbortSignal){return new ServerHandshakeError("cancelled",signal?.reason instanceof Error?signal.reason.message:"Server connection cancelled");}
export function waitForConnection(client:DaemonConnection,timeoutMs=10000,signal?:AbortSignal):Promise<void>{
 if(signal?.aborted)return Promise.reject(cancellation(signal));
 if(client.state.phase==="connected")return Promise.resolve();
 return new Promise((resolve,reject)=>{
  let unsubscribe=()=>{},finished=false;
  const finish=(error?:Error)=>{if(finished)return;finished=true;clearTimeout(timer);unsubscribe();signal?.removeEventListener("abort",cancel);error?reject(error):resolve();};
  const cancel=()=>finish(cancellation(signal));
  const timer=setTimeout(()=>finish(new ServerHandshakeError("timeout","The selected server did not complete its handshake")),timeoutMs);
  unsubscribe=client.subscribeStatus(state=>{
   if(state.phase==="connected")finish();
   else if(state.phase==="auth-rejected"||state.phase==="waiting")finish(new ServerHandshakeError(state.phase,state.reason||"The selected server is unavailable"));
  });
  if(finished){unsubscribe();return;}
  signal?.addEventListener("abort",cancel,{once:true});
  if(signal?.aborted){cancel();return;}
  try{client.start();}catch(error){finish(new ServerHandshakeError("waiting",error instanceof Error?error.message:"The selected server is unavailable"));}
 });
}
/** Verify a selected host before unmounting/saving the current connection.
 * A failed or cancelled dial leaves its workspace and terminal intact. */
export async function prepareServerConnection(server:ServerSelection,current:AppConfig,local:()=>AppConfig,options:{signal?:AbortSignal}={}):Promise<{config:AppConfig;ssh?:SshAttachment}>{
 const {signal}=options;if(signal?.aborted)throw cancellation(signal);
 let ssh:SshAttachment|undefined;
 const localConfig=server.id==="local"?local():undefined;
 try{
  if(server.ssh)ssh=await attachSsh(server.ssh,{signal});
  const config:AppConfig=localConfig?{...localConfig,serverId:"local",collaborative:false,transportIdentity:undefined}:
   {...current,endpoint:ssh?.endpoint||server.endpoint,authToken:ssh?.authToken||server.authToken,pairingToken:ssh?undefined:server.pairingToken??server.token,workspaceId:ssh?.workspaceId||server.workspaceId,serverId:server.id,collaborative:true,transportIdentity:ssh?.identity};
  const probe=new DaemonConnection(config);
  try{await waitForConnection(probe,10000,signal);if(ssh)await prepareSshWorkspace(probe,ssh,{signal});if(signal?.aborted)throw cancellation(signal);}finally{probe.stop();}
  return {config,ssh};
 }catch(error){ssh?.dispose();if(signal?.aborted)throw cancellation(signal);throw error;}
}

/** Retain the authenticated Local Server descriptor when visiting remote hosts;
 * rebuilding from stripped environment defaults can discard its credentials. */
export class LocalServerConfig{
 private config?:AppConfig;
 constructor(initial?:AppConfig){if(initial)this.observe(initial);}
 observe(config:AppConfig){if(!config.collaborative&&!config.transportIdentity&&(!config.serverId||config.serverId==="local")&&localDaemon(config.endpoint))this.config={...config};}
 resolve(fallback:()=>AppConfig){return this.config?{...this.config}:fallback();}
}
