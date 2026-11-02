import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

test("another conversation's background jobs neither flicker this family nor poison its revision",()=>{
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"root",rootSessionId:"root"});
 const send=(session_id:string,revision:number,tasks:unknown[])=>coreCall("agenttimeline.event",{id,message:{BackgroundTasksUpdated:{session_id,epoch:"server",revision,tasks}}});
 const jobs=()=>coreCall<Array<{sessionId:string;backgroundTasks:number}>>("agenttimeline.diagnostics",{}).find(x=>x.sessionId==="root")!.backgroundTasks;
 try{
  send("root",1,[{jobId:"own",sessionId:"root",startedAt:100}]);expect(jobs()).toBe(1);
  send("unrelated",99,[]);expect(jobs()).toBe(1);
  send("unrelated",100,[{jobId:"foreign",sessionId:"unrelated",startedAt:100}]);expect(jobs()).toBe(1);
  send("root",2,[]);expect(jobs()).toBe(0);
  send("unrelated",101,[{jobId:"foreign",sessionId:"unrelated",startedAt:100}]);expect(jobs()).toBe(0);
 }finally{coreCall("agenttimeline.dispose",{id});}
});

test("transcript deltas do not invalidate the conversation catalog",()=>{
 const {id}=coreCall<{id:number}>("agentsidepanel.new",{});
 try{
  for(const message of [
   {ContentDelta:{session_id:"root",message_id:"answer",kind:"Text",text:"new token"}},
   {HistoryChunk:{session_id:"root",messages:[],next_cursor:null}},
  ])expect(coreCall<{needsPaint:boolean}>("agentsidepanel.event",{id,message}).needsPaint).toBe(false);
  expect(coreCall<{needsPaint:boolean}>("agentsidepanel.event",{id,message:{StreamingState:{session_id:"root",state:"Generating",label:null}}}).needsPaint).toBe(true);
 }finally{coreCall("agentsidepanel.dispose",{id});}
});
