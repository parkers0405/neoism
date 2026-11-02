import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

type Text={kind:string;text?:string;x:number;y:number};
type Frame={commands:Text[]};

test("read and grep accordions list files without dumping tool output",async()=>{
 const sessionId="compact-tool-summary";
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId});
 const frame=()=>coreCall<Frame>("agenttimeline.frame",{id});
 const settle=async()=>{const until=Date.now()+400;let value=frame();while(Date.now()<until){await Bun.sleep(10);value=frame();}return value;};
 const text=(value:Frame)=>value.commands.filter(command=>command.kind==="text").map(command=>command.text).join("\n");
 const open=(value:Frame,label:string)=>{
  const title=value.commands.find(command=>command.kind==="text"&&command.text?.includes(label));
  expect(title).toBeDefined();
  coreCall("agenttimeline.pointer",{id,kind:"down",x:title!.x+3,y:title!.y+5});
  coreCall("agenttimeline.pointer",{id,kind:"up",x:title!.x+3,y:title!.y+5});
 };
 try{
  const messages=["src/a.rs","src/a.rs","src/b.rs","src/c.rs","src/d.rs","src/e.rs","src/f.rs"].map((path,i)=>({
   id:`read-${i}`,role:"Assistant",kind:"Tool",tool:"read",status:"completed",title:`Read(${path})`,text:"SECRET_SOURCE",detail:"SECRET_SOURCE",
  }));
  messages.push({id:"grep",role:"Assistant",kind:"Tool",tool:"grep",status:"completed",title:"Grep(needle)",text:"2 matches",detail:"Grep: Found 2 matches in 2 files (this page)\nsrc/first.rs:\n  Line 2: SECRET_MATCH\nsrc/second.rs:\n  Line 5: SECRET_MATCH"});
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:sessionId,messages:[{id:"prompt",role:"User",kind:"User",text:"Inspect these files"}],next_cursor:null}}});
  for(const message of messages)coreCall("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message}}});
  coreCall("agenttimeline.layout",{id,width:1000,height:900});
  let current=await settle();
  expect(text(current)).toContain("Read: 6 files");
  expect(text(current)).toContain("Grep: 2 files");
  expect(text(current)).not.toContain("src/f.rs");
  open(current,"Read: 6 files");current=await settle();
  expect(text(current)).toContain("src/f.rs");
  expect(text(current)).not.toContain("SECRET_SOURCE");
  open(current,"Grep: 2 files");current=await settle();
  expect(text(current)).toContain("src/first.rs");
  expect(text(current)).toContain("src/second.rs");
  expect(text(current)).not.toContain("SECRET_MATCH");
 }finally{coreCall("agenttimeline.dispose",{id});}
});