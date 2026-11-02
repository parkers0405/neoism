import type {WorkspaceContext} from "../workspaces/window-sessions";
import {createEffect,createMemo,createSignal,For,onCleanup,Show,untrack,type JSX} from "solid-js";
import {motion,useGpuixRequired,createWindowSize,type EventPayload,type HostElement} from "@gpuix/solid";
import type {AppConfig} from "../config";
import type {Workspace} from "../daemon/protocol";
import {tabChrome,rgba,workspaceLabel,workspaceTabRoot} from "../layout/tab-chrome";
import {easeOutCubicBezier} from "../design/animation";
import {paintedChromeBounds,type ChromeBounds} from "./chrome-target-bounds";
import {HoverTabTitle} from "../layout/HoverTabTitle";
import {verticalTabScroll,consumeTabScroll} from "../layout/tab-scroll";
import {TabSlide} from "../layout/tab-slide";
import {workspaceIcon,cloudWorkspace} from "../workspaces/workspace-icon";
import {invalidateNativeFocus} from "../native-focus";

/** Window-owned workspace tabs size to their titles and share one horizontal strip with their add button. */
export function WorkspaceTabs(props:{
 workspaceContext?:(id:string)=>WorkspaceContext|undefined;
 config:AppConfig;workspaces:readonly Workspace[];activeId?:string;keyboardIndex?:number;connectedHostId?:string;directoryRoots?:unknown;
 onActivate:(workspace:Workspace)=>void;onNew:()=>void;onClose:(workspaceId:string)=>void;
 onReorder?:(dragId:string,targetId:string,position:"before"|"after")=>void;
 registerHost:(id:string,host:HostElement|undefined)=>void;
 registerNewHost:(host:HostElement|undefined)=>void;
  onBounds:(host:HostElement,bounds:ChromeBounds)=>void;onScroll:()=>void;
  accessory?:JSX.Element;accessoryWidth?:number;
}){
 const context=(id:string):WorkspaceContext=>props.workspaceContext?.(id)||{config:props.config,connectedHostId:props.connectedHostId,directoryRoots:props.directoryRoots};
 const c=props.config.theme,s=props.config.scale,spec=tabChrome(s),chrome=spec.workspace;
 const row={display:"flex" as const,flexDirection:"row" as const,alignItems:"center" as const};
 const text={fontFamily:props.config.fontFamily,fontWeight:props.config.fontWeight,fontSize:chrome.titleFontSize,color:c.fg,whiteSpace:"nowrap" as const,pointerEvents:"none" as const,userSelect:"none" as const};
 const windowSize=createWindowSize(),[viewportWidth,setViewportWidth]=createSignal(0);
 const renderer=useGpuixRequired(),hosts=new Map<string,HostElement>();
 const [drag,setDrag]=createSignal<string>();
 const [slideRevision,setSlideRevision]=createSignal(0),[widthRevision,setWidthRevision]=createSignal(0);
 const widths=new Map<string,number>(),slide=new TabSlide<string>({reducedMotion:props.config.reducedMotion,onChange:()=>setSlideRevision(value=>value+1)});
 let dragOrder:string[]|undefined,grab=0,closeGesture=false;
 let armed:{id:string;x:number;y:number;grab:number}|undefined,scrollHost:HostElement|undefined,suppressActivation=false,pointer={x:0,y:0},scrollX=0,stripOriginX=0,stripOriginY=0,stripWidth=0;
 const layoutSlides=()=>{const workspaces=props.workspaces;const entries=(dragOrder||workspaces.map(workspace=>workspace.id)).map(id=>({id,width:widths.get(id)||0}));if(entries.every(entry=>entry.width>0))slide.layout(entries);};
 createEffect(()=>{widthRevision();layoutSlides();});
 const rememberWidth=(id:string,width:number)=>{if(width>0&&Math.abs((widths.get(id)||0)-width)>.01){widths.set(id,width);setWidthRevision(value=>value+1);}};
 const measureWidths=()=>{for(const[id,host]of hosts){const bounds=host.mounted&&paintedChromeBounds(renderer,host.id);if(bounds)rememberWidth(id,bounds.width);}layoutSlides();};
 const contentPoint=(x:number,refresh=false)=>{
  if(refresh){const clip=scrollHost&&paintedChromeBounds(renderer,scrollHost.id);if(clip){stripOriginX=clip.x;stripOriginY=clip.y;stripWidth=clip.width;}const offset=scrollHost&&renderer.getScrollOffset?.(scrollHost.id);if(offset)scrollX=offset[0]||0;}
  return x-stripOriginX-scrollX;
 };
 // Use the buffer strip's drag-center crossing policy. Neighbors slide as
 // soon as their logical slot changes, while the dragged tab follows the hand.
 const reorderInStrip=()=>{
  const id=drag();if(!id||!dragOrder||!props.onReorder)return;
  if(pointer.x<stripOriginX||pointer.x>stripOriginX+stripWidth||pointer.y<stripOriginY||pointer.y>stripOriginY+chrome.height+36*s)return;
  let order=dragOrder;
  for(let pass=0;pass<order.length;pass++){
   const index=order.indexOf(id);if(index<0)return;
   const width=widths.get(id)||0,start=order.slice(0,index).reduce((sum,id)=>sum+(widths.get(id)||0),0);
   const center=contentPoint(pointer.x)-grab+width/2;
   const target=center>start+width?order[index+1]:center<start?order[index-1]:undefined;
   if(!target)break;
   const position=center<start?"before":"after";
   const next:string[]=order.filter(key=>key!==id);next.splice(next.indexOf(target)+(position==="after"?1:0),0,id);dragOrder=order=next;
   props.onReorder(id,target,position);layoutSlides();
  }
 };
 const move=(event:EventPayload)=>{
  if(event.pressedButton!==0){if(drag())slide.end();armed=undefined;setDrag(undefined);dragOrder=undefined;return;}
  pointer={x:event.x??0,y:event.y??0};
  if(armed&&!drag()&&Math.hypot(pointer.x-armed.x,pointer.y-armed.y)>=5*s){
   dragOrder=props.workspaces.map(workspace=>workspace.id);grab=armed.grab;
   slide.begin(armed.id,contentPoint(pointer.x),grab);setDrag(armed.id);armed=undefined;suppressActivation=true;
  }
  if(drag()){slide.move(contentPoint(pointer.x));reorderInStrip();}
 };
 const finish=(event:EventPayload)=>{
  pointer={x:event.x??0,y:event.y??0};
  if(drag()){contentPoint(pointer.x,true);slide.move(contentPoint(pointer.x));reorderInStrip();slide.end();props.onScroll();}
  armed=undefined;setDrag(undefined);dragOrder=undefined;
 };
 let newHost:HostElement|undefined;
 onCleanup(()=>{slide.dispose();props.registerNewHost(undefined);});
 function Tab(p:{id:string;index:number}){
  const initial=untrack(()=>props.workspaces.find(w=>w.id===p.id))!;
  const workspace=createMemo(()=>props.workspaces.find(w=>w.id===p.id)||initial);
  const title=createMemo(()=>workspaceLabel(workspace().title,workspaceTabRoot(workspace(),context(p.id).config,context(p.id).directoryRoots,context(p.id).workspaceId)));
  const selected=()=>props.activeId===p.id;
  const [hovered,setHovered]=createSignal(false),[closeHovered,setCloseHovered]=createSignal(false);
  const [titleWidth,setTitleWidth]=createSignal([...title()].length*chrome.titleFontSize*.6);
  const titleBudget=()=>Math.min(titleWidth(),Math.max(24*s,(viewportWidth()||windowSize().width)-chrome.tabPaddingX-(9*s+spec.buffer.closeHitSize/2+spec.buffer.padding)-9*s-12*s-spec.buffer.newWidth));
  let closingClick=false;
  const close=(event?:EventPayload)=>{if(event?.button!==undefined&&event.button!==0)return;closingClick=true;closeGesture=true;armed=undefined;suppressActivation=false;props.onClose(p.id);};
  let host:HostElement|undefined,closeHost:HostElement|undefined,closeLayout:ChromeBounds|undefined,queued=false,disposed=false;
  const press=(event:EventPayload)=>{
   if(event.button!==undefined&&event.button!==0)return;
   const closeBounds=(closeHost&&paintedChromeBounds(renderer,closeHost.id))||closeLayout,x=event.x??0,y=event.y??0;
   if(closeBounds&&x>=closeBounds.x&&x<=closeBounds.x+closeBounds.width&&y>=closeBounds.y&&y<=closeBounds.y+closeBounds.height){armed=undefined;closingClick=true;return;}
   // A refused close can end outside the X, so its click never clears the
   // latch. A fresh press outside the current close bounds is a new gesture.
   closeGesture=false;closingClick=false;suppressActivation=false;if(props.onReorder){measureWidths();armed={id:p.id,x,y,grab:Math.max(0,Math.min(widths.get(p.id)||0,contentPoint(x,true)-slide.position(p.id)))};}
   // Activate on press, but consume the later click so release cannot switch again.
   suppressActivation=true;props.onActivate(workspace());
  };
  const reveal=()=>{if(queued)return;queued=true;queueMicrotask(()=>{queued=false;if(disposed||!host?.mounted||!selected()||drag())return;host.root?.mutations.flushMutations();host.scrollIntoView();if(props.workspaces.at(-1)?.id===p.id)newHost?.scrollIntoView();props.onScroll();});};
  createEffect(()=>{if(selected())reveal();});
  onCleanup(()=>{disposed=true;hosts.delete(p.id);widths.delete(p.id);props.registerHost(p.id,undefined);});
  return <div ref={value=>{host=value;hosts.set(p.id,value);props.registerHost(p.id,value);}} tabIndex={0} role="tab" testId={`workspace:${p.id}`} aria-label={title()} aria-selected={selected()} data-keyboard-focused={props.keyboardIndex===p.index}
   onFocus={()=>invalidateNativeFocus(renderer)} onBlur={()=>invalidateNativeFocus(renderer)}
   onLayout={event=>{rememberWidth(p.id,event.width||0);if(host)props.onBounds(host,{x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0});if(selected())reveal();}}
   onMouseDown={press}
   onMouseMove={move} onMouseUp={finish}
   onMouseEnter={()=>setHovered(true)} onMouseLeave={()=>setHovered(false)} onClick={()=>{if(!closingClick&&!closeGesture&&!suppressActivation)props.onActivate(workspace());suppressActivation=false;}}
   style={{...row,position:"relative",left:(slideRevision(),slide.offset(p.id)),opacity:drag()===p.id?0:1,paddingLeft:chrome.tabPaddingX,paddingRight:9*s+spec.buffer.closeHitSize/2+spec.buffer.padding,gap:9*s,height:chrome.height,flexShrink:0,overflow:"hidden",cursor:drag()===p.id?"grabbing":"pointer",backgroundColor:selected()||drag()===p.id?c.bg:undefined,borderTopLeftRadius:chrome.tabRadius,borderTopRightRadius:chrome.tabRadius}}>
   <motion.div initial={false} animate={{opacity:(hovered()||props.keyboardIndex===p.index)&&!selected()?1:0}} transition={{duration:.15,ease:easeOutCubicBezier}} style={{position:"absolute",left:0,right:0,top:0,height:chrome.height,backgroundColor:c.accent+"17",pointerEvents:"none"}}/>
   <text testId={`workspace-icon:${p.id}`} aria-label={cloudWorkspace(workspace(),context(p.id).config,context(p.id).connectedHostId)?"Cloud workspace":"Local workspace"} style={{...text,flexShrink:0,lineHeight:chrome.height,fontFamily:props.config.iconFontFamily,fontSize:12*s,color:rgba(chrome.iconColor)}}>{workspaceIcon(workspace(),context(p.id).config,chrome.iconGlyph,context(p.id).connectedHostId)}</text>
   <text onLayout={event=>{if(event.width)setTitleWidth(event.width);}} style={{...text,position:"absolute",opacity:0,left:0,top:0}}>{title()}</text>
   <HoverTabTitle testId={`workspace-title:${p.id}`} label={title()} width={titleBudget()} naturalWidth={titleWidth()} height={chrome.height} scale={s} hovered={hovered()} reducedMotion={props.config.reducedMotion} style={{...text,lineHeight:chrome.height,color:selected()?c.fg:c.muted}}/>
   <div ref={value=>closeHost=value} role="button" aria-label={`Close ${workspace().title}`} testId={`workspace-close:${p.id}`} onClick={()=>{closingClick=true;}}
    onMouseDown={close}
    onLayout={event=>{closeLayout={x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0};}}
    onMouseEnter={()=>setCloseHovered(true)} onMouseLeave={()=>setCloseHovered(false)}
    style={{...row,position:"absolute",right:spec.buffer.padding-spec.buffer.closeHitSize/2,top:(chrome.height-spec.buffer.closeHitSize)/2,justifyContent:"center",width:spec.buffer.closeHitSize,height:spec.buffer.closeHitSize,flexShrink:0,cursor:"pointer"}}>
    <text style={{...text,fontSize:spec.buffer.fontSize*(closeHovered()?1.16:1),color:closeHovered()?c.red:c.muted,opacity:selected()||hovered()||closeHovered()?1:0}}>×</text>
   </div>
   <div style={{position:"absolute",right:0,top:6*s,bottom:6*s,width:.5,backgroundColor:c.border,pointerEvents:"none"}}/>
  </div>;
 }
 return <div role="tablist" aria-label="Workspaces" testId="workspace-tabs" onLayout={props.onScroll} onMouseMove={move} onMouseUp={finish} style={{height:chrome.height,flexShrink:0,position:"relative",minWidth:0,backgroundColor:c.surface,overflow:"hidden",borderBottomWidth:1,borderColor:c.border}}>
   <div ref={host=>scrollHost=host} testId="workspace-tab-scroll" onLayout={event=>{setViewportWidth(event.width||0);stripWidth=event.width||0;stripOriginX=event.x||0;stripOriginY=event.y||0;}} onScroll={event=>{consumeTabScroll(event);const next=verticalTabScroll(event,scrollX,stripWidth,[...widths.values()].reduce((sum,width)=>sum+width,0)+spec.buffer.newWidth);if(next!==undefined&&next!==scrollX&&scrollHost){consumeTabScroll(event,true);scrollX=next;renderer.scrollTo?.(scrollHost.id,next,0);props.onScroll();}}} onScrollChange={event=>{scrollX=event.scrollX??scrollX;props.onScroll();if(drag()){slide.move(contentPoint(pointer.x));reorderInStrip();}}} style={{...row,position:"absolute",left:0,right:props.accessoryWidth||0,top:0,height:chrome.height,overflowX:"scroll",overflowY:"hidden",minWidth:0}}>
   <For each={props.workspaces.map(w=>w.id)}>{(id,index)=><Tab id={id} index={index()}/>}</For>
   <div ref={host=>{newHost=host;props.registerNewHost(host);}} tabIndex={0} role="button" testId="workspace-new" aria-label="New workspace" data-keyboard-focused={props.keyboardIndex===props.workspaces.length}
    onFocus={()=>invalidateNativeFocus(renderer)} onBlur={()=>invalidateNativeFocus(renderer)}
    onMouseDown={()=>{armed=undefined;closeGesture=false;suppressActivation=false;}} onClick={()=>{if(!closeGesture&&!suppressActivation)props.onNew();}} onLayout={event=>{if(newHost)props.onBounds(newHost,{x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0});}}
    style={{...row,justifyContent:"center",position:"relative",width:spec.buffer.newWidth,height:chrome.height,flexShrink:0,cursor:"pointer",backgroundColor:props.keyboardIndex===props.workspaces.length?c.accent+"1a":undefined,hover:{backgroundColor:c.accent+"0f"}}}>
    <text style={{...text,fontFamily:props.config.iconFontFamily,fontSize:spec.buffer.iconSize,color:c.muted}}>{spec.buffer.newIcon}</text>
   </div>
   </div>
   <Show when={props.accessory}><div style={{position:"absolute",right:0,top:0,bottom:0,width:props.accessoryWidth||0}}>{props.accessory}</div></Show>
  <Show when={drag()}><anchored fill="window" deferred={true} priority={4} occlude={true} style={{backgroundColor:"transparent"}}>
   <div testId="workspace-drag-layer" onMouseMove={move} onMouseUp={finish} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"auto",cursor:"grabbing"}}/>
    <div testId="workspace-drag-preview" aria-hidden={true} style={{...row,position:"absolute",left:(slideRevision(),pointer.x-grab),top:(slideRevision(),pointer.y-chrome.height/2),width:widths.get(drag()!)||0,height:chrome.height,paddingLeft:chrome.tabPaddingX,paddingRight:9*s+spec.buffer.closeHitSize/2+spec.buffer.padding,gap:9*s,overflow:"hidden",backgroundColor:c.bg,borderTopLeftRadius:3*s,borderTopRightRadius:3*s,pointerEvents:"none"}}>
     <text testId="workspace-drag-preview-icon" style={{...text,flexShrink:0,lineHeight:chrome.height,fontFamily:props.config.iconFontFamily,fontSize:12*s,color:rgba(chrome.iconColor)}}>{workspaceIcon(props.workspaces.find(workspace=>workspace.id===drag()),context(drag()!).config,chrome.iconGlyph,context(drag()!).connectedHostId)}</text>
     <text style={{...text,flexShrink:0,lineHeight:chrome.height,color:c.fg}}>{(()=>{const workspace=props.workspaces.find(workspace=>workspace.id===drag());return workspace?workspaceLabel(workspace.title,workspaceTabRoot(workspace,context(workspace.id).config,context(workspace.id).directoryRoots,context(workspace.id).workspaceId)):"Workspace";})()}</text>
     <div style={{...row,position:"absolute",right:spec.buffer.padding-spec.buffer.closeHitSize/2,top:(chrome.height-spec.buffer.closeHitSize)/2,justifyContent:"center",width:spec.buffer.closeHitSize,height:spec.buffer.closeHitSize,flexShrink:0,pointerEvents:"none"}}><text style={{...text,fontSize:spec.buffer.fontSize,color:c.muted}}>×</text></div>
    </div>
  </anchored></Show>
 </div>;
}
