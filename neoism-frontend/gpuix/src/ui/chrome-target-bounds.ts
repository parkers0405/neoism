export interface ChromeBounds {x:number;y:number;width:number;height:number}
export interface ChromeBoundsRenderer {getElementBounds?:(id:number)=>ChromeBounds|null|undefined}
/** onLayout reports size changes, not translated paint positions. Read the
 * current border box after a focus/scroll/layout change, never per trail frame. */
export function paintedChromeBounds(renderer:ChromeBoundsRenderer,id:number):ChromeBounds|undefined{
 const value=renderer.getElementBounds?.(id);
 return value&&[value.x,value.y,value.width,value.height].every(Number.isFinite)&&value.width>0&&value.height>0?{x:value.x,y:value.y,width:value.width,height:value.height}:undefined;
}
