import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

for(const width of [360,900])for(const fixture of [
 {role:"Assistant",text:"START A paragraph with **bold words**, `inline code`, and ordinary words that should copy as one logical paragraph regardless of how narrow the timeline becomes. END",expected:"START A paragraph with bold words, inline code, and ordinary words that should copy as one logical paragraph regardless of how narrow the timeline becomes. END"},
 {role:"Assistant",text:"START https://example.test/"+"verylongunbrokentoken".repeat(5)+" END"},
 {role:"Assistant",text:"START first paragraph with enough words to wrap across several visual rows without changing its clipboard structure.\n\nSecond paragraph also has several words that must stay together. END"},
 {role:"Assistant",text:"- START first item with enough words to wrap across several visual rows in a narrow timeline.\n- Second item stays separate from the first item and wraps correctly too. END",expected:"START first item with enough words to wrap across several visual rows in a narrow timeline.\nSecond item stays separate from the first item and wraps correctly too. END"},
 {role:"Assistant",text:"## START a heading long enough to wrap over multiple display rows END",expected:"START a heading long enough to wrap over multiple display rows END"},
 {role:"Assistant",text:"> START a quoted paragraph with enough words to wrap across several display rows without inserting clipboard line breaks END",expected:"START a quoted paragraph with enough words to wrap across several display rows without inserting clipboard line breaks END"},
 {role:"Assistant",text:"| START | A |\n| --- | --- |\n| B | END |",expected:"START\tA\nB\tEND"},
 {role:"Assistant",text:"```text\nSTART\n    indented code\n\n\n    preserve blank lines\nEND\n```",expected:"START\n    indented code\n\n\n    preserve blank lines\nEND"},
 {role:"User",text:"START prompt https://example.test/"+"longtoken".repeat(5)+"\n\nExplicit break END"},
 {role:"User",text:"START 界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界 e\u0301e\u0301e\u0301e\u0301 👩‍💻👩‍💻 END"},
].filter(fixture=>width>=700||!fixture.text.startsWith("|")))test(`${fixture.role} selection preserves logical text at width ${width}: ${fixture.text.slice(0,30)}`,async()=>{
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"clipboard-format",scale:1});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<any>(`agenttimeline.${op}`,{id,...args});
 try{
  call("event",{message:{HistoryChunk:{session_id:"clipboard-format",messages:[{id:"answer",role:fixture.role,kind:fixture.role,text:fixture.text}],next_cursor:null}}});
  call("layout",{width,height:1800});await Bun.sleep(30);const frame=call("frame");
  const text=frame.commands.filter((c:any)=>c.kind==="text"&&c.text);
  const first=text.find((c:any)=>c.text.startsWith("START")),last=text.findLast((c:any)=>c.text.endsWith("END"));
  expect(first).toBeDefined();expect(last).toBeDefined();
  const start={x:first.x,y:first.y+first.size/2},end={x:last.x+last.width-0.1,y:last.y+last.size/2};
  for(const reverse of [false,true]){
   call("pointer",{kind:"down",...(reverse?end:start)});
   call("pointer",{kind:"move",...(reverse?start:end)});
   const result=call("pointer",{kind:"up",...(reverse?start:end)});
   expect(result.copy).toBe(fixture.expected??fixture.text);
   expect(call("copy").text).toBe(result.copy);
  }
 }finally{call("dispose");}
});