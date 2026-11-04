import {createRenderer,createWindowSize,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {createEffect,createSignal,onCleanup} from "solid-js";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {coreCall} from "../src/native-core";
import {PaintLayer,type PaintCommand} from "../src/ui/PaintLayer";
import {jsonRenderExample,jsonRenderInteractiveExample,jsonRenderDiagramExample} from "../src/agent/json-render-catalog";
import {JsonRenderControls,type JsonUiFrame,type JsonUiEvent} from "../src/agent/JsonRenderControls";

// Visual fixture uses the real chat engine and glyph rasters, without a model
// request, a browser reconstruction, or changes to any saved conversation.
const config={...loadConfig([],process.env),reducedMotion:true};
export const renderer=createRenderer();
export let observedFrame:{commands:PaintCommand[];jsonUi?:JsonUiFrame;jsonUiAccepted?:boolean}|undefined;
export let lastPointer:Record<string,unknown>|undefined;
export const observedText=new Set<string>();
prepareBundledFonts(renderer);
await (renderer as typeof renderer & {initAsync:(options:Record<string,unknown>)=>Promise<void>}).initAsync({width:440,height:820,title:"json-render visual check",appId:"ai.neoism.json-render-check",focus:false});
render(()=>{
  const size=createWindowSize();
  const [commands,setCommands]=createSignal<PaintCommand[]>([]),[jsonUi,setJsonUi]=createSignal<JsonUiFrame>();
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"json-render-visual",scale:1,reducedMotion:true});
  const example=process.argv.includes("--diagram")?jsonRenderDiagramExample:process.argv.includes("--interactive")?jsonRenderInteractiveExample:jsonRenderExample;
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"json-render-visual",messages:[{id:"example",role:"Assistant",kind:"Assistant",text:"```json-render\n"+JSON.stringify(example)+"\n```"}],next_cursor:null}}});
  const call=(op:string,args:Record<string,unknown>={})=>{if(op==="pointer")lastPointer=args;const frame=coreCall<{commands:PaintCommand[];jsonUi?:JsonUiFrame;jsonUiAccepted?:boolean}>(`agenttimeline.${op}`,{id,...args});observedFrame=frame;for(const command of frame.commands)if(command.kind==="text"&&observedText.size<512)observedText.add(command.text);setCommands(frame.commands);setJsonUi(frame.jsonUi);return frame;};
  createEffect(()=>{
    call("layout",{width:size().width||440,height:size().height||820,scale:1,reducedMotion:true});
  });
  onCleanup(()=>coreCall("agenttimeline.dispose",{id}));
  return <div onMouseMove={event=>call("pointer",{kind:"move",x:event.x,y:event.y})} onMouseLeave={()=>call("pointer",{kind:"leave"})}
    onMouseDown={event=>{if(event.button===0)call("pointer",{kind:"down",x:event.x,y:event.y});}} onMouseUp={event=>call("pointer",{kind:"up",x:event.x,y:event.y})}
    onScroll={event=>call("scroll",{deltaY:event.deltaY||0,precise:true})} style={{width:"100%",height:"100%",backgroundColor:"#000000"}}>
    <PaintLayer commands={commands()} config={config}/>
    <JsonRenderControls frame={jsonUi()} config={config} onEvent={(event:JsonUiEvent)=>{const frame=call("json-ui",{...event});return {accepted:frame.jsonUiAccepted===true,frame:frame.jsonUi};}}
      onChartPointer={(kind,event)=>call("pointer",{kind,x:event.x,y:event.y})}/>
  </div>;
},{renderer});
(renderer as typeof renderer & {setStartupRenderingReady:(ready:boolean)=>void}).setStartupRenderingReady(true);
startFrameLoop(renderer,{onTerminated:()=>process.exit(0),onError:error=>console.error(error)});