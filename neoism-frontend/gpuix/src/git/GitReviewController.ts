import * as Effect from "effect/Effect";
import type {AppConfig} from "../config";
import type {WorkspaceModel} from "../model";
import {coreCall,type NativeEditor} from "../native-core";
import {isRecord,type GitClientMessage} from "../daemon/protocol";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import {sharePaintCommands} from "../editor/paint-command";
import type {PaintCommand} from "../ui/PaintLayer";
import type {GitController,GitRect,GitReviewAction} from "./GitController";
import {readClipboard,writeClipboard} from "../clipboard";
import {watchGitStatus} from "../daemon/git-status";
export interface GitReviewDescriptor{mode:"working"|"commit"|"pull-request"|"pull-request-inbox"|"pull-request-create";root:string;path?:string;revision?:string;detail?:unknown}
export interface GitReviewFrame{root:string;path:string;readOnly:boolean;split:boolean;effectiveSplit:boolean;scrollY:number;maxScrollY:number;horizontal:number[];maxHorizontal:number[];column:number|null;editingPath:string|null;documentPath:string|null;error?:string|null;dragging:boolean;loading:boolean;files:Array<{path:string;collapsed:boolean;loaded:boolean;additions:number;deletions:number;staged:boolean;rows:number}>;headers:Array<{path:string;rect:GitRect}>;toolbar:Array<{id:string;rect:GitRect;enabled:boolean}>;fileActions:Array<{path:string;restore:boolean;rect:GitRect;enabled:boolean}>;sourceHits:Array<{path:string;line:number;rect:GitRect}>;editHits:Array<{path:string;line:number;rect:GitRect}>;commands:PaintCommand[];animating:boolean;actions:GitReviewAction[];copy?:string|null;edit?:{root:string;path:string;line:number;column:number}|null;cursorQuad?:number[][]|null;cursorToken?:number[];cursorShape?:string;dirty?:boolean;clipboard?:string|null;hostAction?:string|null;pointer:boolean;textPointer:boolean}
interface Options{model:WorkspaceModel;config:AppConfig;descriptor:GitReviewDescriptor;git:GitController;workspaceRoot?:string;loadEditor?:(root:string,path:string)=>Promise<NativeEditor>;onClose?:()=>void}
/** The original GitDiffView owns review state, hit testing and projected edits.
 * Reviews share scoped status while retaining independent patch/read epochs. */
