import {expect,test} from "bun:test";
import {toArrayBuffer,type Pointer} from "bun:ffi";
import {coreCall} from "../src/native-core";
import type {CanvasRaster} from "../src/ui/canvas";

type Frame={commands:Array<{kind:string;text?:string;raster?:CanvasRaster}>};
const providers=["opencode","claude","codex","neoism"] as const;
const marks=(frame:Frame)=>frame.commands.filter(command=>command.kind==="image").map(command=>{
 const raster=command.raster!;
 return `${raster.width}x${raster.height}:${Bun.hash(toArrayBuffer(raster.ptr as unknown as Pointer,0,raster.len))}`;
});
const history=(agent:string)=>({id:`task-${agent}`,role:"Assistant",kind:"Tool",tool:"task",status:"completed",agent_kind:agent,
 title:`Task(@${agent} · Review parser)`,text:"Finished review",detail:`task_id: child-${agent}\nstatus: completed\nReview passed`});

test("timeline and left tree paint the same four saved agent logos, including Neoism",async()=>{
 const timeline=coreCall<{id:number}>("agenttimeline.new",{sessionId:"branding",scale:1}).id;
 const tree=coreCall<{id:number}>("agentsidepanel.new",{directory:"/branding",theme:"pastel_dark",scale:1}).id;
 try{
  for(const agent of providers)coreCall("agenttimeline.event",{id:timeline,message:{MessageUpdated:{session_id:"branding",message:history(agent)}}});
  coreCall("agenttimeline.layout",{id:timeline,width:900,height:600});
  coreCall("agentsidepanel.frame",{id:tree,width:300,height:600});
  coreCall("agentsidepanel.event",{id:tree,message:{ThreadList:{threads:[{session_id:"branding",title:"Orchestration",directory:"/branding",updated_at:1}],requested_cursor:null,next_cursor:null}}});
  coreCall("agentsidepanel.event",{id:tree,message:{CatalogFamily:{session_id:"branding",snapshot:{directory:"/branding",executionId:"branding-run",familyRevision:1,
   branches:providers.map(agent=>({sessionId:`child-${agent}`,parentSessionId:"branding",agent:agent==="neoism"?"general":agent,title:`Review parser with ${agent}`,status:"outstanding",depth:1}))}}}});
  await Bun.sleep(450);
  const timelineFrame=coreCall<Frame>("agenttimeline.frame",{id:timeline});
  const treeFrame=coreCall<Frame>("agentsidepanel.frame",{id:tree,width:300,height:600});
  const timelineMarks=marks(timelineFrame),treeMarks=marks(treeFrame);
  expect(timelineMarks).toHaveLength(4);
  expect(new Set(timelineMarks).size).toBe(4);
  expect(treeMarks).toHaveLength(5);
  expect(new Set(treeMarks)).toEqual(new Set(timelineMarks));
  const labels=treeFrame.commands.filter(command=>command.kind==="text").map(command=>command.text);
  for(const label of ["OpenCode","Claude Code","Codex","Neoism"])expect(labels).toContain(label);
 }finally{coreCall("agenttimeline.dispose",{id:timeline});coreCall("agentsidepanel.dispose",{id:tree});}
});

test("task logos survive running, completion, error, and a continuation history reload",async()=>{
 for(const agent of providers){
  let logo:string|undefined;
  for(const status of ["running","completed","error"]){
   const sessionId=`branding-${agent}-${status}`,id=coreCall<{id:number}>("agenttimeline.new",{sessionId}).id;
   try{
    coreCall("agenttimeline.event",{id,message:{MessageUpdated:{session_id:sessionId,message:{...history(agent),status:"running"}}}});
    const message={...history(agent),status,...(status==="completed"?{title:`Follow-up(@${agent} · Review parser)`}:{})};
    if(status!=="running")coreCall("agenttimeline.event",{id,message:{ToolUseResult:{session_id:sessionId,tool_use_id:message.id,tool:"task",status:status==="error"?"Failed":"Completed",output:`task_id: child-${agent}\nstatus: ${status}`,error:status==="error"?"Fixture failure":null}}});
    coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:sessionId,messages:[message],next_cursor:null}}});
    coreCall("agenttimeline.layout",{id,width:900,height:600});
    coreCall("agenttimeline.frame",{id});
    await Bun.sleep(450);
    const frame=coreCall<Frame>("agenttimeline.frame",{id}),images=marks(frame);
    expect(images).toHaveLength(1);
    logo??=images[0];expect(images[0]).toBe(logo);
    // Phase captions can disappear as headers compact; their markers remain.
    expect(frame.commands.filter(command=>command.kind==="text").map(command=>command.text)).toContain(status==="completed"?"↳":status==="running"?"●":"error");
   }finally{coreCall("agenttimeline.dispose",{id});}
  }
 }
},20000);