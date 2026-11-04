import {createEffect,createSignal,For} from "solid-js";
import type {EventPayload,HostElement,KeyEvent,StyleDesc} from "@gpuix/solid";
import type {JsonUiControl,JsonUiDiagramCommand} from "./JsonRenderControls";

/** The graph stays native; this overlay only supplies canvas navigation. */
export function JsonRenderDiagramControls(props:{
  control:JsonUiControl;disabled:boolean;scale:number;style:StyleDesc;
  colors:{bg:string;fg:string;muted:string;border:string};
  host:(host:HostElement)=>void;onFocus:()=>void;onBlur:()=>void;
  onKeyDown:(event:KeyEvent)=>void;onPointerFocus:(event:EventPayload)=>void;
  command:(value:JsonUiDiagramCommand)=>void;
}) {
  const s=props.scale,[dragging,setDragging]=createSignal(false);
  let previous:{x:number;y:number}|undefined;
  const end=()=>{previous=undefined;setDragging(false);};
  const point=(event:EventPayload)=>typeof event.x==="number"&&Number.isFinite(event.x)&&typeof event.y==="number"&&Number.isFinite(event.y)?{x:event.x,y:event.y}:undefined;
  const down=(event:EventPayload)=>{
    if(props.disabled||event.button!==0)return;
    props.onPointerFocus(event);previous=point(event);setDragging(!!previous);
  };
  const move=(event:EventPayload)=>{
    const current=point(event);
    if(props.disabled||!previous||!current)return;
    const dx=current.x-previous.x,dy=current.y-previous.y;previous=current;
    if(dx||dy)props.command({dx,dy});
  };
  const zoom=()=>{const value=props.control.value;return typeof value==="object"&&value&&Number.isFinite(value.zoom)?Math.round(value.zoom*100):100;};
  createEffect(()=>{if(props.disabled)end();});
  const buttons=[{command:"zoom-out",label:"Zoom out",text:"-"},{command:"zoom-in",label:"Zoom in",text:"+"}] as const;
  return <div ref={props.host} testId={`json-ui-control:${props.control.id}`} role="group" aria-label={props.control.label}
    aria-description="Diagram canvas. Drag to pan, use plus or minus to zoom, arrow keys to pan, and 0 to fit."
    aria-valuetext={`${zoom()}% of fit`} aria-disabled={props.disabled} tabIndex={props.disabled?-1:0}
    data-preserve-keyboard-focus={true} onFocus={props.onFocus} onBlur={()=>{end();props.onBlur();}} onKeyDown={props.onKeyDown}
    onMouseDown={down} onMouseMove={move} onMouseUp={end} onMouseLeave={end}
    style={{...props.style,overflow:"hidden",cursor:props.disabled?"default":dragging()?"grabbing":"grab"}}>
    <div role="toolbar" aria-label="Diagram navigation" data-preserve-keyboard-focus={true} onMouseDown={()=>{}}
      style={{position:"absolute",right:8*s,top:8*s,display:"flex",flexDirection:"row",alignItems:"center",gap:4*s}}>
      <For each={buttons}>{button=><div testId={`json-ui-diagram:${props.control.id}:${button.command}`} role="button" aria-label={button.label} aria-disabled={props.disabled} tabIndex={-1}
        data-preserve-keyboard-focus={true} onMouseDown={event=>{end();props.onPointerFocus(event);}} onClick={()=>{if(!props.disabled)props.command(button.command);}}
        style={{width:28*s,height:28*s,display:"flex",alignItems:"center",justifyContent:"center",cursor:props.disabled?"default":"pointer"}}>
        <text style={{fontFamily:"Geist",fontSize:16*s,color:props.colors.fg,pointerEvents:"none"}}>{button.text}</text>
      </div>}</For>
    </div>
  </div>;
}