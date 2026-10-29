import{createEffect,createMemo,createSignal,Index,onCleanup}from"solid-js";
import{createWindowSize,useGpuixRequired}from"@gpuix/solid";
import type{AppConfig}from"../config";
import{coreCall}from"../native-core";
import{PaintLayer,type PaintCommand}from"./PaintLayer";
import{sharePaintCommands}from"../editor/paint-command";
import{AnimationFrameScheduler,ANIMATION_FRAME_MS}from"../animation/frame-scheduler";
import{useOverlayViewport,overlayPoint}from"./overlay-viewport";
interface Frame{commands:PaintCommand[];rects:number[][];messages?:string[];active:boolean}
/** Original Rust toast appearance, wrapping, stacking, dedupe and hover lifetime.
 * The native id belongs to the window, so zoom/theme remounts retain messages. */
export function ToastNotifications(props:{id:number;delivery:{message?:string;consumed:boolean;refresh?:()=>void};config:AppConfig;message?:string;consume:()=>void;report:(error:unknown)=>void}){
 const renderer=useGpuixRequired(),viewport=useOverlayViewport(renderer,createWindowSize()),config=props.config;
 const[frame,setFrame]=createSignal<Frame>({commands:[],rects:[],active:false}),animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);let disposed=false;
 const parameters=createMemo(()=>({id:props.id,width:viewport().width,height:viewport().height,scale:config.scale,theme:config.themeName,fontFamily:config.fontFamily,fontWeight:config.fontWeight}));
 const sample=(op="frame",args:Record<string,unknown>={})=>{if(disposed)return;animation.cancel();const started=performance.now();try{const next=coreCall<Frame>(`notifications.${op}`,{...parameters(),...args});setFrame(previous=>({...next,commands:sharePaintCommands(previous.commands,next.commands)}));if(next.active)animation.request(()=>sample(),started);if(!next.rects.length&&!props.delivery.consumed){props.delivery.consumed=true;props.consume();}}catch(error){console.error("[neoism-gpuix] Notification paint failed",error);}};
 const refresh=()=>sample();props.delivery.refresh=refresh;
 createEffect(()=>{parameters();sample();});
 createEffect(()=>{const message=props.message;if(message&&message!==props.delivery.message){props.delivery.message=message;props.delivery.consumed=false;sample("push",{message,level:"error"});}else if(!message)props.delivery.message=undefined;});
 onCleanup(()=>{disposed=true;animation.dispose();try{coreCall("notifications.leave",parameters());}catch{}if(props.delivery.refresh===refresh)props.delivery.refresh=undefined;else props.delivery.refresh?.();});
 return <anchored fill="window" deferred priority={6} occlude={false} style={{pointerEvents:"none",backgroundColor:"transparent"}}>
  <div style={{position:"absolute",left:viewport().x,top:viewport().y,width:viewport().width,height:viewport().height,overflow:"hidden",pointerEvents:"none"}}><PaintLayer commands={frame().commands} config={config}/>
   <Index each={frame().rects}>{(rect,index)=><div testId="app-notification" role="alert" aria-label={frame().messages?.[index]||"Notification"}
    onMouseMove={event=>sample("hover",overlayPoint(viewport(),event))} onMouseLeave={()=>sample("leave")} onClick={()=>sample("dismiss",{index})}
    style={{position:"absolute",left:rect()[0],top:rect()[1],width:rect()[2],height:rect()[3],pointerEvents:"auto",cursor:"pointer"}}/>}</Index>
  </div>
 </anchored>;
}
