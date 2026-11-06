import * as Effect from "effect/Effect";
import type {AppConfig} from "../config";
import type {DaemonConnection} from "../daemon/client";
import {isRecord,type CrdtClientMessage,type CrdtPeerPresence,type Workspace} from "../daemon/protocol";
import {coreCall,NativeEditor} from "../native-core";
import type {PresenceController} from "./PresenceController";
interface Binding {editor:NativeEditor;id:string;path:string;ownerId:string;attached:boolean;recovery?:boolean;seeded:boolean;error?:string;views:Set<NativeEditor>;release:()=>void}
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
   if(typeof id!=="string"||["Presence","PresenceSnapshot","CompactionStatus","BufferClosed"].includes(kind))return;
    for(const binding of this.bindings.values())if(binding.id===id){
     // The AcquireBuffer snapshot is the ordering boundary for a fresh native
     // replica. Ignore broadcasts already in flight from its prior incarnation.
     if(!binding.seeded&&["Sync","Update","Saved"].includes(kind))continue;
    if(kind==="Error"&&typeof body.message==="string"&&!body.message.includes("unknown CRDT buffer")){binding.error=body.message;this.report(new Error(body.message));continue;}
    if(kind==="Snapshot"||kind==="SnapshotFallback")binding.error=undefined;
     try{binding.editor.call("crdt.event",{message:event.message});this.flush(binding);if(!binding.attached&&binding.views.size===0)this.close(binding);}
     catch(error){this.report(error);try{this.sendOwned(binding,{RequestSnapshot:{buffer_id:id}});}catch{}}
   }
  });
  const status=client.subscribeStatus(state=>{
   const connected=state.phase==="connected";if(this.connected===connected)return;this.connected=connected;
   if(connected)for(const binding of this.bindings.values())try{this.ship(binding,coreCall<Drain>("editor.crdt.reconnect",{id:binding.editor.id}));if(!binding.attached&&binding.views.size===0&&binding.seeded)this.close(binding);}catch(error){this.report(error);}
  });
  const peers=presence.subscribe(()=>{for(const binding of this.bindings.values())this.paintPeers(binding);});
  this.cleanup=()=>{events();status();peers();};
 }
 bufferId(editor:NativeEditor){return this.bindings.get(this.owners.get(editor)||editor)?.id;}
 isBound(editor:NativeEditor){return this.bindings.get(this.owners.get(editor)||editor)?.seeded===true;}
 attach(editor:NativeEditor,path:string,workspace?:Workspace){
  // Daemon document ownership also drives agent/disk refresh in private local
  // workspaces. Presence remains independently gated by PresenceController.
  if(this.disposed||this.bindings.has(editor)||!workspace)return;
  try{
   const drain=coreCall<Drain>("editor.crdt.bind",{id:editor.id,path,clientId:++this.clientId});
    const binding:Binding={editor,id:drain.bufferId,path,ownerId:crypto.randomUUID(),attached:true,seeded:drain.seeded,views:new Set(),release:()=>{}};
   this.bindings.set(editor,binding);binding.release=editor.subscribe(()=>this.flush(binding));this.ship(binding,drain);this.paintPeers(binding);
  }catch(error){this.report(error);}
 }
 attachView(view:NativeEditor,editor:NativeEditor){if(this.owners.get(view)===editor)return;const binding=this.bindings.get(editor);if(!binding)return;this.owners.set(view,editor);binding.views.add(view);this.paintPeers(binding);}
 rebind(editor:NativeEditor,path:string,workspace?:Workspace){
  this.detach(editor);editor.call("path",{path});this.attach(editor,path,workspace);
 }
 private acquire(binding:Binding,initialText=""){this.client.send("Crdt",{AcquireBuffer:{buffer_id:binding.id,owner_id:binding.ownerId,initial_text:initialText}});}
 private sendOwned(binding:Binding,message:CrdtClientMessage){this.client.send("Crdt",{Owned:{owner_id:binding.ownerId,message}});}
 private ship(binding:Binding,drain:Drain){binding.seeded=drain.seeded;if(!this.connected)return;for(const message of drain.send)try{if("OpenBuffer" in message)this.acquire(binding,message.OpenBuffer.initial_text);else this.sendOwned(binding,message);}catch(error){this.report(error);break;}}
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
   const reply=await Effect.runPromise(this.client.request("Crdt",{Owned:{owner_id:binding.ownerId,message:{SaveBuffer:{buffer_id:binding.id}}}}));
  if(!isRecord(reply)||!isRecord(reply.Saved))throw new Error(isRecord(reply)&&isRecord(reply.Error)?String(reply.Error.message):"Shared document save failed");
  return true;
 }
  detach(editor:NativeEditor){
   const owner=this.owners.get(editor);if(owner){const binding=this.bindings.get(owner);binding?.views.delete(editor);this.owners.delete(editor);if(binding&&!binding.attached&&binding.views.size===0)this.close(binding);return;}
   const binding=this.bindings.get(editor);if(!binding)return;binding.attached=false;if(binding.views.size===0)this.close(binding);
  }
  private close(binding:Binding){
   // Never authorize eviction after a failed flush or while offline. The
   // daemon retains that lease/recovery document instead of losing local work.
   // Reconnect's drain also replays unacknowledged native transactions: an
   // earlier send may have failed after taking them out of the outbound queue.
   // Yrs updates are idempotent. An unseeded/offline replica is not releasable.
   if(!this.connected||!binding.seeded){this.retain(binding);return;}
   try{const drain=coreCall<Drain>("editor.crdt.reconnect",{id:binding.editor.id});for(const message of drain.send){if("OpenBuffer" in message)this.acquire(binding,message.OpenBuffer.initial_text);else this.sendOwned(binding,message);}this.client.send("Crdt",{CloseBuffer:{buffer_id:binding.id,owner_id:binding.ownerId}});}
   catch(error){this.report(error);this.retain(binding);return;}
   binding.release();this.bindings.delete(binding.editor);if(binding.recovery)binding.editor.dispose();
  }
  private retain(binding:Binding){
   if(binding.recovery)return;
   // closeSurface disposes its native owner immediately after detach. Keep an
   // independent recovery owner while offline, awaiting the initial snapshot,
   // or retrying a failed send; otherwise those pending local bytes disappear.
   const previous=binding.editor;
   let recovery:NativeEditor|undefined;
   try{const snapshot=previous.snapshot();recovery=new NativeEditor(binding.path,"",false);recovery.restore(snapshot);}
   catch(error){recovery?.dispose();this.report(error);return;}
   binding.release();this.bindings.delete(previous);binding.editor=recovery;binding.recovery=true;
   this.bindings.set(recovery,binding);binding.release=recovery.subscribe(()=>this.flush(binding));
   if(this.connected)try{this.ship(binding,coreCall<Drain>("editor.crdt.reconnect",{id:recovery.id}));}catch(error){this.report(error);}
  }
 dispose(){if(this.disposed)return;this.disposed=true;this.cleanup();for(const binding of this.bindings.values()){binding.release();if(binding.recovery)binding.editor.dispose();}this.bindings.clear();this.owners.clear();}
}
