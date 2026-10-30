import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import type {PaintCommand} from "../src/ui/PaintLayer";

test("conversation text continues behind the floating composer instead of cutting off across the pane",()=>{
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"floating-composer",scale:1,fontFamily:"Cascadia Code NF"});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<{commands:PaintCommand[];animating:boolean}>(`agenttimeline.${op}`,{id,...args});
 try{
  call("event",{message:{HistoryChunk:{session_id:"floating-composer",messages:Array.from({length:40},(_,index)=>({id:`row-${index}`,role:"Assistant",kind:"Assistant",text:`Floating transcript row ${index}. ${"Conversation text continues below the composer edge. ".repeat(8)}`})),next_cursor:null}}});
  call("layout",{width:650,height:700,contentBottom:360});call("scroll",{deltaY:1e7,precise:true});
  let frame=call("frame");for(let i=0;i<250&&frame.animating;i++){Bun.sleepSync(5);frame=call("frame");}
  const below=frame.commands.filter(command=>command.kind==="text"&&command.y>380&&command.y<680);
  expect(below.length).toBeGreaterThan(0);
 }finally{call("dispose");}
},15000);

test("live Agent tables accept sideways natural trackpad input and keep a draggable scrollbar above composer",()=>{
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"table-scroll",scale:1,fontFamily:"Cascadia Code NF"});
 type Frame={commands:PaintCommand[];animating:boolean};
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<Frame>(`agenttimeline.${op}`,{id,...args});
 const state=()=>coreCall<Array<{sessionId:string;offset:number}>>("agenttimeline.diagnostics",{}).find(view=>view.sessionId==="table-scroll")!;
 const settle=()=>{let frame=call("frame");for(let i=0;i<250&&frame.animating;i++){Bun.sleepSync(5);frame=call("frame");}return frame;};
 const marker=(frame:Frame)=>frame.commands.find((command):command is Extract<PaintCommand,{kind:"text"}>=>command.kind==="text"&&command.text.includes("ROW00"));
 try{
  const table=["| Column A | Column B | Column C |","| --- | --- | --- |",...Array.from({length:45},(_,i)=>`| ROW${String(i).padStart(2,"0")} ${"first-wide-cell ".repeat(8)} | ${"second-wide-cell ".repeat(8)} | ${"third-wide-cell ".repeat(8)} |`)].join("\n");
  call("event",{message:{HistoryChunk:{session_id:"table-scroll",messages:[{id:"table",role:"Assistant",kind:"Assistant",text:table}],next_cursor:null}}});
  call("layout",{width:650,height:700,contentBottom:360});call("scroll",{deltaY:1e7,precise:true});let frame=settle();
  const initial=marker(frame);expect(initial).toBeDefined();const pointer={x:Math.max(60,initial!.x+20),y:initial!.y+10};
  call("pointer",{kind:"move",...pointer});frame=call("frame");
  const scrollOffset=state().offset;
  // A negative delivered horizontal delta advances the overflow viewport, as
  // in the original Rust natural-scroll adapter. Diagonal drift is consumed.
  frame=call("scroll",{deltaX:-30,deltaY:2,precise:true,...pointer});
  expect(marker(frame)!.x).toBeLessThan(initial!.x-20);expect(state().offset).toBe(scrollOffset);
  frame=call("scroll",{deltaX:30,deltaY:0,precise:true,...pointer});expect(marker(frame)!.x).toBeCloseTo(initial!.x,1);
  // A tall table continues below the composer, but its grab surface follows
  // the visible bottom. Hover there and exercise the actual native hit path.
  frame=call("pointer",{kind:"move",x:pointer.x,y:350});
  const thumbs=frame.commands.filter((command):command is Extract<PaintCommand,{kind:"rect"}>=>command.kind==="rect"&&command.rect[1]>340&&command.rect[1]+command.rect[3]<=360&&command.radii.some(radius=>radius>0));
  expect(thumbs.length).toBeGreaterThan(0);
  call("pointer",{kind:"down",x:pointer.x,y:350});call("pointer",{kind:"move",x:pointer.x+45,y:350});frame=call("pointer",{kind:"up",x:pointer.x+45,y:350});
  expect(marker(frame)?.x??-1000).toBeLessThan(initial!.x-1);expect(state().offset).toBe(scrollOffset);
  // Reverse at the edge consumes the horizontal gesture without scrolling chat.
  call("scroll",{deltaX:1e6,deltaY:2,precise:true,...pointer});call("scroll",{deltaX:1e6,deltaY:2,precise:true,...pointer});expect(state().offset).toBe(scrollOffset);
 }finally{call("dispose");}
},15000);
