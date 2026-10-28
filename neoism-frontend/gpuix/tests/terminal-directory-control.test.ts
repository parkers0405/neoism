import {expect,test} from "bun:test";
import {Effect} from "effect";
import type {DaemonConnection} from "../src/daemon/client";
import type {WireEvent,ConnectionState} from "../src/daemon/protocol";
import {TerminalDirectoryControl} from "../src/terminal/directory-control";
import {terminalSession,disposeTerminalSessions} from "../src/terminal/sessions";
import {palettes} from "../src/design/generated";
function fixture(){
 const events=new Set<(event:WireEvent)=>void>(),statuses=new Set<(state:ConnectionState)=>void>(),sent:Array<{family:string;message:any;outer:number}>=[];let outer=10;
 const client={state:{phase:"connected"},send:(family:string,message:unknown)=>{sent.push({family,message,outer:++outer});return outer;},request:(_family:string,message:any)=>Effect.succeed({PtyCreated:{session_id:message.AttachPty.session_id}}),subscribe:(fn:(event:WireEvent)=>void)=>{events.add(fn);return()=>events.delete(fn);},subscribeStatus:(fn:(state:ConnectionState)=>void)=>{statuses.add(fn);fn({phase:"connected",attempt:0});return()=>statuses.delete(fn);}} as unknown as DaemonConnection;
 return{client,sent,emit(message:unknown,requestId=0){for(const listener of events)listener({family:"Pty",requestId,message});}};
}
const until=async(predicate:()=>boolean)=>{const end=Date.now()+500;while(!predicate()){if(Date.now()>end)throw new Error("Control request did not arrive");await Bun.sleep(1);}};
test("only exact host confirmation releases this terminal's user gate; VT replies and other shells stay live",async()=>{
 const f=fixture(),errors:string[]=[],session=terminalSession(f.client,"one",palettes.pastel_dark,error=>errors.push((error as Error).message)),other=terminalSession(f.client,"two",palettes.pastel_dark,()=>{});
 try{
  const pending=session.changeDirectory("/requested");await until(()=>f.sent.some(entry=>entry.message.ChangeTerminalDirectory));
  const request=f.sent.find(entry=>entry.message.ChangeTerminalDirectory)!,body=request.message.ChangeTerminalDirectory;
  session.send(new TextEncoder().encode("x"));other.send(new TextEncoder().encode("y"));await session.buffer.write(new TextEncoder().encode("\x1b[6n"));
  expect(errors).toHaveLength(1);expect(f.sent.some(entry=>entry.message.PtyInput?.session_id==="one"&&entry.message.PtyInput.bytes[0]===120)).toBe(false);
  expect(f.sent.some(entry=>entry.message.PtyInput?.session_id==="two")).toBe(true);expect(f.sent.some(entry=>entry.message.PtyInput?.session_id==="one"&&entry.message.PtyInput.bytes[0]===27)).toBe(true);
  f.emit({TerminalDirectoryChanged:{session_id:"one",request_id:"stale",directory:"/wrong"}});f.emit("Ack",request.outer+100);expect(session.directoryControl.blocked).toBe(true);
  f.emit({TerminalDirectoryChanged:{session_id:"one",request_id:body.request_id,directory:"/canonical"}});expect(await pending).toBe("/canonical");expect(session.directoryControl.blocked).toBe(false);
  session.send(new TextEncoder().encode("z"));expect(f.sent.at(-1)!.message).toEqual({PtyInput:{session_id:"one",bytes:[122]}});
  expect(f.sent.some(entry=>entry.message.PtyInput?.bytes.some((byte:number)=>byte===99))).toBe(false);
 }finally{disposeTerminalSessions(f.client);}
});
test("generic Ack is unsupported, and uncertain timeout sends exact cancellation while retaining its input gate",async()=>{
 const f=fixture(),control=new TerminalDirectoryControl(f.client,"one",()=>undefined,5);
 try{
  const first=control.request("/target");await until(()=>f.sent.some(entry=>entry.message.ChangeTerminalDirectory));const request=f.sent.at(-1)!;
  f.emit("Ack",request.outer);await expect(first).rejects.toThrow("does not support");expect(control.blocked).toBe(false);
  const second=control.request("/later");const rejection=expect(second).rejects.toThrow("remains paused");await rejection;
  const cancel=f.sent.at(-1)!.message.ChangeTerminalDirectory;expect(cancel.directory).toBe("");expect(control.blocked).toBe(true);
  f.emit({TerminalDirectoryChangeFailed:{session_id:"one",request_id:cancel.request_id,message:"Cancelled before dispatch"}});expect(control.blocked).toBe(false);
 }finally{control.dispose();}
});
