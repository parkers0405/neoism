import type { Surface } from "../model";
import type { Workspace } from "../daemon/protocol";
import { posix,win32 } from "node:path";
import {localDaemon} from "../menu-actions/web-frontend";

export function absoluteTerminalDirectory(value:unknown):value is string {
 return typeof value==="string"&&value.length>0&&!value.includes("\0")&&(posix.isAbsolute(value)||win32.isAbsolute(value));
}
export function mainTerminalSurface(workspace:Workspace|undefined,surfaces:readonly Surface[],remembered:unknown):Surface|undefined{
 if(!workspace)return;const terminals=surfaces.filter(surface=>surface.kind==="terminal"&&surface.workspaceId===workspace.id);
 return terminals.find(surface=>surface.sessionId===remembered)||terminals.find(surface=>surface.sessionId===workspace.main_session_id)||terminals[0];
}
export interface TerminalTransport {endpoint:string;serverId?:string;transportIdentity?:string;remote?:boolean}
export function quickSshWorkspace(id:string){return id==="neoism-quick-ssh-home"||id.startsWith("neoism-quick-ssh-home-");}
/** Private Quick SSH behaves like the original local terminal. A forwarded
 * loopback address alone must never classify a joined server as local. */
export function terminalDirectoryFollowsWorkspace(workspace:Workspace,transport:TerminalTransport){
 const localKind=!workspace.host_kind||workspace.host_kind==="local";
 if(!localKind)return false;
 const ssh=transport.transportIdentity?.startsWith("ssh:")||transport.serverId?.startsWith("ssh:");
 if(ssh)return quickSshWorkspace(workspace.id);
 return !transport.remote&&(!transport.serverId||transport.serverId==="local")&&localDaemon(transport.endpoint);
}
/** Desktop's root-terminal cwd follow is a local navigation choice. A joined
 * host's declared root and every secondary terminal remain independent. */
export function followsTerminalDirectory(workspace:Workspace,surface:Surface|undefined,main:Surface|undefined,local:boolean|TerminalTransport){
 return (typeof local==="boolean"?local&&(!workspace.host_kind||workspace.host_kind==="local"):terminalDirectoryFollowsWorkspace(workspace,local))&&surface?.kind==="terminal"&&surface.id===main?.id;
}
