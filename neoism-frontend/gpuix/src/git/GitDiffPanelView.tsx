import {createEffect,createMemo,createSignal,For,onCleanup,Show} from "solid-js";
import {createWindowSize,useGpuixRequired,type EventPayload,type HostElement,type KeyEvent} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {GitController,type GitFrame} from "./GitController";
import {PaintLayer} from "../ui/PaintLayer";
import {useOverlayViewport,type OverlayViewport} from "../ui/overlay-viewport";
import type {ChromeCursorTarget} from "../ui/ChromeFocusCursor";
import {focusNativeElement,invalidateNativeFocus} from "../native-focus";
import {handleApplicationShortcut} from "../commands/application-shortcuts";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";

/** A reserved right-hand slot for the original GitDiffPanel paint/input.
 * The original renderer still lays out against the full body width, so its
 * opening animation and native hit coordinates retain one coordinate space. */
export function GitDiffPanelView(props:{controller:GitController;config:AppConfig;viewport?:OverlayViewport;
 active?:boolean;focusToken?:number;onFocus?:()=>void;onCustomKey?:(event:KeyEvent)=>boolean;
 onCursorTarget?:(target:ChromeCursorTarget|undefined)=>void;onClose?:()=>void;onHost?:(host:HostElement|undefined)=>void}){
 const renderer=useGpuixRequired(),fallback=useOverlayViewport(renderer,createWindowSize());
 const viewport=createMemo(()=>props.viewport||fallback());
 const [frame,setFrame]=createSignal<GitFrame>(props.controller.frame),[focused,setFocused]=createSignal(false),[dragging,setDragging]=createSignal(false);
 const visible=createMemo(()=>frame().visible);
 const pointerFrames=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 let host:HostElement|undefined,queuedFocus=false,disposed=false,lastLayout={width:0,height:0},pointer={x:0,y:0},wasVisible=props.controller.frame.visible,publishedCursor="";
 const call=(op:string,args:Record<string,unknown>={})=>props.controller.call(op,args);
 createEffect(()=>{const controller=props.controller;setFrame(controller.frame);onCleanup(controller.subscribe(setFrame));});
 createEffect(()=>{props.controller.configure(props.config);});
 createEffect(()=>{const bounds=viewport();if(bounds.width>0&&bounds.height>0&&(bounds.width!==lastLayout.width||bounds.height!==lastLayout.height)){lastLayout={width:bounds.width,height:bounds.height};props.controller.layout(bounds.width,bounds.height);}});
 createEffect(()=>{const next=visible();if(wasVisible&&!next)props.onClose?.();wasVisible=next;});
 const focus=(force=false)=>{
  if(force){if(disposed||!host?.mounted||!frame().visible)return;host.root?.mutations.flushMutations();focusNativeElement(renderer,host.id);return;}
  if(queuedFocus)return;queuedFocus=true;queueMicrotask(()=>{queuedFocus=false;if(disposed||!host?.mounted||props.active===false||!frame().visible)return;host.root?.mutations.flushMutations();focusNativeElement(renderer,host.id);});
 };
 createEffect(()=>{props.focusToken;if(props.active!==false&&visible())focus();});
 createEffect(()=>{const rect=frame().cursorRect,bounds=viewport(),target:ChromeCursorTarget|undefined=host&&focused()&&rect?{host,kind:"sidebar",bounds:{x:bounds.x+rect[0],y:bounds.y+rect[1],width:rect[2],height:rect[3]}}:undefined;
  const key=target?`${target.host.id}:${target.bounds!.x}:${target.bounds!.y}:${target.bounds!.width}:${target.bounds!.height}`:"";if(key!==publishedCursor){publishedCursor=key;props.onCursorTarget?.(target);}
 });
 onCleanup(()=>{disposed=true;pointerFrames.dispose();props.onCursorTarget?.(undefined);props.onHost?.(undefined);if(focused())call("focus",{focused:false});});
 const local=(event:{x?:number;y?:number})=>({x:(event.x||0)-viewport().x,y:(event.y||0)-viewport().y});
 const move=(event:EventPayload)=>{pointer=local(event);pointerFrames.request(()=>call("pointer",{kind:dragging()?"move":"hover",...pointer}));};
 const finish=(event:EventPayload)=>{pointerFrames.cancel();pointer=local(event);if(dragging())call("pointer",{kind:"move",...pointer});call("pointer",{kind:"up",...pointer});setDragging(false);};
 const keyDown=(event:KeyEvent)=>{
  if(handleApplicationShortcut(renderer,event)||props.onCustomKey?.(event))return;
  const logical=event.key||"",char=event.keyChar||(logical.length===1?logical:"");
  const key=logical.length===1?char:logical.toLowerCase();
  event.preventDefault();event.stopPropagation();call("key",{key:key===" "?"space":key,char,...event.modifiers});
 };
 const label=(id:string)=>id.startsWith("file:")?frame().files[Number(id.slice(5))]?.path||"Changed file":id.startsWith("stage:")?`Stage ${frame().files[Number(id.slice(6))]?.path||"file"}`:id.replaceAll("-"," ");
 return <Show when={visible()}>
  <div ref={value=>{host=value;props.onHost?.(value);}} testId="git-diff-panel" tabIndex={0} role="region" aria-label="Git changes"
   onFocus={()=>{invalidateNativeFocus(renderer);setFocused(true);call("focus",{focused:true});props.onFocus?.();}}
   onBlur={()=>{invalidateNativeFocus(renderer);setFocused(false);call("focus",{focused:false});}}
   onKeyDown={keyDown}
   onMouseDown={event=>{if(event.button!==undefined&&event.button!==0)return;focus(true);pointerFrames.cancel();pointer=local(event);call("pointer",{kind:"down",...pointer});setDragging(true);}}
   onMouseMove={move} onMouseUp={finish}
   onMouseLeave={()=>{if(!dragging())call("pointer",{kind:"hover",x:-1,y:-1});}}
   onScroll={event=>call("scroll",{...local(event),delta:(event.deltaY||0)*(event.precise?1:40),precise:!!event.precise})}
   style={{position:"relative",width:frame().width,height:viewport().height,flexShrink:0,minWidth:0,minHeight:0,overflow:"hidden",userSelect:"none"}}>
   <div testId="git-panel-native-paint" style={{position:"absolute",left:frame().width-viewport().width,top:0,width:viewport().width,height:viewport().height,pointerEvents:"none"}}>
    <PaintLayer commands={frame().commands} config={props.config}/>
   </div>
   <For each={frame().hits.map(hit=>hit.id)}>{id=>{const hit=()=>frame().hits.find(hit=>hit.id===id);return <div testId={`git-panel-hit:${id}`} aria-label={label(id)} style={{position:"absolute",left:(hit()?.rect[0]||0)-viewport().width+frame().width,top:hit()?.rect[1]||0,width:hit()?.rect[2]||0,height:hit()?.rect[3]||0,pointerEvents:"none"}}/>;}}</For>
  </div>
  <Show when={dragging()}><anchored fill="window" deferred priority={4} occlude style={{backgroundColor:"transparent"}}><div testId="git-panel-drag-layer" onMouseMove={move} onMouseUp={finish} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"auto"}}/></anchored></Show>
 </Show>;
}
