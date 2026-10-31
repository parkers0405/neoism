import {createEffect,createSignal,onCleanup,onMount} from "solid-js";
import {createWindowSize,useGpuixRequired,type HostElement} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {PaintLayer,type PaintCommand} from "./PaintLayer";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {focusNativeElement} from "../native-focus";
import {readClipboard} from "../clipboard";

export interface ContextItem {id:string;label:string;hint?:string}
export interface SidebarContextSpec {
 kind?:"menu"|"form";title:string;x?:number;y?:number;items?:ContextItem[];
 body?:string;meta?:string;input?:string;placeholder?:string;accept?:string;
 onSelect:(command:string,value?:string)=>void;
}
interface Frame {commands:PaintCommand[];animating:boolean;rect?:number[];result?:{command?:string;value?:string;cancelled?:boolean}}
/** Shared Rust menu/modal painter, with one overlay owning the gesture and focus. */
export function SidebarContext(props:{config:AppConfig;spec:SidebarContextSpec;onClose:()=>void}){
 const renderer=useGpuixRequired(),size=createWindowSize(),config=props.config;
 const previous=renderer.getFocusedElementId?.(),spec=props.spec;
 const initial=coreCall<{id:number;frame:Frame}>("sidebarcontext.new",{...spec,onSelect:undefined,...size(),scale:config.scale,theme:config.themeName,fontFamily:config.fontFamily,fontWeight:config.fontWeight});
 const [frame,setFrame]=createSignal(initial.frame),animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 let host:HostElement|undefined,disposed=false,done=false;
 const call=(op:string,args:Record<string,unknown>={})=>{
  if(disposed||done)return;animation.cancel();const start=performance.now();
  const next=coreCall<Frame>(`sidebarcontext.${op}`,{id:initial.id,...args});setFrame(next);
  if(next.result?.cancelled||next.result?.command){done=true;props.onClose();if(next.result.command)spec.onSelect(next.result.command,next.result.value);return;}
  if(next.animating)animation.request(()=>call("frame"),start);
 };
 createEffect(()=>{const viewport=size();call("layout",{...viewport});});
 onMount(()=>{if(host)focusNativeElement(renderer,host.id);});
 onCleanup(()=>{disposed=true;animation.dispose();coreCall("sidebarcontext.dispose",{id:initial.id});if(previous!=null&&renderer.getFocusedElementId?.()===host?.id)focusNativeElement(renderer,previous);});
 return <anchored fill="window" deferred priority={6} occlude={true} style={{backgroundColor:"transparent"}}>
  <div ref={host} tabIndex={0} testId="sidebar-context" role={spec.kind==="form"?"dialog":"menu"} aria-label={spec.title||"Actions"}
   onKeyDown={event=>{event.preventDefault();event.stopPropagation();const key=event.key?.toLowerCase()||"";if((event.modifiers?.ctrl||event.modifiers?.cmd)&&key==="v"&&spec.kind==="form"){void readClipboard().then(text=>call("text",{text}));return;}call("key",{key,char:event.modifiers?.ctrl||event.modifiers?.cmd?"":event.keyChar||(key.length===1?key:""),shift:event.modifiers?.shift});}}
   onMouseDown={event=>{if(event.button===0)call("pointer",{kind:"down",x:event.x,y:event.y});}}
   onMouseMove={event=>call("pointer",{kind:"move",x:event.x,y:event.y})}
   onScroll={event=>call("scroll",{delta:(event.deltaY||0)*(event.precise?1:40*config.scale)})}
   style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",userSelect:"none"}}>
   <PaintLayer config={config} commands={frame().commands}/>
  </div>
 </anchored>;
}
