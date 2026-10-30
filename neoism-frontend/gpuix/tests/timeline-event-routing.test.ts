import {expect,test} from "bun:test";
import {timelineReceivesEvent} from "../src/agent/timeline-event-routing";
import {noteAgentFamily} from "../src/agent/family";

test("large history is delivered to its owning open family without reading or copying its payload",()=>{
 const roots=new Set(["first","second","third"]);
 const body={session_id:"second",get messages(){throw Error("routing must not traverse history");}};
 expect([...roots].filter(root=>timelineReceivesEvent(root,{},roots,{HistoryChunk:body}))).toEqual(["second"]);
 for(const tag of ["ContentDelta","MessageUpdated","MessageEnd"]){
  expect([...roots].filter(root=>timelineReceivesEvent(root,{},roots,{[tag]:{session_id:"second"}}))).toEqual(["second"]);
 }
});

test("nested and completed children keep reaching their family when another conversation is visible",()=>{
 const roots=new Set(["parent","other"]);
 let ui=noteAgentFamily({}, {CatalogFamily:{session_id:"parent",snapshot:{branches:[
  {sessionId:"child",parentSessionId:"parent",status:"running"},
  {sessionId:"nested",parentSessionId:"child",status:"running"},
 ]}}})!;
 const message={ContentDelta:{session_id:"nested",text:"still streaming"}};
 expect(timelineReceivesEvent("parent",ui,roots,message)).toBe(true);
 expect(timelineReceivesEvent("other",ui,roots,message)).toBe(false);
 ui=noteAgentFamily(ui,{CatalogFamily:{session_id:"parent",snapshot:{branches:[]}}})!;
 expect(timelineReceivesEvent("parent",ui,roots,{HistoryChunk:{session_id:"nested"}})).toBe(true);
 expect(timelineReceivesEvent("other",ui,roots,{HistoryChunk:{session_id:"nested"}})).toBe(false);
});

test("unknown-family transcripts and shared lifecycle events retain their delivery",()=>{
 const roots=new Set(["parent","other"]);
 for(const root of roots){
  expect(timelineReceivesEvent(root,{},roots,{ContentDelta:{session_id:"not-yet-catalogued"}})).toBe(true);
  for(const message of [{SessionIdle:{session_id:"other"}},{MessageStart:{session_id:"other"}},
   {CatalogFamily:{session_id:"other",snapshot:{branches:[]}}},{Notice:{message:"reconnected"}},
   {HistoryChunk:{messages:[]}},null])expect(timelineReceivesEvent(root,{},roots,message)).toBe(true);
 }
});
