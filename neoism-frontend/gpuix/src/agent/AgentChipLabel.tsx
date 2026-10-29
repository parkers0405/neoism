import {createEffect,createSignal,onCleanup} from "solid-js";
import {useGpuixRequired} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {PaintLayer,type PaintCommand} from "../ui/PaintLayer";
import type {WordmarkHosts} from "../ui/AnimatedWordmark";

interface ChipFrame {commands:PaintCommand[];width:number;height:number;animating:boolean;phase:"idle"|"loading"|"settling"|"changing"}
/** Loading clocks and agent changes belong to the tab, not its current mount. */
export class AgentChipHosts {
 private entries=new Map<string,number>();
 acquire(scope:string,kind:string,config:AppConfig){
  const key=JSON.stringify([scope,kind,config.fontFamily,config.fontWeight]);
  let id=this.entries.get(key);
  if(id===undefined){id=coreCall<{id:number}>("agentcomposer.new",{fontFamily:config.fontFamily,fontWeight:config.fontWeight}).id;this.entries.set(key,id);}
  return id;
 }
 dispose(){for(const id of this.entries.values())coreCall("agentcomposer.dispose",{id});this.entries.clear();}
}

/** The original composer label renderer owns glyph selection, color, width
 * and the shared loading/settling clock. Presentation only schedules paint. */
export function AgentChipLabel(props:{config:AppConfig;hosts:AgentChipHosts;paintPins:WordmarkHosts;scope:string;kind:"agent"|"model"|"thinking";label:string;displayLabel?:string;maxWidth?:number;size:number;loading:boolean}){
 const renderer=useGpuixRequired(),animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const [frame,setFrame]=createSignal<ChipFrame>();let disposed=false;
 const draw=()=>{
  if(disposed)return;animation.cancel();
  const id=props.hosts.acquire(props.scope,props.kind,props.config);
  const next=coreCall<ChipFrame>("agentcomposer.chip-frame",{id,kind:props.kind,label:props.label,displayLabel:props.displayLabel,fontSize:props.size*props.config.scale,theme:props.config.themeName,loading:props.loading,reducedMotion:props.config.reducedMotion});
  setFrame(next);if(next.animating)animation.request(draw);
 };
 createEffect(draw);
 onCleanup(()=>{disposed=true;animation.dispose();});
 return <div testId={`agent-chip-label:${props.kind}`} aria-label={props.label} style={{position:"relative",width:Math.min(frame()?.width||0,props.maxWidth??Infinity),overflow:"hidden",height:frame()?.height||0,flexShrink:0,pointerEvents:"none"}}>
  <PaintLayer config={props.config} commands={frame()?.commands||[]} pinImage={(source,raster)=>props.paintPins.pinPaintImage(renderer,source,raster)}/>
 </div>;
}
