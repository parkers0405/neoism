import {createEffect,createMemo,createSignal,Index,onCleanup,onMount} from "solid-js";
import {createWindowSize,useGpuixRequired,type HostElement,type KeyEvent} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {coreCall,type NativeEditor} from "../native-core";
import {PaintLayer,type PaintCommand} from "./PaintLayer";
import {sharePaintCommands} from "../editor/paint-command";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {focusedNativeElement,focusNativeElement} from "../native-focus";
import {useDialogViewport,overlayPoint} from "./overlay-viewport";
import {handleApplicationShortcut} from "../commands/application-shortcuts";
import {writeClipboard} from "../clipboard";
interface Frame {visible:boolean;animating:boolean;rect:number[]|null;rows:Array<{index:number;rect:number[];name:string;state:string}>;selected:number;clipboard?:string|null;commands:PaintCommand[]}

/** Paint and interaction are owned by the original Rust LspPopup. */
export function LspStatusPopup(props:{config:AppConfig;editor:NativeEditor;anchor:readonly number[];onDone:()=>void;report:(error:unknown)=>void}){
 const renderer=useGpuixRequired(),viewport=useDialogViewport(renderer,createWindowSize()),config=props.config;
 const{id}=coreCall<{id:number}>("lsppopup.new",{editorId:props.editor.id,fontFamily:config.fontFamily,fontWeight:config.fontWeight,iconFontFamily:config.iconFontFamily});
 const[frame,setFrame]=createSignal<Frame>({visible:true,animating:false,rect:null,rows:[],selected:0,commands:[]});
 const animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);let host:HostElement|undefined,disposed=false,opened=false,finished=false;
 const previous=focusedNativeElement(renderer);
 const parameters=createMemo(()=>({id,width:viewport().width,height:viewport().height,scale:config.scale,theme:config.themeName,anchorX:props.anchor[0]-viewport().x,anchorY:props.anchor[1]-viewport().y,anchorWidth:props.anchor[2],anchorHeight:props.anchor[3]}));
 const sample=(op="frame",args:Record<string,unknown>={})=>{
  if(disposed)return;const started=performance.now();animation.cancel();
  try{
   const next=coreCall<Frame>(`lsppopup.${opened?op:"open"}`,{...parameters(),...args});opened=true;
   setFrame(previous=>({...next,commands:sharePaintCommands(previous.commands,next.commands)}));
   if(next.clipboard!=null)void writeClipboard(next.clipboard).catch(props.report);
   if(next.animating)animation.request(()=>sample(),started);
   if(!next.visible&&!finished){finished=true;props.onDone();}
  }catch(error){props.report(error);props.onDone();}
 };
 createEffect(()=>{parameters();sample();});
 let revision=props.editor.frame.lsp?.statusRevision,diagnostics=props.editor.frame.lsp?.diagnostics;
 onCleanup(props.editor.subscribe(frame=>{
  if(frame.lsp?.statusRevision===revision&&frame.lsp?.diagnostics===diagnostics)return;
  revision=frame.lsp?.statusRevision;diagnostics=frame.lsp?.diagnostics;sample();
 }));
 onMount(()=>{if(host)focusNativeElement(renderer,host.id);});
 onCleanup(()=>{disposed=true;animation.dispose();coreCall("lsppopup.dispose",{id});if(host&&focusedNativeElement(renderer)===host.id&&previous)focusNativeElement(renderer,previous);});
 const point=(event:{x?:number;y?:number})=>overlayPoint(viewport(),event);
 const key=(event:KeyEvent)=>{if(handleApplicationShortcut(renderer,event))return;const name=event.key?.toLowerCase();if(!name||!["escape","up","down"].includes(name)||event.modifiers?.alt||event.modifiers?.ctrl||event.modifiers?.cmd)return;event.preventDefault();event.stopPropagation();sample("key",{key:name});};
 return <anchored fill="window" deferred priority={3} occlude={false} style={{pointerEvents:"none",backgroundColor:"transparent"}}>
  <div style={{position:"absolute",left:viewport().x,top:viewport().y,width:viewport().width,height:viewport().height,pointerEvents:"none",overflow:"hidden"}}>
   <PaintLayer commands={frame().commands} config={config}/>
   <div ref={value=>host=value} tabIndex={0} testId="lsp-status-popup" role="dialog" aria-label="Language servers" onKeyDown={key}
    onMouseDownOutside={event=>{const[x,y,w,h]=props.anchor;if(event.x!==undefined&&event.y!==undefined&&event.x>=x&&event.x<x+w&&event.y>=y&&event.y<y+h)return;sample("close");}}
    onMouseMove={event=>sample("pointer",point(event))}
    onMouseDown={event=>{if(event.button===undefined||event.button===0)sample("pointer",{...point(event),activate:true});}}
    onScroll={event=>sample("wheel",{...point(event),deltaY:event.deltaY||0,precise:!!event.precise})}
    style={{position:"absolute",left:frame().rect?.[0]||0,top:frame().rect?.[1]||0,width:frame().rect?.[2]||0,height:frame().rect?.[3]||0,pointerEvents:"auto"}}>
    <Index each={frame().rows}>{row=><div role="option" testId={`lsp-server:${row().name}`} aria-label={`${row().name}: ${row().state}`} aria-selected={frame().selected===row().index}
     style={{position:"absolute",left:row().rect[0]-(frame().rect?.[0]||0),top:row().rect[1]-(frame().rect?.[1]||0),width:row().rect[2],height:row().rect[3],pointerEvents:"none"}}/>}</Index>
   </div>
  </div>
 </anchored>;
}
