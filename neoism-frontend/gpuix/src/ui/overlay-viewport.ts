import {createMemo,createSignal,type Accessor,type Setter} from "solid-js";
export interface OverlayViewport{x:number;y:number;width:number;height:number}
const dialogs=new WeakMap<object,{get:Accessor<OverlayViewport|undefined>;set:Setter<OverlayViewport|undefined>}>();
const scopes=new WeakMap<object,{get:Accessor<OverlayViewport|undefined>;set:Setter<OverlayViewport|undefined>}>();
function scope(renderer:object){let value=scopes.get(renderer);if(!value){const[get,set]=createSignal<OverlayViewport>();value={get,set};scopes.set(renderer,value);}return value;}
/** Focused pane BODY in window coordinates, after chrome and both tab strips. */
export function setOverlayViewport(renderer:object,bounds:OverlayViewport|undefined){
 const next=bounds&&[bounds.x,bounds.y,bounds.width,bounds.height].every(Number.isFinite)&&bounds.width>0&&bounds.height>0?{...bounds}:undefined;
 scope(renderer).set(previous=>previous&&next&&previous.x===next.x&&previous.y===next.y&&previous.width===next.width&&previous.height===next.height?previous:next);
}
export function overlayViewport(renderer:object){return scope(renderer).get;}
/** Native menus lay out within this local viewport; their paint and pointer
 * host use the same origin. A standalone fixture can omit a pane publisher. */
export function useOverlayViewport(renderer:object,window:Accessor<{width:number;height:number}>){
 const current=overlayViewport(renderer);
 return createMemo<OverlayViewport>(()=>current()||{x:0,y:0,width:window().width,height:window().height});
}
export function overlayPoint(bounds:OverlayViewport,event:{x?:number;y?:number}){return{x:(event.x||0)-bounds.x,y:(event.y||0)-bounds.y};}

/** Global dialogs span the whole window below the chrome/tab rows. Anchored
 * menus continue to use the focused pane's separate overlay viewport. */
function dialogScope(renderer:object){let value=dialogs.get(renderer);if(!value){const[get,set]=createSignal<OverlayViewport>();value={get,set};dialogs.set(renderer,value);}return value;}
export function setDialogViewport(renderer:object,bounds:OverlayViewport|undefined){
 const next=bounds&&[bounds.x,bounds.y,bounds.width,bounds.height].every(Number.isFinite)&&bounds.width>0&&bounds.height>0?{...bounds}:undefined;
 dialogScope(renderer).set(previous=>previous&&next&&previous.x===next.x&&previous.y===next.y&&previous.width===next.width&&previous.height===next.height?previous:next);
}
export function useDialogViewport(renderer:object,window:Accessor<{width:number;height:number}>){
 const published=dialogScope(renderer).get,current=overlayViewport(renderer);
 return createMemo<OverlayViewport>(()=>{const explicit=published();if(explicit)return explicit;const size=window(),top=Math.min(size.height,Math.max(0,current()?.y||0));return{x:0,y:top,width:size.width,height:Math.max(0,size.height-top)};});
}
/** Keep the native renderer's original upper anchor, with only a bottom-edge
 * clamp for short windows. Paint, input, hits and caret share this translation. */
export function dialogOverlayOffset(bounds:Pick<OverlayViewport,"width"|"height">,rect:readonly number[]){
 return{x:Math.max(0,(bounds.width-rect[2])/2)-rect[0],y:Math.max(0,Math.min(rect[1],bounds.height-rect[3]))-rect[1]};
}

export function placeOverlayRect(rect:readonly number[],bounds:OverlayViewport):[number,number,number,number]{
 const width=rect[2],height=rect[3];
 return[Math.max(bounds.x,Math.min(rect[0],bounds.x+bounds.width-width)),Math.max(bounds.y,Math.min(rect[1],bounds.y+bounds.height-height)),width,height];
}
