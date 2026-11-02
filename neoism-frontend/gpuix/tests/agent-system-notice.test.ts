import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import type {HistoryMessage} from "../src/daemon/protocol";

test("slash-command system notices decode in live updates and history",()=>{
 const sessionId="system-notices";
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId});
 const message:HistoryMessage={id:"notice",role:"System",kind:"System",title:"Directory",tool:"location_notice",text:"Switched location to /project"};
 try{
  expect(()=>coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:sessionId,messages:[message],next_cursor:null}}})).not.toThrow();
  expect(()=>coreCall("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message:{...message,id:"live-notice",title:"Commands"}}}})).not.toThrow();
 }finally{coreCall("agenttimeline.dispose",{id});}
});