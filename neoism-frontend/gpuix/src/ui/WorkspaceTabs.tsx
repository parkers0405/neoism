import {createEffect,createMemo,createSignal,For,onCleanup,Show,untrack} from "solid-js";
import {motion,useGpuixRequired,createWindowSize,type EventPayload,type HostElement} from "@gpuix/solid";
import type {AppConfig} from "../config";
import type {Workspace} from "../daemon/protocol";
import {tabChrome,rgba,workspaceLabel} from "../layout/tab-chrome";
import {easeOutCubicBezier} from "../design/animation";
import {paintedChromeBounds,type ChromeBounds} from "./chrome-target-bounds";
import {AnimationFrameScheduler} from "../animation/frame-scheduler";
import {workspaceDropTarget,type WorkspaceDropTarget} from "../workspaces/drag-target";
import {HoverTabTitle} from "../layout/HoverTabTitle";
import {verticalTabScroll,consumeTabScroll} from "../layout/tab-scroll";
import {TabSlide} from "../layout/tab-slide";
import {workspaceIcon,cloudWorkspace} from "../workspaces/workspace-icon";

/** Window-owned workspace tabs size to their titles and share one horizontal strip with their add button. */
export function WorkspaceTabs(props:{
 config:AppConfig;workspaces:readonly Workspace[];activeId?:string;keyboardIndex?:number;connectedHostId?:string;
 onActivate:(workspace:Workspace)=>void;onNew:()=>void;onClose:(workspaceId:string)=>void;
 onReorder?:(dragId:string,targetId:string,position:"before"|"after")=>void;
 registerHost:(id:string,host:HostElement|undefined)=>void;
 registerNewHost:(host:HostElement|undefined)=>void;
 onBounds:(host:HostElement,bounds:ChromeBounds)=>void;onScroll:()=>void;
}){
 const c=props.config.theme,s=props.config.scale,spec=tabChrome(s),chrome=spec.workspace;
 const row={display:"flex" as const,flexDirection:"row" as const,alignItems:"center" as const};
 const text={fontFamily:props.config.fontFamily,fontWeight:props.config.fontWeight,fontSize:chrome.titleFontSize,color:c.fg,whiteSpace:"nowrap" as const,pointerEvents:"none" as const,userSelect:"none" as const};
 const windowSize=createWindowSize(),[viewportWidth,setViewportWidth]=createSignal(0);
 const renderer=useGpuixRequired(),hosts=new Map<string,HostElement>(),pointerFrames=new AnimationFrameScheduler(1000/60);
 const [drag,setDrag]=createSignal<string>(),[drop,setDrop]=createSignal<WorkspaceDropTarget>();
 const [slideRevision,setSlideRevision]=createSignal(0),[widthRevision,setWidthRevision]=createSignal(0);
 const widths=new Map<string,number>(),slide=new TabSlide<string>({reducedMotion:props.config.reducedMotion,onChange:()=>setSlideRevision(value=>value+1)});
 let armed:{id:string;x:number;contentX:number}|undefined,scrollHost:HostElement|undefined,suppressActivation=false,pointer={x:0,y:0},scrollX=0,stripOriginX=0,stripOriginY=0,stripWidth=0;
 const layoutSlides=()=>{const entries=props.workspaces.map(workspace=>({id:workspace.id,width:widths.get(workspace.id)||0}));if(entries.every(entry=>entry.width>0))slide.layout(entries);};
 createEffect(()=>{widthRevision();layoutSlides();});
 const rememberWidth=(id:string,width:number)=>{if(width>0&&Math.abs((widths.get(id)||0)-width)>.01){widths.set(id,width);setWidthRevision(value=>value+1);}};
 const measureWidths=()=>{for(const[id,host]of hosts){const bounds=host.mounted&&paintedChromeBounds(renderer,host.id);if(bounds)rememberWidth(id,bounds.width);}layoutSlides();};
 const contentPoint=(x:number,refresh=false)=>{
  if(refresh){const clip=scrollHost&&paintedChromeBounds(renderer,scrollHost.id);if(clip){stripOriginX=clip.x;stripOriginY=clip.y;stripWidth=clip.width;}const offset=scrollHost&&renderer.getScrollOffset?.(scrollHost.id);if(offset)scrollX=offset[0]||0;}
  return x-stripOriginX-scrollX;
 };
 const measureDrop=()=>{
  const id=drag(),clip=scrollHost&&paintedChromeBounds(renderer,scrollHost.id);if(!id||!clip){setDrop(undefined);return;}
  stripOriginX=clip.x;stripOriginY=clip.y;stripWidth=clip.width;
  const offset=renderer.getScrollOffset?.(scrollHost!.id);if(offset)scrollX=offset[0]||0;
  const tabs=props.workspaces.flatMap(workspace=>{const width=widths.get(workspace.id);return width?[{id:workspace.id,bounds:{x:clip.x+slide.logical(workspace.id)+scrollX,y:clip.y,width,height:clip.height}}]:[];});
  setDrop(workspaceDropTarget(tabs,clip,id,pointer.x,pointer.y));
 };
 const move=(event:EventPayload)=>{
  pointer={x:event.x??0,y:event.y??0};
  if(armed&&!drag()&&Math.abs(pointer.x-armed.x)>=5*s){setDrag(armed.id);slide.begin(armed.id,armed.contentX);armed=undefined;suppressActivation=true;}
  if(drag()){slide.move(contentPoint(pointer.x));pointerFrames.request(measureDrop);}
 };
 const finish=(event:EventPayload)=>{
  pointerFrames.cancel();pointer={x:event.x??0,y:event.y??0};const id=drag();
  if(id){measureDrop();slide.move(contentPoint(pointer.x));const target=drop();if(target)props.onReorder?.(id,target.id,target.position);layoutSlides();slide.end();props.onScroll();}
  armed=undefined;setDrag(undefined);setDrop(undefined);
 };
 let newHost:HostElement|undefined;
 onCleanup(()=>{pointerFrames.dispose();slide.dispose();props.registerNewHost(undefined);});
 function Tab(p:{id:string;index:number}){
  const initial=untrack(()=>props.workspaces.find(w=>w.id===p.id))!;
  const workspace=createMemo(()=>props.workspaces.find(w=>w.id===p.id)||initial);
  const title=createMemo(()=>workspaceLabel(workspace().title,workspace().root_dir));
  const selected=()=>props.activeId===p.id;
  const [hovered,setHovered]=createSignal(false),[closeHovered,setCloseHovered]=createSignal(false);
  const [titleWidth,setTitleWidth]=createSignal([...title()].length*chrome.titleFontSize*.6);
  const titleBudget=()=>Math.min(titleWidth(),Math.max(24*s,(viewportWidth()||windowSize().width)-chrome.tabPaddingX-(9*s+spec.buffer.closeHitSize/2+spec.buffer.padding)-9*s-12*s-spec.buffer.newWidth));
  let closingClick=false,closePressed=false;
  const close=(event?:EventPayload)=>{if(event?.button!==undefined&&event.button!==0)return;closingClick=true;armed=undefined;suppressActivation=false;closePressed=true;props.onClose(p.id);};
  let host:HostElement|undefined,closeHost:HostElement|undefined,closeLayout:ChromeBounds|undefined,queued=false,disposed=false;
  const press=(event:EventPayload)=>{
   if(event.button!==undefined&&event.button!==0)return;
   const closeBounds=(closeHost&&paintedChromeBounds(renderer,closeHost.id))||closeLayout,x=event.x??0,y=event.y??0;
   if(closeBounds&&x>=closeBounds.x&&x<=closeBounds.x+closeBounds.width&&y>=closeBounds.y&&y<=closeBounds.y+closeBounds.height){armed=undefined;closingClick=true;return;}
   // A refused close can end outside the X, so its click never clears the
   // latch. A fresh press outside the current close bounds is a new gesture.
   closePressed=false;closingClick=false;suppressActivation=false;if(props.onReorder){measureWidths();armed={id:p.id,x,contentX:contentPoint(x,true)};}
  };
  const reveal=()=>{if(queued)return;queued=true;queueMicrotask(()=>{queued=false;if(disposed||!host?.mounted||!selected()||drag())return;host.root?.mutations.flushMutations();host.scrollIntoView();if(props.workspaces.at(-1)?.id===p.id)newHost?.scrollIntoView();props.onScroll();});};
  createEffect(()=>{if(selected())reveal();});
  onCleanup(()=>{disposed=true;hosts.delete(p.id);widths.delete(p.id);props.registerHost(p.id,undefined);});
  return <div ref={value=>{host=value;hosts.set(p.id,value);props.registerHost(p.id,value);}} tabIndex={0} role="tab" testId={`workspace:${p.id}`} aria-label={workspace().title} aria-selected={selected()} data-keyboard-focused={props.keyboardIndex===p.index}
   onLayout={event=>{rememberWidth(p.id,event.width||0);if(host)props.onBounds(host,{x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0});if(selected())reveal();}}
   onMouseDown={press}
   onMouseMove={move} onMouseUp={finish}
   onMouseEnter={()=>setHovered(true)} onMouseLeave={()=>setHovered(false)} onClick={()=>{if(!closingClick&&!suppressActivation)props.onActivate(workspace());}}
   style={{...row,position:"relative",left:(slideRevision(),slide.offset(p.id)),opacity:drag()===p.id?0:1,paddingLeft:chrome.tabPaddingX,paddingRight:9*s+spec.buffer.closeHitSize/2+spec.buffer.padding,gap:9*s,height:chrome.height,flexShrink:0,overflow:"hidden",cursor:drag()===p.id?"grabbing":"pointer",backgroundColor:selected()||drag()===p.id?c.bg:undefined,borderTopLeftRadius:chrome.tabRadius,borderTopRightRadius:chrome.tabRadius}}>
   <motion.div initial={false} animate={{opacity:(hovered()||props.keyboardIndex===p.index)&&!selected()?1:0}} transition={{duration:.15,ease:easeOutCubicBezier}} style={{position:"absolute",left:0,right:0,top:0,height:chrome.height,backgroundColor:c.accent+"17",pointerEvents:"none"}}/>
   <text testId={`workspace-icon:${p.id}`} aria-label={cloudWorkspace(workspace(),props.config,props.connectedHostId)?"Cloud workspace":"Local workspace"} style={{...text,flexShrink:0,lineHeight:chrome.height,fontFamily:props.config.iconFontFamily,fontSize:12*s,color:rgba(chrome.iconColor)}}>{workspaceIcon(workspace(),props.config,chrome.iconGlyph,props.connectedHostId)}</text>
   <text onLayout={event=>{if(event.width)setTitleWidth(event.width);}} style={{...text,position:"absolute",opacity:0,left:0,top:0}}>{title()}</text>
   <HoverTabTitle testId={`workspace-title:${p.id}`} label={title()} width={titleBudget()} naturalWidth={titleWidth()} height={chrome.height} scale={s} hovered={hovered()} reducedMotion={props.config.reducedMotion} style={{...text,lineHeight:chrome.height,color:selected()?c.fg:c.muted}}/>
   <div ref={value=>closeHost=value} role="button" aria-label={`Close ${workspace().title}`} testId={`workspace-close:${p.id}`} onClick={event=>{if(closePressed){closePressed=false;closingClick=true;}else if(!suppressActivation)close(event);}}
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
  <div ref={host=>scrollHost=host} testId="workspace-tab-scroll" onLayout={event=>{setViewportWidth(event.width||0);stripWidth=event.width||0;stripOriginX=event.x||0;stripOriginY=event.y||0;}} onScroll={event=>{consumeTabScroll(event);const next=verticalTabScroll(event,scrollX,stripWidth,[...widths.values()].reduce((sum,width)=>sum+width,0)+spec.buffer.newWidth);if(next!==undefined&&next!==scrollX&&scrollHost){consumeTabScroll(event,true);scrollX=next;renderer.scrollTo?.(scrollHost.id,next,0);props.onScroll();}}} onScrollChange={event=>{scrollX=event.scrollX??scrollX;props.onScroll();if(drag()){slide.move(contentPoint(pointer.x));pointerFrames.request(measureDrop);}}} style={{...row,position:"absolute",left:0,right:0,top:0,height:chrome.height,overflowX:"scroll",overflowY:"hidden",minWidth:0}}>
   <For each={props.workspaces.map(w=>w.id)}>{(id,index)=><Tab id={id} index={index()}/>}</For>
   <div ref={host=>{newHost=host;props.registerNewHost(host);}} tabIndex={0} role="button" testId="workspace-new" aria-label="New workspace" data-keyboard-focused={props.keyboardIndex===props.workspaces.length}
    onMouseDown={()=>{armed=undefined;suppressActivation=false;}} onClick={()=>{if(!suppressActivation)props.onNew();}} onLayout={event=>{if(newHost)props.onBounds(newHost,{x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0});}}
    style={{...row,justifyContent:"center",position:"relative",width:spec.buffer.newWidth,height:chrome.height,flexShrink:0,cursor:"pointer",backgroundColor:props.keyboardIndex===props.workspaces.length?c.accent+"1a":undefined,hover:{backgroundColor:c.accent+"0f"}}}>
    <text style={{...text,fontFamily:props.config.iconFontFamily,fontSize:spec.buffer.iconSize,color:c.muted}}>{spec.buffer.newIcon}</text>
   </div>
  </div>
  <Show when={drag()}><anchored fill="window" deferred={true} priority={4} occlude={true} style={{backgroundColor:"transparent"}}>
   <div testId="workspace-drag-layer" onMouseMove={move} onMouseUp={finish} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"auto",cursor:"grabbing"}}/>
   <div style={{position:"absolute",left:stripOriginX,top:stripOriginY,width:stripWidth,height:chrome.height,overflow:"hidden",pointerEvents:"none"}}>
    <div testId="workspace-drag-preview" aria-hidden={true} style={{...row,position:"absolute",left:(slideRevision(),scrollX+slide.position(drag()!)),top:0,width:widths.get(drag()!)||0,height:chrome.height,paddingLeft:chrome.tabPaddingX,paddingRight:9*s+spec.buffer.closeHitSize/2+spec.buffer.padding,gap:9*s,backgroundColor:c.bg,borderTopLeftRadius:chrome.tabRadius,borderTopRightRadius:chrome.tabRadius,pointerEvents:"none"}}>
     <text testId="workspace-drag-preview-icon" style={{...text,flexShrink:0,lineHeight:chrome.height,fontFamily:props.config.iconFontFamily,fontSize:12*s,color:rgba(chrome.iconColor)}}>{workspaceIcon(props.workspaces.find(workspace=>workspace.id===drag()),props.config,chrome.iconGlyph,props.connectedHostId)}</text>
     <text style={{...text,flexShrink:0,lineHeight:chrome.height,color:props.activeId===drag()?c.fg:c.muted}}>{workspaceLabel(props.workspaces.find(workspace=>workspace.id===drag())?.title||"Workspace",props.workspaces.find(workspace=>workspace.id===drag())?.root_dir)}</text>
     <div style={{...row,position:"absolute",right:spec.buffer.padding-spec.buffer.closeHitSize/2,top:(chrome.height-spec.buffer.closeHitSize)/2,justifyContent:"center",width:spec.buffer.closeHitSize,height:spec.buffer.closeHitSize,flexShrink:0,pointerEvents:"none"}}><text style={{...text,fontSize:spec.buffer.fontSize,color:c.muted}}>×</text></div>
    </div>
   </div>
   <Show when={drop()}>{target=><div testId="workspace-drop-indicator" style={{position:"absolute",left:target().bounds.x+(target().position==="after"?target().bounds.width:0)-s,top:target().bounds.y,width:2*s,height:target().bounds.height,backgroundColor:c.accent,pointerEvents:"none"}}/>}</Show>
  </anchored></Show>
 </div>;
}
