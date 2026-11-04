import {createEffect,createSignal,For,Show,onCleanup,untrack} from "solid-js";
import {createWindowSize,useGpuixRequired,type HostElement,type KeyEvent,type EventPayload} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {focusNativeElement,invalidateNativeFocus} from "../native-focus";
import {paintedChromeBounds} from "../ui/chrome-target-bounds";
import {JsonRenderDiagramControls} from "./JsonRenderDiagramControls";

export interface JsonUiDiagramTransform {zoom:number;panX:number;panY:number}
export type JsonUiDiagramCommand="zoom-in"|"zoom-out"|"fit"|"pan-left"|"pan-right"|"pan-up"|"pan-down"|{dx:number;dy:number};
export interface JsonUiControl {
  id:string;kind:"button"|"input"|"select"|"checkbox"|"chart"|"diagram";
  rect:number[];label:string;value?:string|boolean|number|JsonUiDiagramTransform;options?:string[];
  placeholder?:string;disabled?:boolean;multiline?:boolean;selected?:boolean;
}
export interface JsonUiFrame {controls:JsonUiControl[]}
export interface JsonUiEvent {control:string;kind:"activate"|"change";value?:string|boolean|number|{dx:number;dy:number}}
export interface JsonUiEventResult {accepted:boolean;frame?:JsonUiFrame}
type PointerEvent={button?:number};
const inputValue=(value:string)=>Array.from(value).slice(0,2000).join("");

/** Native editors over the original painter's measured controls. Key by ID,
 * not frame object, so a repaint never remounts a field or resets its caret. */
