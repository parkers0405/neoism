import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import type {JsonUiFrame,JsonUiControl,JsonUiDiagramTransform} from "../src/agent/JsonRenderControls";
import {jsonRenderDiagramExample} from "../src/agent/json-render-catalog";

interface Frame {commands:Array<{kind:string;text?:string;y?:number}>;jsonUi?:JsonUiFrame;jsonUiAccepted?:boolean;send?:unknown[]}
const body=(frame:Frame)=>frame.commands.filter(command=>command.kind==="text").map(command=>command.text).join(" ").replace(/\s+/g," ");
const fence=(elements:Record<string,unknown>,root="root")=>"```json-render\n"+JSON.stringify({root,elements})+"\n```";
const noSubmit=(frame:Frame)=>{expect(Object.hasOwn(frame.jsonUi||{},"action")).toBe(false);expect(frame.send||[]).toEqual([]);};
function engine(messages:Array<{id:string;text:string}>,height=2400){
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"chat-a",scale:1,reducedMotion:true});
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"chat-a",messages:messages.map(message=>({...message,role:"Assistant",kind:"Assistant"})),next_cursor:null}}});
  let current=coreCall<Frame>("agenttimeline.layout",{id,width:900,height,contentBottom:height,reducedMotion:true});
  return {id,get frame(){return current;},control(label:string,kind?:JsonUiControl["kind"]){const control=current.jsonUi?.controls.find(control=>control.label===label&&(!kind||control.kind===kind));expect(control).toBeDefined();return control!;},
    event(control:string,kind:string,value?:unknown){current=coreCall<Frame>("agenttimeline.json-ui",{id,control,kind,value});return current;},
    call(op:string,args:Record<string,unknown>={}){current=coreCall<Frame>(`agenttimeline.${op}`,{id,...args});return current;},
    dispose(){coreCall("agenttimeline.dispose",{id});}};
}

test("tabs, collapsibles and filters update native layout without requests",()=>{
  const e=engine([{id:"dashboard",text:fence({
    root:{type:"Card",props:{title:"Interactive dashboard"},children:["tabs","collapse","filter"]},
    tabs:{type:"Tabs",props:{labels:["Overview","Details"]},children:["overview","details"]},
    overview:{type:"Text",props:{text:"Overview-only content"}},details:{type:"Text",props:{text:"Details-only content"}},
    collapse:{type:"Collapsible",props:{title:"Advanced"},children:["advanced"]},advanced:{type:"Text",props:{text:"Advanced setting detail"}},
    filter:{type:"Filter",props:{label:"Search projects"},children:["alpha","beta"]},
    alpha:{type:"Card",props:{title:"Project Alpha"}},beta:{type:"Card",props:{title:"Project Beta"}},
  })+"\n\nFollowing paragraph."}]);
  try{
    expect(body(e.frame)).toContain("Overview-only content");expect(body(e.frame)).not.toContain("Details-only content");
    e.event(e.control("Details").id,"activate");expect(body(e.frame)).toContain("Details-only content");
    const following=()=>e.frame.commands.find(c=>c.kind==="text"&&c.text?.includes("Following"))!.y!;
    const before=following();e.event(e.control("Advanced").id,"activate");expect(body(e.frame)).toContain("Advanced setting detail");expect(following()).toBeGreaterThan(before);
    const filter=e.control("Search projects").id;e.event(filter,"change","Alpha");expect(body(e.frame)).not.toContain("Project Beta");
    e.call("layout",{width:420,height:2400,contentBottom:2400});expect(e.control("Search projects").id).toBe(filter);expect(e.control("Search projects").value).toBe("Alpha");noSubmit(e.frame);
  }finally{e.dispose();}
});

test("local fields remain editable but cannot submit or acknowledge actions",()=>{
  const e=engine([{id:"fields",text:fence({root:{type:"Stack",props:{},children:["topic","team","notes"]},
    topic:{type:"Input",props:{label:"Topic",multiline:true}},team:{type:"Select",props:{label:"Team",options:["Sales","Support"]}},notes:{type:"Checkbox",props:{label:"Include team notes"}},
  })}]);
  try{
    for(const [label,value] of [["Topic","Literal ${state.secret}\nnotes"],["Team","Support"],["Include team notes",true]] as const){e.event(e.control(label).id,"change",value);expect(e.frame.jsonUiAccepted).toBe(true);expect(e.control(label).value).toBe(value);noSubmit(e.frame);}
    for(const status of ["sending","sent","cancelled","failed"]){e.event(e.control("Topic").id,"action-result",status);expect(e.frame.jsonUiAccepted).toBe(false);noSubmit(e.frame);}
    e.event("arbitrary-shell-command","activate","rm -rf /");expect(e.frame.jsonUiAccepted).toBe(false);noSubmit(e.frame);
  }finally{e.dispose();}
});

test("old Form and Button sources are invalid and expose no execution controls",()=>{
  for(const [type,props] of [["Button",{label:"Send",prompt:"p"}],["Form",{submitLabel:"Send",prompt:"p"}]] as const){
    const e=engine([{id:"old",text:fence({root:{type,props}})}]);try{expect(e.frame.jsonUi?.controls||[]).toEqual([]);e.event("Send","activate");expect(e.frame.jsonUiAccepted).toBe(false);noSubmit(e.frame);}finally{e.dispose();}
  }
});

