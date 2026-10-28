import {createSignal} from "solid-js";
import type {HostElement} from "@gpuix/solid";
interface SurfaceCaret {host:HostElement;quad:()=>number[][]}
interface CursorHandoff {carets:Map<number,SurfaceCaret>;suppressed:()=>boolean;suppress:(value:boolean)=>void}
const windows=new WeakMap<object,CursorHandoff>();
function state(renderer:object){let value=windows.get(renderer);if(!value){const [suppressed,suppress]=createSignal(false);value={carets:new Map(),suppressed,suppress};windows.set(renderer,value);}return value;}
/** Cache WINDOW-coordinate quads (view getters include their cached layout origin).
 * No native UI query or second origin adjustment is needed during travel. */
export function registerSurfaceCaret(renderer:object,host:HostElement,quad:()=>number[][]){const value=state(renderer),caret={host,quad};value.carets.set(host.id,caret);return()=>{if(value.carets.get(host.id)===caret)value.carets.delete(host.id);};}
export function surfaceCursorSuppressed(renderer:object){return state(renderer).suppressed();}
export function suppressSurfaceCursor(renderer:object,value:boolean){state(renderer).suppress(value);}
export function surfaceCaret(renderer:object,id:number|null|undefined){const caret=id==null?undefined:state(renderer).carets.get(id);if(!caret?.host.mounted)return;const points=caret.quad();if(points.length!==4)return;const xs=points.map(point=>point[0]!),ys=points.map(point=>point[1]!);return {host:caret.host,bounds:{x:Math.min(...xs),y:Math.min(...ys),width:Math.max(...xs)-Math.min(...xs),height:Math.max(...ys)-Math.min(...ys)}};}
