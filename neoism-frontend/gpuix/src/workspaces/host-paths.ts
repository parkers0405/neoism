import {posix,win32} from "node:path";

/** The daemon's filesystem determines path syntax, including Windows UNC and
 * verbatim roots returned by canonicalization, regardless of this client's OS. */
export function hostPaths(root:string){
 return /^[a-z]:[\\/]/i.test(root)||root.startsWith("\\\\")?win32:posix;
}
