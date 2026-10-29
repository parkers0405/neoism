import type {ServerSelection} from "../menu-actions/ChromeActionsController";
import type {SshAttachment} from "./ssh-transport";
import type {WorkspaceModel} from "../model";
/** Rebuild only the selected SSH tunnel. The same recipe and stable recovery
 * identity resume daemon-owned sessions; failure never selects Local Server. */
export class SelectedSshReconnect{
 private selected?:{server:ServerSelection;attachment:Pick<SshAttachment,"alive">};
 private pending?:{abort:AbortController;generation:number};
 private generation=0;private paused=0;private attempts=0;private retryAt=0;private reportedAt?:number;private disposed=false;
 private timer?:ReturnType<typeof setInterval>;
 constructor(private readonly reconnect:(server:ServerSelection)=>Promise<unknown>,private readonly report:(error:unknown)=>void,private readonly options:{intervalMs?:number;now?:()=>number;automatic?:boolean}={}){
  if(options.automatic!==false){this.timer=setInterval(()=>{void this.check();},options.intervalMs??1000);this.timer.unref();}
 }
 select(server?:ServerSelection,attachment?:Pick<SshAttachment,"alive">){
  this.generation++;this.pending?.abort.abort(new Error("SSH selection changed"));this.pending=undefined;
  this.selected=server?.ssh&&attachment?{server:{...server,signal:undefined,ssh:{...server.ssh,args:server.ssh.args?[...server.ssh.args]:undefined}},attachment}:undefined;
  this.attempts=0;this.retryAt=0;this.reportedAt=undefined;
 }
 /** Manual selection gets priority even while a reconnect is awaiting SSH. */
 pause(){
  this.paused++;this.pending?.abort.abort(new Error("SSH reconnect superseded"));let released=false;
  return()=>{if(!released){released=true;this.paused=Math.max(0,this.paused-1);}};
 }
 async check(){
  const now=(this.options.now||Date.now)(),selected=this.selected;
  if(this.disposed||this.paused||this.pending||!selected||now<this.retryAt)return;
  try{if(selected.attachment.alive())return;}catch{}
  const pending={abort:new AbortController(),generation:this.generation};this.pending=pending;
  try{await this.reconnect({...selected.server,signal:pending.abort.signal});}
  catch(error){
   if(pending.abort.signal.aborted||pending.generation!==this.generation)return;
   const failedAt=(this.options.now||Date.now)();this.attempts++;this.retryAt=failedAt+Math.min(15000,500*2**Math.min(5,this.attempts-1));
   if(this.reportedAt===undefined||failedAt-this.reportedAt>=10000){this.reportedAt=failedAt;this.report(error);}
  }finally{if(this.pending===pending)this.pending=undefined;}
 }
 dispose(){this.disposed=true;clearInterval(this.timer);this.select();}
}
/** Cold startup opens Home; a tunnel reconnect instead returns to the exact
 * pre-disconnect workspace/tab once its preserved model finishes bootstrapping. */
export function preserveReconnectFocus(model:WorkspaceModel,selection:{workspaceId?:string;surfaceId?:string}){
 let finished=false;let unsubscribe=()=>{};
 const cleanup=()=>{if(finished)return;finished=true;clearTimeout(timer);unsubscribe();};
 const check=()=>{
  const state=model.state;if(state.loading||state.activeWorkspace?.id!==selection.workspaceId)return;
  const surface=state.surfaces.find(surface=>surface.id===selection.surfaceId&&surface.workspaceId===selection.workspaceId);if(!surface)return;
  cleanup();
  // Don't override a user's explicit selection made during bootstrap.
  if(!state.activeSurfaceId||state.activeSurfaceId===`welcome:${selection.workspaceId}`||state.activeSurfaceId===surface.id)model.selectSurface(surface.id);
 };
 const timer=setTimeout(cleanup,15000);timer.unref();unsubscribe=model.subscribe(check);queueMicrotask(check);return cleanup;
}
