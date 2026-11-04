import { callNativeCore as coreCall } from "../../../native-bridge";
import type { CommandTarget } from "./CommandController";
import type { ChromeKey } from "../../../chrome-navigation";
export { navigateChrome } from "../../../chrome-navigation";
export type { ChromeKey, ChromeFocus, ChromeNavigationContext, ChromeNavigation } from "../../../chrome-navigation";
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
