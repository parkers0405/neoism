import {createMemo,createSignal,type Accessor,type Setter} from "solid-js";
export interface OverlayViewport{x:number;y:number;width:number;height:number}
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

/** Move a native dialog's entire coordinate space together, including its
 * painter, input hit targets and caret. Bounds exclude the sidebar and tabs. */
export function centeredOverlayOffset(bounds:Pick<OverlayViewport,"width"|"height">,rect:readonly number[]){
 return{x:Math.max(0,(bounds.width-rect[2])/2)-rect[0],y:Math.max(0,(bounds.height-rect[3])/2)-rect[1]};
}

export function placeOverlayRect(rect:readonly number[],bounds:OverlayViewport):[number,number,number,number]{
 const width=rect[2],height=rect[3];
 return[Math.max(bounds.x,Math.min(rect[0],bounds.x+bounds.width-width)),Math.max(bounds.y,Math.min(rect[1],bounds.y+bounds.height-height)),width,height];
}
