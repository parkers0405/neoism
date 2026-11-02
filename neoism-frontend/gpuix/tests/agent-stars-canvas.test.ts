import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {paintOps,type PaintCommand,type PreparedCanvas} from "../src/ui/PaintLayer";
import {CanvasImages} from "../src/ui/canvas";
import {loadConfig} from "../src/config";

test("direct star canvas retains the original primitives at each animation phase",()=>{
 const {id}=coreCall<{id:number}>("agentcomposer.new",{}),config=loadConfig([],{},{});
 const images=new CanvasImages({});
 try{for(const seconds of [0,.125,1,123.75]){
  const args={id,width:752,height:102,radius:16,scale:1,seconds};
  const old=coreCall<{commands:PaintCommand[]}>("agentcomposer.stars-frame",args);
  const next=coreCall<{canvas:PreparedCanvas;animating:boolean}>("agentcomposer.stars-frame",{...args,canvasPaint:true});
  expect(next.animating).toBe(true);
  const normalize=(ops:any[])=>ops.map(op=>({...op,b:op.b?.map(Math.fround),r:op.r?.map(Math.fround)}));
  expect(normalize(JSON.parse(next.canvas.commands))).toEqual(normalize(paintOps(old.commands,config,images)));
 }}finally{images.dispose();coreCall("agentcomposer.dispose",{id});}
});

test("retained timeline chunks preserve global paint order and inherited clips",()=>{
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"chunk-test",canvasPaint:true,chunkedPaint:true});
 const config=loadConfig([],{},{}),images=new CanvasImages({});
 const table="| Item | Value | Notes |\n|---|---|---|\n"+Array.from({length:40},(_,i)=>`| Row ${i} | **${i*17}** | wrapped value with detail ${i} |`).join("\n");
 const flatten=(chunks:any[][])=>chunks.flatMap(ops=>{let clip:unknown;const out:any[]=[];for(const op of ops){if(op.k==="cl")clip=op.b;else if(op.k==="u")clip=undefined;else out.push({...op,clip});}return out;});
 const normalize=(v:any):any=>typeof v==="number"?Math.fround(v):Array.isArray(v)?v.map(normalize):v&&typeof v==="object"?Object.fromEntries(Object.entries(v).map(([k,x])=>[k,normalize(x)])):v;
 try{
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"chunk-test",messages:[{id:"a",kind:"Assistant",role:"Assistant",text:table}],next_cursor:null}}});
  coreCall("agenttimeline.layout",{id,width:800,height:700});
  const frame=coreCall<{commands:PaintCommand[];canvas:PreparedCanvas}>("agenttimeline.frame",{id,includeCommands:true});
  expect(frame.canvas.chunks!.length).toBeGreaterThan(1);
  expect(normalize(flatten(frame.canvas.chunks!.map(s=>JSON.parse(s))))).toEqual(normalize(flatten([paintOps(frame.commands,config,images)])));
 }finally{images.dispose();coreCall("agenttimeline.dispose",{id});}
});
