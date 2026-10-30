import {createEffect,createMemo,createSignal,Index,onCleanup,onMount,Show,untrack} from "solid-js";
import {useGpuixRequired,type HostElement,type KeyEvent} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {PaintLayer,type PaintCommand} from "./PaintLayer";
import {AnimatedWordmark,type WordmarkHosts} from "./AnimatedWordmark";
import {sharePaintCommands} from "../editor/paint-command";
import {focusNativeElement} from "../native-focus";
interface SplashFrame{commands:PaintCommand[];rows:Array<{action:string;label:string;key:string;rect:number[]}>;wordmark:number[];alpha:number}
/** Independent Welcome tab: original splash composition, without a terminal. */
export function SplashView(props:{contextId:string;config:AppConfig;wordmarkHosts?:WordmarkHosts;initialViewport?:{width:number;height:number};active?:boolean;focusToken?:number;onCustomKey?:(event:KeyEvent)=>boolean;onFocus?:()=>void;onAction:(action:string)=>void}){
 const config=props.config,renderer=useGpuixRequired();
 const id=coreCall<{id:number}>("splash.new",{fontFamily:config.fontFamily,fontWeight:config.fontWeight}).id;
 const [viewport,setViewport]=createSignal(props.initialViewport&&props.initialViewport.width>0&&props.initialViewport.height>0?props.initialViewport:props.wordmarkHosts?.splashViewport(props.contextId)||{width:0,height:0});
 const [frame,setFrame]=createSignal<SplashFrame>({commands:[],rows:[],wordmark:[0,0,0,0],alpha:1});
 const active=createMemo(()=>props.active!==false);let host:HostElement|undefined,disposed=false,pointer:{x:number;y:number}|undefined;
 const focus=()=>{if(host&&active()){host.root?.mutations.flushMutations();focusNativeElement(renderer,host.id);}};
 const draw=()=>{
  if(disposed)return;const view=viewport();if(view.width<=0||view.height<=0)return;
  const next=coreCall<SplashFrame>("splash.frame",{id,...view,cols:Math.floor(view.width/(config.fontSize*.6)),rows:Math.floor(view.height/(config.fontSize*1.3)),cellHeight:config.fontSize*1.3,scale:config.scale,theme:config.themeName,...(pointer?{pointer:true,...pointer}:{})});
  setFrame(before=>({...next,commands:sharePaintCommands(before.commands,next.commands)}));
 };
 // The parent already knows the native pane bounds. Prepare menu glyphs and
 // logo geometry in the mount transaction, before any asynchronous onLayout.
 // Cached bounds remain a fallback for hosts without initial geometry.
 draw();
 createEffect(()=>{viewport();untrack(draw);});
 createEffect(()=>{props.focusToken;if(active())focus();});onMount(focus);
 onCleanup(()=>{disposed=true;coreCall("splash.dispose",{id});});
 return <div ref={host} testId="welcome-page" tabIndex={0} role="region" aria-label="Welcome" onFocus={props.onFocus} onMouseDown={()=>{props.onFocus?.();focus();}} onKeyDown={event=>{props.onCustomKey?.(event);}}
  onLayout={event=>{const next={width:event.width||0,height:event.height||0};props.wordmarkHosts?.rememberSplashViewport(props.contextId,next);setViewport(before=>before.width===next.width&&before.height===next.height?before:next);}}
  style={{position:"absolute",left:8*config.scale,top:8*config.scale,right:8*config.scale,bottom:8*config.scale}}>
  <PaintLayer commands={frame().commands} config={config} pinImage={(source,raster)=>props.wordmarkHosts?.pinPaintImage(renderer,source,raster)}/>
  <Show when={frame().wordmark[2]>0}><AnimatedWordmark config={config} kind="splash" startupKey={props.contextId} hosts={props.wordmarkHosts} testId="splash-wordmark" paintBounds={[-frame().wordmark[0],-frame().wordmark[1],viewport().width,viewport().height]}
   style={{position:"absolute",left:frame().wordmark[0],top:frame().wordmark[1],width:frame().wordmark[2],height:frame().wordmark[3],pointerEvents:"auto"}}/></Show>
  <Index each={frame().rows}>{item=><div role="button" aria-label={item().label} testId={`splash-action:${item().action}`} onClick={()=>props.onAction(item().action)} onMouseEnter={()=>{pointer={x:item().rect[0]+item().rect[2]/2,y:item().rect[1]+item().rect[3]/2};draw();}} onMouseLeave={()=>{pointer=undefined;draw();}}
   style={{position:"absolute",left:item().rect[0],top:item().rect[1],width:item().rect[2],height:item().rect[3],cursor:"pointer"}}/>}</Index>
 </div>;
}
