import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {loadConfig} from "../../src/config";
import {JsonRenderControls,type JsonUiFrame,type JsonUiEvent,type JsonUiDiagramTransform} from "../../src/agent/JsonRenderControls";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const config=loadConfig(["--config","/nonexistent/json-ui-host-test"],{});
let frame:JsonUiFrame={controls:[
  {id:"name",kind:"input",label:"Project name",rect:[10,10,280,36],value:""},
  {id:"notes",kind:"input",label:"Notes",rect:[10,60,280,90],value:"",multiline:true},
  {id:"mode",kind:"select",label:"Mode",rect:[10,160,280,36],value:"Quick",options:["Quick","Thorough"]},
  {id:"agree",kind:"checkbox",label:"Include details",rect:[10,210,280,30],value:false},
  {id:"chart",kind:"chart",label:"Revenue",rect:[10,250,280,120],value:0,options:["Mon","Tue","Wed"]},
  {id:"tab",kind:"button",label:"Options",rect:[10,390,280,36]},
  {id:"collapse",kind:"button",label:"Details",rect:[10,430,280,36]},
  {id:"diagram",kind:"diagram",label:"Flow",rect:[10,480,300,320],value:{zoom:1,panX:0,panY:0}},
]};
let setFrame!:(value:JsonUiFrame)=>void,stage="mount",exits=0;
const events:JsonUiEvent[]=[];
const update=(value:JsonUiFrame)=>{frame=value;setFrame(value);};
const event=(event:JsonUiEvent)=>{
  events.push(event);
  if(event.control==="diagram"){
    update({...frame,controls:frame.controls.map(control=>{
      if(control.id!=="diagram")return control;
      const value={...control.value as JsonUiDiagramTransform},command=event.value;
      if(typeof command==="object"){value.panX+=command.dx;value.panY+=command.dy;}
      else if(command==="fit"){value.zoom=1;value.panX=0;value.panY=0;}
      else if(command==="zoom-in")value.zoom*=1.25;
      else if(command==="zoom-out")value.zoom/=1.25;
      else if(command==="pan-right")value.panX+=40;
      return {...control,value};
    })});
  }else if((event.kind==="change"||event.control==="chart")&&typeof event.value!=="object"){
    const value=event.value;update({...frame,controls:frame.controls.map(control=>control.id===event.control?{...control,value}:control)});
  }
  return {accepted:true,frame};
};
root.render(()=>{
  const [current,setCurrent]=createSignal(frame);setFrame=setCurrent;
  return <div><input testId="composer" value="Keep this caret"/><JsonRenderControls frame={current()} config={config} onEvent={event} onExit={()=>{exits++;}}/></div>;
});
const node=(id:string)=>{const result=host.find(id);assert.ok(result,`${stage}: missing ${id}`);return result;};
const dispatch=async(id:string,eventType:string,extra:Record<string,unknown>={})=>{root.dispatch({elementId:node(id).id,eventType,...extra});await host.pump(root);};
try{
  await host.pump(root);
  const inputId=node("json-ui-control:name").id,composerId=node("composer").id;
  stage="preserve native editor/composer focus";
  // The recording renderer tracks explicit focus claims. Paired with native
  // preserve-keyboard-focus props, this catches widget handlers stealing the
  // live editor (and therefore its selection/caret) during local interactions.
  for(const focused of [inputId,composerId]){
    host.renderer.focusElement?.(focused);await host.pump(root);
    for(const id of ["chart","diagram","agree","tab","collapse"]){
      assert.equal(node(`json-ui-control:${id}`).props["data-preserve-keyboard-focus"],true);
      await dispatch(`json-ui-control:${id}`,"mouseDown",{button:0,x:20,y:500});
      await dispatch(`json-ui-control:${id}`,"mouseUp",{button:0,x:20,y:500});
      await dispatch(`json-ui-control:${id}`,"click",{button:0});
      assert.equal(host.focused,focused,`${id} must preserve existing input focus`);
    }
    await dispatch("json-ui-diagram:diagram:zoom-in","mouseDown",{button:0});
    await dispatch("json-ui-diagram:diagram:zoom-in","click",{button:0});assert.equal(host.focused,focused);
    await dispatch("json-ui-control:mode","mouseDown",{button:0});await dispatch("json-ui-control:mode","click",{button:0});assert.equal(host.focused,focused);
    await dispatch("json-ui-option:1","click",{button:0});assert.equal(host.focused,focused,"Selecting an option must not claim focus");
    await dispatch("json-ui-control:mode","click",{button:0});
    await dispatch("json-ui-options","mouseDownOutside",{x:900,y:900});assert.equal(host.focused,focused,"Dismissing options must not claim focus");
  }
  stage="text editors legitimately claim focus";
  await dispatch("json-ui-control:name","mouseDown",{button:0});assert.equal(host.focused,inputId);
  await dispatch("json-ui-control:notes","mouseDown",{button:0});assert.equal(host.focused,node("json-ui-control:notes").id);
  assert.ok(!node("json-ui-control:notes").events.has("submit"),"Multiline Enter never submits");
  stage="local fields and stable caret host";
  await dispatch("json-ui-control:name","change",{value:"Neoism"});assert.equal(node("json-ui-control:name").id,inputId);assert.equal(node("json-ui-control:name").props.value,"Neoism");
  await dispatch("json-ui-control:name","change",{value:"🙂".repeat(2001)});assert.equal(Array.from(String(events.at(-1)?.value)).length,2000);
  await dispatch("json-ui-control:name","change",{value:"🙂".repeat(2002)});assert.equal(Array.from(String(node("json-ui-control:name").props.value)).length,2000);
  stage="keyboard navigation remains available";
  await dispatch("json-ui-control:name","keyDown",{key:"tab",modifiers:{}});assert.equal(host.focused,node("json-ui-control:notes").id);
  await dispatch("json-ui-control:notes","keyDown",{key:"tab",modifiers:{shift:true}});assert.equal(host.focused,inputId);
  await dispatch("json-ui-control:mode","focus");await dispatch("json-ui-control:mode","keyDown",{key:"enter",modifiers:{}});
  await dispatch("json-ui-control:mode","keyDown",{key:"enter",modifiers:{}});assert.equal(host.focused,node("json-ui-control:mode").id);
  await dispatch("json-ui-control:agree","keyDown",{key:"tab",modifiers:{}});assert.equal(host.focused,node("json-ui-control:chart").id);
  await dispatch("json-ui-control:chart","keyDown",{key:"right",modifiers:{}});assert.equal(frame.controls.find(c=>c.id==="chart")?.value,1);
  await dispatch("json-ui-control:chart","keyDown",{key:"tab",modifiers:{}});assert.equal(host.focused,node("json-ui-control:tab").id);
  await dispatch("json-ui-control:tab","focus");await dispatch("json-ui-control:tab","keyDown",{key:"shift",modifiers:{shift:true}});assert.equal(node("json-ui-control:tab").style.borderWidth,config.scale);
  await dispatch("json-ui-control:tab","keyDown",{key:"enter",modifiers:{}});assert.equal(events.at(-1)?.control,"tab");
  stage="diagram navigation";
  const diagramId=node("json-ui-control:diagram").id,transform=()=>frame.controls.find(c=>c.id==="diagram")!.value as JsonUiDiagramTransform;
  await dispatch("json-ui-control:diagram","keyDown",{key:"0",modifiers:{}});
  await dispatch("json-ui-control:diagram","mouseDown",{button:0,x:20,y:500});await dispatch("json-ui-control:diagram","mouseMove",{x:45,y:510});await dispatch("json-ui-control:diagram","mouseUp",{button:0,x:45,y:510});
  assert.deepEqual(transform(),{zoom:1,panX:25,panY:10});assert.equal(node("json-ui-control:diagram").id,diagramId);
  await dispatch("json-ui-control:diagram","mouseMove",{x:100,y:550});assert.equal(transform().panX,25);
  await dispatch("json-ui-control:diagram","keyDown",{key:"right",modifiers:{}});assert.equal(transform().panX,65);
  await dispatch("json-ui-control:diagram","keyDown",{key:"tab",modifiers:{}});assert.equal(exits,1);
  assert.equal(Object.hasOwn(frame,"action"),false);assert.equal(host.find("json-ui-review"),undefined);assert.ok(events.every(event=>event.kind==="activate"||event.kind==="change"));
  stage="stale controls";update({controls:[]});await host.pump(root);const before=events.length;root.dispatch({elementId:inputId,eventType:"change",value:"late"});await host.pump(root);assert.equal(events.length,before);
  root.unmount();console.log("interactive widget host verified");
}catch(error){console.error({stage,events:events.slice(-5)});throw error;}