import {coreCall} from "../native-core";
import type {ServerSelection} from "./ChromeActionsController";
export interface ServerEntry {id:string;name:string;address:string;local?:boolean;temporary?:boolean;hosted?:boolean;online?:boolean}
export interface ServerDraft {mode:"host"|"join";fast:boolean;directory:string;address:string;name:string;password:string;serverId?:string}
/** Native workers own server provisioning; polling never blocks a frame. */
export async function serverJob(action:string,args:Record<string,unknown>={}):Promise<ServerSelection>{
 const {id}=coreCall<{id:number}>("server.start",{action,...args});
 for(;;){
  const result=coreCall<{phase:"pending"|"ready"|"failed";result?:ServerSelection;message?:string}>("server.poll",{id});
  if(result.phase==="ready")return result.result!;
  if(result.phase==="failed")throw new Error(result.message||"Server request failed");
  await new Promise(resolve=>setTimeout(resolve,80));
 }
}
export function serverDraftValues(draft:ServerDraft,values:readonly [string,string][]):ServerDraft{
 const fields=Object.fromEntries(values),directory=fields.fast_host_dir??fields.create_dir,address=fields.fast_join_address??fields.address;
 return {...draft,...(directory!==undefined?{directory}:{}),...(address!==undefined?{address}:{}),name:fields.name??draft.name,password:fields.token??draft.password,serverId:fields.server_id??draft.serverId};
}
export function serverFormOperation(draft:ServerDraft){
 if(draft.serverId)return {action:"save",args:{serverId:draft.serverId,address:draft.address,name:draft.name,password:draft.password}};
 if(draft.mode==="host"){
  if(!draft.directory.trim())throw new Error("Project folder is required");
  if(draft.fast&&!draft.password.trim())throw new Error("Password is required to host a Fast Server");
  return {action:draft.fast?"host-fast":"host",args:{directory:draft.directory,name:draft.name,password:draft.password}};
 }
 if(!draft.address.trim())throw new Error("Server address is required");
 return {action:draft.fast?"join-fast":"save",args:{address:draft.address,name:draft.name,password:draft.password}};
}
