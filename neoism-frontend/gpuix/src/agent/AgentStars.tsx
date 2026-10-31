import {createEffect,createMemo,createSignal,onCleanup} from "solid-js";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {AnimationFrameScheduler} from "../animation/frame-scheduler";
import {PaintLayer,type PaintCommand} from "../ui/PaintLayer";

/** Decorative background only: never participates in input or prompt layout. */
export function AgentStars(props:{config:AppConfig;composerId:number;width:number;height:number;radius:number}){
 const animation=new AnimationFrameScheduler(1000/60);
 const [commands,setCommands]=createSignal<PaintCommand[]>([]);
 let disposed=false;
 const parameters=createMemo(()=>({id:props.composerId,width:props.width,height:props.height,radius:props.radius,scale:props.config.scale,theme:props.config.themeName,enabled:props.config.agentStars!==false,reducedMotion:props.config.reducedMotion}),undefined,{equals:(a,b)=>JSON.stringify(a)===JSON.stringify(b)});
 const draw=()=>{
  if(disposed)return;
  animation.cancel();
  const started=performance.now();
  const next=coreCall<{commands:PaintCommand[];animating:boolean}>("agentcomposer.stars-frame",{...parameters(),seconds:started/1000});
  setCommands(next.commands);
  if(next.animating)animation.request(draw,started);
 };
 createEffect(draw);
 onCleanup(()=>{disposed=true;animation.dispose();});
 return <div testId="agent-stars" style={{position:"absolute",left:0,right:0,top:0,bottom:0,overflow:"hidden",borderRadius:props.radius,pointerEvents:"none"}}>
  <PaintLayer commands={commands()} config={props.config}/>
 </div>;
}
