import * as Effect from "effect/Effect";
import type {AppConfig} from "../config";
import type {DaemonConnection} from "../daemon/client";
import {isRecord,type CrdtClientMessage,type CrdtPeerPresence,type Workspace} from "../daemon/protocol";
import {coreCall,NativeEditor} from "../native-core";
import {collaborativeWorkspace,type PresenceController} from "./PresenceController";
interface Binding {editor:NativeEditor;id:string;path:string;seeded:boolean;error?:string;views:Set<NativeEditor>;release:()=>void}
interface Drain {send:CrdtClientMessage[];seeded:boolean;bufferId:string}
/** Native Yrs owns edit/history semantics. This controller only routes opaque
 * updates to the authenticated daemon and keeps split views on one owner. */
export class DocumentController {
 private bindings=new Map<NativeEditor,Binding>();
 private owners=new Map<NativeEditor,NativeEditor>();
 private cleanup:()=>void;
 private disposed=false;
 private connected=false;
 private clientId=crypto.getRandomValues(new Uint32Array(1))[0]*1024;
 constructor(private readonly client:DaemonConnection,private readonly config:AppConfig,private readonly presence:PresenceController,private readonly report:(error:unknown)=>void){
  const events=client.subscribe(event=>{
   if(event.family!=="Crdt"||!isRecord(event.message))return;
   const [kind,body]=Object.entries(event.message)[0]||[];
   if(!isRecord(body))return;
   const payload=isRecord(body.envelope)?body.envelope:isRecord(body.update)?body.update:body;
   const id=payload.buffer_id;
   if(typeof id!=="string"||["Presence","PresenceSnapshot","CompactionStatus"].includes(kind))return;
   for(const binding of this.bindings.values())if(binding.id===id){
    if(kind==="Error"&&typeof body.message==="string"&&!body.message.includes("unknown CRDT buffer")){binding.error=body.message;this.report(new Error(body.message));continue;}
    if(kind==="Snapshot"||kind==="SnapshotFallback")binding.error=undefined;
    try{binding.editor.call("crdt.event",{message:event.message});this.flush(binding);}
    catch(error){this.report(error);try{this.client.send("Crdt",{RequestSnapshot:{buffer_id:id}});}catch{}}
   }
  });
  const status=client.subscribeStatus(state=>{
   const connected=state.phase==="connected";if(this.connected===connected)return;this.connected=connected;
   if(connected)for(const binding of this.bindings.values())try{this.ship(binding,coreCall<Drain>("editor.crdt.reconnect",{id:binding.editor.id}));}catch(error){this.report(error);}
  });
  const peers=presence.subscribe(()=>{for(const binding of this.bindings.values())this.paintPeers(binding);});
  this.cleanup=()=>{events();status();peers();};
 }
 bufferId(editor:NativeEditor){return this.bindings.get(this.owners.get(editor)||editor)?.id;}
 isBound(editor:NativeEditor){return this.bindings.get(this.owners.get(editor)||editor)?.seeded===true;}
 attach(editor:NativeEditor,path:string,workspace?:Workspace){
  if(this.disposed||this.bindings.has(editor)||!collaborativeWorkspace(this.config,workspace))return;
  try{
   const drain=coreCall<Drain>("editor.crdt.bind",{id:editor.id,path,clientId:++this.clientId});
   const binding:Binding={editor,id:drain.bufferId,path,seeded:drain.seeded,views:new Set(),release:()=>{}};
   this.bindings.set(editor,binding);binding.release=editor.subscribe(()=>this.flush(binding));this.ship(binding,drain);this.paintPeers(binding);
  }catch(error){this.report(error);}
 }
 attachView(view:NativeEditor,editor:NativeEditor){if(this.owners.get(view)===editor)return;const binding=this.bindings.get(editor);if(!binding)return;this.owners.set(view,editor);binding.views.add(view);this.paintPeers(binding);}
 rebind(editor:NativeEditor,path:string,workspace?:Workspace){
  this.detach(editor);editor.call("path",{path});this.attach(editor,path,workspace);
 }
 private ship(binding:Binding,drain:Drain){binding.seeded=drain.seeded;if(!this.connected)return;for(const message of drain.send)try{this.client.send("Crdt",message);}catch(error){this.report(error);break;}}
 private flush(binding:Binding){if(this.disposed)return;try{this.ship(binding,coreCall<Drain>("editor.crdt.flush",{id:binding.editor.id}));}catch(error){this.report(error);}}
 private paintPeers(binding:Binding){
  const peers=this.presence.filePeers(binding.id) as CrdtPeerPresence[];
  for(const editor of [binding.editor,...binding.views])try{editor.call("presence",{peers,fontFamily:this.config.fontFamily,fontWeight:this.config.fontWeight,fontSize:this.config.fontSize,theme:this.config.themeName});}catch(error){this.report(error);}
 }
 /** Saving is daemon-owned, after every queued local Yrs edit on this socket.
  * Request-id handling and ordering are the existing daemon transport's own. */
 async save(editor:NativeEditor){const binding=this.bindings.get(this.owners.get(editor)||editor);if(!binding)return false;
  const deadline=Date.now()+10000;
  while(!binding.seeded){if(binding.error)throw new Error(binding.error);if(!this.connected)throw new Error("Reconnect to the server before saving this shared document");if(this.disposed||!this.bindings.has(binding.editor))throw new Error("Shared document was closed");if(Date.now()>deadline)throw new Error("Shared document has not synchronized yet");await new Promise(resolve=>setTimeout(resolve,20));}
  this.flush(binding);
  if(!this.connected)throw new Error("Reconnect to the server before saving this shared document");
  const reply=await Effect.runPromise(this.client.request("Crdt",{SaveBuffer:{buffer_id:binding.id}}));
  if(!isRecord(reply)||!isRecord(reply.Saved))throw new Error(isRecord(reply)&&isRecord(reply.Error)?String(reply.Error.message):"Shared document save failed");
  return true;
 }
 detach(editor:NativeEditor){
  const owner=this.owners.get(editor);if(owner){this.bindings.get(owner)?.views.delete(editor);this.owners.delete(editor);return;}
  const binding=this.bindings.get(editor);if(!binding)return;binding.release();for(const view of binding.views)this.owners.delete(view);this.bindings.delete(editor);
 }
 dispose(){if(this.disposed)return;this.disposed=true;this.cleanup();for(const binding of this.bindings.values())binding.release();this.bindings.clear();this.owners.clear();}
}
