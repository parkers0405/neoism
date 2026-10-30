import {createEffect,createMemo,createSignal,onCleanup,Show} from "solid-js";
import {useGpuixRequired,type HostElement} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {focusedNativeElement} from "../native-focus";
import {cssColor,type CanvasOp} from "./canvas";
import {suppressSurfaceCursor,surfaceCaret,registerSurfaceCaret} from "./cursor-handoff";
import {CursorBlinkClock} from "./cursor-blink";
export interface ChromeCursorTarget {host:HostElement;kind:"buffer"|"workspace"|"tree"|"sidebar"|"content";plus?:boolean;padding?:number;rowY?:number;rowHeight?:number;origin?:number[];bounds?:{x:number;y:number;width:number;height:number}}
/** One original TrailCursor crosses strips, sidebar rows and surface carets. */
export function ChromeFocusCursor(props:{config:AppConfig;target?:ChromeCursorTarget;onSettled?:(target:ChromeCursorTarget)=>void}){
 const renderer=useGpuixRequired(),id=coreCall<{id:number}>("keyboard.cursor.new").id,animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const [quad,setQuad]=createSignal<number[][]>([]),[focused,setFocused]=createSignal(false),[blink,setBlink]=createSignal(false),[moving,setMoving]=createSignal(false),[color,setColor]=createSignal(props.config.cursorColor||props.config.theme.accent);
 const blinkClock=new CursorBlinkClock(setBlink,{scope:renderer});
 createEffect(()=>{const target=props.target;if(target&&target.kind!=="content")onCleanup(registerSurfaceCaret(renderer,target.host,quad));});
 let seeded:number[]|undefined;
 const draw=()=>{
  animation.cancel();const target=props.target,bounds=target?.kind==="content"?surfaceCaret(renderer,target.host.id)?.bounds:target?.bounds;if(!target?.host.mounted||!bounds||bounds.width<=0||bounds.height<=0){setQuad([]);setMoving(false);if(target?.kind==="content")props.onSettled?.(target);return;}
  if(target.origin&&seeded!==target.origin){coreCall("keyboard.cursor.seed",{id,rect:target.origin});seeded=target.origin;}
  const start=performance.now(),frame=coreCall<{quad:number[][];animating:boolean;hold:boolean}>("keyboard.cursor.frame",{id,kind:target.kind==="content"||target.kind==="sidebar"?"rect":target.kind==="workspace"&&target.plus?"buffer":target.kind,...bounds,scale:props.config.scale,padding:target.padding,plus:target.plus,rowY:bounds.y+(target.rowY||0),rowHeight:target.rowHeight});
  setQuad(frame.quad);setMoving(frame.animating);
  if(frame.animating)animation.request(draw,start);
  else if(target.kind==="content")queueMicrotask(()=>{if(props.target===target)props.onSettled?.(target);});
 };
 createEffect(()=>{const target=props.target;if(!target){animation.cancel();setFocused(false);suppressSurfaceCursor(renderer,false);return;}setFocused(focusedNativeElement(renderer)===target.host.id);draw();});
 createEffect(()=>{const target=props.target,bounds=target?.kind==="content"?surfaceCaret(renderer,target.host.id)?.bounds:target?.bounds;
  blinkClock.update({active:!!target&&focused(),enabled:props.config.cursorBlink,intervalMs:props.config.cursorBlinkInterval,animating:moving(),activityToken:target&&bounds?`${target.host.id}:${target.kind}:${bounds.x}:${bounds.y}:${bounds.width}:${bounds.height}:${target.padding}:${target.plus}:${target.rowY}:${target.rowHeight}`:undefined});
 });
 let owner:number|null|undefined;
 const tick=setInterval(()=>{const target=props.target;if(!target){setFocused(false);return;}if(!target.host.mounted&&target.kind==="content"){props.onSettled?.(target);return;}const next=focusedNativeElement(renderer),active=next===target.host.id;setFocused(active);if(active&&owner!==next)draw();owner=next;if(!active)return;
  if(props.config.cursorStyle==="rainbow"){const cursor=coreCall<{rainbow:number[]}>("cursor",{seconds:performance.now()/1000});setColor("#"+cursor.rainbow.slice(0,3).map(value=>value.toString(16).padStart(2,"0")).join(""));}
 },1000/60);
 onCleanup(()=>{clearInterval(tick);animation.dispose();blinkClock.dispose();suppressSurfaceCursor(renderer,false);coreCall("keyboard.cursor.dispose",{id});});
 // The canvas paints in window coordinates. This nonpainting marker reports
 // the actual ink bounds, without the old SVG mask's extra pixel padding.
 const paint=createMemo(()=>{
  const points=quad();if(!points.length)return {left:0,top:0,width:1,height:1};
  const xs=points.map(point=>point[0]!),ys=points.map(point=>point[1]!);
  const left=Math.min(...xs),top=Math.min(...ys);
  return {left,top,width:Math.max(...xs)-left,height:Math.max(...ys)-top};
 });
 const commands=createMemo(()=>JSON.stringify([{k:"p",c:cssColor(color()),p:[quad().flatMap(point=>[point[0]!,point[1]!])]}] satisfies CanvasOp[]));
 const name=()=>props.target?.kind==="workspace"?"workspaces":props.target?.kind==="tree"||props.target?.kind==="sidebar"?"sidebar":props.target?.kind==="content"?"content":"buffers";
 return <Show when={props.target&&focused()&&blink()}><>
  <canvas testId="chrome-focus-paint" commands={commands()} style={{position:"absolute",left:0,top:0,right:0,bottom:0,pointerEvents:"none"}}/>
  <div testId={`chrome-focus-cursor:${name()}`} data-cursor-quad={JSON.stringify(quad())} data-cursor-animating={moving()} style={{position:"absolute",left:paint().left,top:paint().top,width:paint().width,height:paint().height,pointerEvents:"none"}}/>
 </></Show>;
}
