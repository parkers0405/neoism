import{createEffect,createSignal,onCleanup,onMount}from"solid-js";
import{createWindowSize,useGpuixRequired,type HostElement}from"@gpuix/solid";
import type{AppConfig}from"../config";
import{coreCall}from"../native-core";
import{PaintLayer,type PaintCommand}from"../ui/PaintLayer";
import{useDialogViewport,overlayPoint}from"../ui/overlay-viewport";
import{AnimationFrameScheduler,ANIMATION_FRAME_MS}from"../animation/frame-scheduler";
import{focusNativeElement}from"../native-focus";
import{handleApplicationShortcut}from"../commands/application-shortcuts";
export interface GitRestoreTarget{root:string;path:string;block?:unknown}
interface Frame{commands:PaintCommand[];active:boolean;animating:boolean;rect:number[]|null;confirmed:boolean|null;target:GitRestoreTarget}
/** The original UniversalModal and review_actions spec. This host only returns
 * a decision; all mutation, dirty-document and daemon checks stay upstream. */
export function GitRestoreDialog(props:{target:GitRestoreTarget;config:AppConfig;onDone:(confirmed:boolean)=>void}){
 const renderer=useGpuixRequired(),config=props.config,viewport=useDialogViewport(renderer,createWindowSize());
 const{id}=coreCall<{id:number}>("gitconfirm.new",{target:props.target,scale:config.scale,theme:config.themeName,fontFamily:config.fontFamily,fontWeight:config.fontWeight,width:viewport().width,height:viewport().height});
 const[frame,setFrame]=createSignal(coreCall<Frame>("gitconfirm.frame",{id}));let host:HostElement|undefined,done=false;
 const animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const call=(op:string,args:Record<string,unknown>={})=>{if(done)return;const start=performance.now();animation.cancel();const next=coreCall<Frame>(`gitconfirm.${op}`,{id,...args});setFrame(next);if(next.confirmed!==null){done=true;props.onDone(next.confirmed);return;}if(next.animating)animation.request(()=>call("frame"),start);};
 createEffect(()=>{const box=viewport();call("layout",{width:box.width,height:box.height});});
 onMount(()=>{if(host)focusNativeElement(renderer,host.id);});onCleanup(()=>{animation.dispose();coreCall("gitconfirm.dispose",{id});});
 return <anchored fill="window" deferred priority={4} occlude={false} style={{backgroundColor:"transparent",pointerEvents:"none"}}>
  <div ref={host} testId="git-restore-confirmation" role="dialog" aria-label={props.target.block?"Restore this edit?":"Restore unstaged changes?"} tabIndex={0}
   onKeyDown={event=>{if(handleApplicationShortcut(renderer,event))return;event.preventDefault();event.stopPropagation();call("key",{key:event.key?.toLowerCase()||"",shift:!!event.modifiers?.shift});}}
   onMouseDown={event=>{if(event.button===0)call("pointer",{kind:"down",...overlayPoint(viewport(),event)});}}
   onMouseMove={event=>call("pointer",{kind:"move",...overlayPoint(viewport(),event)})}
   onMouseLeave={()=>call("pointer",{kind:"leave"})}
   style={{position:"absolute",left:viewport().x,top:viewport().y,width:viewport().width,height:viewport().height,overflow:"hidden",pointerEvents:"auto",userSelect:"none"}}>
   <PaintLayer commands={frame().commands} config={config}/>
  </div>
 </anchored>;
}
