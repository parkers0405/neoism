import {createEffect,createMemo,createSignal,onCleanup,untrack} from "solid-js";
import type {AppConfig} from "../config";
import type {EditorFrame,NativeEditor} from "../native-core";
import {coreCall} from "../native-core";
import {PaintLayer,type PaintCommand} from "../ui/PaintLayer";
import {sharePaintCommands} from "./paint-command";
export interface BreadcrumbFrame{height:number;segments:string[];tail:string;commands:PaintCommand[];nextTickMs?:number|null}
/** Original Rust file breadcrumbs are a pure view; notebook/document controls
 * have separate source actions. The body below owns all editor input. */
export function BreadcrumbsView(props:{editor:NativeEditor;frame:EditorFrame;config:AppConfig;path?:string;root?:string;report:(error:unknown)=>void}){
 const id=coreCall<{id:number}>("breadcrumbs.new",{fontFamily:props.config.fontFamily,fontWeight:props.config.fontWeight}).id;
 const [width,setWidth]=createSignal(0),[frame,setFrame]=createSignal<BreadcrumbFrame>({height:26*props.config.scale,segments:[],tail:"",commands:[]});
 let timer:ReturnType<typeof setTimeout>|undefined,disposed=false;
 const draw=()=>{
  clearTimeout(timer);if(disposed||width()<=0)return;
  try{
   const next=coreCall<BreadcrumbFrame>("breadcrumbs.frame",{id,editorId:props.editor.id,width:width(),path:props.path,root:props.root,scale:props.config.scale,theme:props.config.themeName,scrollAnimating:props.frame.animating});
   setFrame(before=>({...next,commands:sharePaintCommands(before.commands,next.commands)}));
   if(next.nextTickMs!=null)timer=setTimeout(draw,Math.max(1,next.nextTickMs));
  }catch(error){props.report(error);}
 };
 // Column moves and paint-only frames do not redo measurements or parsing.
 const sourceKey=createMemo(()=>`${props.frame.revision}:${props.frame.cursor.line}:${props.frame.animating}`);
 createEffect(()=>{sourceKey();width();props.path;props.root;untrack(draw);});
 onCleanup(()=>{disposed=true;clearTimeout(timer);coreCall("breadcrumbs.dispose",{id});});
 return <div testId="code-breadcrumbs" aria-label="Code breadcrumbs" data-breadcrumb-segments={JSON.stringify(frame().segments)} data-breadcrumb-tail={frame().tail}
  onLayout={event=>setWidth(event.width||0)} style={{height:frame().height,minHeight:frame().height,flexShrink:0,position:"relative",overflow:"hidden",pointerEvents:"none"}}>
  <PaintLayer config={props.config} commands={frame().commands}/>
 </div>;
}
