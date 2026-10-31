import * as Effect from "effect/Effect";
import {coreCall} from "../native-core";
import type {DaemonConnection} from "./client";
import {expectVariant,isRecord} from "./protocol";

export interface SshRecipe {systemId?:string;target?:string;args?:readonly string[]}
export interface SshHost {target:string;hostname?:string|null;user?:string|null;port?:number|null}
export interface SavedSystem {id:string;name:string;target:string;ssh_args:string[];enabled:boolean;connection?:"connected"|"connecting"|"disconnected"|"error";error?:string}
export type SshNativeCall=<T>(op:string,args?:Record<string,unknown>)=>T;
type Poll={phase:"pending"|"closed"}|{phase:"failed";message:string}|{phase:"ready";endpoint:string;authToken:string;workspaceId:string;target:string};
export interface SshAttachment {
 readonly id:number;readonly endpoint:string;readonly authToken:string;readonly workspaceId:string;readonly target:string;
 /** Stable across ephemeral forwarded ports, never contains a credential. */
 readonly identity:string;
 alive():boolean;dispose():void;
}
function aborted(signal?:AbortSignal){if(signal?.aborted)throw signal.reason instanceof Error?signal.reason:new Error("SSH connection cancelled");}
function sleep(ms:number,signal?:AbortSignal){return new Promise<void>((resolve,reject)=>{
 const done=()=>{signal?.removeEventListener("abort",cancel);resolve();};
 const timer=setTimeout(done,ms),cancel=()=>{clearTimeout(timer);signal?.removeEventListener("abort",cancel);reject(signal?.reason instanceof Error?signal.reason:new Error("SSH connection cancelled"));};
 signal?.addEventListener("abort",cancel,{once:true});if(signal?.aborted)cancel();
});}
/** The original Rust engine owns SSH argv, daemon bootstrap, token retrieval
 * and tunnel lifetime. Never execute an ssh command inside a local PTY. */
export async function attachSsh(recipe:SshRecipe,options:{signal?:AbortSignal;call?:SshNativeCall;pollMs?:number;timeoutMs?:number}={}):Promise<SshAttachment>{
 const {signal,call=coreCall,pollMs=80,timeoutMs=30000}=options;aborted(signal);
 const {id}=call<{id:number}>("ssh.start",{...recipe,args:recipe.args?[...recipe.args]:undefined});
 let closed=false;
 const dispose=()=>{if(!closed){closed=true;call("ssh.close",{id});}};
 try{
  const deadline=Date.now()+timeoutMs;
  for(;;){
   aborted(signal);const frame=call<Poll>("ssh.poll",{id});
   if(frame.phase==="failed")throw new Error(frame.message);
   if(frame.phase==="closed")throw new Error("SSH connection closed");
   if(frame.phase==="ready"){
    if(!/^ws:\/\/127\.0\.0\.1:\d+\/session$/.test(frame.endpoint)||!frame.authToken||!frame.workspaceId.startsWith("neoism-quick-ssh-home-"))throw new Error("SSH host returned an invalid daemon attachment");
    return {id,endpoint:frame.endpoint,authToken:frame.authToken,workspaceId:frame.workspaceId,target:frame.target,identity:`ssh:${frame.workspaceId}`,dispose,alive:()=>!closed&&call<Poll>("ssh.poll",{id}).phase==="ready"};
   }
   if(Date.now()>=deadline)throw new Error("SSH daemon connection timed out");
   await sleep(pollMs,signal);
  }
 }catch(error){dispose();throw error;}
}
/** Create/reuse the canonical home workspace on the selected SSH daemon.
 * '~' is deliberately resolved by that host; local paths never enter this flow. */
export async function prepareSshWorkspace(client:DaemonConnection,attachment:Pick<SshAttachment,"workspaceId"|"target">,options:{signal?:AbortSignal;timeoutMs?:number}={}){
 const {signal,timeoutMs=15000}=options;aborted(signal);
 client.start();
 if(client.state.phase!=="connected")await new Promise<void>((resolve,reject)=>{
  let unsubscribe=()=>{},finished=false;
  const finish=(error?:Error)=>{if(finished)return;finished=true;clearTimeout(timer);unsubscribe();signal?.removeEventListener("abort",cancel);error?reject(error):resolve();};
  const cancel=()=>finish(signal?.reason instanceof Error?signal.reason:new Error("SSH connection cancelled"));
  const timer=setTimeout(()=>finish(new Error("SSH daemon handshake timed out")),timeoutMs);
  unsubscribe=client.subscribeStatus(state=>{if(state.phase==="connected")finish();else if(state.phase==="auth-rejected")finish(new Error(state.reason||"SSH daemon authentication rejected"));});
  if(finished)unsubscribe();else{signal?.addEventListener("abort",cancel,{once:true});if(signal?.aborted)cancel();}
 });
 aborted(signal);
 const reply=expectVariant(await Effect.runPromise(client.request("Workspace",{CreateWorkspace:{workspace_id:attachment.workspaceId,title:`${attachment.target} · Home`,root_dir:"~"}}),{signal}),"HostWorkspaceUpserted");
 if(!isRecord(reply)||!isRecord(reply.workspace)||reply.workspace.id!==attachment.workspaceId)throw new Error("SSH daemon returned a different workspace");
 return reply.workspace;
}
