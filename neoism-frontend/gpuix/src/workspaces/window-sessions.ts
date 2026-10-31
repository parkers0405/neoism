import type {AppConfig} from "../config";
import type {Workspace} from "../daemon/protocol";
import type {WorkspaceModel} from "../model";
import {localDaemon} from "../menu-actions/web-frontend";

export interface WorkspaceSession {config:AppConfig;model:WorkspaceModel}
export interface WorkspaceContext {config:AppConfig;connectedHostId?:string;directoryRoots?:unknown}
export interface WorkspaceNavigation {
 subscribe(listener:()=>void):()=>void;
 workspaces():readonly Workspace[];
 activeId():string|undefined;
 context(id:string):WorkspaceContext|undefined;
 activate(workspace:Workspace):Promise<void>;
 newWorkspace():Promise<Workspace|undefined>;
 close(discard?:boolean,id?:string):Promise<boolean>;
 reorder(dragId:string,targetId:string,position:"before"|"after"):boolean;
}
export const localWorkspaceSession=(session:WorkspaceSession)=>!session.config.collaborative&&!session.config.transportIdentity&&(!session.config.serverId||session.config.serverId==="local")&&localDaemon(session.config.endpoint);

/** A window owns tabs; each connection continues to own its model, editors,
 * terminals and credentials. Workspace IDs are scoped to their connection. */
