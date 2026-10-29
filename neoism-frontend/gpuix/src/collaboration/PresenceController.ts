import type {AppConfig} from "../config";
import type {DaemonConnection} from "../daemon/client";
import {isRecord,type Workspace} from "../daemon/protocol";
import {localPresenceIdentity,type LocalPresenceIdentity} from "../agent/local-presence";
import {localDaemon} from "../menu-actions/web-frontend";
import {coreCall} from "../native-core";

export interface PresenceCursor {line:number;column:number;offset?:number|null}
export interface PresenceSelection {anchor:PresenceCursor;head:PresenceCursor}
export interface PresencePeer {buffer_id:string;peer_id:string;display_name:string;client_os?:string|null;color:{r:number;g:number;b:number};cursor:PresenceCursor;selection?:PresenceSelection|null;insert:boolean;rainbow:boolean;updated_at_ms:number}
export interface PresenceAvatar {peer_id:string;display_name:string;client_os?:string|null;color:[number,number,number];rainbow:boolean}
interface Frame {peers:PresenceAvatar[];buffers:Record<string,PresencePeer[]>;send:unknown[];changed:boolean}
interface Focus {bufferId:string;cursor:PresenceCursor;selection:PresenceSelection|null;insert:boolean}
const empty=():Frame=>({peers:[],buffers:{},send:[],changed:false});
export function collaborativeWorkspace(config:Pick<AppConfig,"endpoint"|"collaborative">,workspace?:Workspace){return !!workspace&&((config.collaborative??!localDaemon(config.endpoint))||workspace.visibility==="shared"||workspace.visibility==="team");}
/** One canonical presence plane per connection/model, shared by all views.
 * Original Rust coalescing/heartbeats, identity, peer dedupe and TTL policy. */
export class PresenceController {
 readonly identity:LocalPresenceIdentity;
 private readonly id:number;
 private frame=empty();
 private workspace?:string;
 private enabled=false;
 private connected=false;
 private focus?:Focus;
 private listeners=new Set<()=>void>();
 private knownBuffers=new Map<string,Set<string>>();
 private timer?:ReturnType<typeof setInterval>;
 private cleanup:()=>void;
 private disposed=false;
 constructor(private readonly client:DaemonConnection,private readonly config:AppConfig,identity=localPresenceIdentity(config)){
  this.identity=identity;
  this.id=coreCall<{id:number}>("presence.new",{...identity,clientOS:process.platform==="win32"?"windows":process.platform==="darwin"?"macos":process.platform}).id;
  const events=client.subscribe(event=>{
   if(event.family!=="Crdt"||!this.enabled||!isRecord(event.message)||!("Presence"in event.message||"PresenceSnapshot"in event.message))return;
   this.apply(coreCall<Frame>("presence.event",{id:this.id,message:event.message}));
  });
  const status=client.subscribeStatus(state=>{
   const connected=state.phase==="connected";if(connected===this.connected)return;
   this.connected=connected;
   this.apply(coreCall<Frame>("presence.reset",{id:this.id}));
   if(connected&&this.enabled)this.snapshots();
   this.schedule();
  });
  this.cleanup=()=>{events();status();};
 }
 workspaceId=()=>this.workspace;
 isCollaborative=()=>this.enabled;
 bufferId=(path:string)=>coreCall<{bufferId:string}>("presence.buffer-id",{path}).bufferId;
 row=(bufferId:string,config:AppConfig)=>coreCall<{commands:import("../ui/PaintLayer").PaintCommand[];width:number;height:number}>("presence.row",{id:this.id,bufferId,scale:config.scale,theme:config.themeName,family:config.fontFamily,weight:config.fontWeight});
 peers=()=>this.frame.peers;
 filePeers=(bufferId:string)=>this.frame.buffers[bufferId]||[];
 subscribe=(listener:()=>void)=>{this.listeners.add(listener);return()=>{this.listeners.delete(listener);};};
 private apply(frame:Frame){
  const changed=frame.changed;this.frame=frame;
  if(this.workspace){let known=this.knownBuffers.get(this.workspace);if(!known){known=new Set();this.knownBuffers.set(this.workspace,known);}for(const bufferId of Object.keys(frame.buffers))known.add(bufferId);}
  if(this.connected)for(const message of frame.send){try{this.client.send("Crdt",message as never);}catch{break;}}
  if(changed)for(const listener of this.listeners)listener();
 }
 private color():[number,number,number]{
  const color=this.config.cursorColor||this.config.theme.accent;
  const match=/^#([\da-f]{6})$/i.exec(color||"");return match?[0,2,4].map(i=>parseInt(match[1].slice(i,i+2),16))as[number,number,number]:this.identity.color;
 }
 tick=(now=Date.now())=>{
  if(this.disposed)return;
  const active=this.enabled&&this.connected?(this.focus||{bufferId:"workspace://presence",cursor:{line:0,column:0},selection:null,insert:false}):null;
  this.apply(coreCall<Frame>("presence.tick",{id:this.id,now,active,color:this.color(),rainbow:this.config.cursorColor==="rainbow"}));
 };
 private schedule(){clearInterval(this.timer);this.timer=undefined;if(this.enabled&&this.connected){this.tick();this.timer=setInterval(this.tick,80);this.timer.unref?.();}}
 private snapshot(bufferId=this.focus?.bufferId||"workspace://presence"){
  if(this.enabled&&this.connected)try{this.client.send("Crdt",{RequestPresenceSnapshot:{buffer_id:bufferId,exclude_peer_id:this.identity.peerId}});}catch{}
 }
 private snapshots(){
  const buffers=new Set(["workspace://presence",...(this.workspace?this.knownBuffers.get(this.workspace)||[]:[])]);
  if(this.focus)buffers.add(this.focus.bufferId);
  for(const buffer of buffers)this.snapshot(buffer);
 }
 /** Call before changing the daemon workspace subscription. */
 leaveWorkspace(){this.enabled=false;this.tick();this.focus=undefined;this.workspace=undefined;this.apply(coreCall<Frame>("presence.reset",{id:this.id}));this.schedule();}
 /** Call only after the selected workspace subscription is acknowledged. */
 setWorkspace(workspace?:Workspace){
  const enabled=collaborativeWorkspace(this.config,workspace);
  if(this.workspace===workspace?.id&&this.enabled===enabled)return;
  if(this.workspace&&this.workspace!==workspace?.id)this.leaveWorkspace();
  const leaving=this.enabled&&!enabled;this.workspace=workspace?.id;this.enabled=enabled;
  if(leaving){this.tick();this.apply(coreCall<Frame>("presence.reset",{id:this.id}));}
  this.snapshots();
  this.schedule();
 }
 updateLocal(bufferId:string,cursor:PresenceCursor,selection:PresenceSelection|null=null,insert=false){
  const changed=this.focus?.bufferId!==bufferId;
  this.focus={bufferId,cursor,selection,insert};if(changed)this.snapshot(bufferId);this.tick();
 }
 clearLocal(bufferId?:string){if(bufferId&&this.focus?.bufferId!==bufferId)return;this.focus=undefined;this.tick();}
 dispose(){if(this.disposed)return;this.leaveWorkspace();this.disposed=true;clearInterval(this.timer);this.cleanup();this.knownBuffers.clear();this.listeners.clear();coreCall("presence.dispose",{id:this.id});}
}
