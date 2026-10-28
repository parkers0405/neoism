import { createMemo, onCleanup } from "solid-js";
import { useGpuixRequired } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { CanvasImages, RecentCanvasImages, cssColor, paintColor as packPaintColor, RasterSource, type CanvasOp,type CanvasRaster } from "./canvas";
export interface PaintRaster { id:number;ptr:number;len:number;width:number;height:number;scale?:number;dx:number;dy:number }
export interface PreparedCanvas {commands:string;images:Array<{source:number;raster:CanvasRaster}>}
export type PaintCommand =
 | {kind:"raster-text";rect:number[];raster:PaintRaster;layer:number;clip?:number[]|null}
 | {kind:"rect";rect:number[];color:number[];radii:number[];clip?:number[]|null;depth:number;order:number;layer:number}
 | {kind:"ring";outer:number[][];inner:number[][];color:number[];depth:number;order:number;layer:number}
 | {kind:"polygon";points:number[][];color:number[];depth:number;layer:number}
 | {kind:"text";x:number;y:number;text:string;size:number;width:number;color:number[];bold:boolean;italic:boolean;extrude:boolean;family?:string|null;clip?:number[]|null;layer:number;raster?:PaintRaster|null}
 | {kind:"image";rect:number[];source:number[];depth:number;layer:number;raster:PaintRaster};
export const paintColor = (color:number[],bytes=false)=>`rgba(${color.slice(0,3).map(c=>Math.round(c*(bytes?1:255))).join(",")},${(color[3]??(bytes?255:1))/(bytes?255:1)})`;

const order=(a:PaintCommand,b:PaintCommand)=>a.layer-b.layer||("depth" in a&&"depth" in b?a.depth-b.depth:0)||("order"in a&&"order"in b?a.order-b.order:0);
const sameClip=(a?:number[]|null,b?:number[]|null)=>a===b||!!a&&!!b&&a[0]===b[0]&&a[1]===b[1]&&a[2]===b[2]&&a[3]===b[3];
const flat=(points:number[][])=>points.flatMap(point=>[point[0],point[1]]);

/** Translate original painter commands into one native display list. The
 * paint order (layer, depth, order) and clip rects are the painter's own. */
export function paintOps(commands:readonly PaintCommand[],config:AppConfig,images:CanvasImages):CanvasOp[]{
 const ops:CanvasOp[]=[];let clip:number[]|null|undefined;
 for(const command of [...commands].sort(order)){
  const next="clip" in command?command.clip:undefined;
  if(!sameClip(clip,next)){if(clip)ops.push({k:"u"});if(next)ops.push({k:"cl",b:next});clip=next;}
  switch(command.kind){
   case "raster-text":ops.push({k:"i",id:images.use(RasterSource.painter,command.raster),b:command.rect});break;
   case "rect":ops.push({k:"r",b:command.rect,c:packPaintColor(command.color),r:command.radii});break;
   case "ring":ops.push({k:"p",c:packPaintColor(command.color),p:[flat(command.outer),flat(command.inner)]});break;
   case "polygon":ops.push({k:"p",c:packPaintColor(command.color),p:[flat(command.points)]});break;
   case "image":{const full=command.source[0]===0&&command.source[1]===0&&command.source[2]===1&&command.source[3]===1;ops.push({k:"i",id:images.use(RasterSource.paintImage,command.raster),b:command.rect,...(full?{}:{s:command.source})});break;}
   case "text":{
    const raster=command.raster;
    if(raster){const scale=raster.scale||1;ops.push({k:"i",id:images.use(RasterSource.painter,raster),b:[command.x+(raster.dx||0),command.y+(raster.dy||0),raster.width/scale,raster.height/scale]});}
    // Blank advances remain in the original layout/selection commands, but
    // have no ink. Sending them as text also disables native canvas caching.
    else if(!/^[ \t\r\n]*$/.test(command.text))ops.push({k:"t",x:command.x,y:command.y,t:command.text,s:command.size,c:packPaintColor(command.color,true),f:command.family||config.fontFamily,w:command.bold?800:config.fontWeight,i:command.italic,lh:command.size});
   }
  }
 }
 if(clip)ops.push({k:"u"});
 images.commit();
 return ops;
}
export { cssColor };

/** Exact original paint geometry and cached glyph pixels as one GPUI element.
 * No second Markdown layout or text shaping engine. `extra` paints on top. */
export function PaintLayer(props:{commands:readonly PaintCommand[];prepared?:PreparedCanvas;config:AppConfig;extra?:readonly CanvasOp[];cacheImageBytes?:number;pinImage?:(source:number,raster:CanvasRaster)=>void}){
 const renderer=useGpuixRequired(),recent=props.cacheImageBytes?new RecentCanvasImages(renderer,props.cacheImageBytes):undefined;
 const images=new CanvasImages(renderer,(source,raster)=>{props.pinImage?.(source,raster);recent?.use(source,raster);});
 onCleanup(()=>{images.dispose();recent?.dispose();});
 const source=createMemo(()=>{
  if(props.prepared){
   for(const image of props.prepared.images)images.use(image.source,image.raster);
   images.commit();
   return props.extra?.length?JSON.stringify([...JSON.parse(props.prepared.commands),...props.extra]):props.prepared.commands;
  }
  const ops=paintOps(props.commands,props.config,images);if(props.extra?.length)ops.push(...props.extra);return JSON.stringify(ops);
 });
 return <canvas testId="original-paint-layer" commands={source()} style={{position:"absolute",left:0,right:0,top:0,bottom:0,pointerEvents:"none"}}/>;
}
