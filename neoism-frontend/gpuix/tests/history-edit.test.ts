import {expect,test} from "bun:test";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import type {HistoryMessage} from "../src/daemon/protocol";

const rows:HistoryMessage[]=[{id:"u1",kind:"User",role:"User",text:"first"},{id:"a1",kind:"Assistant",role:"Assistant",text:"one"},{id:"u2",kind:"User",role:"User",text:"second"},{id:"a2",kind:"Assistant",role:"Assistant",text:"two"}];
const deferred=()=>{let resolve!:()=>void,reject!:(error:Error)=>void;const promise=new Promise<void>((ok,no)=>{resolve=ok;reject=no;});return{promise,resolve,reject};};
function fixture(){
 const config=loadConfig(["--config","/nonexistent/optimistic-history"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const sent:any[]=[],events:any[]=[];client.send=((_family,body)=>{sent.push(body);return sent.length;})as typeof client.send;
 model.update({history:{chat:rows,other:[rows[0]]}});model.subscribeAgentEvents((message,kind)=>events.push({message,kind}));
 const page=(messages:readonly HistoryMessage[])=>(model as any).receive({family:"Agent",requestId:1,message:{HistoryChunk:{session_id:"chat",messages,next_cursor:null}}});
 return{model,sent,events,page,ids:()=>model.state.history.chat.map(row=>row.id),close:()=>{model.dispose();client.stop();}};
}
test("undo/undo/redo project immediately while server mutations run serially",async()=>{
 const f=fixture(),a=deferred(),b=deferred(),c=deferred(),calls:string[]=[];
 try{
  const first=f.model.editSessionHistory("chat","undo",()=>{calls.push("undo1");return a.promise;});
  expect(f.ids()).toEqual(["u1","a1"]);expect(f.events.at(-1).kind).toBe("replace");
  const second=f.model.editSessionHistory("chat","undo",()=>{calls.push("undo2");return b.promise;});expect(f.ids()).toEqual([]);
  const third=f.model.editSessionHistory("chat","redo",()=>{calls.push("redo");return c.promise;});expect(f.ids()).toEqual(["u1","a1"]);
  await Promise.resolve();await Promise.resolve();expect(calls).toEqual(["undo1"]);
  a.resolve();await first;await Promise.resolve();expect(calls).toEqual(["undo1","undo2"]);expect(f.ids()).toEqual(["u1","a1"]);
  b.resolve();await second;await Promise.resolve();expect(calls).toEqual(["undo1","undo2","redo"]);
  c.resolve();await third;
  expect(f.sent.filter(message=>message.GetHistory)).toHaveLength(1);
  f.page(rows.slice(0,2));
  const final=deferred(),redo=f.model.editSessionHistory("chat","redo",()=>final.promise);
  expect(f.ids()).toEqual(["u1","a1","u2","a2"]);
  final.resolve();await redo;expect(f.model.state.history.other).toEqual([rows[0]]);
 }finally{f.close();}
});
test("failed undo rebases queued edits and rolls the local transcript back",async()=>{
 const f=fixture(),a=deferred(),b=deferred();
 try{
  const first=f.model.editSessionHistory("chat","undo",()=>a.promise);
  const rejected=first.catch(error=>error);
  const second=f.model.editSessionHistory("chat","undo",()=>b.promise);expect(f.ids()).toEqual([]);
  a.reject(new Error("denied"));expect((await rejected).message).toBe("denied");expect(f.ids()).toEqual(["u1","a1"]);
  const rejectedSecond=second.catch(error=>error);b.reject(new Error("also denied"));expect((await rejectedSecond).message).toBe("also denied");
  expect(f.ids()).toEqual(rows.map(row=>row.id));expect(f.sent.filter(message=>message.GetHistory)).toHaveLength(1);
 }finally{f.close();}
});
test("a pre-undo history response cannot flash removed rows back into the optimistic timeline",async()=>{
 const f=fixture(),server=deferred();
 try{
  f.model.sendAgent({GetHistory:{session_id:"chat",cursor:null,limit:200}});
  const undo=f.model.editSessionHistory("chat","undo",()=>server.promise);
  f.page(rows);expect(f.ids()).toEqual(["u1","a1"]);
  server.resolve();await undo;await Promise.resolve();
  f.page(rows.slice(0,2));expect(f.ids()).toEqual(["u1","a1"]);
  expect(f.events.every(event=>event.message.HistoryChunk.messages.length===2)).toBe(true);
 }finally{f.close();}
});