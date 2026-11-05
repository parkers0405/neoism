import {afterEach,beforeEach,expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {settledTimelineFrame} from "./support/settled-timeline-frame";

type Text={kind:string;text?:string;x:number;y:number};
type Frame={commands:Text[];reason:string|null;preparing:boolean};

async function fixture(){
 let phase="create native timeline";
 const started=performance.now();
 const diagnostics=process.platform==="win32"||!!process.env.NEOISM_TEST_PHASES;
 const timings=new Map<string,{calls:number;ms:number;maxMs:number}>();
 let frameCount=0;
 const call=<T=unknown>(op:string,args:Record<string,unknown>)=>{
  const start=performance.now();
  try{return coreCall<T>(op,args);}finally{
   const key=`${phase}/${op}`,ms=performance.now()-start,previous=timings.get(key)||{calls:0,ms:0,maxMs:0};
   timings.set(key,{calls:previous.calls+1,ms:previous.ms+ms,maxMs:Math.max(previous.maxMs,ms)});
   if(op==="agenttimeline.frame")frameCount++;
  }
 };
 const summary=()=>JSON.stringify([...timings].map(([phase,value])=>({phase,calls:value.calls,ms:Math.round(value.ms),maxMs:Math.round(value.maxMs)})));
 const sessionId="composition-disclosure";
 if(diagnostics)console.error(`[composition] start ${phase}`);
 const {id}=call<{id:number}>("agenttimeline.new",{sessionId});
 const frame=()=>call<Frame>("agenttimeline.frame",{id});
 // Bound geometry/preparation polling, not decorative animations. The native
 // call is synchronous: its cold font/code preparation time is recorded too,
 // since a polling deadline cannot interrupt FFI or explain a runner timeout.
 const settle=async(label:string,ready:(value:Frame)=>boolean)=>{
  phase=label;
  const start=performance.now(),first=frameCount;
  if(diagnostics)console.error(`[composition] start ${phase}`);
  const value=await settledTimelineFrame(frame,{label,ready});
  const ms=Math.round(performance.now()-start);
  if(diagnostics||ms>=250)console.error(`[composition] ${phase}: ${ms}ms, frames=${frameCount-first}, preparing=${value.preparing}, reason=${value.reason}`);
  return value;
 };
 const texts=(value:Frame)=>value.commands.filter(command=>command.kind==="text");
 const text=(value:Frame)=>texts(value).map(command=>command.text).join("\n");
 const row=(value:Frame,label:string)=>{const found=texts(value).find(command=>command.text===label);expect(found,`Missing ${label} in ${text(value)}`).toBeDefined();return found!;};
 // Pointer dispatch still uses the real painted hit rectangles and native
 // toggle path. Coalesce down/up into the next settled paint via deferFrame;
 // neither intermediate full paint/JSON result is inspected by this test.
 const click=(value:Frame,label:string)=>{phase=`click ${label}`;const title=row(value,label),point={id,x:title.x+3,y:title.y+5,deferFrame:true};call("agenttimeline.pointer",{...point,kind:"down"});call("agenttimeline.pointer",{...point,kind:"up"});};
 const children=["src/alpha.rs","src/beta.rs"].map((path,index)=>({id:`compose/compose/read-${index}`,role:"Assistant",kind:"Tool",tool:"read",status:"completed",title:`Read(${path})`,text:"PRIVATE_SOURCE",detail:"PRIVATE_SOURCE"}));
 const message={id:"compose",role:"Assistant",kind:"Tool",tool:"compose",status:"completed",title:"Tool Composition(3 calls)",text:"",detail:JSON.stringify({neoismToolDetail:"composition",source:"return 42;",result:"PARENT_RESULT"}),composition_children:[...children,{id:"compose/compose/shell",role:"Assistant",kind:"Tool",tool:"bash",status:"completed",title:"Bash(echo child)",text:"CHILD_OUTPUT",detail:"CHILD_OUTPUT"}]};
 const has=(value:Frame,label:string)=>texts(value).some(command=>command.text===label);
 const dispose=()=>{
  call("agenttimeline.dispose",{id});
  if(diagnostics||performance.now()-started>=2000)console.error(`[composition] native phases: ${summary()}`);
 };
 try{
  // Only tool disclosure is under test. A user prompt adds an always-animated
  // presence orb and its paint commands to every frame without any assertion.
  call("agenttimeline.event",{id,message:{HistoryChunk:{session_id:sessionId,messages:[],next_cursor:null}}});
  call("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message}}});
  call("agenttimeline.layout",{id,width:1100,height:1200,deferFrame:true});
  const initial=await settle("initial inset geometry",value=>has(value,"Tool Composition  3 calls"));
  click(initial,"Read  src/alpha.rs");
  const read=await settle("read child disclosure",value=>has(value,"src/alpha.rs"));
  click(read,"Bash  echo child");
  const both=await settle("bash child disclosure",value=>text(value).includes("CHILD_OUTPUT"));
  return {initial,read,both,has,texts,text,row,click,settle,call,id,sessionId,message,children,dispose};
 }catch(error){
  console.error(`[composition] failed in ${phase} after ${Math.round(performance.now()-started)}ms: ${summary()}`);
  dispose();throw error;
 }
}

let view:Awaited<ReturnType<typeof fixture>>|undefined;
// Initial paint + first native read-detail preparation is fixture work, not
// part of the parent/collapse/update interaction budget. Capture actual frames
// from actual clicks; each test receives an independent timeline. Windows CI
// measured 18.5s in that first synchronous read frame on a cold runner; later
// fixtures take milliseconds. Keep convergence at 1s and test bodies at 5s.
beforeEach(async()=>{view=await fixture();},30000);
afterEach(()=>{view?.dispose();view=undefined;});

test("composition calls stay inset with independent child disclosures",()=>{
 const {initial:current,read,both,row,texts,text}=view!;
 const parent=row(current,"Tool Composition  3 calls");
 for(const label of ["Read  src/alpha.rs","Read  src/beta.rs","Bash  echo child"]){
  const child=row(current,label);expect(child.x-parent.x).toBeCloseTo(20,3);expect(child.y).toBeGreaterThan(parent.y);
  expect(texts(current).some(command=>command.text==="›"&&Math.abs(command.y-child.y)<3)).toBe(true);
  expect(texts(current).some(command=>command.x<child.x&&command.x>child.x-25&&Math.abs(command.y-child.y)<8)).toBe(true);
 }
 expect(text(current)).not.toContain("PARENT_RESULT");expect(text(current)).not.toContain("CHILD_OUTPUT");
 expect(texts(read).some(command=>command.text==="src/alpha.rs")).toBe(true);
 expect(texts(read).some(command=>command.text==="src/beta.rs")).toBe(false);
 expect(text(read)).not.toContain("PRIVATE_SOURCE");
 expect(text(both)).toContain("CHILD_OUTPUT");
});

test("composition parent disclosure and live updates preserve independent child identity",async()=>{
 const {both,has,texts,text,click,settle,call,id,sessionId,message,children}=view!;
 let current=both;
 click(current,"Tool Composition  3 calls");current=await settle("parent source/result disclosure",value=>text(value).includes("PARENT_RESULT"));expect(text(current)).toContain("PARENT_RESULT");
 click(current,"Tool Composition  3 calls");current=await settle("parent collapse retains children",value=>!text(value).includes("PARENT_RESULT")&&text(value).includes("CHILD_OUTPUT")&&has(value,"src/alpha.rs"));
 expect(text(current)).not.toContain("PARENT_RESULT");expect(text(current)).toContain("CHILD_OUTPUT");
 expect(texts(current).some(command=>command.text==="src/alpha.rs")).toBe(true);
 // Live updates retain each child's identity and expansion without adding roots.
 call("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message:{...message,composition_children:[...children,{...message.composition_children[2],text:"UPDATED_CHILD",detail:"UPDATED_CHILD"}]}}}});
 current=await settle("live child update",value=>text(value).includes("UPDATED_CHILD"));expect(text(current)).toContain("UPDATED_CHILD");
 expect(texts(current).filter(command=>command.text==="Read  src/alpha.rs")).toHaveLength(1);
});