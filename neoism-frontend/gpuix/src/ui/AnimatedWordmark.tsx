import {createEffect,createMemo,createSignal,onCleanup,type JSX} from "solid-js";
import {useGpuixRequired,type StyleDesc} from "@gpuix/solid";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {CanvasImages,RasterSource,rasterKey,type CanvasOp,type CanvasRaster} from "./canvas";
import {paintOps,type PaintCommand} from "./PaintLayer";

/** Scope this to the App/window. Tab remounts retain the original shuffle bag
 * and startup eligibility; disposing the scope releases every native host. */
export class WordmarkHosts {
 private entries=new Map<string,{id:number;users:number;pins:Map<object,CanvasImages>}>();
 private splashViewports=new Map<string,{width:number;height:number}>();
 private paintPins=new Map<object,Map<number,CanvasImages>>();
 private paintLRU=new Map<CanvasImages,{owner:Map<number,CanvasImages>;key:number}>();
 private disposed=false;
 acquire(key:string,kind:"agent"|"splash"){
  if(this.disposed)throw new Error("Wordmark host scope has closed");
  let host=this.entries.get(key);if(!host){host={id:coreCall<{id:number}>("wordmark.new",{kind}).id,users:0,pins:new Map()};this.entries.set(key,host);}
  host.users++;return host.id;
 }
 release(key:string){const host=this.entries.get(key);if(host)host.users=Math.max(0,host.users-1);}
 // A new Welcome tab (e.g. another workspace's) shares the pane area, so it
 // starts from the latest splash bounds instead of an empty first frame.
 private lastSplashViewport?:{width:number;height:number};
 splashViewport(key:string){return this.splashViewports.get(key)??this.lastSplashViewport;}
 rememberSplashViewport(key:string,viewport:{width:number;height:number}){if(!this.disposed&&viewport.width>0&&viewport.height>0){this.splashViewports.set(key,{...viewport});this.lastSplashViewport={...viewport};}}
 /** Keep the static GPU image alive while a font-config generation unmounts
  * its canvas. The replacement canvas takes its reference before reuse. */
 pinImage(key:string,renderer:object,raster:CanvasRaster){
  const host=this.entries.get(key);if(!host||this.disposed)return;
  let images=host.pins.get(renderer);if(!images){images=new CanvasImages(renderer);host.pins.set(renderer,images);}
  images.use(RasterSource.wordmark,raster);images.commit();
 }
 /** Separate bounded leases keep splash menu glyphs alive while old and new
  * component generations exchange their canvas references. */
 pinPaintImage(renderer:object,source:number,raster:CanvasRaster){
  if(this.disposed)return;
  let owner=this.paintPins.get(renderer);if(!owner){owner=new Map();this.paintPins.set(renderer,owner);}
  const key=rasterKey(source,raster.id);let images=owner.get(key);
  if(!images){images=new CanvasImages(renderer);owner.set(key,images);}
  images.use(source,raster);images.commit();
  this.paintLRU.delete(images);this.paintLRU.set(images,{owner,key});
  while(this.paintLRU.size>512){const oldest=this.paintLRU.entries().next().value!;oldest[0].dispose();oldest[1].owner.delete(oldest[1].key);this.paintLRU.delete(oldest[0]);}
 }
 dispose(){if(this.disposed)return;this.disposed=true;for(const host of this.entries.values()){for(const images of host.pins.values())images.dispose();coreCall("wordmark.dispose",{id:host.id});}for(const images of this.paintLRU.keys())images.dispose();this.paintLRU.clear();this.paintPins.clear();this.entries.clear();this.splashViewports.clear();this.lastSplashViewport=undefined;}
}

export function initialWordmarkSize(style?:StyleDesc){return {x:0,y:0,width:typeof style?.width==="number"?style.width:0,height:typeof style?.height==="number"?style.height:0};}

export interface WordmarkFrame {
 image?:CanvasRaster;
 letters:Array<{rect:number[];glow:number[]}>;
 commands:PaintCommand[];
 showOriginal:boolean;
 animating:boolean;
 clickAnimating?:boolean;
 effect?:string|null;
 progress?:number|null;
}
/** Image UV strips are expressed as exact clipped full-image draws. This
 * preserves the original fractional sixths of the 1196-pixel artwork. */
export function wordmarkImageOps(frame:WordmarkFrame,imageId:number,kind:"agent"|"splash"):CanvasOp[]{
 if(!frame.showOriginal)return [];
 const ops:CanvasOp[]=[];
 const slice=(rect:number[],index:number)=>{
  const [x,y,w,h]=rect;ops.push({k:"cl",b:rect},{k:"i",id:imageId,b:[x-index*w,y,w*6,h]},{k:"u"});
 };
 frame.letters.forEach((letter,index)=>{
  if(kind==="agent"){slice(letter.glow,index);slice(letter.rect,index);}
  else{slice(letter.rect,index);slice(letter.glow,index);}
 });
 return ops;
}
export type WordmarkPaintBounds=[number,number,number,number];
/** A canvas clips to its own native bounds. Move the complete original paint
 * list into a pane-sized canvas while leaving the logo's hitbox untouched. */