test("identical instances keep independent values and offscreen controls cannot act",()=>{
  const source=fence({root:{type:"Input",props:{label:"Name"}}});const e=engine([{id:"one",text:source},{id:"two",text:source}]);
  try{
    const names=e.frame.jsonUi!.controls.filter(c=>c.label==="Name");expect(names).toHaveLength(2);expect(names[0].id).not.toBe(names[1].id);
    e.event(names[0].id,"change","First only");expect(e.frame.jsonUi!.controls.find(c=>c.id===names[0].id)?.value).toBe("First only");expect(e.frame.jsonUi!.controls.find(c=>c.id===names[1].id)?.value||"").toBe("");
    e.call("layout",{width:900,height:100,contentBottom:20});expect(e.frame.jsonUi?.controls.some(c=>c.id===names[0].id)).toBe(false);e.event(names[0].id,"change","late");expect(e.frame.jsonUiAccepted).toBe(false);noSubmit(e.frame);
  }finally{e.dispose();}
});

test("chart supports native hover and persistent selection",()=>{
  const e=engine([{id:"chart",text:fence({root:{type:"LineGraph",props:{title:"Revenue",data:[{label:"Mon",value:10},{label:"Tue",value:20},{label:"Wed",value:15}]}}})}]);
  try{const c=e.control("Revenue","chart");expect(c.options).toEqual(["Mon","Tue","Wed"]);e.call("pointer",{kind:"move",x:c.rect[0]+c.rect[2]*0.5,y:c.rect[1]+c.rect[3]*0.5});expect(body(e.frame)).toContain("20");e.event(c.id,"activate",2);expect(e.control("Revenue").value).toBe(2);e.call("pointer",{kind:"leave"});expect(e.control("Revenue").value).toBe(2);noSubmit(e.frame);}finally{e.dispose();}
});

test("replacing native source resets local state and revokes old IDs",()=>{
  const spec=(value:string)=>fence({root:{type:"Input",props:{label:"Name",defaultValue:value}}});const e=engine([{id:"revision",text:spec("before")}]);
  try{const old=e.control("Name").id;e.event(old,"change","local edit");coreCall("agenttimeline.event",{id:e.id,historyKind:"replace",message:{HistoryChunk:{session_id:"chat-a",messages:[{id:"revision",role:"Assistant",kind:"Assistant",text:spec("after")}],next_cursor:null}}});e.call("frame");expect(e.control("Name").value).toBe("after");expect(e.control("Name").id).not.toBe(old);e.event(old,"change","late");expect(e.frame.jsonUiAccepted).toBe(false);noSubmit(e.frame);}finally{e.dispose();}
});

test("diagram zoom, pan and fit keep native isolated local state",()=>{
  const source=fence(jsonRenderDiagramExample.elements,"flow"),e=engine([{id:"diagram-a",text:source},{id:"diagram-b",text:source}]);
  try{
    const diagrams=e.frame.jsonUi!.controls.filter(c=>c.kind==="diagram");expect(diagrams).toHaveLength(2);const first=diagrams[0].id,second=diagrams[1].id;
    const state=(id=first)=>e.frame.jsonUi!.controls.find(c=>c.id===id)!.value as JsonUiDiagramTransform;
    e.event(first,"activate","zoom-in");expect(state().zoom).toBeGreaterThan(1);expect(state(second).zoom).toBe(1);
    e.event(first,"activate",{dx:25,dy:10});e.call("layout",{width:420,height:2400,contentBottom:2400});expect(state().panX).toBe(25);
    for(let i=0;i<20;i++)e.event(first,"activate","zoom-in");expect(state().zoom).toBeLessThanOrEqual(4);
    for(let i=0;i<40;i++)e.event(first,"activate","zoom-out");expect(state().zoom).toBeGreaterThanOrEqual(0.25);
    e.event(first,"activate","fit");expect(state()).toEqual({zoom:1,panX:0,panY:0});
    for(const value of ["execute",2,{zoom:100},{dx:1,dy:2,command:"bash"},{dx:"1",dy:0}]){e.event(first,"activate",value);expect(e.frame.jsonUiAccepted).toBe(false);}
    noSubmit(e.frame);e.call("layout",{width:420,height:100,contentBottom:20});e.event(first,"activate","zoom-in");expect(e.frame.jsonUiAccepted).toBe(false);
    e.call("layout",{width:420,height:2400,contentBottom:2400});expect(state()).toEqual({zoom:1,panX:0,panY:0});
    const replacement=fence({flow:{...jsonRenderDiagramExample.elements.flow,props:{...jsonRenderDiagramExample.elements.flow.props,title:"Revised flow"}}},"flow");
    coreCall("agenttimeline.event",{id:e.id,historyKind:"replace",message:{HistoryChunk:{session_id:"chat-a",messages:[{id:"diagram-a",role:"Assistant",kind:"Assistant",text:replacement}],next_cursor:null}}});e.call("frame");expect(e.control("Revised flow").id).not.toBe(first);e.event(first,"activate","zoom-in");expect(e.frame.jsonUiAccepted).toBe(false);
  }finally{e.dispose();}
});