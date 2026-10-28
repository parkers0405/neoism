/** GPUI already applies horizontal deltas to overflowX. Map only vertical
 * gestures, using the desktop buffer_tabs_scroll_dx 60px wheel step and .5px
 * noise threshold. Delivered compositor signs also drive native offsets. */
export function verticalTabScroll(event:{deltaX?:number;deltaY?:number;precise?:boolean},offset:number,viewport:number,content:number):number|undefined{
 const x=Number.isFinite(event.deltaX)?event.deltaX!:0,y=Number.isFinite(event.deltaY)?event.deltaY!:0;
 if(!y||(event.precise?Math.abs(x)>Math.abs(y):x!==0))return;
 const delta=y*(event.precise?1:60);if(Math.abs(delta)<.5)return;
 return Math.max(-Math.max(0,content-viewport),Math.min(0,offset+delta))||0;
}
/** Raw native scroll events have no browser default; synthetic hosts may
 * additionally expose cancellation. Tab strips are siblings of pane bodies. */
export function consumeTabScroll(event:object,preventDefault=false){const controls=event as {stopPropagation?:()=>void;preventDefault?:()=>void};controls.stopPropagation?.();if(preventDefault)controls.preventDefault?.();}
