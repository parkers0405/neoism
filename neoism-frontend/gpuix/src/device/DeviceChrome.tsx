import {Tooltip,TooltipContent,TooltipProvider,TooltipTrigger,type EventPayload,type HostElement} from "@gpuix/solid";
import type {JSX} from "solid-js";
import type {AppConfig} from "../config";
import {DeviceIcon,type DeviceIconName} from "./DeviceIcon";

export const DEVICE_UI_FONT="Inter";
/** The device is staged on a neutral surface, independent of editor syntax
 * accents. This keeps the materials readable in both light and dark themes. */
export function devicePanelConfig(config:AppConfig):AppConfig {
  const hex=config.theme.bg.replace(/^#/,""),rgb=/^[\da-f]{6}$/i.test(hex)?[0,2,4].map(offset=>parseInt(hex.slice(offset,offset+2),16)):[0,0,0];
  const light=rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722>155;
  const colors=light?{bg:"#fcfcfc",surface:"#ffffff",border:"#dedee3",fg:"#27272a",dim:"#71717a",muted:"#a1a1aa"}:{bg:"#0b0b0b",surface:"#18181b",border:"#29292f",fg:"#d4d4d8",dim:"#a1a1aa",muted:"#71717a"};
  return {...config,theme:{...config.theme,...colors}};
}
export function DeviceIconButton(props:{config:AppConfig;icon:DeviceIconName;label:string;id?:string;pressed?:boolean;disabled?:boolean;size?:number;onHost?:(host:HostElement)=>void;onClick:(event:EventPayload)=>void}) {
  const c=props.config.theme,s=props.config.scale;
  return <Tooltip delayDuration={400}>
    <TooltipTrigger tabIndex={-1} style={{display:"flex",flexShrink:0}}><div ref={props.onHost} role="button" tabIndex={props.disabled?-1:0} testId={props.id} aria-label={props.label} aria-pressed={props.pressed} aria-disabled={!!props.disabled}
      onClick={event=>{if(!props.disabled)props.onClick(event);}} onKeyDown={event=>{if(event.key==="enter"||event.key==="space"){event.preventDefault();event.stopPropagation();if(!props.disabled)props.onClick(event);}}}
      style={{display:"flex",alignItems:"center",justifyContent:"center",width:(props.size||28)*s,height:(props.size||28)*s,minWidth:(props.size||28)*s,flexShrink:0,borderRadius:6*s,
        opacity:props.disabled ? .38 : 1,cursor:props.disabled?"default":"pointer",...(props.pressed?{backgroundColor:c.surface}:{}),hover:props.disabled?undefined:{backgroundColor:c.surface},active:props.disabled?undefined:{backgroundColor:c.border}}}>
      <DeviceIcon name={props.icon} color={props.pressed?c.fg:c.dim} size={16*s}/>
    </div></TooltipTrigger>
    <TooltipContent side="left" sideOffset={8*s} style={{paddingLeft:9*s,paddingRight:9*s,paddingTop:5*s,paddingBottom:5*s,borderRadius:5*s,borderWidth:1,borderColor:c.border,backgroundColor:c.surface}}>
      <text style={{fontFamily:DEVICE_UI_FONT,fontSize:11*s,color:c.fg}}>{props.label}</text>
    </TooltipContent>
  </Tooltip>;
}
export function DeviceRail(props:{config:AppConfig;children:JSX.Element;label?:string}) {
  const s=props.config.scale,c=props.config.theme;
  const border=/^#[\da-f]{6}$/i.test(c.border)?`${c.border}80`:c.border;
  return <div role="group" aria-label={props.label||"Device controls"} style={{display:"flex",flexDirection:"column",alignItems:"center",gap:4*s,padding:8*s,borderRadius:30*s,borderWidth:1,borderColor:border,backgroundColor:c.bg,pointerEvents:"auto",flexShrink:0}}>{props.children}</div>;
}
export function DeviceRailDivider(props:{config:AppConfig}) {
  return <div style={{width:20*props.config.scale,height:1,backgroundColor:props.config.theme.border,marginTop:3*props.config.scale,marginBottom:3*props.config.scale,pointerEvents:"none"}}/>;
}
export function DeviceMenuItem(props:{config:AppConfig;label:string;icon?:DeviceIconName;id?:string;selected?:boolean;disabled?:boolean;onClick:()=>void;trailing?:JSX.Element}) {
  const s=props.config.scale,c=props.config.theme;
  return <div role="menuitem" tabIndex={props.disabled?-1:0} aria-label={props.label} aria-disabled={!!props.disabled} testId={props.id}
    onClick={()=>{if(!props.disabled)props.onClick();}} onKeyDown={event=>{if(event.key==="enter"||event.key==="space"){event.preventDefault();event.stopPropagation();if(!props.disabled)props.onClick();}}}
    style={{display:"flex",flexDirection:"row",alignItems:"center",gap:9*s,minHeight:32*s,paddingLeft:9*s,paddingRight:9*s,borderRadius:4*s,opacity:props.disabled ? .4 : 1,cursor:props.disabled?"default":"pointer",backgroundColor:c.surface,hover:props.disabled?undefined:{backgroundColor:c.border}}}>
    {props.icon?<DeviceIcon name={props.icon} color={c.dim} size={15*s}/>:undefined}
    <text style={{fontFamily:DEVICE_UI_FONT,fontSize:12*s,color:c.fg,flexGrow:1,whiteSpace:"nowrap"}}>{props.label}</text>
    {props.trailing}
    {props.selected?<DeviceIcon name="check" color={c.fg} size={14*s}/>:undefined}
  </div>;
}
export {TooltipProvider as DeviceTooltipProvider};