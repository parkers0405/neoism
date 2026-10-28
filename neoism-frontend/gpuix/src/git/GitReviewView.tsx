import {createEffect,createMemo,createSignal,onCleanup,onMount,Show} from "solid-js";
import {useGpuixRequired,type HostElement,type KeyEvent} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {PaintLayer} from "../ui/PaintLayer";
import {cssColor,type CanvasOp} from "../ui/canvas";
import {focusNativeElement,invalidateNativeFocus} from "../native-focus";
import {registerSurfaceCaret,surfaceCursorSuppressed} from "../ui/cursor-handoff";
import {CursorBlinkClock} from "../ui/cursor-blink";
import {wheelPixels} from "../ui/wheel";
import type {GitReviewController} from "./GitReviewController";
/** Original GitDiffView painter and input host, including the canonical source
 * document's projected working column. No second editor buffer is created. */
export function GitReviewView(props:{controller:GitReviewController;config:AppConfig;active?:boolean;focusToken?:number;onFocus?:()=>void;onCustomKey?:(event:KeyEvent)=>boolean;onState?:(state:{mode:string;line:number;col:number;dirty:boolean})=>void}){
 const renderer=useGpuixRequired(),controller=props.controller,config=props.config;
 const [frame,setFrame]=createSignal(controller.frame),[focused,setFocused]=createSignal(false),[blink,setBlink]=createSignal(false);
 const active=createMemo(()=>props.active!==false);
 const [bounds,setBounds]=createSignal({x:0,y:0,width:0,height:0});let host:HostElement|undefined;
 const clock=new CursorBlinkClock(setBlink,{scope:renderer});onCleanup(()=>clock.dispose());onCleanup(controller.subscribe(setFrame));
 createEffect(()=>{controller.configure(config);});
 createEffect(()=>{const current=frame();props.onState?.({mode:current.cursorShape||"Normal",line:current.cursorToken?.[0]||0,col:current.cursorToken?.[1]||0,dirty:!!current.dirty});});
 createEffect(()=>{props.focusToken;if(active()&&host)focusNativeElement(renderer,host.id);});
 createEffect(()=>{const current=frame();clock.update({active:focused()&&active()&&!!current.editingPath&&!surfaceCursorSuppressed(renderer),enabled:config.cursorBlink,intervalMs:config.cursorBlinkInterval,animating:current.animating,activityToken:current.cursorToken?.join(":")+":"+current.cursorShape});});
 let releaseCaret=()=>{};onMount(()=>{if(host){releaseCaret=registerSurfaceCaret(renderer,host,()=>frame().cursorQuad?.map(([x,y])=>[x+bounds().x,y+bounds().y])||[]);if(active())focusNativeElement(renderer,host.id);}});onCleanup(()=>releaseCaret());
 const cursor=()=>focused()&&active()&&blink()&&!surfaceCursorSuppressed(renderer)&&frame().cursorQuad?.length===4;
 const extra=createMemo<CanvasOp[]>(()=>cursor()?[{k:"p",p:[frame().cursorQuad!.flat()],c:cssColor(config.cursorColor||config.theme.accent)}]:[]);
 const caretBounds=createMemo(()=>{const q=frame().cursorQuad||[];if(q.length!==4)return;const xs=q.map(p=>p[0]),ys=q.map(p=>p[1]);return{x:Math.min(...xs),y:Math.min(...ys),width:Math.max(...xs)-Math.min(...xs),height:Math.max(...ys)-Math.min(...ys)};});
 const keyDown=(event:KeyEvent)=>{if(props.onCustomKey?.(event)){event.preventDefault();event.stopPropagation();return;}if(event.modifiers?.alt)return;
  const key=event.key?.toLowerCase()||"";if((event.modifiers?.ctrl||event.modifiers?.cmd)&&key==="s"){event.preventDefault();event.stopPropagation();void controller.save().catch(controller.options.model.report);return;}
  event.preventDefault();event.stopPropagation();void controller.queueKey({key,char:event.keyChar||"",ctrl:!!event.modifiers?.ctrl,shift:!!event.modifiers?.shift,alt:false,cmd:!!event.modifiers?.cmd});
 };
 return <div ref={host} testId="git-review" role="document" aria-label={controller.descriptor.mode==="working"?"Uncommitted Changes":"Git review"} tabIndex={0}
  onFocus={()=>{invalidateNativeFocus(renderer);setFocused(true);controller.call("focus",{focused:true});props.onFocus?.();}}
  onBlur={()=>{invalidateNativeFocus(renderer);setFocused(false);controller.call("focus",{focused:false});}}
  onKeyDown={keyDown}
  onLayout={event=>{const next={x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0},old=bounds();if(next.x!==old.x||next.y!==old.y||next.width!==old.width||next.height!==old.height)setBounds(next);if(next.width>0&&next.height>0&&(next.width!==old.width||next.height!==old.height))controller.layout(next.width,next.height);}}
  onScroll={event=>{const wheel=wheelPixels(event,config.fontSize);controller.call("scroll",{deltaY:event.modifiers?.shift&&Math.abs(wheel.x)<.01?0:-wheel.y,deltaX:event.modifiers?.shift&&Math.abs(wheel.x)<.01?-wheel.y:-wheel.x,x:(event.x||0)-bounds().x,y:(event.y||0)-bounds().y});}}
  onMouseDown={event=>{if(event.button!==0)return;props.onFocus?.();if(host)focusNativeElement(renderer,host.id);controller.pointer("down",(event.x||0)-bounds().x,(event.y||0)-bounds().y);}}
  onMouseMove={event=>controller.pointer("move",(event.x||0)-bounds().x,(event.y||0)-bounds().y)}
  onMouseUp={event=>controller.pointer("up",(event.x||0)-bounds().x,(event.y||0)-bounds().y)}
  onMouseLeave={()=>controller.pointer("leave",0,0)}
  style={{flexGrow:1,minHeight:0,minWidth:0,position:"relative",overflow:"hidden",userSelect:"none",cursor:frame().pointer?"pointer":frame().textPointer?"text":"default"}}>
  <PaintLayer commands={frame().commands} extra={extra()} config={config}/>
  <Show when={cursor()&&caretBounds()}>{rect=><div testId="git-review-caret" style={{position:"absolute",left:rect().x,top:rect().y,width:rect().width,height:rect().height,pointerEvents:"none"}}/>}</Show>
 </div>;
}
