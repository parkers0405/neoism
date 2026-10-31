import { coreCall } from "../native-core";
import { homedir } from "node:os";
import type { Workspace } from "../daemon/protocol";
import {absoluteTerminalDirectory,terminalDirectoryFollowsWorkspace,type TerminalTransport} from "../terminal/workspace";
export interface TabChrome {
  workspace: { height: number; titleFontSize: number; tabPaddingX: number; tabRadius: number; marginRight: number; iconGlyph: string; iconColor: number[] };
  buffer: { height: number; fontSize: number; iconSize: number; iconGap: number; padding: number; closeSize: number; closeHitSize: number; newWidth: number; newIcon: string; width: number; titleWidth: number; iconWidth: number; closeCenter: number | null };
}
export const tabChrome = (scale: number, titleWidth = 0, iconWidth = 12 * scale, close = true) => coreCall<TabChrome>("layout.chrome", { scale, titleWidth, iconWidth, close });
/** A workspace is its declared directory: the tab names that folder (`~` for
 * home), so re-rooting it never leaves a stale title. Title only without one. */
export function workspaceLabel(title: string, root?: string | null, home = process.env.HOME || homedir()) {
  const normalize=(path:string)=>{
    if(/^[a-z]:[\\/]/i.test(path)||path.startsWith("\\\\"))path=path.replace(/^\\\\\?\\UNC\\/i,"\\\\").replace(/^\\\\\?\\/,"").replaceAll("\\","/");
    return path.replace(/\/+$/, "");
  };
  const directory = root == null ? root : normalize(root);
  if (directory !== undefined && directory !== null && root) {
    const windows=/^[a-z]:\//i.test(directory)||directory.startsWith("//");
    const equal=(a:string,b:string)=>windows?a.toLowerCase()===b.toLowerCase():a===b;
    if (home && equal(directory,normalize(home))) return "~";
    return directory.split("/").pop() || "/";
  }
  return normalize(title).split("/").pop() || "~";
}
/** The Explorer's first-terminal cwd follow is window-local. Display its
 * confirmed directory without rewriting the saved host workspace metadata. */
export function workspaceTabRoot(workspace:Workspace,transport:TerminalTransport,roots:unknown):string|null|undefined{
  const entry=roots&&typeof roots==="object"&&!Array.isArray(roots)?(roots as Record<string,unknown>)[workspace.id]:undefined;
  if(terminalDirectoryFollowsWorkspace(workspace,transport)&&entry&&typeof entry==="object"&&!Array.isArray(entry)){
    const observed=entry as Record<string,unknown>;
    if(observed.declaredRoot===(workspace.root_dir||null)&&absoluteTerminalDirectory(observed.root))return observed.root;
  }
  return workspace.root_dir;
}
export const rgba = (color: readonly number[]) => "#" + color.map((n) => Math.round(n).toString(16).padStart(2, "0")).join("");
export function inactiveTabIcon(color: string) {
  const bytes = color.replace(/^#/, "").match(/../g)?.slice(0, 3).map((n) => Math.max(0, parseInt(n, 16) - 60)) || [140, 140, 140];
  return rgba([...bytes, 160]);
}
