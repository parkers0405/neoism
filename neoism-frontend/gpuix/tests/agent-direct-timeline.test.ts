import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

test("direct timeline preserves visible text and scroll metrics without a second transcript projection",()=>{
 const ids=[false,true].map(virtualTimeline=>coreCall<{id:number}>("agenttimeline.new",{sessionId:"direct-parity",virtualTimeline}).id);
 const messages=Array.from({length:200},(_,i)=>({id:`message-${i}`,role:"Assistant",kind:"Assistant",text:`Message ${i}\n\n| Item | Value |\n|---|---|\n| **${i}** | sample |\n\n\`\`\`ts\nconst value = ${i};\n\`\`\``}));
 const frame=(id:number)=>coreCall<{commands:Array<{kind:string;text?:string;x?:number;y?:number}>}>("agenttimeline.frame",{id});
 const ink=(id:number)=>frame(id).commands.filter(c=>c.kind==="text"&&c.y!>=0&&c.y!<600).map(c=>[c.text,c.x,c.y]);
 try{
  for(const id of ids){
   coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"direct-parity",messages,next_cursor:null}}});
   coreCall("agenttimeline.layout",{id,width:900,height:600});
  }
  for(const offset of [0,350,1200,5000,100,0]){
   for(const id of ids)coreCall("agenttimeline.restore-position",{id,offset,follow:offset===0});
   expect(ink(ids[0]!)).toEqual(ink(ids[1]!));
  }
  const diagnostics=coreCall<Array<{sessionId:string;visibleRange:unknown;scrollbar:unknown}>>("agenttimeline.diagnostics",{}).filter(x=>x.sessionId==="direct-parity");
  expect(diagnostics.some(x=>x.visibleRange===null)).toBe(true);
  expect(diagnostics.some(x=>x.visibleRange!==null)).toBe(true);
  expect(diagnostics[0]!.scrollbar).toEqual(diagnostics[1]!.scrollbar);
 }finally{for(const id of ids)coreCall("agenttimeline.dispose",{id});}
});

test("appending complete Markdown sections matches a fresh layout across cached prefix seams",()=>{
 const ids=[0,1].map(()=>coreCall<{id:number}>("agenttimeline.new",{sessionId:"prefix-parity"}).id);
 const section=(i:number)=>`## Section ${i}\n\n${"A paragraph with **bold** and *italic* words. ".repeat(12)}\n\n| Item | Value |\n|---|---|\n| row ${i} | **${i}** |\n\n\`\`\`rust\nlet value = ${i};\n\n// a blank inside the fence\n\`\`\`\n\n`;
 const event=(id:number,message:unknown)=>coreCall("agenttimeline.event",{id,message});
 const frame=(id:number)=>coreCall<{commands:Array<{kind:string;text?:string;x?:number;y?:number}>}>("agenttimeline.frame",{id});
 try{
  let text=section(0).repeat(4);
  event(ids[0]!,{HistoryChunk:{session_id:"prefix-parity",messages:[{id:"answer",role:"Assistant",kind:"Assistant",text}],next_cursor:null}});
  for(const id of ids)coreCall("agenttimeline.layout",{id,width:900,height:600});
  for(let i=1;i<=8;i++){
   const delta=section(i);text+=delta;
   event(ids[0]!,{ContentDelta:{session_id:"prefix-parity",message_id:"answer",kind:"Text",text:delta}});frame(ids[0]!);
  }
  event(ids[1]!,{HistoryChunk:{session_id:"prefix-parity",messages:[{id:"answer",role:"Assistant",kind:"Assistant",text}],next_cursor:null}});frame(ids[1]!);
  event(ids[0]!,{SessionIdle:{session_id:"prefix-parity"}});
  // Compare settled ink: live reveal splits glyph runs and the activity row
  // deliberately retains its height for 400ms after authoritative idle.
  Bun.sleepSync(450);
  for(const id of ids)frame(id);
  for(const offset of [0,700,2000,4500]){
   for(const id of ids)coreCall("agenttimeline.restore-position",{id,offset,follow:false});
   const ink=ids.map(id=>frame(id).commands.filter(c=>c.kind==="text"&&c.y!>=0&&c.y!<600).map(c=>[c.text,c.x,c.y]));
   expect(ink[0]).toEqual(ink[1]);
  }
 }finally{for(const id of ids)coreCall("agenttimeline.dispose",{id});}
});
