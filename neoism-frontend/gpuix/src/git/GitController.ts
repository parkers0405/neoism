import * as Effect from "effect/Effect";
import type {AppConfig} from "../config";
import type {WorkspaceModel,Surface} from "../model";
import type {GitClientMessage} from "../daemon/protocol";
import {isRecord} from "../daemon/protocol";
import {coreCall} from "../native-core";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import type {PaintCommand} from "../ui/PaintLayer";
import {sharePaintCommands} from "../editor/paint-command";
import {writeClipboard} from "../clipboard";
import {watchGitStatus} from "../daemon/git-status";
export type GitRect=[number,number,number,number];
export interface GitFile{path:string;status:string;additions:number;deletions:number;staged:boolean}
export interface GitFrame{rect:GitRect;width:number;visible:boolean;focused:boolean;loading:boolean;error?:string|null;branch?:string|null;upstream?:string|null;repoRoot?:string|null;files:GitFile[];selected?:number|null;cursorRect?:GitRect|null;commitText:string;commitFocused:boolean;branchQuery:string;branchMenu:boolean;actionsMenu:boolean;animating:boolean;commands:PaintCommand[];hits:Array<{id:string;rect:GitRect;branch?:string}>;filesScrollbar:GitRect;diffScrollbar:GitRect;requests:GitClientMessage[];effects:GitEffect[]}
export type GitEffect={kind:string;root?:string;path?:string;text?:string;detail?:unknown;reply?:unknown;[key:string]:unknown};
export type GitReviewAction={kind:string;root:string;path?:string;unstage?:boolean;restore?:boolean;block?:unknown;request?:unknown;line?:number;column?:number;url?:string};
export interface GitHooks{openReview?:(target:{root:string;path?:string})=>void|Promise<void>;openCommit?:(target:{root:string;detail:unknown})=>void|Promise<void>;openPullRequest?:(effect:GitEffect)=>void|Promise<void>;pullRequestReply?:(reply:unknown)=>void;notice?:(text:string)=>void;confirmRestore?:(target:{root:string;path:string;block?:unknown})=>Promise<boolean>;openSource?:(target:{root:string;path:string;line:number;column:number})=>void|Promise<void>;openUrl?:(url:string)=>void|Promise<void>}
interface Options{model:WorkspaceModel;config:AppConfig;hooks?:GitHooks}
interface Pending{request:GitClientMessage;epoch:number;workspaceRoot:string;key?:string}
const requestKind=(request:GitClientMessage)=>typeof request==="string"?request:Object.keys(request)[0];
const mutationKinds=new Set(["Stage","Unstage","Restore","ApplyBlock","Commit","Checkout","Fetch","Pull","Push","Publish"]);
/** Canonical Rust state/paint, actual daemon Git transport. A controller belongs
 * to one window/session and survives view/font component generations. */