export function JsonRenderControls(props:{frame?:JsonUiFrame;config:AppConfig;
  onEvent:(event:JsonUiEvent)=>JsonUiEventResult;onExit?:()=>void;
  onChartPointer?:(kind:"move"|"down"|"up"|"leave",event:EventPayload)=>void}) {
  const renderer=useGpuixRequired(),viewport=createWindowSize(),s=props.config.scale;
  const rgb=Number.parseInt(props.config.theme.bg.replace("#","").slice(0,6),16);
  const light=((rgb>>16)&255)*299+((rgb>>8)&255)*587+(rgb&255)*114>128000;
  const c=light?{bg:"#ffffff",fg:"#18181b",muted:"#52525b",border:"#e4e4e7",hover:"#f4f4f5"}:{bg:"#080808",fg:"#fafafa",muted:"#929292",border:"#292929",hover:"#202020"};
  const theme={appearance:light?"light" as const:"dark" as const,bg:"transparent",border:"transparent",text:c.fg,textMuted:c.muted,textFaint:c.muted,caret:c.fg,accent:c.fg,fontSans:"Geist",fontMono:"Geist"};
  const textStyle=(size=13)=>({fontFamily:"Geist",fontWeight:400,fontSize:size*s,color:c.fg});
  const hosts=new Map<string,HostElement>(),optionHosts=new Map<number,HostElement>();
  const [focused,setFocused]=createSignal<string>(),[openSelect,setOpenSelect]=createSignal<string>();
  const [pointerFocused,setPointerFocused]=createSignal<string>();
  const [optionIndex,setOptionIndex]=createSignal(0);
  let disposed=false;
  const controls=()=>props.frame?.controls.filter(control=>control.id&&control.rect.length===4&&control.rect.every(Number.isFinite)&&control.rect[2]>0&&control.rect[3]>0)||[];
  const byId=(id:string)=>controls().find(control=>control.id===id);
  const disabled=(control?:JsonUiControl)=>!control||!!control.disabled;
  const focus=(id:string)=>{const host=hosts.get(id);if(host)focusNativeElement(renderer,host.id);};
  const change=(id:string,value:string|boolean|number)=>{if(!disabled(byId(id)))props.onEvent({control:id,kind:"change",value});};
  const edit=(id:string,value:string)=>{
    const bounded=inputValue(value),host=hosts.get(id);
    // Native text editing precedes onChange. Force the capped value back even
    // when the canonical value did not change (continued typing at the limit).
    if(value!==bounded&&host?.mounted)host.root?.mutations.setCustomProp(host.id,"value",bounded);
    change(id,bounded);
  };
  const activate=(id:string,value?:number)=>{if(!disabled(byId(id)))props.onEvent({control:id,kind:"activate",value});};
  const diagramCommand=(id:string,value:JsonUiDiagramCommand)=>{if(!disabled(byId(id))&&byId(id)?.kind==="diagram")props.onEvent({control:id,kind:"activate",value});};
  const moveFocus=(id:string,backward=false)=>{
    const ids=controls().filter(control=>!disabled(control)).map(control=>control.id),index=ids.indexOf(id)+(backward?-1:1);
    if(index<0||index>=ids.length)props.onExit?.();else focus(ids[index]);
  };
  const closeSelect=(keyboard=false)=>{const id=openSelect();setOpenSelect(undefined);if(id&&keyboard)focus(id);};
  const toggleSelect=(id:string,keyboard=false)=>{
    if(disabled(byId(id)))return;
    if(openSelect()===id){closeSelect(keyboard);return;}
    setOptionIndex(Math.max(0,byId(id)?.options?.indexOf(String(byId(id)?.value??""))??0));setOpenSelect(id);if(keyboard)focus(id);
  };
  const key=(id:string,event:KeyEvent)=>{
    const control=byId(id);if(!control)return;
    setPointerFocused(undefined);
    const name=(event.key||"").toLowerCase();event.stopPropagation();
    if(name==="tab"){event.preventDefault();setOpenSelect(undefined);moveFocus(id,!!event.modifiers?.shift);return;}
    if(name==="escape"){event.preventDefault();if(openSelect())closeSelect(true);else props.onExit?.();return;}
    if(disabled(control)){event.preventDefault();return;}
    if(control.kind==="diagram"){
      const command:Record<string,JsonUiDiagramCommand>={"+":"zoom-in","=":"zoom-in","-":"zoom-out","0":"fit",home:"fit",arrowleft:"pan-left",left:"pan-left",arrowright:"pan-right",right:"pan-right",arrowup:"pan-up",up:"pan-up",arrowdown:"pan-down",down:"pan-down"};
      if(command[name]){event.preventDefault();diagramCommand(id,command[name]);}
    }else if(control.kind==="select"){
      const options=control.options||[];
      if(["arrowdown","down","arrowup","up","home","end"].includes(name)){
        event.preventDefault();if(openSelect()!==id)toggleSelect(id,true);
        setOptionIndex(index=>name==="home"?0:name==="end"?Math.max(0,options.length-1):Math.max(0,Math.min(options.length-1,index+(["arrowup","up"].includes(name)?-1:1))));
      }else if(["enter"," ","space"].includes(name)){
        event.preventDefault();if(openSelect()===id){const option=options[optionIndex()];if(option!==undefined)change(id,option);closeSelect(true);}else toggleSelect(id,true);
      }
    }else if(control.kind==="chart"){
      if(["arrowleft","left","arrowright","right","home","end"].includes(name)){
        event.preventDefault();const last=Math.max(0,(control.options?.length||1)-1),current=typeof control.value==="number"?control.value:0;
        activate(id,name==="home"?0:name==="end"?last:Math.max(0,Math.min(last,current+(["arrowleft","left"].includes(name)?-1:1))));
      }
    }else if(control.kind!=="input"&&["enter"," ","space"].includes(name)){
      event.preventDefault();if(control.kind==="checkbox")change(id,!control.value);else activate(id);
    }
  };
  const pointer=(id:string,event:PointerEvent)=>{
    // Native pointer payloads do not have DOM stopPropagation/preventDefault.
    // The preserve-focus attribute handles native hit testing; only editors
    // explicitly claim focus. Local widget clicks must leave the caret alone.
    if(!disabled(byId(id))&&(event.button===0||event.button===undefined)){setPointerFocused(id);if(byId(id)?.kind==="input")focus(id);}
  };
  const click=(id:string)=>{
    const control=byId(id);if(!control||disabled(control)||control.kind==="chart")return;
    if(control.kind==="select")toggleSelect(id);else if(control.kind==="checkbox")change(id,!control.value);else if(control.kind==="button")activate(id);
  };
  createEffect(()=>{
    const ids=controls().map(control=>control.id);
    untrack(()=>{
      if(openSelect()&&!ids.includes(openSelect()!))setOpenSelect(undefined);
      if(focused()&&!ids.includes(focused()!)){setFocused(undefined);invalidateNativeFocus(renderer);}
    });
  });
  createEffect(()=>{const id=openSelect(),index=optionIndex();if(id)queueMicrotask(()=>{if(!disposed&&openSelect()===id){const host=optionHosts.get(index);if(host)renderer.scrollIntoView?.(host.id);}});});
  onCleanup(()=>{disposed=true;invalidateNativeFocus(renderer);});
  const selectRect=()=>{
    const host=hosts.get(openSelect()||""),bounds=host&&paintedChromeBounds(renderer,host.id);
    const width=Math.min(bounds?.width||240*s,Math.max(1,viewport().width-16*s));
    const height=Math.min((byId(openSelect()||"")?.options?.length||1)*34*s,280*s,Math.max(1,viewport().height-24*s));
    const left=Math.max(8*s,Math.min(bounds?.x||8*s,viewport().width-width-8*s)),below=(bounds?.y||0)+(bounds?.height||0)+4*s;
    return {left,top:below+height<=viewport().height-8*s?below:Math.max(8*s,(bounds?.y||0)-height-4*s),width,height};
  };
  return <>
    <For each={controls().map(control=>control.id)}>{id=>{
      const control=()=>byId(id)!;
      const ref=(host:HostElement)=>{hosts.set(id,host);};onCleanup(()=>hosts.delete(id));
      const style=()=>({position:"absolute" as const,left:control().rect[0],top:control().rect[1],width:control().rect[2],height:control().rect[3],backgroundColor:"transparent",borderWidth:focused()===id&&pointerFocused()!==id?s:0,borderColor:c.fg,borderRadius:6*s,opacity:disabled(control())?0.6:1,pointerEvents:disabled(control())?"none" as const:"auto" as const});
      const focusIn=()=>{setFocused(id);invalidateNativeFocus(renderer);},focusOut=()=>{if(focused()===id)setFocused(undefined);if(pointerFocused()===id)setPointerFocused(undefined);invalidateNativeFocus(renderer);};
      return <Show when={control().kind==="diagram"} fallback={<Show when={control().kind==="input"} fallback={<div ref={ref} testId={`json-ui-control:${id}`} data-preserve-keyboard-focus={true} role={control().kind==="checkbox"?"checkbox":control().kind==="select"?"combobox":control().kind==="chart"?"group":"button"}
        aria-label={control().label} aria-checked={control().kind==="checkbox"?!!control().value:undefined} aria-expanded={control().kind==="select"?openSelect()===id:undefined}
        aria-disabled={disabled(control())} aria-valuetext={control().kind==="select"?String(openSelect()===id?control().options?.[optionIndex()]??control().value:control().value??""):control().kind==="chart"?control().options?.[Number(control().value)||0]:undefined}
        tabIndex={disabled(control())?-1:0} onFocus={focusIn} onBlur={focusOut} onKeyDown={event=>key(id,event)}
        onMouseDown={event=>{pointer(id,event);if(control().kind==="chart"&&event.button===0)props.onChartPointer?.("down",event);}}
        onMouseMove={control().kind==="chart"?event=>props.onChartPointer?.("move",event):undefined}
        onMouseLeave={control().kind==="chart"?event=>props.onChartPointer?.("leave",event):undefined}
        onMouseUp={event=>{if(control().kind==="chart")props.onChartPointer?.("up",event);}} onClick={()=>click(id)} style={{...style(),cursor:control().kind==="chart"?"crosshair":"pointer"}}/>}>
        <Show when={control().multiline} fallback={<input ref={ref} testId={`json-ui-control:${id}`} data-preserve-keyboard-focus={true} aria-label={control().label} value={String(control().value??"")} placeholder={control().placeholder||""} theme={theme} readOnly={disabled(control())} tabIndex={disabled(control())?-1:0}
          onChange={event=>edit(id,event.value||"")} onKeyDown={event=>key(id,event)} onSubmit={()=>{}} onMouseDown={event=>pointer(id,event)} onMouseUp={()=>{}} onClick={()=>{}}
          onFocus={focusIn} onBlur={focusOut} style={{...style(),...textStyle(),backgroundColor:c.bg,borderWidth:s,borderColor:focused()===id?c.fg:c.border,paddingLeft:10*s,paddingRight:10*s,userSelect:"text"}}/>}>
          <textarea ref={ref} testId={`json-ui-control:${id}`} data-preserve-keyboard-focus={true} aria-label={control().label} value={String(control().value??"")} placeholder={control().placeholder||""} theme={theme} minRows={3} maxRows={3} readOnly={disabled(control())} tabIndex={disabled(control())?-1:0}
            onChange={event=>edit(id,event.value||"")} onKeyDown={event=>key(id,event)} onMouseDown={event=>pointer(id,event)} onMouseUp={()=>{}} onClick={()=>{}}
            onFocus={focusIn} onBlur={focusOut} style={{...style(),...textStyle(),backgroundColor:c.bg,borderWidth:s,borderColor:focused()===id?c.fg:c.border,padding:10*s,userSelect:"text"}}/>
        </Show>
      </Show>}>
        <JsonRenderDiagramControls control={control()} disabled={disabled(control())} scale={s} style={style()} colors={c} host={ref} onFocus={focusIn} onBlur={focusOut}
          onKeyDown={event=>key(id,event)} onPointerFocus={event=>pointer(id,event)} command={value=>diagramCommand(id,value)}/>
      </Show>;
    }}</For>
    <Show when={openSelect()}>{id=><anchored fill="window" deferred priority={5} occlude style={{backgroundColor:"transparent",pointerEvents:"none"}}>
      <div testId="json-ui-options" data-preserve-keyboard-focus={true} role="listbox" aria-label={byId(id())?.label} onKeyDown={event=>key(id(),event)} onMouseDownOutside={event=>{
        const host=hosts.get(id()),box=host&&paintedChromeBounds(renderer,host.id);
        if(box&&event.x!==undefined&&event.y!==undefined&&event.x>=box.x&&event.x<=box.x+box.width&&event.y>=box.y&&event.y<=box.y+box.height)return;closeSelect();
      }} onMouseDown={()=>{}} onMouseUp={()=>{}} onClick={()=>{}}
        style={{position:"absolute",...selectRect(),overflowY:"scroll",backgroundColor:c.bg,borderColor:c.border,borderWidth:s,borderRadius:8*s,pointerEvents:"auto"}}>
        <For each={byId(id())?.options||[]}>{(option,index)=><div ref={host=>{optionHosts.set(index(),host);onCleanup(()=>optionHosts.delete(index()));}} testId={`json-ui-option:${index()}`} data-preserve-keyboard-focus={true} role="option" aria-label={option} aria-selected={index()===optionIndex()} onMouseMove={()=>setOptionIndex(index())}
          onClick={()=>{change(id(),option);closeSelect();}} style={{height:34*s,paddingLeft:10*s,paddingRight:10*s,display:"flex",alignItems:"center",backgroundColor:index()===optionIndex()?c.hover:"transparent",cursor:"pointer"}}>
          <text style={{...textStyle(),whiteSpace:"nowrap",textOverflow:"ellipsis",pointerEvents:"none"}}>{option}</text>
        </div>}</For>
      </div>
    </anchored>}</Show>
  </>;
}