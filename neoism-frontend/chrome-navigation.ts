import { callNativeCore as coreCall } from "./native-bridge";

export interface ChromeKey {key?:string;keyChar?:string;modifiers?:{alt?:boolean;ctrl?:boolean;shift?:boolean;cmd?:boolean}}
export type ChromeFocus = {zone:"content"|"sidebar"|"details"|"device"}|{zone:"buffers"|"workspaces";index:number};
export interface ChromeNavigationContext {bufferCount:number;activeBuffer:number;workspaceCount:number;workspacePlus?:boolean;activeWorkspace:number;sidebar:boolean}
export interface ChromeNavigation {focus:ChromeFocus;activate?:"buffer"|"workspace";direction?:"left"|"right"|"up"|"down"}

/** Shared desktop close_focus.rs ladder. Hosts own directional pane changes;
 * the Rust focus-step policy owns tab wrapping, not a second frontend policy. */
export function navigateChrome(focus:ChromeFocus,event:ChromeKey,context:ChromeNavigationContext):ChromeNavigation|undefined{
 const key=event.key?.toLowerCase(),mods=event.modifiers||{},alt=!!mods.alt&&!mods.ctrl&&!mods.shift&&!mods.cmd;
 const plain=!mods.alt&&!mods.ctrl&&!mods.shift&&!mods.cmd;
 if(alt&&key==="up"){
  if(focus.zone==="workspaces")return {focus};
  if(focus.zone==="buffers")return {focus:{zone:"workspaces",index:Math.max(0,context.activeWorkspace)}};
  return {focus:context.bufferCount?{zone:"buffers",index:Math.max(0,context.activeBuffer)}:{zone:"workspaces",index:Math.max(0,context.activeWorkspace)},direction:focus.zone==="content"?"up":undefined};
 }
 if((alt||plain)&&key==="down"){
  if(focus.zone==="workspaces")return {focus:context.bufferCount?{zone:"buffers",index:Math.max(0,context.activeBuffer)}:{zone:"content"}};
  if(focus.zone==="buffers")return {focus:{zone:"content"}};
  if(alt)return {focus,direction:focus.zone==="content"?"down":undefined};
 }
 if((alt||plain)&&(key==="left"||key==="right")){
  if(focus.zone==="buffers"||focus.zone==="workspaces"){
   if(alt&&key==="left"&&focus.index===0&&context.sidebar)return {focus:{zone:"sidebar"}};
   const index=coreCall<number>("keyboard.focus-step",{kind:focus.zone==="buffers"||context.workspacePlus?"buffer":"workspace",count:focus.zone==="buffers"?context.bufferCount:context.workspaceCount,index:focus.index,previous:key==="left"});
   return {focus:{...focus,index}};
  }
  if(alt){if(focus.zone==="sidebar")return {focus:key==="right"?{zone:"content"}:focus};if(focus.zone==="details"||focus.zone==="device")return {focus:key==="left"?{zone:"content"}:focus};return {focus,direction:key};}
 }
 if(plain&&(focus.zone==="buffers"||focus.zone==="workspaces")){
  if(key==="enter")return {focus,activate:focus.zone==="buffers"?"buffer":"workspace"};
  if(key==="escape")return {focus:focus.zone==="workspaces"?{zone:"buffers",index:Math.max(0,context.activeBuffer)}:{zone:"content"}};
  return {focus};
 }
}