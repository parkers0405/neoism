import { createEffect, createMemo, createSignal, untrack, onCleanup, onMount, Show } from "solid-js";
import { useGpuixRequired,type HostElement,type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { NativeMarkdown,type MarkdownFrame } from "./NativeMarkdown";
import { cursorPaint } from "../editor/paint-frame";
import { MarkdownCover } from "./MarkdownCover";
import { PaintLayer } from "../ui/PaintLayer";
import { cssColor,type CanvasOp } from "../ui/canvas";
import { MarkdownContextMenu } from "./MarkdownContextMenu";
import { coreCall,type SearchHost } from "../native-core";
import { readClipboard,writeClipboard } from "../clipboard";
import { ANIMATION_FRAME_MS,AnimationFrameScheduler } from "../animation/frame-scheduler";
import { focusedNativeElement,focusNativeElement,invalidateNativeFocus } from "../native-focus";
import {registerSurfaceCaret,surfaceCursorSuppressed} from "../ui/cursor-handoff";
import {wheelPixels} from "../ui/wheel";
import {CursorBlinkClock} from "../ui/cursor-blink";
export function MarkdownView(props:{markdown:NativeMarkdown;config:AppConfig;active?:boolean;focusToken:number;onFocus?:()=>void;onClose?:()=>void;onCustomKey?:(event:KeyEvent)=>boolean;save:()=>Promise<void>;report:(error:unknown)=>void;onState:(state:{mode:string;line:number;col:number;dirty:boolean})=>void;openLocation:(path:string,line?:number,col?:number)=>void|Promise<void>;prompt:(kind:"/"|"?"|":")=>void;onSearch?:(kind:"/"|"?",host:SearchHost)=>void}){
 const renderer=useGpuixRequired(),markdown=props.markdown,config=props.config;
 const [focused,setFocused]=createSignal(false),[blink,setBlink]=createSignal(false),[color,setColor]=createSignal(config.cursorColor||config.theme.accent);
 const active=createMemo(()=>props.active!==false);
 const [frame,setFrame]=createSignal(markdown.frame),[bounds,setBounds]=createSignal({x:0,y:0,width:0,height:0});
 const blinkClock=new CursorBlinkClock(setBlink,{scope:renderer});
 onCleanup(()=>blinkClock.dispose());
 createEffect(()=>{const current=frame();blinkClock.update({active:focused()&&active()&&!!current.cursor.rect&&!surfaceCursorSuppressed(renderer),enabled:config.cursorBlink,intervalMs:config.cursorBlinkInterval,animating:current.animating,activityToken:current.cursor.rect?.join(":")});});
 let host:HostElement|undefined,disposed=false,queue=Promise.resolve();
 const animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const focus=()=>{if(host)focusNativeElement(renderer,host.id);};
 let wheelX=0,wheelY=0,wheelPoint={x:0,y:0};
 const schedule=()=>{if(disposed)return;animation.request(()=>{if(wheelX||wheelY){const delta=wheelY,deltaX=wheelX;wheelX=wheelY=0;markdown.call("scroll",{delta,deltaX,...wheelPoint});}else if(markdown.frame.animating)markdown.call("frame");},markdown.frameStartedAt);};
 let lastState="";
 const display=(next:MarkdownFrame)=>{
  setFrame(next);const state={mode:next.mode,line:next.cursor.line,col:next.cursor.col,dirty:markdown.editor.frame.dirty},key=JSON.stringify(state);if(key!==lastState){lastState=key;props.onState(state);}
  if(next.animating||wheelX||wheelY)schedule();
  const fx=next.effects;
  if(fx?.clipboard!=null)void writeClipboard(fx.clipboard).catch(props.report);
  if(fx?.search!=null){if(props.onSearch)props.onSearch(fx.search?"?":"/",{
   preview:query=>{markdown.call("search",{query});},cancel:()=>{markdown.call("search.cancel");},focus,
   commit:query=>{const first=markdown.call("search",{query}).effects?.matches?.[0];if(first)markdown.call("search.commit",{line:first[0],col:first[1]});}
  });else props.prompt(fx.search?"?":"/");}
  if(fx?.palette)props.prompt(":");
  if(fx?.link?.path)void Promise.resolve(props.openLocation(fx.link.path,Math.max(0,(fx.link.line||1)-1),0)).catch(props.report);
  if(fx?.link?.url){const protocol=fx.link.url.split(":")[0].toLowerCase();if(["https","http","mailto","tel"].includes(protocol)){const child=Bun.spawn([process.platform==="darwin"?"open":"xdg-open",fx.link.url],{stderr:"ignore",stdout:"ignore"});void child.exited;}}
 };
 onCleanup(markdown.subscribe(display));
 const focusTicker=setInterval(()=>{const owner=focusedNativeElement(renderer);if(host&&owner!==undefined)setFocused(owner===host.id);if(!focused()||config.cursorStyle!=="rainbow")return;const style=coreCall<{rainbow:number[]}>("cursor",{seconds:performance.now()/1000});setColor("#"+style.rainbow.slice(0,3).map(n=>n.toString(16).padStart(2,"0")).join(""));},1000/60);
 onCleanup(()=>{disposed=true;animation.dispose();clearInterval(focusTicker);});
 createEffect(()=>{const size=bounds();if(size.width>0&&size.height>0)untrack(()=>markdown.call("layout",{width:size.width,height:size.height}));});
 createEffect(()=>{props.focusToken;if(active()&&host)focus();});
 let releaseCaret=()=>{};
 onMount(()=>{if(host)releaseCaret=registerSurfaceCaret(renderer,host,()=>frame().cursorQuad.map(point=>[point[0]+bounds().x,point[1]+bounds().y]));markdown.configure(config);display(markdown.frame);if(active())focus();});
 onCleanup(()=>releaseCaret());
 const pointer=(kind:string,event:{x?:number;y?:number;modifiers?:{ctrl?:boolean;cmd?:boolean};clickCount?:number})=>markdown.call("pointer",{kind,x:(event.x||0)-bounds().x,y:(event.y||0)-bounds().y,ctrl:!!(event.modifiers?.ctrl||event.modifiers?.cmd),clicks:event.clickCount||1});
 const keyDown=(event:KeyEvent)=>{
  const key=event.key?.toLowerCase()||"",primary=event.modifiers?.ctrl||event.modifiers?.cmd;
  if(frame().menu&&(["escape","enter","tab","up","down"].includes(key)||primary&&["n","p"].includes(key))){event.preventDefault();event.stopPropagation();markdown.call("key",{key,ctrl:!!event.modifiers?.ctrl,cmd:!!event.modifiers?.cmd});return;}
  if(!(primary&&["c","x","v"].includes(key))&&props.onCustomKey?.(event))return;
  if(primary&&key==="s"){event.preventDefault();event.stopPropagation();void props.save().then(()=>markdown.call("saved",{},false)).catch(props.report);return;}
  if(event.modifiers?.alt||event.modifiers?.ctrl&&["tab","p","t","`"].includes(key))return;
  event.preventDefault();event.stopPropagation();
  queue=queue.then(async()=>{
   let clipboard:string|undefined;
   if(primary&&key==="v"||!primary&&markdown.frame.mode!=="Insert"&&["p","P","."].includes(event.keyChar||key))clipboard=await readClipboard();
   const result=markdown.call("key",{key,char:event.keyChar||(key.length===1?key:""),ctrl:!!event.modifiers?.ctrl,cmd:!!event.modifiers?.cmd,alt:!!event.modifiers?.alt,shift:!!event.modifiers?.shift,clipboard});
   if(result.hostAction==="close")props.onClose?.();
  }).catch(props.report);
 };
 const caret=createMemo(()=>cursorPaint(frame().cursorQuad));
 const caretShown=createMemo(()=>!!(focused()&&!surfaceCursorSuppressed(renderer)&&frame().cursor.rect&&blink()&&frame().cursorQuad.length));
 const caretOps=createMemo(():CanvasOp[]=>caretShown()?[{k:"p",c:cssColor(color()),p:[frame().cursorQuad.flatMap(point=>[point[0],point[1]])]}]:[]);
 return <div ref={host} testId="rich-markdown-editor" role="textbox" aria-label="Rich Markdown editor" tabIndex={0} onKeyDown={keyDown}
  onFocus={()=>{invalidateNativeFocus(renderer);setFocused(true);props.onFocus?.();}} onBlur={()=>{invalidateNativeFocus(renderer);setFocused(false);if(markdown.frame.dragging)markdown.call("pointer",{kind:"up"});}}
  onLayout={event=>{const next={x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0};setBounds(old=>old.x===next.x&&old.y===next.y&&old.width===next.width&&old.height===next.height?old:next);}}
  onMouseDown={event=>{if(event.button===2){props.onFocus?.();focus();markdown.call("context",{x:(event.x||0)-bounds().x,y:(event.y||0)-bounds().y});}else if(event.button===0){props.onFocus?.();focus();pointer("down",event);}}}
  onMouseMove={event=>{if(markdown.frame.dragging||event.pressedButton===0)pointer("move",event);else markdown.call("hover",{x:(event.x||0)-bounds().x,y:(event.y||0)-bounds().y});}}
  onMouseLeave={()=>markdown.call("hover")}
  onMouseUp={event=>pointer("up",event)}
  onScroll={event=>{const wheel=wheelPixels(event,config.fontSize);wheelY+=wheel.y;wheelX+=wheel.x;wheelPoint={x:(event.x||0)-bounds().x,y:(event.y||0)-bounds().y};schedule();}}
  style={{position:"relative",flexGrow:1,minHeight:0,minWidth:0,overflow:"hidden",userSelect:"none",cursor:"text"}}>
  <PaintLayer commands={frame().commands} config={config} extra={caretOps()}/>
  <MarkdownCover markdown={markdown} cover={frame().cover}/>
  {/* The canvas paints the caret; this empty box only locates it for automation. */}
  <Show when={caretShown()}><div testId="rich-markdown-caret" style={{position:"absolute",left:caret().left,top:caret().top,width:caret().width,height:caret().height,pointerEvents:"none"}}/></Show>
  <MarkdownContextMenu markdown={markdown} config={config} menu={frame().menu} origin={bounds()}/>
 </div>;
}
