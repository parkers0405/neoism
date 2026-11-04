import {expect,test} from "bun:test";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import type {HistoryMessage} from "../src/daemon/protocol";

function fixture(connected=false){
 const config=loadConfig(["--config","/nonexistent/late-transcript"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const sent:unknown[]=[];
 client.send=((_family,message)=>{sent.push(message);return sent.length;}) as typeof client.send;
 if(connected)client.state={phase:"connected",attempt:0};
 const events:Array<{message:any;kind?:string}>=[];
 model.subscribeAgentEvents((message,kind)=>events.push({message,kind}));
 const receive=(message:unknown)=>(model as any).receive({family:"Agent",requestId:1,message});
 const rows:HistoryMessage[]=[{id:"old-user",kind:"User",role:"User",text:"old",created_at:100},{id:"old-answer",kind:"Assistant",role:"Assistant",text:"finished",created_at:200},{id:"new-user",kind:"User",role:"User",text:"new",created_at:1000}];
 model.update({history:{chat:rows},streaming:{chat:"Generating"}});
 const update=(message:HistoryMessage)=>receive({MessageUpdated:{session_id:"chat",message,parent_message_id:"parent"}});
 return{model,events,sent,rows,receive,update,close:()=>{model.dispose();client.stop();}};
}

test("old tools absent from a long chat's loaded page never append or reach the live timeline",()=>{
 const f=fixture();
 try{
  for(const status of ["running","completed"]){
   f.update({id:"old-tool",kind:"Tool",role:"Assistant",tool:"bash",status,created_at:150});
   expect(f.model.state.history.chat).toEqual(f.rows);
   expect(f.events).toEqual([]);
   expect(f.model.state.streaming.chat).toBe("Generating");
  }
  const tool:HistoryMessage={id:"old-tool",kind:"Tool",role:"Assistant",tool:"bash",status:"completed",created_at:150};
  f.receive({HistoryChunk:{session_id:"chat",messages:[f.rows[0],tool,...f.rows.slice(1)],next_cursor:null}});
  expect(f.model.state.history.chat.map(row=>row.id)).toEqual(["old-user","old-tool","old-answer","new-user"]);
  expect(f.events.at(-1)?.kind).toBe("head");
 }finally{f.close();}
});

test("updates to retained historical parts keep their slot without opening live trace",()=>{
 const f=fixture();
 try{
  const tool:HistoryMessage={id:"tool",kind:"Tool",role:"Assistant",tool:"bash",status:"running",created_at:150};
  f.model.update({history:{chat:[f.rows[0],tool,...f.rows.slice(1)]}});
  f.update({...tool,status:"completed",detail:"result"});
  expect(f.model.state.history.chat.map(row=>row.id)).toEqual(["old-user","tool","old-answer","new-user"]);
  expect(f.model.state.history.chat[1].detail).toBe("result");
  expect(f.events).toHaveLength(1);
  expect(f.events[0].kind).toBe("head");
  expect(f.events[0].message.MessageUpdated).toBeUndefined();
 }finally{f.close();}
});

test("fresh tool snapshots stream immediately and repeated updates do not duplicate them",()=>{
 const f=fixture();
 try{
  const tool:HistoryMessage={id:"fresh-tool",kind:"Tool",role:"Assistant",tool:"bash",status:"running",created_at:1100};
  f.update(tool);f.update({...tool,status:"completed"});
  expect(f.model.state.history.chat.map(row=>row.id)).toEqual([...f.rows.map(row=>row.id),"fresh-tool"]);
  expect(f.events).toHaveLength(2);
  expect(f.events.every(event=>!!event.message.MessageUpdated)).toBe(true);
 }finally{f.close();}
});

test("undated unknown legacy tool snapshots require canonical positioning",()=>{
 const f=fixture();
 try{
  f.update({id:"undated-tool",kind:"Tool",role:"Assistant",tool:"bash",status:"completed",created_at:0});
  expect(f.model.state.history.chat).toEqual(f.rows);
  expect(f.events).toEqual([]);
 }finally{f.close();}
});

test("an optimistic undated prompt and a page starting mid-turn still fence old snapshots",()=>{
 const f=fixture();
 try{
  f.model.update({history:{chat:[f.rows[1],{...f.rows[2],created_at:undefined}]}});
  f.update({id:"old-tool",kind:"Tool",role:"Assistant",tool:"bash",status:"completed",created_at:150});
  expect(f.events).toEqual([]);
  f.model.update({history:{chat:[f.rows[1]]}});
  f.update({id:"old-tool",kind:"Tool",role:"Assistant",tool:"bash",status:"running",created_at:150});
  expect(f.events).toEqual([]);
 }finally{f.close();}
});

test("a delayed new tool within the same current turn remains live",()=>{
 const f=fixture();
 try{
  const first:HistoryMessage={id:"first",kind:"Tool",role:"Assistant",tool:"bash",status:"running",created_at:1200};
  f.update(first);
  f.update({...first,id:"parallel",created_at:1100});
  expect(f.events).toHaveLength(2);
  expect(f.events.every(event=>!!event.message.MessageUpdated)).toBe(true);
 }finally{f.close();}
});

test("a burst of old snapshots coalesces into one canonical history request",async()=>{
 const f=fixture(true);
 try{
  for(let i=0;i<100;i++)f.update({id:`old-${i}`,kind:"Tool",role:"Assistant",tool:"bash",status:"completed",created_at:150});
  await Bun.sleep(10);
  expect(f.sent).toEqual([{GetHistory:{session_id:"chat",cursor:null,limit:200}}]);
  expect(f.events).toEqual([]);
 }finally{f.close();}
});