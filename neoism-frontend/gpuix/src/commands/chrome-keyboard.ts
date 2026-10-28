import { coreCall } from "../native-core";
import type { CommandTarget } from "./CommandController";
export interface ChromeKey {key?:string;keyChar?:string;modifiers?:{alt?:boolean;ctrl?:boolean;shift?:boolean;cmd?:boolean}}
export type ChromeFocus = {zone:"content"|"sidebar"|"details"}|{zone:"buffers"|"workspaces";index:number};
export interface ChromeNavigationContext {bufferCount:number;activeBuffer:number;workspaceCount:number;workspacePlus?:boolean;activeWorkspace:number;sidebar:boolean}
export interface ChromeNavigation {focus:ChromeFocus;activate?:"buffer"|"workspace";direction?:"left"|"right"|"down"}
/** Linux/Windows desktop SelectPrev/NextBufferTab defaults. Modified vertical
 * arrows remain available to each editor/terminal's own input engine. */
export function bufferTabShortcut(event:ChromeKey):"previous"|"next"|undefined{
 const modifiers=event.modifiers||{};
 if(!modifiers.ctrl||!modifiers.shift||modifiers.alt||modifiers.cmd)return;
 const key=event.key?.toLowerCase().replace(/^arrow/,"");
 return key==="left"?"previous":key==="right"?"next":undefined;
}
export function workspaceShortcut(event:ChromeKey):number|undefined{
 const key=/^\d$/.test(event.key||"")?event.key!:event.keyChar||event.key||"";if(!/^\d$/.test(key)||!event.modifiers?.alt)return;
 return coreCall<number|null>("keyboard.workspace-index",{key,alt:!!event.modifiers?.alt,ctrl:!!event.modifiers?.ctrl,shift:!!event.modifiers?.shift,cmd:!!event.modifiers?.cmd})??undefined;
}
export function lineCommandTarget(query:string):CommandTarget|undefined{
 return coreCall<CommandTarget|null>("keyboard.ex-target",{query})??undefined;
}
/** Desktop close_focus.rs focus ladder. BufferTabs and Island themselves own
 * the wrapping cursor policy, including the buffer strip's trailing + slot. */
export function navigateChrome(focus:ChromeFocus,event:ChromeKey,context:ChromeNavigationContext):ChromeNavigation|undefined{
 const key=event.key?.toLowerCase(),mods=event.modifiers||{},alt=!!mods.alt&&!mods.ctrl&&!mods.shift&&!mods.cmd;
 const plain=!mods.alt&&!mods.ctrl&&!mods.shift&&!mods.cmd;
 if(alt&&key==="up"){
  if(focus.zone==="workspaces")return {focus};
  if(focus.zone==="buffers")return {focus:{zone:"workspaces",index:Math.max(0,context.activeWorkspace)}};
  return {focus:context.bufferCount?{zone:"buffers",index:Math.max(0,context.activeBuffer)}:{zone:"workspaces",index:Math.max(0,context.activeWorkspace)}};
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
  if(alt){if(focus.zone==="sidebar")return {focus:key==="right"?{zone:"content"}:focus};if(focus.zone==="details")return {focus:key==="left"?{zone:"content"}:focus};return {focus,direction:key};}
 }
 if(plain&&(focus.zone==="buffers"||focus.zone==="workspaces")){
  if(key==="enter")return {focus,activate:focus.zone==="buffers"?"buffer":"workspace"};
  if(key==="escape")return {focus:focus.zone==="workspaces"?{zone:"buffers",index:Math.max(0,context.activeBuffer)}:{zone:"content"}};
  return {focus};
 }
}