export class WindowWorkspaceSessions<S extends WorkspaceSession> implements WorkspaceNavigation {
 private entries=new Map<string,{session:S;unsubscribe:()=>void}>();
 private rows:Workspace[]=[];
 private owners=new Map<string,{session:S;workspace:Workspace}>();
 private order:string[]=[];
 private listeners=new Set<()=>void>();
 private fingerprint="";
 private dependencies:unknown[]=[];
 private selected:S;
 private localPending?:Promise<S>;
 constructor(initial:S,private hooks:{activate:(session:S)=>void;local:()=>Promise<S>;dispose:(session:S)=>Promise<void>}){
  this.selected=initial;this.add(initial);
 }
 sessions=()=>[...this.entries.values()].map(entry=>entry.session);
 subscribe=(listener:()=>void)=>{this.listeners.add(listener);return()=>{this.listeners.delete(listener);};};
 workspaces=()=>this.rows;
 activeId=()=>this.tabId(this.selected,this.selected.model.state.activeWorkspace?.id);
 context=(id:string):WorkspaceContext|undefined=>{
  const owner=this.owners.get(id);if(!owner)return;
  const {session,workspace}=owner,roots=session.model.state.ui.terminalWorkspaceRoots as Record<string,unknown>|undefined;
  return {config:session.config,connectedHostId:session.model.state.connection.connectedHostId,directoryRoots:roots?{[id]:roots[workspace.id]}:undefined};
 };
 private key(session:S){return [...this.entries].find(([,entry])=>entry.session===session)?.[0];}
 private tabId(session:S,id?:string){const key=this.key(session);return key&&id?JSON.stringify([key,id]):undefined;}
 add(session:S,key=localWorkspaceSession(session)?"local":crypto.randomUUID()){
  if(this.key(session))return;
  if(this.entries.has(key))throw new Error("This window already owns that connection");
  this.entries.set(key,{session,unsubscribe:session.model.subscribe(()=>this.refresh())});this.refresh();
 }
 replace(previous:S,next:S){
  const key=this.key(previous);if(!key)throw new Error("The connection is no longer open");
  this.entries.get(key)!.unsubscribe();
  this.entries.set(key,{session:next,unsubscribe:next.model.subscribe(()=>this.refresh())});
  if(this.selected===previous)this.select(next);else this.refresh();
 }
 select(session:S){
  if(!this.key(session))this.add(session);
  const changed=this.selected!==session;this.selected=session;this.refresh();
  if(changed)this.hooks.activate(session);
 }
 private refresh(force=false){
  const dependencies:unknown[]=[this.selected];
  for(const {session}of this.entries.values())dependencies.push(session,session.model.state.workspaces,session.model.state.ui.openWorkspaceIds,session.model.state.activeWorkspace?.id,session.model.state.connection.connectedHostId,session.model.state.ui.terminalWorkspaceRoots);
  if(!force&&dependencies.length===this.dependencies.length&&dependencies.every((value,index)=>value===this.dependencies[index]))return;
  this.dependencies=dependencies;
  const owners=new Map<string,{session:S;workspace:Workspace}>();
  for(const [key,{session}]of this.entries)for(const workspace of session.model.windowWorkspaces())owners.set(JSON.stringify([key,workspace.id]),{session,workspace});
  const order=[...this.order.filter(id=>owners.has(id)),...[...owners.keys()].filter(id=>!this.order.includes(id))];
  const signature=JSON.stringify([this.activeId(),order.map(id=>{const {session,workspace}=owners.get(id)!;return[id,workspace,session.model.state.connection.connectedHostId,session.model.state.ui.terminalWorkspaceRoots];})]);
  this.owners=owners;this.order=order;
  if(signature===this.fingerprint)return;
  this.fingerprint=signature;this.rows=order.map(id=>({...owners.get(id)!.workspace,id}));
  for(const listener of this.listeners)listener();
 }
 activate=async(workspace:Workspace)=>{
  const owner=this.owners.get(workspace.id);if(!owner)return;
  const pending=owner.session.model.activateWorkspace(owner.workspace);this.select(owner.session);await pending;
 };
 private async local(){
  const existing=this.sessions().find(localWorkspaceSession);if(existing)return existing;
  if(!this.localPending)this.localPending=this.hooks.local().then(session=>{this.add(session);return session;}).finally(()=>{this.localPending=undefined;});
  return this.localPending;
 }
 newWorkspace=async()=>{
  const local=await this.local();
  await waitForWorkspace(local.model);
  // The plus belongs to the window, never to the selected remote connection.
  const hostId=local.model.state.connection.connectedHostId;
  const parent=hostId
   ? local.model.state.workspaces.find(workspace=>workspace.id===local.model.state.activeWorkspace?.id&&workspace.host_id===hostId)
     ||local.model.state.workspaces.find(workspace=>workspace.host_id===hostId)
   : local.model.state.activeWorkspace;
  if(!parent)throw new Error("The local host has no available workspace");
  const pending=local.model.newWorkspace(parent);this.select(local);return await pending;
 };
 close=async(discard=false,id=this.activeId())=>{
  const owner=id&&this.owners.get(id);if(!owner)return false;
  const last=owner.session.model.windowWorkspaces().length===1;
  if(!last||localWorkspaceSession(owner.session))return owner.session.model.closeWorkspace(discard,owner.workspace.id);
  if(!await owner.session.model.closeWorkspace(discard,owner.workspace.id,false))return false;
  const key=this.key(owner.session)!;this.entries.get(key)!.unsubscribe();this.entries.delete(key);
  if(this.selected===owner.session)this.select(this.sessions()[0]||await this.local());else this.refresh();
  await this.hooks.dispose(owner.session);return true;
 };
 reorder=(dragId:string,targetId:string,position:"before"|"after")=>{
  if(dragId===targetId||!this.owners.has(dragId)||!this.owners.has(targetId))return false;
  const next=this.order.filter(id=>id!==dragId);next.splice(next.indexOf(targetId)+(position==="after"?1:0),0,dragId);
  this.order=next;this.refresh(true);return true;
 };
 dispose(){for(const entry of this.entries.values())entry.unsubscribe();this.listeners.clear();}
}

export async function waitForWorkspace(model:WorkspaceModel,timeoutMs=10000){
 if(model.state.activeWorkspace)return;
 await new Promise<void>((resolve,reject)=>{
  let unsubscribe=()=>{};
  const timer=setTimeout(()=>{unsubscribe();reject(new Error("The local workspace did not finish opening"));},timeoutMs);
  const check=()=>{if(model.state.activeWorkspace){clearTimeout(timer);unsubscribe();resolve();}};
  unsubscribe=model.subscribe(check);check();
 });
}
