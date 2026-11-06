import {fileURLToPath} from "node:url";
import {coreCall,NativeEditor} from "../../src/native-core";
import * as Effect from "effect/Effect";
import type {DaemonConnection} from "../../src/daemon/client";

/** In-memory daemon authority using the real native Yrs binding, not canned
 * save acknowledgements. No fixture path is ever opened on the host. */
export function createCrdtFixture(read:(path:string)=>string|undefined,write:(path:string,text:string)=>void){
 const documents=new Map<string,{editor:NativeEditor;path:string}>();let actor=4000;
 const handle=(message:any,reply:(message:any)=>void)=>{
  const spec=message.Owned?.message||message;
  if(spec.AcquireBuffer){const {buffer_id,initial_text}=spec.AcquireBuffer;
   if(!documents.has(buffer_id)){
    const path=fileURLToPath(buffer_id),editor=new NativeEditor(path,"",false);
    coreCall("editor.crdt.bind",{id:editor.id,path,clientId:++actor});editor.call("crdt.event",{message:{Snapshot:{buffer_id,update_v1:[0,0],state_vector_v1:[0]}}});
    editor.call("paste",{text:read(path)??initial_text??""});editor.call("saved");documents.set(buffer_id,{editor,path});
   }
   const document=(documents.get(buffer_id)!.editor.snapshot() as any).collaboration.document;
   reply({Snapshot:{buffer_id,...document}});return;
  }
  if(spec.ApplySync){const envelope=spec.ApplySync.envelope,entry=documents.get(envelope.buffer_id);if(!entry)throw Error("Unknown fixture document");entry.editor.call("crdt.event",{message:{Sync:{envelope}}});reply({Sync:{envelope}});return;}
  if(spec.SaveBuffer){const {buffer_id}=spec.SaveBuffer,entry=documents.get(buffer_id);if(!entry)throw Error("Unknown fixture document");const text=entry.editor.text();write(entry.path,text);reply({Saved:{buffer_id,bytes_written:new TextEncoder().encode(text).length}});return;}
  if(spec.RequestSnapshot){const {buffer_id}=spec.RequestSnapshot,entry=documents.get(buffer_id);if(entry)reply({Snapshot:{buffer_id,...(entry.editor.snapshot() as any).collaboration.document}});return;}
  if(spec.CloseBuffer){const entry=documents.get(spec.CloseBuffer.buffer_id);entry?.editor.dispose();documents.delete(spec.CloseBuffer.buffer_id);}
 };
 return{handle,dispose(){for(const {editor}of documents.values())editor.dispose();documents.clear();}};
}

/** Attach the same authority to isolated Files-only transports. Install before
 * creating the model, so document status and event subscriptions are real. */
export function installCrdtFixture(client:DaemonConnection,read:(path:string)=>string|undefined,write:(path:string,text:string)=>void){
 const documents=createCrdtFixture(read,write),listeners=new Set<(event:any)=>void>();
 const subscribe=client.subscribe.bind(client),status=client.subscribeStatus.bind(client),send=client.send.bind(client),request=client.request.bind(client);
 client.subscribe=(listener=>{listeners.add(listener);const release=subscribe(listener);return()=>{listeners.delete(listener);release();};}) as typeof client.subscribe;
 client.subscribeStatus=(listener=>status(state=>listener({...state,phase:"connected"}))) as typeof client.subscribeStatus;
 const emit=(message:any)=>queueMicrotask(()=>{for(const listener of listeners)listener({family:"Crdt",requestId:0,message});});
 client.send=((family:any,message:any,root?:string)=>family==="Crdt"?documents.handle(message,emit):send(family,message,root)) as typeof client.send;
 client.request=((family:any,message:any,root?:string)=>family==="Crdt"?Effect.sync(()=>{let reply:any;documents.handle(message,value=>{reply=value;emit(value);});return reply;}):request(family,message,root)) as typeof client.request;
 return documents;
}