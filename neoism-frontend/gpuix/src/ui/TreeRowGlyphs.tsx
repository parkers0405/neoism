import {createMemo,createSignal,onCleanup} from "solid-js";
import {useGpuixRequired} from "@gpuix/solid";
import {CanvasImages,RasterSource,type CanvasOp,type CanvasRaster} from "./canvas";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
interface Raster{id:number;ptr:number;len:number;width:number;height:number;scale:number}
export interface TreeGitDecoration{path:string;directory:boolean;status:string;marker:string;color:number[];markerWidth:number;trailingWidth:number}
interface Frame{raster:Raster;label:string;labelX:number;labelY:number;iconX:number;rowHeight:number}
export function createTreeGlyphHost(config:AppConfig){
 const id=coreCall<{id:number}>("treeglyphs.new",{family:config.fontFamily,weight:config.fontWeight,fonts:[config.iconFontFamily,"Symbols Nerd Font Mono"]}).id;
 const gitCache=new WeakMap<object,Map<string,TreeGitDecoration[]>>();
 return {git:(snapshot:unknown,appearance:Pick<AppConfig,"themeName"|"scale">=config)=>{
  if(!snapshot||typeof snapshot!=="object")return coreCall<TreeGitDecoration[]>("treeglyphs.git-project",{id,snapshot,theme:appearance.themeName,scale:appearance.scale});
  const key=JSON.stringify([appearance.themeName,appearance.scale]);let cached=gitCache.get(snapshot);
  if(!cached){cached=new Map();gitCache.set(snapshot,cached);}
  let rows=cached.get(key);if(!rows){rows=coreCall<TreeGitDecoration[]>("treeglyphs.git-project",{id,snapshot,theme:appearance.themeName,scale:appearance.scale});cached.set(key,rows);}return rows;
 },row:(args:Record<string,unknown>)=>coreCall<Frame|null>("treeglyphs.row",{id,...args}),dispose:()=>coreCall("treeglyphs.dispose",{id})};
}
/** Native row interaction/reveal stays on its parent. This one image contains
 * the original FileTree icon ink centers, shaped label and truncation. */
export function TreeRowGlyphs(props:{host:ReturnType<typeof createTreeGlyphHost>;config:AppConfig;name:string;chevron:string;icon:string;labelColor:string;iconColor:string;chevronColor:string;gitStatus?:string;directory?:boolean;initialWidth?:number;offsetX?:number;trailingWidth?:number;onWidth?:(width:number)=>void;pinImage?:(raster:CanvasRaster)=>void}){
 const initialWidth=props.initialWidth||0;
 const [width,setWidth]=createSignal(Number.isFinite(initialWidth)&&initialWidth>0?initialWidth:0);
 const images=new CanvasImages(useGpuixRequired());
 onCleanup(()=>images.dispose());
 // One display-list image; pixels are copied inside this synchronous request,
 // before a sibling row can evict the native cache entry.
 const commands=createMemo(()=>{
  const next=width()>0?props.host.row({width:Math.max(1,width()),trailingWidth:props.trailingWidth||0,gitStatus:props.gitStatus,directory:props.directory,theme:props.config.themeName,scale:props.config.scale,name:props.name,chevron:props.chevron,icon:props.icon,labelColor:props.labelColor,iconColor:props.iconColor,chevronColor:props.chevronColor}):null;
  if(next)props.pinImage?.(next.raster);
  const ops:CanvasOp[]=next?[{k:"i",id:images.use(RasterSource.tree,next.raster),b:[0,0,next.raster.width/next.raster.scale,next.raster.height/next.raster.scale]}]:[];
  images.commit();
  return JSON.stringify(ops);
 });
 return <div onLayout={event=>{const next=Number.isFinite(event.width)?Math.max(0,event.width||0):0;setWidth(next);if(next>0)props.onWidth?.(next);}} aria-label={props.name} style={{position:"relative",left:props.offsetX||0,flexGrow:1,minWidth:0,height:26*props.config.scale,pointerEvents:"none"}}>
  <canvas commands={commands()} style={{position:"absolute",left:0,top:0,right:0,bottom:0,pointerEvents:"none"}}/>
 </div>;
}
