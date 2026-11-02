import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

type Text={kind:string;text?:string;x:number;y:number};
type Frame={commands:Text[];reason:string|null;preparing:boolean};

test("composition calls stay inset with independent parent and child disclosures",async()=>{
 const sessionId="composition-disclosure";
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId});
 const frame=()=>coreCall<Frame>("agenttimeline.frame",{id});
 // Wait for actual native work, not six unconditional 400ms render loops. In
 // particular, serializing debug paint frames is expensive on Windows CI.
 const settle=async()=>{
  const deadline=Date.now()+1000;
  let value=frame();
  // Decorative animations (such as the user orb) intentionally never finish.
  const geometryMotion=new Set(["tool_motion","text_reveal","tool_expansion","timeline_inertia","timeline_interaction_settle"]);
  while(value.preparing||geometryMotion.has(value.reason||"")){
   if(Date.now()>=deadline)throw new Error(`Composition timeline did not settle: ${value.reason}`);
   await Bun.sleep(16);value=frame();
  }
  return value;
 };
 const texts=(value:Frame)=>value.commands.filter(command=>command.kind==="text");
 const text=(value:Frame)=>texts(value).map(command=>command.text).join("\n");
 const row=(value:Frame,label:string)=>{const found=texts(value).find(command=>command.text===label);expect(found,`Missing ${label} in ${text(value)}`).toBeDefined();return found!;};
 const click=(value:Frame,label:string)=>{const title=row(value,label),point={id,x:title.x+3,y:title.y+5};coreCall("agenttimeline.pointer",{...point,kind:"down"});coreCall("agenttimeline.pointer",{...point,kind:"up"});};
 const children=["src/alpha.rs","src/beta.rs"].map((path,index)=>({id:`compose/compose/read-${index}`,role:"Assistant",kind:"Tool",tool:"read",status:"completed",title:`Read(${path})`,text:"PRIVATE_SOURCE",detail:"PRIVATE_SOURCE"}));
 const message={id:"compose",role:"Assistant",kind:"Tool",tool:"compose",status:"completed",title:"Tool Composition(3 calls)",text:"",detail:JSON.stringify({neoismToolDetail:"composition",source:"return 42;",result:"PARENT_RESULT"}),composition_children:[...children,{id:"compose/compose/shell",role:"Assistant",kind:"Tool",tool:"bash",status:"completed",title:"Bash(echo child)",text:"CHILD_OUTPUT",detail:"CHILD_OUTPUT"}]};
 try{
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:sessionId,messages:[{id:"user",role:"User",kind:"User",text:"Run these tools"}],next_cursor:null}}});
  coreCall("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message}}});
  coreCall("agenttimeline.layout",{id,width:1100,height:1200});
  let current=await settle();
  const parent=row(current,"Tool Composition  3 calls");
  for(const label of ["Read  src/alpha.rs","Read  src/beta.rs","Bash  echo child"]){
   const child=row(current,label);expect(child.x-parent.x).toBeCloseTo(20,3);expect(child.y).toBeGreaterThan(parent.y);
   expect(texts(current).some(command=>command.text==="›"&&Math.abs(command.y-child.y)<3)).toBe(true);
   expect(texts(current).some(command=>command.x<child.x&&command.x>child.x-25&&Math.abs(command.y-child.y)<8)).toBe(true);
  }
  expect(text(current)).not.toContain("PARENT_RESULT");expect(text(current)).not.toContain("CHILD_OUTPUT");
  click(current,"Read  src/alpha.rs");current=await settle();
  expect(texts(current).some(command=>command.text==="src/alpha.rs")).toBe(true);
  expect(texts(current).some(command=>command.text==="src/beta.rs")).toBe(false);
  expect(text(current)).not.toContain("PRIVATE_SOURCE");
  click(current,"Bash  echo child");current=await settle();expect(text(current)).toContain("CHILD_OUTPUT");
  click(current,"Tool Composition  3 calls");current=await settle();expect(text(current)).toContain("PARENT_RESULT");
  click(current,"Tool Composition  3 calls");current=await settle();
  expect(text(current)).not.toContain("PARENT_RESULT");expect(text(current)).toContain("CHILD_OUTPUT");
  expect(texts(current).some(command=>command.text==="src/alpha.rs")).toBe(true);
  // Live updates retain each child's identity and expansion without adding roots.
  coreCall("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message:{...message,composition_children:[...children,{...message.composition_children[2],text:"UPDATED_CHILD",detail:"UPDATED_CHILD"}]}}}});
  current=await settle();expect(text(current)).toContain("UPDATED_CHILD");
  expect(texts(current).filter(command=>command.text==="Read  src/alpha.rs")).toHaveLength(1);
 }finally{coreCall("agenttimeline.dispose",{id});}
});