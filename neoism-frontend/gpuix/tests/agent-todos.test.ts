import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

type Text={kind:string;text?:string;x:number;y:number};
type Frame={commands:Text[]};

test("todo snapshot and tool updates share one full checklist at the call position",async()=>{
 const sessionId="todo-disclosure";
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId});
 const event=(message:unknown)=>coreCall("agenttimeline.event",{id,message});
 const frame=()=>coreCall<Frame>("agenttimeline.frame",{id});
 const settle=async()=>{const until=Date.now()+400;let value=frame();while(Date.now()<until){await Bun.sleep(10);value=frame();}return value.commands.filter(command=>command.kind==="text");};
 const todos=[{content:"FINISHED_TASK",status:"completed"},{content:"ACTIVE_TASK",status:"in_progress"},{content:"PENDING_TASK",status:"pending"}];
 const message={id:"todo-call",role:"Assistant",kind:"Tool",tool:"todowrite",title:"Todos",status:"completed",todos,lang:"json"};
 try{
  event({HistoryChunk:{session_id:sessionId,messages:[{id:"user",role:"User",kind:"User",text:"Update the tasks"}],next_cursor:null}});
  event({TodoUpdate:{session_id:sessionId,todos}});
  event({MessageUpdated:{session_id:sessionId,message:{id:"between",role:"Assistant",kind:"Assistant",text:"Before the canonical call"}}});
  event({MessageUpdated:{session_id:sessionId,message}});
  coreCall("agenttimeline.layout",{id,width:1100,height:1200});
  let rows=await settle();
  const assertChecklist=()=>{
   expect(rows.filter(row=>row.text==="Todos")).toHaveLength(1);
   for(const todo of todos)expect(rows.filter(row=>row.text===todo.content)).toHaveLength(1);
   const before=rows.find(row=>row.text==="Before");
   expect(before,rows.map(row=>row.text).join("\n")).toBeDefined();
   expect(rows.find(row=>row.text==="Todos")!.y).toBeGreaterThan(before!.y);
   expect(rows.some(row=>row.text==="running")).toBe(false);
  };
  assertChecklist();
  event({TodoUpdate:{session_id:sessionId,todos}});
  event({MessageUpdated:{session_id:sessionId,message:{...message,todos:[]}}});
  rows=await settle();assertChecklist();
 }finally{coreCall("agenttimeline.dispose",{id});}
});

test.each([false,true])("empty sync is invisible and full checklists survive reloads (virtual=%s)",async(virtualTimeline)=>{
 const sessionId="todo-history";
 let {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId,virtualTimeline});
 const event=(message:unknown)=>coreCall("agenttimeline.event",{id,message});
 const frame=()=>coreCall<Frame>("agenttimeline.frame",{id});
 const settle=async()=>{const until=Date.now()+400;let value=frame();while(Date.now()<until){await Bun.sleep(10);value=frame();}return value.commands.filter(command=>command.kind==="text");};
 const user={id:"user",role:"User",kind:"User",text:"Read a file"};
 const todos=Array.from({length:14},(_,index)=>({content:`ChecklistItem${index}`,status:["pending","in_progress","completed"][index%3]}));
 const message={id:"plan",role:"Assistant",kind:"Tool",tool:"todowrite",title:"Todos",status:"completed",output_kind:"Todos",todos};
 try{
  event({HistoryChunk:{session_id:sessionId,messages:[user],next_cursor:null}});
  coreCall("agenttimeline.layout",{id,width:1100,height:1200});
  for(let index=0;index<3;index++){
   event({MessageUpdated:{session_id:sessionId,message:{id:`answer${index}`,role:"Assistant",kind:"Assistant",text:"Ordinary answer"}}});
   event({TodoUpdate:{session_id:sessionId,todos:[]}});
   const rows=await settle();
   expect(rows.some(row=>row.text==="Todos"||row.text==="todos updated")).toBe(false);
  }
  // Cold history uses the archived visibility path, not a live tool event.
  coreCall("agenttimeline.dispose",{id});
  id=coreCall<{id:number}>("agenttimeline.new",{sessionId,virtualTimeline}).id;
  event({HistoryChunk:{session_id:sessionId,messages:[user,message],next_cursor:null}});
  coreCall("agenttimeline.layout",{id,width:1100,height:1200});
  let rows=await settle();
  for(const todo of todos)expect(rows.filter(row=>row.text===todo.content)).toHaveLength(1);
  expect(rows.filter(row=>row.text==="Todos")).toHaveLength(1);
  expect(rows.some(row=>row.text==="✓")).toBe(true);
  expect(rows.some(row=>row.text==="●")).toBe(true);
  event({TodoUpdate:{session_id:sessionId,todos:[]}});
  rows=await settle();
  // Clearing current task state doesn't erase the historical checklist or paint a placeholder.
  expect(rows.filter(row=>row.text==="Todos")).toHaveLength(1);
  expect(rows.some(row=>row.text==="todos updated")).toBe(false);
  for(const todo of todos)expect(rows.filter(row=>row.text===todo.content)).toHaveLength(1);
 }finally{coreCall("agenttimeline.dispose",{id});}
});