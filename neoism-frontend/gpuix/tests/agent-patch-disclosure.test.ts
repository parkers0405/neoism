import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

type Text={kind:string;text?:string;x:number;y:number};
type Frame={commands:Text[]};

test("completed patch shows file cards with independently disclosed diffs",async()=>{
 const sessionId="patch-disclosure";
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId});
 const frame=()=>coreCall<Frame>("agenttimeline.frame",{id});
 const settle=async()=>{const until=Date.now()+400;let value=frame();while(Date.now()<until){await Bun.sleep(10);value=frame();}return value;};
 const text=(value:Frame)=>value.commands.filter(command=>command.kind==="text").map(command=>command.text).join("\n");
 const click=(value:Frame,label:string)=>{
  const title=value.commands.find(command=>command.kind==="text"&&command.text===label);
  expect(title).toBeDefined();
  const point={id,x:title!.x+3,y:title!.y+5};
  coreCall("agenttimeline.pointer",{...point,kind:"down"});
  coreCall("agenttimeline.pointer",{...point,kind:"up"});
 };
 try{
  const patchText="*** Begin Patch\n*** Update File: src/alpha.txt\n@@\n-BEFORE_ALPHA\n-BEFORE_SECOND\n+AFTER_ALPHA\n+AFTER_SECOND\n*** Add File: src/beta.txt\n+NEW_BETA\n*** End Patch";
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:sessionId,messages:[{id:"user",role:"User",kind:"User",text:"Apply these changes"}],next_cursor:null}}});
  const message={id:"patch",role:"Assistant",kind:"Tool",tool:"apply_patch",title:"ApplyPatch",status:"completed",text:"Applied",detail:JSON.stringify({neoismToolDetail:"edit",tool:"apply_patch",input:{patchText}})};
  coreCall("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message}}});
  coreCall("agenttimeline.layout",{id,width:1100,height:900});
  let current=await settle();
  expect(text(current)).toContain("Apply patch");
  expect(text(current)).toContain("+3");
  expect(text(current)).toContain("-2");
   expect(text(current)).toContain("alpha.txt");
  expect(text(current)).toContain("beta.txt");
  expect(text(current)).not.toContain("AFTER_ALPHA");
  click(current,"alpha.txt");current=await settle();
  expect(text(current)).toContain("AFTER_ALPHA");
  expect(text(current)).not.toContain("NEW_BETA");
   click(current,"beta.txt");current=await settle();
   expect(text(current)).toContain("NEW_BETA");
   expect(text(current)).toContain("AFTER_ALPHA");
   click(current,"alpha.txt");current=await settle();
   expect(text(current)).toContain("alpha.txt");
   expect(text(current)).not.toContain("AFTER_ALPHA");
   expect(text(current)).toContain("NEW_BETA");
   click(current,"beta.txt");current=await settle();
   expect(text(current)).not.toContain("NEW_BETA");
   expect(text(current)).toContain("beta.txt");
 }finally{coreCall("agenttimeline.dispose",{id});}
});