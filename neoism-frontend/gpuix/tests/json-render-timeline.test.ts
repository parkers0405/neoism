import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {jsonRenderExample} from "../src/agent/json-render-catalog";

interface Frame {commands:Array<{kind:string;text?:string;size?:number;y?:number;color?:number[];points?:number[][]}>}
const source=JSON.stringify(jsonRenderExample,null,2);
const body=(frame:Frame)=>frame.commands.filter(command=>command.kind==="text").map(command=>command.text).join(" ").replace(/\s+/g," ");
// The stroke is one continuous opaque path; translucent paths are the fill.
const chartStroke=(frame:Frame)=>frame.commands.filter(command=>command.kind==="polygon"&&command.color?.[3]===1);
function timeline(text:string,scale=1){
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"json-ui",scale,reducedMotion:true});
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"json-ui",messages:[{id:"answer",role:"Assistant",kind:"Assistant",text}],next_cursor:null}}});
  return id;
}

test("json-render catalog example paints native chat widgets instead of JSON, including narrow layouts",()=>{
  for(const [width,scale] of [[900,1],[360,1],[520,1.5]]){
    const id=timeline("```json-render\n"+source+"\n```\n\nAfter the native card.",scale);
    try{
      const frame=coreCall<Frame>("agenttimeline.layout",{id,width,height:2200,scale,reducedMotion:true});
      const text=body(frame);
      for(const label of ["Team performance","Sample data","Weekly revenue","$12,400","+18%","Revenue trend","Mon","Sun","Deals closed","Retention","After the native card."])expect(text).toContain(label);
      expect(text).not.toContain('"elements"');
      expect(text).not.toContain('"props"');
      expect(frame.commands.some(command=>command.kind==="text"&&command.text?.includes("$12,400")&&(command.size||0)>16*scale)).toBe(true);
       expect(chartStroke(frame).length).toBe(1);
       expect(chartStroke(frame)[0].points!.length).toBeGreaterThan(100);
      expect(chartStroke(frame).every(command=>command.points?.flat().every(Number.isFinite))).toBe(true);
      const progress=frame.commands.find(command=>command.kind==="text"&&command.text?.includes("Retention"));
      const following=frame.commands.find(command=>command.kind==="text"&&command.text?.includes("After"));
      expect(following!.y!).toBeGreaterThan(progress!.y!);
    }finally{coreCall("agenttimeline.dispose",{id});}
  }
});

test("streamed closing fence promotes the same message from source to native UI",()=>{
  const id=timeline("```json-render\n"+source+"\n");
  try{
    const pending=coreCall<Frame>("agenttimeline.layout",{id,width:900,height:3000,reducedMotion:true});
    expect(body(pending)).toContain('"elements"');
    coreCall("agenttimeline.event",{id,message:{ContentDelta:{session_id:"json-ui",message_id:"answer",kind:"Text",text:"```"}}});
    const complete=coreCall<Frame>("agenttimeline.frame",{id});
    expect(body(complete)).toContain("Team performance");
    expect(body(complete)).toContain("+18%");
     expect(chartStroke(complete).length).toBe(1);
     expect(chartStroke(complete)[0].points!.length).toBeGreaterThan(100);
    expect(body(complete)).not.toContain('"elements"');
  }finally{coreCall("agenttimeline.dispose",{id});}
});

test("ordinary JSON and invalid native specs retain a readable source fallback",()=>{
  for(const [language,json] of [
    ["json",source],
    ["json-render",JSON.stringify({root:"r",elements:{r:{type:"Button",props:{label:"Run",action:"bash"}}}},null,2)],
    ["json-render",JSON.stringify({root:"r",elements:{r:{type:"Stack",props:{},children:["r"]}}},null,2)],
    ["json-render",JSON.stringify({root:"r",elements:{r:{type:"Text",props:{text:{$state:"secret"}}}}},null,2)],
    ["json-render",JSON.stringify({root:"r",elements:{r:{type:"LineGraph",props:{data:[{label:"Mon",value:1}]}}}},null,2)],
    ["json-render",JSON.stringify({root:"r",elements:{r:{type:"Separator",props:{},children:["t"]},t:{type:"Text",props:{text:"Not allowed"}}}},null,2)],
  ]){
    const id=timeline("```"+language+"\n"+json+"\n```");
    try{
      const frame=coreCall<Frame>("agenttimeline.layout",{id,width:900,height:3000,reducedMotion:true});
      expect(body(frame)).toContain('"elements"');
    }finally{coreCall("agenttimeline.dispose",{id});}
  }
});