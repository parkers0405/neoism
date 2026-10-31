import * as Effect from "effect/Effect";
import type {DaemonConnection} from "../daemon/client";
import {isRecord,variant} from "../daemon/protocol";
import {absoluteTerminalDirectory} from "./workspace";

interface Pending {
 id:string;outer?:number;resolve:(directory:string)=>void;reject:(error:Error)=>void;
 timer?:ReturnType<typeof setTimeout>;attach:AbortController;
}
/** Explicit host control, never shell text. A late/uncertain host operation
 * keeps this one terminal's user input paused until its correlated result. */
export class TerminalDirectoryControl {
 private pending?:Pending;
 private disposed=false;
 private cleanups:Array<()=>void>=[];
 constructor(private client:DaemonConnection,private sessionId:string,private cursor:()=>number|undefined,private timeoutMs=15_000){
  this.cleanups.push(client.subscribe(event=>{
   const pending=this.pending;if(!pending||event.family!=="Pty")return;
   const entry=variant(event.message);if(!entry)return;const[tag,body]=entry;
   if(isRecord(body)&&body.session_id===sessionId&&body.request_id===pending.id){
    if(tag==="TerminalDirectoryChanged"){
     if(!absoluteTerminalDirectory(body.directory)||/[\x00-\x1f\x7f]/.test(body.directory))this.finish(new Error("The host returned an invalid terminal directory"));
     else this.finish(undefined,body.directory);
    }else if(tag==="TerminalDirectoryChangeFailed")this.finish(new Error(typeof body.message==="string"?body.message:"The host could not change terminal directory"));
   }else if(tag==="PtyClosed"&&isRecord(body)&&body.session_id===sessionId)this.finish(new Error("Terminal exited before confirming its directory"));
   else if(event.requestId===pending.outer&&(tag==="Ack"||tag==="Error"))this.finish(new Error(tag==="Error"&&isRecord(body)&&typeof body.message==="string"?body.message:"This daemon does not support host-confirmed terminal directory changes"));
  }));
  this.cleanups.push(client.subscribeStatus(state=>{if(state.phase!=="connected")this.finish(new Error("Terminal connection changed before confirming its directory"));}));
 }
 get blocked(){return !!this.pending;}
 request(directory:string):Promise<string>{
  if(this.disposed)return Promise.reject(new Error("Terminal is no longer available"));
  if(this.pending)return Promise.reject(new Error("This terminal is still waiting for its directory change"));
  if(!absoluteTerminalDirectory(directory)||/[\x00-\x1f\x7f]/.test(directory))return Promise.reject(new Error("Choose an absolute directory without control characters"));
  return new Promise((resolve,reject)=>{
   const pending:Pending={id:crypto.randomUUID(),resolve,reject,attach:new AbortController()};this.pending=pending;
   void(async()=>{
    // Existing sessions may be hidden/unmounted. Validate and attach this
    // connection's own reply route; an Ack is never an attachment proof.
    const cursor=this.cursor();
    const reply=variant(await Effect.runPromise(this.client.request("Pty",{AttachPty:{session_id:this.sessionId,...(cursor===undefined?{}:{cursor})}}),{signal:pending.attach.signal}));
    if(this.pending!==pending)return;
    if(reply?.[0]!=="PtyCreated"||!isRecord(reply[1])||reply[1].session_id!==this.sessionId)throw new Error("The host did not validate this terminal attachment");
    pending.outer=this.client.send("Pty",{ChangeTerminalDirectory:{session_id:this.sessionId,directory,request_id:pending.id}});
    pending.timer=setTimeout(()=>{
     if(this.pending!==pending)return;
     // Cancellation can retract a queued request, never an already-dispatched
     // cd. Keep the gate until the host says which case actually completed.
     try{this.client.send("Pty",{ChangeTerminalDirectory:{session_id:this.sessionId,directory:"",request_id:pending.id}});}catch{}
     pending.reject(new Error("Waiting for the host to confirm or cancel the terminal directory change; input to this terminal remains paused"));
    },this.timeoutMs);
   })().catch(error=>{if(this.pending===pending)this.finish(error instanceof Error?error:new Error("Could not reach terminal directory control"));});
  });
 }
 private finish(error?:Error,directory?:string){
  const pending=this.pending;if(!pending)return;this.pending=undefined;clearTimeout(pending.timer);pending.attach.abort();
  if(error)pending.reject(error);else pending.resolve(directory!);
 }
 dispose(){
  if(this.disposed)return;this.disposed=true;
  if(this.pending?.outer!==undefined&&this.client.state.phase==="connected")try{this.client.send("Pty",{ChangeTerminalDirectory:{session_id:this.sessionId,directory:"",request_id:this.pending.id}});}catch{}
  this.finish(new Error("Terminal directory control was closed"));for(const cleanup of this.cleanups.splice(0))cleanup();
 }
}