export class GitReviewController{
 readonly id:number;frame:GitReviewFrame;readonly descriptor:GitReviewDescriptor;
 private listeners=new Set<(frame:GitReviewFrame)=>void>();private animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 private disposed=false;private revision=0;private configuration:string;private source?:NativeEditor;private releaseSource?:()=>void;private publishingSource=false;private attachSerial=0;private pending=new Set<string>();private unconnection:()=>void;private releaseWatch?:()=>void;private connection:string;private input=Promise.resolve();private workspaceRoot:string;private pointerDown?:{x:number;y:number};
 constructor(readonly options:Options){
  this.descriptor=options.descriptor;this.workspaceRoot=options.workspaceRoot||options.descriptor.root;this.configuration=this.signature(options.config);
  this.id=coreCall<{id:number}>("gitreview.new",{...this.descriptor,fontFamily:options.config.fontFamily,fontWeight:options.config.fontWeight,fontSize:options.config.fontSize,theme:options.config.themeName}).id;
  this.frame=coreCall<GitReviewFrame>("gitreview.frame",{id:this.id});this.connection=options.model.client.state.phase;
  this.unconnection=options.model.client.subscribeStatus(state=>{if(state.phase===this.connection)return;this.connection=state.phase;if(state.phase==="connected"){this.watch();this.refresh();}});
  this.apply(this.frame);this.watch();this.refresh();
 }
 private signature(config:AppConfig){return JSON.stringify([config.fontFamily,config.fontWeight,config.fontSize,config.themeName]);}
 configure(config:AppConfig){const signature=this.signature(config);if(signature===this.configuration)return;this.configuration=signature;this.call("configure",{fontFamily:config.fontFamily,fontWeight:config.fontWeight,fontSize:config.fontSize,theme:config.themeName});}
 subscribe=(listener:(frame:GitReviewFrame)=>void)=>{this.listeners.add(listener);return()=>{this.listeners.delete(listener);};};
 private watch(){if(this.descriptor.mode==="working"&&!this.releaseWatch)this.releaseWatch=watchGitStatus(this.options.model.client,this.workspaceRoot,(snapshot,token)=>{if(this.connection==="connected")this.status({RepoStatus:{token,snapshot}});});}
 private report=(error:unknown)=>{if(!this.disposed)this.options.model.report(error);};
 private async read(request:GitClientMessage){return Effect.runPromise(this.options.model.client.request("Git",request,this.workspaceRoot));}
 private async request(request:GitClientMessage,key:string,revision?:number){
  if(this.disposed||this.pending.has(key))return;this.pending.add(key);
  try{const reply=await this.read(request);if(this.disposed||revision!==undefined&&revision!==this.revision)return;if(isRecord(reply)&&(reply.ChangedFiles||reply.RepoStatus))this.status(reply);else this.call("reply",{message:reply});}
  catch(error){if(!this.disposed){this.call("reply",{message:{Error:{message:error instanceof Error?error.message:String(error)}}});this.report(error);}}
  finally{this.pending.delete(key);}
 }
 refresh(){if(this.disposed||this.connection!=="connected")return;
  if(this.descriptor.mode==="working")void this.request("ChangedFiles","status");
  else if(this.descriptor.mode==="commit"&&!this.descriptor.detail&&this.descriptor.revision)void this.request({ReadCommit:{revision:this.descriptor.revision}},"commit");
 }
 private status(message:unknown){if(this.disposed||this.descriptor.mode!=="working")return;this.revision++;const revision=this.revision;this.call("reply",{message});const paths=this.frame.files.map(file=>file.path);if(paths.length)void this.request({DiffFiles:{paths}},`diff:${revision}`,revision);}
 call(op:string,args:Record<string,unknown>={}):GitReviewFrame{
  if(this.disposed)return this.frame;const started=performance.now();this.animation.cancel();const next=coreCall<GitReviewFrame>(`gitreview.${op}`,{id:this.id,...args});this.apply(next);
  if(next.animating)this.animation.request(()=>this.call("frame"),started);return next;
 }
 private apply(next:GitReviewFrame){next.commands=sharePaintCommands(this.frame?.commands||[],next.commands);this.frame=next;for(const listener of this.listeners)listener(next);
  if(next.copy!=null)void writeClipboard(next.copy).catch(this.report);if(next.clipboard!=null)void writeClipboard(next.clipboard).catch(this.report);if(next.hostAction==="close")this.options.onClose?.();
  for(const action of next.actions||[])void this.execute(action).catch(this.report);
  if(next.edit)void this.attach(next.edit).catch(this.report);
 }
 private async execute(action:GitReviewAction){
  if(action.kind==="pull-request"){
   // The canonical form builds expected-head/branch guards and serializes its
   // pending state. The daemon retains permission and repository ownership.
   await this.request({PullRequest:{request:action.request}} as GitClientMessage,`pr:${JSON.stringify(action.request)}`);return;
  }
  this.call("loading",{loading:true});try{await this.options.git.execute({...action,root:action.root||this.descriptor.root},this.workspaceRoot);if(!this.disposed&&this.descriptor.mode==="working")this.refresh();}finally{if(!this.disposed)this.call("loading",{loading:false});}
 }
 private async attach(edit:NonNullable<GitReviewFrame["edit"]>){
  const serial=++this.attachSerial,loader=this.options.loadEditor||((root:string,path:string)=>(this.options.model as WorkspaceModel & {loadGitEditor(root:string,path:string):Promise<NativeEditor>}).loadGitEditor(root,path));
  const editor=await loader(edit.root,edit.path);if(this.disposed||serial!==this.attachSerial||this.frame.documentPath!==edit.root.replace(/[\\/]+$/,"")+"/"+edit.path.replace(/^\/+/,""))return;
  if(this.source!==editor){this.releaseSource?.();this.source=editor;this.releaseSource=editor.subscribe(()=>{if(!this.publishingSource&&!this.disposed)this.call("frame");});}
  this.call("attach",{editorID:editor.id,line:edit.line,column:edit.column});if(this.pointerDown)this.call("select",this.pointerDown);this.publishSource();
  void this.request({ReadBase:{path:edit.path}},`base:${edit.path}`,this.revision);
 }
 private publishSource(){if(!this.source||this.disposed)return;this.publishingSource=true;try{this.source.call("frame");this.source.cancelFrame();}finally{this.publishingSource=false;}this.call("frame");}
 layout(width:number,height:number){return this.call("layout",{width,height});}
 reveal(path:string){return this.call("reveal",{path});}
 key(args:Record<string,unknown>){this.call("key",args);if(this.frame.editingPath)this.publishSource();}
 pointer(kind:string,x:number,y:number){if(kind==="down")this.pointerDown={x,y};else if(kind==="up")this.pointerDown=undefined;return this.call("pointer",{kind,x,y});}
 queueKey(args:Record<string,unknown>){const key=String(args.key||""),primary=!!args.ctrl||!!args.cmd;this.input=this.input.then(async()=>{
  if(this.disposed)return;if(primary&&["c","x"].includes(key)&&this.source&&this.frame.editingPath){const result=coreCall<{text:string}>(`editor.${key==="c"?"copy":"cut"}`,{id:this.source.id});await writeClipboard(result.text);this.publishSource();}
  else if(primary&&key==="v"){const text=await readClipboard();if(text){if(this.source&&this.frame.editingPath){this.source.call("paste",{text});this.source.cancelFrame();this.call("frame");}else this.call("text",{text});}}
  else this.key(args);
 }).catch(this.report);return this.input;}
 save(){return this.source?(this.options.model as WorkspaceModel & {saveGitEditor(editor:NativeEditor):Promise<void>}).saveGitEditor(this.source):Promise.resolve();}
 dispose(){if(this.disposed)return;this.disposed=true;this.attachSerial++;this.releaseWatch?.();this.animation.dispose();this.releaseSource?.();this.unconnection();this.listeners.clear();coreCall("gitreview.dispose",{id:this.id});}
}
