import{expect,test}from"bun:test";
import{coreCall}from"../src/native-core";
type Frame={send:Array<Record<string,any>>};
test("home catalog discovery and large baseline events do not multiply frontend requests; original project pump keeps three in flight",()=>{
 const{id}=coreCall<{id:number}>("agentsidepanel.new",{directory:process.env.HOME||"/",theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF"});const frame=()=>coreCall<Frame>("agentsidepanel.frame",{id,width:300,height:600});const event=(message:unknown)=>coreCall("agentsidepanel.event",{id,message});
 try{const discovery=frame().send;expect(discovery).toHaveLength(1);expect(discovery[0].ListSessionProjects).toBeDefined();for(let i=0;i<30;i++)expect(frame().send).toEqual([]);const request=discovery[0].ListSessionProjects;
  event({SessionProjects:{...request,items:Array.from({length:825},(_,i)=>({directory:`/fixture/project-${String(i).padStart(4,"0")}`,updatedMs:825-i})),pinnedSessions:[]}});const first=frame().send;expect(first.length).toBeLessThanOrEqual(3);expect(first.length).toBeGreaterThan(0);expect(first.every(message=>!!message.ListProjectSessions)).toBe(true);
  // A large legitimate server baseline consists of family/activity updates,
  // rather than UI-triggered root-list requests. It must not feed back into
  // another discovery or another project page while those three are pending.
  for(let i=0;i<825;i++){event({CatalogFamily:{session_id:`root-${i}`,snapshot:{directory:`/fixture/project-${String(i).padStart(4,"0")}`,executionId:null,familyRevision:1,branches:[]}}});event({CatalogActivity:{session_id:`root-${i}`,activity:"idle"}});}
  for(let i=0;i<30;i++)expect(frame().send).toEqual([]);
 }finally{coreCall("agentsidepanel.dispose",{id});}
});
test("original timeline frame replay does not resend subscriptions or history for baseline and cached-session updates",()=>{
 const{id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"root",rootSessionId:"root",theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF"});try{coreCall("agenttimeline.family",{id,rootSessionId:"root",sessionId:"root",roster:[]});for(let i=0;i<200;i++)coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:`other-${i}`,messages:[],next_cursor:null}}});for(let i=0;i<30;i++){const frame=coreCall<Frame>("agenttimeline.frame",{id});expect(frame.send).toEqual([]);}}finally{coreCall("agenttimeline.dispose",{id});}
});
