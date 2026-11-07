import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {timelineReceivesEvent} from "../src/agent/timeline-event-routing";

type Frame={commands:Array<{kind:string;text?:string;x?:number;y?:number;size?:number}>};
const body=(frame:Frame)=>frame.commands.filter(c=>c.kind==="text").map(c=>c.text).join(" ");

// Actual SDK part envelopes, not prebuilt Tool timeline rows. Exercise both
// the selected member and its parked cache before any child completes.
test("raw child parts stream text and inspectable tool bodies across family switches",()=>{
 const{id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"root",scale:1,fontFamily:"Cascadia Code NF",reducedMotion:true});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<Frame>(`agenttimeline.${op}`,{id,...args});
 const roster=["native","codex","opencode","claude"].map(agent=>({id:agent,title:"Uninformative title",agent,depth:1,status:"running"}));
 const select=(sessionId:string)=>call("family",{rootSessionId:"root",sessionId,roster});
 const raw=(session_id:string,kind:string,properties:unknown)=>call("event",{message:{SessionEvent:{session_id,kind,properties}}});
 const frame=()=>{call("layout",{width:1000,height:1200,reducedMotion:true,deferFrame:true});return call("frame");};
 try{
  select("root");
  for(const {id:child}of roster){
   raw(child,"message.part.updated",{part:{id:`${child}-text`,messageID:`${child}-message`,sessionID:child,type:"text",text:"BeforeSwitchToken"}});
   raw(child,"message.part.delta",{sessionID:child,messageID:`${child}-message`,partID:`${child}-text`,partType:"text",field:"text",delta:" CachedDeltaToken"});
   raw(child,"message.part.updated",{part:{id:`${child}-tool`,messageID:`${child}-tools`,sessionID:child,type:"tool",tool:"bash",callID:`${child}-call`,state:{status:"running",title:"InspectLiveTool",input:{command:"printf LiveCommandToken"},time:{start:1000}}}});
   select(child);
   expect(body(frame())).toContain("BeforeSwitchToken");expect(body(frame())).toContain("CachedDeltaToken");
   raw(child,"message.part.delta",{sessionID:child,messageID:`${child}-message`,partID:`${child}-text`,partType:"text",field:"text",delta:" ViewedDeltaToken"});
   // A running tool snapshot can carry incremental output before completion.
   raw(child,"message.part.updated",{part:{id:`${child}-tool`,messageID:`${child}-tools`,sessionID:child,type:"tool",tool:"bash",callID:`${child}-call`,metadata:{terminalOutput:"LiveToolBodyToken"},state:{status:"running",title:"InspectLiveTool",input:{command:"printf LiveCommandToken"},time:{start:1000}}}});
   let painted=frame();expect(body(painted)).toContain("ViewedDeltaToken");expect(body(painted)).toContain("LiveCommandToken");
   const header=painted.commands.find(c=>c.kind==="text"&&c.text?.includes("LiveCommandToken"));expect(header).toBeDefined();
   const point={x:header!.x!+3,y:header!.y!+(header!.size||14)/2};call("pointer",{kind:"down",...point});call("pointer",{kind:"up",...point});
   painted=frame();expect(body(painted)).toContain("LiveToolBodyToken");
   select("root");select(child);expect(body(frame())).toContain("LiveToolBodyToken");select("root");
  }
 }finally{call("dispose");}
});

test("late child history shows only the unfinished turn's trace",()=>{
 const{id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"root",scale:1,reducedMotion:true});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<Frame>(`agenttimeline.${op}`,{id,...args});
 try{
  call("family",{rootSessionId:"root",sessionId:"root",roster:[{id:"child",depth:1,status:"running"}]});
  const tool=(id:string,status:string)=>({id,kind:"Tool",role:"Assistant",tool:"bash",title:id,status,detail:id});
  call("event",{message:{HistoryChunk:{session_id:"child",next_cursor:null,messages:[{id:"old-user",kind:"User",role:"User",text:"Old turn"},tool("ArchivedToolToken","completed"),{id:"new-user",kind:"User",role:"User",text:"Current turn"},tool("InFlightToolToken","running")]}}});
  call("family",{rootSessionId:"root",sessionId:"child",roster:[{id:"child",depth:1,status:"running"}]});
  const painted=call("layout",{width:1000,height:1200,reducedMotion:true});
  expect(body(painted)).toContain("InFlightToolToken");expect(body(painted)).not.toContain("ArchivedToolToken");
 }finally{call("dispose");}
});

test("raw child parts do not leak into another known family engine",()=>{
 const event={SessionEvent:{session_id:"child",kind:"message.part.delta",properties:{field:"text",delta:"token"}}};
 const ui={agentFamilyRoots:{child:"root"}};
 expect(timelineReceivesEvent("other",ui,new Set(["root","other"]),event)).toBe(false);
 expect(timelineReceivesEvent("root",ui,new Set(["root","other"]),event)).toBe(true);
});