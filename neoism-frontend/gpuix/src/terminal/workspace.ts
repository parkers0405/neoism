import type { Surface } from "../model";
import type { Workspace } from "../daemon/protocol";
import { posix,win32 } from "node:path";

export function absoluteTerminalDirectory(value:unknown):value is string {
 return typeof value==="string"&&value.length>0&&!value.includes("\0")&&(posix.isAbsolute(value)||win32.isAbsolute(value));
}
export function mainTerminalSurface(workspace:Workspace|undefined,surfaces:readonly Surface[],remembered:unknown):Surface|undefined{
 if(!workspace)return;const terminals=surfaces.filter(surface=>surface.kind==="terminal"&&surface.workspaceId===workspace.id);
 return terminals.find(surface=>surface.sessionId===remembered)||terminals.find(surface=>surface.sessionId===workspace.main_session_id)||terminals[0];
}
/** Desktop's root-terminal cwd follow is a local navigation choice. A joined
 * host's declared root and every secondary terminal remain independent. */
export function followsTerminalDirectory(workspace:Workspace,surface:Surface|undefined,main:Surface|undefined,local:boolean){
 return local&&(!workspace.host_kind||workspace.host_kind==="local")&&surface?.kind==="terminal"&&surface.id===main?.id;
}
