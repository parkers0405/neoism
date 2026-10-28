import type { AppConfig } from "../config";
import { coreCall, NativeEditor } from "../native-core";
import type { PaintCommand } from "../ui/PaintLayer";
import { sharePaintCommands } from "../editor/paint-command";
export interface MarkdownSnapshot {anchor?:unknown;width?:number;height?:number;version:1;line:number;col:number;scrollY:number;mode:string;vim:boolean}
export interface MarkdownFrame {
 menu?:{rect:number[];commands:PaintCommand[]}|null;assets?:string[];cover?:{value:string;rect:number[]}|null;cursorQuad:number[][];cursorHold:boolean;hostAction?:"save"|"save-close"|"close"|null;commands:PaintCommand[];revision:number;mode:"Normal"|"Insert"|"Visual";vim:boolean;dirty:boolean;scrollY:number;contentHeight:number;animating:boolean;dragging:boolean;
 cursor:{line:number;col:number;rect:number[]|null};effects?:{clipboard?:string|null;search?:boolean|null;palette?:boolean;blockMenu?:boolean;yank?:string;titleRename?:string;link?:{url?:string;path?:string;line?:number;target?:string}|null;matches?:Array<[number,number,string]>}|null;
}
/** Owns the original rich Pane independently of mount/unmount. The existing
 * canonical document retains daemon saves, recovery, and split synchronization. */
export class NativeMarkdown {
 readonly id:number;
 frame:MarkdownFrame;
 frameStartedAt=performance.now();
 private configuration:string;
 private pendingSnapshot:unknown;
 private listeners=new Set<(frame:MarkdownFrame)=>void>();
 private cleanup:()=>void;
 private syncing=false;
 private disposed=false;
 private richRevision:number;
 private editorRevision:number;
 private lastDirty:boolean;
 private editorCursor:string;
 private assetLoads=new Map<string,Promise<Uint8Array>>();
 private assetDelivered=new Set<string>();
 constructor(readonly editor:NativeEditor,config:AppConfig,path:string,private readonly readAsset:(path:string)=>Promise<Uint8Array>=async path=>new Uint8Array(await Bun.file(path).arrayBuffer())){
  this.configuration=this.signature(config);
  this.id=coreCall<{id:number}>("markdown.new",{path,text:editor.text(),vim:editor.frame.vim??config.vim,fontFamily:config.fontFamily,markdownFontFamily:config.markdownFontFamily,fontWeight:config.fontWeight,scale:config.scale,theme:config.themeName}).id;
  this.frame=coreCall("markdown.goto",{id:this.id,line:editor.frame.cursor.line,col:editor.frame.cursor.col});this.richRevision=this.frame.revision;this.editorRevision=editor.frame.revision;this.lastDirty=editor.frame.dirty;this.editorCursor=`${editor.frame.cursor.line}:${editor.frame.cursor.col}`;
  this.cleanup=editor.subscribe(frame=>{
   if(this.syncing||this.disposed)return;
   if(frame.revision!==this.editorRevision){this.editorRevision=frame.revision;this.call("source",{text:editor.text()},false);}
   const cursor=`${frame.cursor.line}:${frame.cursor.col}`;if(cursor!==this.editorCursor){this.editorCursor=cursor;this.call("goto",{line:frame.cursor.line,col:frame.cursor.col},false);}
   if(frame.vim!==undefined&&frame.vim!==this.frame.vim)this.call("toggle-vim",{},false);
   if(this.lastDirty&&!frame.dirty)this.call("saved",{},false);this.lastDirty=frame.dirty;
  });
 }
 private signature(config:AppConfig){return JSON.stringify([config.fontFamily,config.markdownFontFamily,config.fontWeight,config.scale,config.themeName]);}
 configure(config:AppConfig){const next=this.signature(config);if(next===this.configuration)return;this.configuration=next;this.assetDelivered.clear();return this.call("configure",{fontFamily:config.fontFamily,markdownFontFamily:config.markdownFontFamily,fontWeight:config.fontWeight,scale:config.scale,theme:config.themeName},false);}
 private assetBytes(path:string){let pending=this.assetLoads.get(path);if(!pending){pending=this.readAsset(path);this.assetLoads.set(path,pending);}return pending;}
 private loadAssets(frame:MarkdownFrame){for(const path of frame.assets||[]){if(this.assetDelivered.has(path))continue;this.assetDelivered.add(path);void this.assetBytes(path).then(bytes=>{if(!this.disposed)this.call("asset",{path,text:new TextDecoder().decode(bytes)},false);},()=>{if(!this.disposed)this.call("asset",{path,text:null},false);});}}
 async coverBytes(value:string){for(const path of coreCall<string[]>("markdown.cover-paths",{id:this.id,value})){try{const bytes=await this.assetBytes(path);if(bytes.length)return bytes;}catch{}}return undefined;}
 subscribe(listener:(frame:MarkdownFrame)=>void){this.listeners.add(listener);return()=>{this.listeners.delete(listener);};}
 call(op:string,args:Record<string,unknown>={},sync=true){
  if(this.disposed)throw new Error("Rich Markdown view is disposed");
  this.frameStartedAt=performance.now();
  let frame=coreCall<MarkdownFrame>(`markdown.${op}`,{id:this.id,...args});
  if(op==="layout"&&this.pendingSnapshot!==undefined){const snapshot=this.pendingSnapshot;this.pendingSnapshot=undefined;
   frame=coreCall<MarkdownFrame>("markdown.restore",{id:this.id,snapshot});
   if(snapshot&&typeof snapshot==="object"&&"anchor"in snapshot&&snapshot.anchor)frame=coreCall<MarkdownFrame>("markdown.restore-anchor",{id:this.id,anchor:snapshot.anchor});
  }
  frame.commands=sharePaintCommands(this.frame.commands,frame.commands);
  if(this.frame.menu&&frame.menu)frame.menu.commands=sharePaintCommands(this.frame.menu.commands,frame.menu.commands);
  this.frame=frame;
  if(frame.revision!==this.richRevision){this.richRevision=frame.revision;if(sync){this.syncing=true;try{this.editor.call("replace-source",{text:this.text()});this.editorRevision=this.editor.frame.revision;this.lastDirty=this.editor.frame.dirty;}finally{this.syncing=false;}}}
  if(sync&&["key","pointer","goto","restore","block-template","command","menu.pointer"].includes(op)){
   this.syncing=true;try{this.editor.call("goto",{line:frame.cursor.line,col:frame.cursor.col});this.editorCursor=`${this.editor.frame.cursor.line}:${this.editor.frame.cursor.col}`;}finally{this.syncing=false;}
  }
  for(const listener of this.listeners)listener(frame);this.loadAssets(frame);return frame;
 }
 text(){return coreCall<{text:string}>("markdown.text",{id:this.id}).text;}
 snapshot(){return this.pendingSnapshot===undefined?coreCall<MarkdownSnapshot>("markdown.snapshot",{id:this.id}):this.pendingSnapshot as MarkdownSnapshot;}
 restore(snapshot:unknown,deferUntilLayout=false){if(deferUntilLayout){this.pendingSnapshot=snapshot;return this.frame;}return this.call("restore",{snapshot},false);}
 dispose(){if(this.disposed)return;this.cleanup();this.listeners.clear();this.disposed=true;coreCall("markdown.dispose",{id:this.id});}
}
