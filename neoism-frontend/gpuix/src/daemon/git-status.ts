import {DaemonConnection} from "./client";
import {isRecord} from "./protocol";
export interface GitStatusSnapshot {workspace_root:string;repo_root:string|null;branch:string|null;upstream:string|null;files:unknown[];error?:string|null}
type Listener=(snapshot:GitStatusSnapshot,token:string)=>void;
interface Scope{listeners:Set<Listener>;dispose:()=>void}
interface Broker{scopes:Map<string,Scope>;cached:Map<string,{snapshot:GitStatusSnapshot;token:string}>}
const brokers=new WeakMap<DaemonConnection,Broker>();
function broker(client:DaemonConnection){let entry=brokers.get(client);if(!entry){entry={scopes:new Map(),cached:new Map()};brokers.set(client,entry);}return entry;}
/** Last successful host snapshot; transport gaps never pretend a repo vanished. */
export function cachedGitStatus(client:DaemonConnection,root:string){return brokers.get(client)?.cached.get(root)?.snapshot;}
/** The daemon supports one watched root per socket. All UI consumers of a
 * root therefore share one read-only status connection; another review root
 * gets its own scope without replacing the workspace's watch. */
export function watchGitStatus(client:DaemonConnection,root:string,listener:Listener){
 const owner=broker(client);let scope=owner.scopes.get(root);
 if(!scope){
  const connection=new DaemonConnection(client.config),token=`gpuix-git-status:${crypto.randomUUID()}`,listeners=new Set<Listener>();let closed=false;
  const events=connection.subscribe(event=>{
   if(event.family!=="Git"||!isRecord(event.message))return;const status=event.message.RepoStatus;
   if(!isRecord(status)||status.token!==token||!isRecord(status.snapshot))return;
   const value=status.snapshot;
   if(value.workspace_root!==root||!Array.isArray(value.files)||!(value.branch===null||typeof value.branch==="string")||!(value.repo_root===null||typeof value.repo_root==="string"))return;
   const snapshot=value as unknown as GitStatusSnapshot;
   if(!snapshot.error){owner.cached.delete(root);owner.cached.set(root,{snapshot,token});while(owner.cached.size>64)owner.cached.delete(owner.cached.keys().next().value!);}
   for(const receive of [...listeners])receive(snapshot,token);
  });
  const status=connection.subscribeStatus(state=>{if(state.phase==="connected")connection.send("Git",{WatchStatus:{token}},root);});
  const source=client.subscribeStatus(state=>{if(closed)return;if(state.phase==="connected")connection.start();else if(state.phase==="closed"||state.phase==="auth-rejected")connection.stop();});
  scope={listeners,dispose(){closed=true;source();events();status();if(connection.state.phase==="connected")try{connection.send("Git",{UnwatchStatus:{token}},root);}catch{}connection.stop();}};
  owner.scopes.set(root,scope);
 }
 scope.listeners.add(listener);const cached=owner.cached.get(root);if(cached)listener(cached.snapshot,cached.token);
 let disposed=false;return()=>{if(disposed)return;disposed=true;scope!.listeners.delete(listener);if(!scope!.listeners.size){scope!.dispose();if(owner.scopes.get(root)===scope)owner.scopes.delete(root);}};
}