export function translateWordmarkOps(ops:readonly CanvasOp[],dx:number,dy:number):CanvasOp[]{
 const points=(values:number[])=>values.map((value,index)=>value+(index%2?dy:dx));
 return ops.map(op=>{
  switch(op.k){
   case "r":case "i":case "cl":return {...op,b:[op.b[0]+dx,op.b[1]+dy,...op.b.slice(2)]};
   case "t":return {...op,x:op.x+dx,y:op.y+dy};
   case "p":return {...op,p:op.p.map(points)};
   case "l":return {...op,p:points(op.p)};
   case "u":return op;
  }
 });
}

/** Original Rust animation bag, startup/hover policy and artwork, painted by
 * one native GPU canvas. Only the static PNG pixels are uploaded. */
export function AnimatedWordmark(props:{config:AppConfig;style?:StyleDesc;kind?:"agent"|"splash";testId?:string;startupKey?:string;hosts?:WordmarkHosts;paintBounds?:WordmarkPaintBounds}):JSX.Element{
 const renderer=useGpuixRequired(),images=new CanvasImages(renderer);
 const retained=props.hosts&&props.startupKey?{hosts:props.hosts,key:props.startupKey}:undefined;
 const id=retained?retained.hosts.acquire(retained.key,props.kind||"agent"):coreCall<{id:number}>("wordmark.new",{kind:props.kind||"agent"}).id;
 const animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 const [size,setSize]=createSignal(initialWordmarkSize(props.style));
 const [frame,setFrame]=createSignal<WordmarkFrame>({letters:[],commands:[],showOriginal:true,animating:false});
 const paintBounds=createMemo<WordmarkPaintBounds>(()=>props.paintBounds||[-size().width,-size().height*4,size().width*3,size().height*9]);
 let mouse:number[]|undefined,disposed=false;
 const draw=()=>{
  if(disposed)return;const bounds=size();if(bounds.width<=0||bounds.height<=0)return;
  const next=coreCall<WordmarkFrame>("wordmark.frame",{id,width:bounds.width,height:bounds.height,clip:paintBounds(),mouse,fg:props.config.theme.fg,
   autoAnimate:!!props.config.splashAutoAnimate,reducedMotion:props.config.reducedMotion});
  if(next.image&&retained)retained.hosts.pinImage(retained.key,renderer,next.image);
  setFrame(next);if(next.animating)animation.request(draw);
 };
 // Both splash and Agent already know the source-computed logo rectangle.
 // Paint it in the first mount batch rather than waiting another layout.
 draw();
 createEffect(()=>{size();props.config.theme.fg;props.config.reducedMotion;props.config.splashAutoAnimate;draw();});
 onCleanup(()=>{disposed=true;animation.dispose();images.dispose();if(retained)retained.hosts.release(retained.key);else coreCall("wordmark.dispose",{id});});
 const commands=createMemo(()=>{
  const next=frame(),image=next.image;
  // Keep the one static texture referenced during click effects too, so the
  // settle tail never needs to upload or decode the PNG again.
  const key=image?images.use(RasterSource.wordmark,image):undefined;
  const ops=key===undefined?[]:wordmarkImageOps(next,key,props.kind||"agent");
  ops.push(...paintOps(next.commands,props.config,images));
  const clip=paintBounds();return JSON.stringify(translateWordmarkOps(ops,-clip[0],-clip[1]));
 });
 const replay=()=>{if(props.config.reducedMotion)return;coreCall("wordmark.replay",{id});draw();};
 return <div testId={props.testId||`neoism-wordmark:${props.kind||"agent"}`} role="button" aria-label="Animate Neoism wordmark"
  onLayout={event=>setSize(previous=>{const next={x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0};return Object.keys(next).every(key=>previous[key as keyof typeof previous]===next[key as keyof typeof next])?previous:next;})}
  onMouseMove={event=>{const bounds=size();mouse=[(event.x||0)-bounds.x,(event.y||0)-bounds.y];if(!props.config.reducedMotion)animation.request(draw);}}
  onMouseLeave={()=>{mouse=undefined;if(!props.config.reducedMotion)animation.request(draw);}}
  onClick={replay}
  style={{...props.style,overflow:"visible",cursor:props.config.reducedMotion?"default":"pointer",pointerEvents:"auto"}}>
  <canvas commands={commands()} style={{position:"absolute",left:paintBounds()[0],top:paintBounds()[1],width:paintBounds()[2],height:paintBounds()[3],pointerEvents:"none"}}/>
 </div>;
}
