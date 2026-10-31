import * as Effect from "effect/Effect";
import {posix,win32} from "node:path";
import type {WorkspaceModel} from "../model";
import {expectVariant} from "../daemon/protocol";
import type {DirEntry,FilesClientMessage} from "../../../../neoism-protocol/typescript/types";
import type {ContextItem} from "./SidebarContext";

export interface TreeContextTarget {workspaceId:string;root:string;notes:boolean;path?:string;directory:string}
const clipboard=new WeakMap<WorkspaceModel,{root:string;path:string}>();
const parent=(path:string)=>posix.dirname(path)==="."?"":posix.dirname(path);
const pathApi=(root:string)=>/^[a-z]:[\\/]/i.test(root)||root.startsWith("\\\\")?win32:posix;
export function treeContextItems(path:string|undefined,showHidden:boolean):ContextItem[]{
 return [...(path!==undefined?[{id:"open",label:"Edit / Open",hint:"e"},{id:"copy",label:"Copy",hint:"c"}]:[]),
  {id:"paste",label:"Paste Here",hint:"p"},{id:"new-file",label:"New File",hint:"n"},{id:"new-folder",label:"New Folder",hint:"f"},
  ...(path!==undefined?[{id:"rename",label:"Rename",hint:"r"},{id:"delete",label:"Delete",hint:"d"}]:[]),
  {id:"hidden",label:showHidden?"Hide Dotfiles":"Show Dotfiles",hint:"h"}];
}
export function treeContextCurrent(model:WorkspaceModel,target:TreeContextTarget){const workspace=model.state.activeWorkspace;return workspace?.id===target.workspaceId&&(target.notes?workspace.notes_vault_dir:workspace.root_dir)===target.root;}
function name(value:string){if(!value.trim()||value==="."||value===".."||/[\\/\0]/.test(value))throw new Error("Enter a file name without path separators");return value;}
/** Root and workspace are captured when the menu opens, never taken from a
 * different tab after a delayed confirmation or request. All IO is daemon IO. */
export async function performTreeContext(model:WorkspaceModel,target:TreeContextTarget,action:string,value?:string){
 if(!treeContextCurrent(model,target))throw new Error("This file menu belongs to a different workspace");
 const request=async<T>(message:FilesClientMessage,tag:string,root=target.root)=>expectVariant(await Effect.runPromise(model.client.request("Files",message,root)),tag) as T;
 const listing=(path:string,root=target.root)=>request<{entries:DirEntry[]}>({ListDir:{path}},"DirListing",root);
 if(action==="copy"){
  if(target.path===undefined)return;
  await request({Stat:{path:target.path}},"Stat");clipboard.set(model,{root:target.root,path:target.path});return;
 }
 if(action==="rename"){
  if(target.path===undefined)return;
  await model.moveTreePath(target.path,parent(target.path),target.notes,name(value||""));return;
 }
 if(action==="delete"){
  if(target.path===undefined)return;
  await request({Delete:{path:target.path}},"Deleted");
  const under=(path:string)=>path===target.path||path.startsWith(target.path+"/");
  const copied=clipboard.get(model);if(copied?.root===target.root&&under(copied.path))clipboard.delete(model);
  for(const surface of model.state.surfaces.filter(surface=>surface.kind==="file"&&!surface.document&&surface.workspaceId===target.workspaceId&&surface.rootDir===target.root&&surface.path&&under(surface.path)))model.closeSurface(surface.id,true);
 }else if(action==="new-file"||action==="new-folder"){
  const entry=name(value||"");await request(action==="new-file"?{CreateFile:{dir:target.directory,name:entry}}:{CreateDir:{dir:target.directory,name:entry}},"FileCreated");
 }else if(action==="paste"){
  const source=clipboard.get(model);if(!source)throw new Error("Nothing copied in the file tree");
  const paths=pathApi(target.root),sourcePath=paths.resolve(source.root,source.path),destination=paths.resolve(target.root,target.directory);
  const relative=paths.relative(sourcePath,destination);
  const stat=await request<{entry:DirEntry}>({Stat:{path:source.path}},"Stat",source.root);
  if(stat.entry.is_dir&&(relative===""||(!relative.startsWith(".."+paths.sep)&&relative!==".."&&!paths.isAbsolute(relative))))throw new Error("A folder cannot be pasted inside itself");
  const entries=await listing(target.directory),names=new Set(entries.entries.map(entry=>paths===win32?entry.name.toLowerCase():entry.name));
  const original=posix.basename(source.path),ext=posix.extname(original),stem=original.slice(0,original.length-ext.length);let candidate=original;
  for(let index=1;names.has(paths===win32?candidate.toLowerCase():candidate);index++)candidate=`${stem} copy${index===1?"":` ${index}`}${ext}`;
  const copy=async(from:string,to:string,dir:boolean,depth:number):Promise<void>=>{
   if(depth>100)throw new Error("Folder nesting is too deep to copy");
   if(dir){const children=await listing(from,source.root);await request({CreateDir:{dir:parent(to),name:posix.basename(to)}},"FileCreated");for(const child of children.entries){name(child.name);await copy(posix.join(from,child.name),posix.join(to,child.name),child.is_dir,depth+1);}}
   else{const content=await request<{bytes:number[]}>({ReadFile:{path:from}},"FileContent",source.root);await request({CreateFile:{dir:parent(to),name:posix.basename(to)}},"FileCreated");await request({WriteFile:{path:to,bytes:content.bytes}},"FileWritten");}
  };
  await copy(source.path,posix.join(target.directory,candidate),stat.entry.is_dir,0);
 }else return;
 if(treeContextCurrent(model,target)){
  const directories=target.notes?model.state.notesDirectories:model.state.directories;
  const removed=action==="delete"?target.path:undefined;
  const retained=Object.fromEntries(Object.entries(directories).filter(([path])=>removed===undefined||path!==removed&&!path.startsWith(removed+"/")));
  model.update(target.notes?{notesDirectories:retained}:{directories:retained});
  await model.refreshTreeDirectory(action==="delete"?parent(target.path!):target.directory,target.notes);
  if(action==="new-file"&&treeContextCurrent(model,target))await model.openFile(posix.join(target.directory,value!),target.root);
 }
}
