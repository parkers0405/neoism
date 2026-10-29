import{createEffect,createMemo,createSignal,Index,onCleanup,onMount}from"solid-js";
import{createWindowSize,useGpuixRequired,type HostElement,type KeyEvent}from"@gpuix/solid";
import type{AppConfig}from"../config";
import{coreCall,type NativeEditor}from"../native-core";
import{PaintLayer,type PaintCommand}from"./PaintLayer";
import{sharePaintCommands}from"../editor/paint-command";
import{AnimationFrameScheduler,ANIMATION_FRAME_MS}from"../animation/frame-scheduler";
import{focusedNativeElement,focusNativeElement}from"../native-focus";
import{useDialogViewport,overlayPoint}from"./overlay-viewport";
import{handleApplicationShortcut}from"../commands/application-shortcuts";
interface Frame{visible:boolean;interactive:boolean;animating:boolean;rect:number[]|null;rows:Array<{index:number;rect:number[];line:number;message:string}>;selected:number;target?:{line:number;column:number}|null;commands:PaintCommand[]}
/** The original Rust badge popover: severity/count, rows, animation and hit
 * tests all stay in DiagnosticsPopup and the active editor's LSP snapshot. */
export function DiagnosticsPopup(props:{config:AppConfig;editor:NativeEditor;pill:"Error"|"Warn";anchor:readonly number[];onActivate:(line:number,column:number)=>void;onDone:()=>void;report:(error:unknown)=>void}){
 const renderer=useGpuixRequired(),viewport=useDialogViewport(renderer,createWindowSize()),config=props.config;
 const{id}=coreCall<{id:number}>("diagnosticpopup.new",{editorId:props.editor.id,pill:props.pill,fontFamily:config.fontFamily,fontWeight:config.fontWeight,iconFontFamily:config.iconFontFamily});
 const[frame,setFrame]=createSignal<Frame>({visible:true,interactive:false,animating:false,rect:null,rows:[],selected:0,commands:[]});
 const animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);let host:HostElement|undefined,disposed=false,opened=false,finished=false;
 const previous=focusedNativeElement(renderer);
 const parameters=createMemo(()=>({id,width:viewport().width,height:viewport().height,scale:config.scale,theme:config.themeName,anchorX:props.anchor[0]-viewport().x,anchorY:props.anchor[1]-viewport().y}));
 const sample=(op="frame",args:Record<string,unknown>={})=>{
  if(disposed)return;const started=performance.now();animation.cancel();
  try{
   const next=coreCall<Frame>(`diagnosticpopup.${opened?op:"open"}`,{...parameters(),...args});opened=true;
   setFrame(previous=>({...next,commands:sharePaintCommands(previous.commands,next.commands)}));
   if(next.target)props.onActivate(next.target.line,next.target.column);
   if(next.animating)animation.request(()=>sample(),started);
   if(!next.visible&&!finished){finished=true;props.onDone();}
  }catch(error){props.report(error);props.onDone();}
 };
 createEffect(()=>{parameters();sample();});
 onCleanup(props.editor.subscribe(()=>sample()));
 onMount(()=>{if(host)focusNativeElement(renderer,host.id);});
 onCleanup(()=>{disposed=true;animation.dispose();coreCall("diagnosticpopup.dispose",{id});if(host&&focusedNativeElement(renderer)===host.id&&previous)focusNativeElement(renderer,previous);});
 const point=(event:{x?:number;y?:number})=>overlayPoint(viewport(),event);
 const key=(event:KeyEvent)=>{if(handleApplicationShortcut(renderer,event))return;const name=event.key?.toLowerCase();if(!name||!["escape","enter","up","down"].includes(name)||event.modifiers?.alt||event.modifiers?.ctrl||event.modifiers?.cmd)return;event.preventDefault();event.stopPropagation();if(name==="escape"||frame().interactive)sample("key",{key:name});};
 return <anchored fill="window" deferred priority={3} occlude={false} style={{pointerEvents:"none",backgroundColor:"transparent"}}>
  <div style={{position:"absolute",left:viewport().x,top:viewport().y,width:viewport().width,height:viewport().height,pointerEvents:"none",overflow:"hidden"}}>
   <PaintLayer commands={frame().commands} config={config}/>
   <div ref={value=>host=value} tabIndex={0} testId="diagnostics-popup" role="dialog" aria-label={props.pill==="Error"?"Errors":"Warnings"} onKeyDown={key}
    onMouseDownOutside={()=>sample("close")}
    onScroll={event=>{sample("wheel",{...point(event),deltaX:event.deltaX||0,deltaY:event.deltaY||0,precise:!!event.precise});}}
    style={{position:"absolute",left:frame().rect?.[0]||0,top:frame().rect?.[1]||0,width:frame().rect?.[2]||0,height:frame().rect?.[3]||0,pointerEvents:frame().interactive?"auto":"none"}}>
    <Index each={frame().rows}>{row=><div role="option" testId={`diagnostic-row:${row().index}`} aria-label={`Line ${row().line}: ${row().message}`} aria-selected={frame().selected===row().index}
      onMouseMove={event=>sample("pointer",point(event))} onMouseDown={event=>{if(event.button===undefined||event.button===0){sample("pointer",{...point(event),activate:true});}}}
      style={{position:"absolute",left:row().rect[0]-(frame().rect?.[0]||0),top:row().rect[1]-(frame().rect?.[1]||0),width:row().rect[2],height:row().rect[3],cursor:"pointer",pointerEvents:frame().interactive?"auto":"none"}}/>}</Index>
   </div>
  </div>
 </anchored>;
}
