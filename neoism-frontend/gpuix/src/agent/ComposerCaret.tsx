import {createEffect,createMemo,createSignal,onCleanup,onMount,Show} from "solid-js";
import {useGpuixRequired,type HostElement} from "@gpuix/solid";
import {registerEventHandler,unregisterEventHandler} from "@gpuix/native/host";
import type {EventPayload} from "@gpuix/native";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {registerSurfaceCaret,surfaceCursorSuppressed} from "../ui/cursor-handoff";
import {cssColor,type CanvasOp} from "../ui/canvas";
import {CursorBlinkClock} from "../ui/cursor-blink";

export interface ComposerCaretGeometry{x:number;y:number;fontSize:number;remainingWidth:number;visible:boolean;offset:number;selectionStart:number;selectionEnd:number}
/** Geometry comes from the native editor's shaped insertion point, including
 * wrap, IME and scrolling. Never infer a caret from text length or key events. */
export function composerCaretGeometry(event:Pick<EventPayload,"x"|"y"|"value">):ComposerCaretGeometry|undefined{
 try{
  const value=JSON.parse(event.value||"null");
  if(!value||![event.x,event.y,value.fontSize,value.remainingWidth,value.offset,value.selectionStart,value.selectionEnd].every(number=>typeof number==="number"&&Number.isFinite(number)))return;
  return{x:event.x!,y:event.y!,fontSize:Math.max(0,value.fontSize),remainingWidth:Math.max(0,value.remainingWidth),visible:value.visible===true,offset:value.offset,selectionStart:value.selectionStart,selectionEnd:value.selectionEnd};
 }catch{return;}
}
/** Original Agent block geometry and original TrailCursor on a native GPU
 * canvas. The textarea retains platform IME, selection and all text editing. */
export function ComposerCaret(props:{config:AppConfig;input:()=>HostElement|undefined}){
 const renderer=useGpuixRequired(),id=coreCall<{id:number}>("keyboard.cursor.new").id,animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const [geometry,setGeometry]=createSignal<ComposerCaretGeometry>(),[quad,setQuad]=createSignal<number[][]>([]),[moving,setMoving]=createSignal(false),[blink,setBlink]=createSignal(false),[color,setColor]=createSignal(props.config.cursorColor||props.config.theme.accent);
 const blinkClock=new CursorBlinkClock(setBlink,{scope:renderer});
 createEffect(()=>{const target=geometry();blinkClock.update({active:!!target?.visible&&!surfaceCursorSuppressed(renderer),enabled:props.config.cursorBlink,intervalMs:props.config.cursorBlinkInterval,animating:moving(),activityToken:target?`${target.x}:${target.y}:${target.fontSize}:${target.remainingWidth}`:undefined});});
 let rect:number[]|undefined,disposed=false,detach=()=>{};
 const tickCursor=()=>rect?coreCall<{quad:number[][];animating:boolean;hold:boolean}>("keyboard.cursor.frame",{id,kind:"rect",x:rect[0],y:rect[1],width:rect[2],height:rect[3],scale:props.config.scale}):undefined;
 const draw=()=>{
  if(disposed||!geometry()?.visible)return;const start=performance.now(),next=tickCursor();if(!next)return;
  setQuad(next.quad);setMoving(next.animating);
  if(next.animating)animation.request(draw,start);
 };
 const receive=(event:EventPayload)=>{
  const next=composerCaretGeometry(event);if(!next)return;setGeometry(next);
  if(!next.visible){animation.cancel();setMoving(false);return;}
  rect=coreCall<number[]>("agentcomposer.caret-rect",{...next,scale:props.config.scale});
  if(rect[2]<=0||rect[3]<=0){animation.cancel();setQuad([]);return;}
  animation.request(draw);
 };
 onMount(()=>{
  const host=props.input();if(!host?.root)return;const hostRoot=host.root;
  // Public host registry supports custom native events without changing the
  // installed Solid adapter's closed JSX event-property table.
  registerEventHandler(hostRoot.eventHandlers,host.id,"caretGeometry",receive);
  hostRoot.mutations.setEventListener(host.id,"caretGeometry",true);
  const release=registerSurfaceCaret(renderer,host,quad);
  detach=()=>{release();unregisterEventHandler(hostRoot.eventHandlers,host.id,"caretGeometry");if(host.mounted)hostRoot.mutations.setEventListener(host.id,"caretGeometry",false);};
 });
 const rainbow=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const tint=()=>{if(disposed||!geometry()?.visible||props.config.cursorStyle!=="rainbow")return;const style=coreCall<{rainbow:number[]}>("cursor",{seconds:performance.now()/1000});setColor("#"+style.rainbow.slice(0,3).map(value=>value.toString(16).padStart(2,"0")).join(""));rainbow.request(tint);};
 createEffect(()=>{if(geometry()?.visible&&props.config.cursorStyle==="rainbow")rainbow.request(tint);else rainbow.cancel();});
 onCleanup(()=>{disposed=true;detach();blinkClock.dispose();rainbow.dispose();animation.dispose();coreCall("keyboard.cursor.dispose",{id});});
 const shown=createMemo(()=>geometry()?.visible&&!surfaceCursorSuppressed(renderer)&&quad().length===4&&blink());
 const commands=createMemo(()=>JSON.stringify([{k:"p",c:cssColor(color()),p:[quad().flat()]}] satisfies CanvasOp[]));
 const bounds=createMemo(()=>{const points=quad(),xs=points.map(point=>point[0]),ys=points.map(point=>point[1]);return points.length?{left:Math.min(...xs),top:Math.min(...ys),width:Math.max(...xs)-Math.min(...xs),height:Math.max(...ys)-Math.min(...ys)}:{left:0,top:0,width:0,height:0};});
 return <Show when={shown()}><anchored fill="window" deferred priority={4} occlude={false} style={{pointerEvents:"none",backgroundColor:"transparent"}}>
  <canvas testId="agent-composer-cursor-paint" commands={commands()} style={{position:"absolute",left:0,top:0,right:0,bottom:0,pointerEvents:"none"}}/>
  <div testId="agent-composer-cursor" style={{position:"absolute",...bounds(),pointerEvents:"none"}}/>
 </anchored></Show>;
}
