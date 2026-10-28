import {createEffect,createSignal,onCleanup,type JSX} from "solid-js";
import type {StyleDesc} from "@gpuix/solid";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
/** Exact shared primitives/text.rs::hover_title_offset policy. */
export function hoverTitleFrame(elapsed:number,distance:number,scale:number){
 if(elapsed<.6)return{offset:0,full:false,nextMs:(.6-elapsed)*1000};
 const duration=Math.max(distance/(35*scale),2),phase=((elapsed-.6)/duration)%2;
 const progress=phase<=1?phase:2-phase;
 const offset=distance*Math.max(0,Math.min(1,(progress-.15)/.7));
 const hold=phase<.15?.15-phase:phase>=.85&&phase<1.15?1.15-phase:phase>=1.85?2.15-phase:0;
 return{offset,full:true,nextMs:hold>0?hold*duration*1000:ANIMATION_FRAME_MS};
}
/** Only the title moves: its clipping box, icon, close target and tab stay put. */
export function HoverTabTitle(props:{label:string;width:number;naturalWidth:number;height:number;scale:number;hovered:boolean;reducedMotion:boolean;style:StyleDesc;testId?:string}):JSX.Element{
 const [paint,setPaint]=createSignal({offset:0,full:false});const animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);let started=0;
 createEffect(()=>{
  const hovered=props.hovered,distance=Math.max(0,props.naturalWidth-props.width),scale=props.scale,reduced=props.reducedMotion;props.label;
  animation.cancel();setPaint({offset:0,full:false});if(!hovered||reduced||distance<=.5)return;
  started=performance.now();const draw=()=>{const next=hoverTitleFrame((performance.now()-started)/1000,distance,scale);setPaint({offset:next.offset,full:next.full});animation.request(draw,performance.now(),next.nextMs);};draw();
 });
 onCleanup(()=>animation.dispose());
 return <div style={{position:"relative",width:props.width,height:props.height,flexShrink:0,overflow:"hidden",pointerEvents:"none"}}>
  <text testId={props.testId} style={{...props.style,position:"absolute",left:-paint().offset,top:0,width:paint().full?props.naturalWidth:props.width,height:props.height,textOverflow:paint().full?undefined:"ellipsis",pointerEvents:"none",userSelect:"none"}}>{props.label}</text>
 </div>;
}
