import type {KeyEvent} from "@gpuix/solid";
import {bufferTabShortcut} from "./chrome-keyboard";
export type ApplicationShortcut="new-terminal"|"new-workspace"|"zoom-in"|"zoom-out"|"previous-buffer"|"next-buffer";
export function applicationShortcut(event:Pick<KeyEvent,"key"|"keyChar"|"modifiers">):ApplicationShortcut|undefined{
 const key=event.key?.toLowerCase()||"",character=event.keyChar||key,mods=event.modifiers;
 const buffer=bufferTabShortcut(event);
 if(buffer)return buffer==="previous"?"previous-buffer":"next-buffer";
 if(mods?.alt&&!mods.ctrl&&!mods.cmd&&!mods.shift&&key==="t")return "new-terminal";
 if((mods?.ctrl||mods?.cmd)&&mods.shift&&!mods.alt&&key==="w")return "new-workspace";
 if((mods?.ctrl||mods?.cmd)&&!mods.alt){
  if(["=","+","add","numpadadd"].includes(key)||character==="+")return "zoom-in";
  if(["-","subtract","numpadsubtract"].includes(key)||character==="-")return "zoom-out";
 }
}
interface Dispatcher{handler:(action:ApplicationShortcut,event:KeyEvent)=>void;seen:WeakSet<object>}
const dispatchers=new WeakMap<object,Dispatcher>();
/** One window-owned handler, shared by focused surfaces and modal/popup key
 * handlers that otherwise stop propagation before the application root. */
export function registerApplicationShortcuts(renderer:object,handler:Dispatcher["handler"]){
 const value:Dispatcher={handler,seen:new WeakSet()};dispatchers.set(renderer,value);
 return()=>{if(dispatchers.get(renderer)===value)dispatchers.delete(renderer);};
}
export function handleApplicationShortcut(renderer:object,event:KeyEvent):boolean{
 const dispatcher=dispatchers.get(renderer);if(!dispatcher)return false;
 if(dispatcher.seen.has(event))return true;
 const action=applicationShortcut(event);if(!action)return false;
 dispatcher.seen.add(event);event.preventDefault();event.stopPropagation();dispatcher.handler(action,event);return true;
}