export class GitController{
 readonly id:number;frame:GitFrame;
 private hooks:GitHooks;
 private listeners=new Set<(frame:GitFrame)=>void>();
 private replies=new Set<(message:unknown,workspaceRoot?:string)=>void>();
 private animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 private workspaceRoot?:string;
 private epoch=0;
 private statusRevision=0;
 private releaseWatch?:()=>void;
 private queue:Pending[]=[];
 private reads=new Set<string>();
 private pumping=false;
 private mutating=false;
 private disposed=false;
 private configuration:string;
 private cleanupConnection:()=>void;
 private connection:string;
 private viewport={width:1440,height:800};
 constructor(readonly options:Options){
  this.hooks=options.hooks||{};this.configuration=this.signature(options.config);
  this.id=coreCall<{id:number}>("gitpanel.new",{fontFamily:options.config.fontFamily,fontWeight:options.config.fontWeight}).id;
  this.frame=coreCall<GitFrame>("gitpanel.frame",{id:this.id});
  this.connection=options.model.client.state.phase;
  this.cleanupConnection=options.model.client.subscribeStatus(state=>{if(state.phase===this.connection)return;this.connection=state.phase;if(state.phase==="connected"&&this.frame.visible&&this.workspaceRoot){this.watch();this.call("refresh");}});
 }
 private signature(config:AppConfig){return JSON.stringify([config.fontFamily,config.fontWeight,config.themeName,config.scale]);}
 configure(config:AppConfig){this.options.config=config;const signature=this.signature(config);if(signature===this.configuration)return;this.configuration=signature;this.call("configure",{fontFamily:config.fontFamily,fontWeight:config.fontWeight});this.layout(this.viewport.width,this.viewport.height);}
 setHooks(hooks:GitHooks){this.hooks=hooks;return()=>{if(this.hooks===hooks)this.hooks={};};}
 subscribe=(listener:(frame:GitFrame)=>void)=>{this.listeners.add(listener);return()=>{this.listeners.delete(listener);};};
 subscribeReplies=(listener:(reply:unknown,workspaceRoot?:string)=>void)=>{this.replies.add(listener);return()=>{this.replies.delete(listener);};};
 get root(){return this.workspaceRoot;}
 private watch(){
  if(this.releaseWatch||!this.workspaceRoot)return;
  this.releaseWatch=watchGitStatus(this.options.model.client,this.workspaceRoot,(snapshot,token)=>{if(!this.mutating&&this.options.model.client.state.phase==="connected")this.feed({RepoStatus:{token,snapshot}});});
 }
 private unwatch(){this.releaseWatch?.();this.releaseWatch=undefined;}
 open(workspaceRoot:string){
  if(this.disposed)return;
  if(this.workspaceRoot!==workspaceRoot){this.unwatch();this.epoch++;this.statusRevision++;this.queue=[];this.reads.clear();this.workspaceRoot=workspaceRoot;}
  // Cached status is delivered synchronously. Open first so its first frame
  // cannot publish visible:false and cancel the user's persisted panel flag.
  this.call("open",{root:workspaceRoot});this.watch();return this.frame;
 }
 close(){this.unwatch();return this.call("close");}
 layout(width:number,height:number){this.viewport={width,height};return this.call("layout",{width,height,scale:this.options.config.scale,theme:this.options.config.themeName});}
 call(op:string,args:Record<string,unknown>={}):GitFrame{
  if(this.disposed)return this.frame;
  const started=performance.now();this.animation.cancel();
  const next=coreCall<GitFrame>(`gitpanel.${op}`,{id:this.id,...args});next.commands=sharePaintCommands(this.frame.commands,next.commands);this.frame=next;
  for(const listener of this.listeners)listener(next);
  for(const request of next.requests)this.enqueue(request);
  for(const effect of next.effects)void this.effect(effect).catch(this.options.model.report);
  if(next.animating)this.animation.request(()=>this.call("frame"),started);
  if(!next.visible)this.unwatch();
  return next;
 }
 private feed(message:unknown){
  if(!isRecord(message)||this.disposed)return;
  if(message.RepoStatus||message.ChangedFiles)this.statusRevision++;
  this.call("reply",{message});for(const listener of this.replies)listener(message,this.workspaceRoot);
 }
 private enqueue(request:GitClientMessage){
  if(!this.workspaceRoot||this.disposed)return;
  const kind=requestKind(request),key=mutationKinds.has(kind)||kind==="PullRequest"?undefined:JSON.stringify([this.epoch,request]);
  if(key&&this.reads.has(key))return;if(key)this.reads.add(key);
  this.queue.push({request,epoch:this.epoch,workspaceRoot:this.workspaceRoot,key});void this.pump();
 }
 private async pump(){
  if(this.pumping||this.disposed)return;this.pumping=true;
  try{while(this.queue.length&&!this.disposed){const pending=this.queue.shift()!,{request,epoch,workspaceRoot,key}=pending;let readKey=key;
   try{
    if(epoch!==this.epoch)continue;
    const kind=requestKind(request),revision=this.statusRevision;
    this.mutating=mutationKinds.has(kind);
    if(kind==="Stage"||kind==="ApplyBlock"&&typeof request!=="string"&&"ApplyBlock"in request&&!request.ApplyBlock.restore){const body=(request as unknown as Record<string,{path:string}>)[kind];if(body?.path)await this.saveCanonical(this.frame.repoRoot||workspaceRoot,body.path);}
    if(epoch!==this.epoch||this.disposed)continue;
    const reply=await Effect.runPromise(this.options.model.client.request("Git",request,workspaceRoot));
    if(epoch!==this.epoch||this.disposed)continue;
    if(["DiffFiles","ReadBase"].includes(kind)&&revision!==this.statusRevision){
     // Release this read before asking native policy to queue its replacement.
     // Its finally block must not remove the new request's identical key.
     if(readKey)this.reads.delete(readKey);readKey=undefined;this.call("request-review");continue;
    }
    this.feed(reply);
    if(kind==="Commit"&&isRecord(reply)&&isRecord(reply.ChangedFiles)&&!reply.ChangedFiles.error&&typeof request!=="string"&&"Commit"in request)this.call("commit-succeeded",{message:request.Commit.message});
   }catch(error){if(epoch===this.epoch&&!this.disposed){this.feed({Error:{message:error instanceof Error?error.message:String(error)}});this.options.model.report(error);}}
   finally{this.mutating=false;if(readKey)this.reads.delete(readKey);}
  }}finally{this.pumping=false;}
 }
 private async saveCanonical(root:string,path:string){
  const absolute=(root.replace(/[\\/]+$/,"")+"/"+path).replace(/\\/g,"/");
  const saved=new Set<string>();
  for(const surface of this.options.model.state.surfaces){if(surface.kind!=="file"||!surface.path)continue;const candidate=/^(?:[A-Za-z]:[\\/]|[\\/])/.test(surface.path)?surface.path:(surface.rootDir||this.options.model.state.activeWorkspace?.root_dir||"").replace(/[\\/]+$/,"")+"/"+surface.path;
   if(candidate.replace(/\\/g,"/")!==absolute||saved.has(candidate))continue;
   const editor=this.options.model.getEditor(surface as Surface);if(editor.frame.dirty){await this.options.model.saveFile(surface);saved.add(candidate);}
  }
 }
 private async effect(effect:GitEffect){
  switch(effect.kind){
   case"copy":if(effect.text!==undefined)await writeClipboard(effect.text);break;
   case"review":if(effect.root)await this.hooks.openReview?.({root:effect.root,path:effect.path});break;
   case"commit-detail":if(effect.root)await this.hooks.openCommit?.({root:effect.root,detail:effect.detail});break;
   case"pr-inbox":case"pr-create":case"pr-detail":await this.hooks.openPullRequest?.(effect);break;
   case"pr-reply":this.hooks.pullRequestReply?.(effect.reply);break;
   case"notice":if(effect.text)this.hooks.notice?.(effect.text);break;
  }
 }
 async read(request:GitClientMessage,workspaceRoot=this.workspaceRoot){
  if(!workspaceRoot||this.disposed)throw new Error("Select a Git workspace first");
  return Effect.runPromise(this.options.model.client.request("Git",request,workspaceRoot));
 }
 async execute(action:GitReviewAction,workspaceRootOverride?:string){
  const scope=workspaceRootOverride||this.workspaceRoot||this.options.model.state.activeWorkspace?.root_dir;
  if(!scope||this.disposed)return false;
  if(!workspaceRootOverride&&this.frame.repoRoot&&action.root!==this.frame.repoRoot)throw new Error("Git action belongs to another repository");
  const epoch=this.epoch,current=()=>!this.disposed&&(workspaceRootOverride!==undefined||this.epoch===epoch);
  const run=async(request:GitClientMessage)=>{
   if(!current())return;
   const reply=await this.read(request,scope);
   if(current()&&scope===this.workspaceRoot)this.feed(reply);else if(current())for(const listener of this.replies)listener(reply,scope);
   if(isRecord(reply)&&isRecord(reply.Error))throw new Error(String(reply.Error.message));
   if(isRecord(reply)&&isRecord(reply.PullRequest)&&isRecord(reply.PullRequest.reply)&&isRecord(reply.PullRequest.reply.Error))throw new Error(String(reply.PullRequest.reply.Error.message));
   if(isRecord(reply)&&isRecord(reply.ChangedFiles)&&reply.ChangedFiles.error)throw new Error(String(reply.ChangedFiles.error));
   return reply;
  };
  switch(action.kind){
   case"stage":if(action.path){if(!action.unstage)await this.saveCanonical(action.root,action.path);if(!current())return false;await run(action.unstage?{Unstage:{path:action.path}}:{Stage:{path:action.path}});}break;
   case"stage-all":{
    const snapshot=await this.read("ChangedFiles",scope);
    if(!isRecord(snapshot)||!isRecord(snapshot.ChangedFiles)||!Array.isArray(snapshot.ChangedFiles.files))throw new Error("Invalid Git changed-file snapshot");
    const files=snapshot.ChangedFiles.files.filter(isRecord).filter(file=>!file.staged||file.status==="Mixed");
    for(const file of files){const path=String(file.path);await this.saveCanonical(action.root,path);await run({Stage:{path}});}break;
   }
   case"commit":if(this.workspaceRoot!==scope||!this.frame.visible)this.open(scope);this.call("focus-commit");break;
   case"restore":case"block":{
    if(!action.path)return false;
    if(action.kind==="restore"||action.restore){if(!await this.hooks.confirmRestore?.({root:action.root,path:action.path,block:action.block})||!current())return false;}
    let reply:unknown;
    if(action.kind==="restore")reply=await run({Restore:{path:action.path}});
    else{if(!action.restore)await this.saveCanonical(action.root,action.path);reply=await run({ApplyBlock:{path:action.path,block:action.block,restore:!!action.restore}} as GitClientMessage);}
    if((action.kind==="restore"||action.restore)&&reply!==undefined&&current())await this.options.model.refreshGitSource(action.root,action.path);
    break;
   }
   case"pull-request":await run({PullRequest:{request:action.request}} as GitClientMessage);break;
   case"open-source":if(action.path)await this.hooks.openSource?.({root:action.root,path:action.path,line:action.line||0,column:action.column||0});break;
   case"open-url":if(action.url)await this.hooks.openUrl?.(action.url);break;
   default:return false;
  }return true;
 }

 dispose(){if(this.disposed)return;this.unwatch();this.disposed=true;this.epoch++;this.queue=[];this.reads.clear();this.animation.dispose();this.cleanupConnection();this.listeners.clear();this.replies.clear();coreCall("gitpanel.dispose",{id:this.id});}
}
