import {Worker} from "node:worker_threads";
import type {SavedSession} from "./session-store";
import {profileStart,profileEnd} from "./profiling";

// Inline JavaScript keeps the worker available in the compiled desktop bundle.
// JSON parsing/encoding belongs here too: asynchronous filesystem calls alone
// still leave large recovery snapshots blocking the UI every two seconds.
const source=String.raw`
const {parentPort}=require('node:worker_threads');
const {readFile,mkdir,writeFile,chmod,rename}=require('node:fs/promises');
const {dirname}=require('node:path');
const record=v=>typeof v==='object'&&v!==null&&!Array.isArray(v);
const editorsByWriter=new Map();
parentPort.on('message',async({id,writer,release,editorKeys,path,identity,profiles,session})=>{
 if(release!==undefined){editorsByWriter.delete(release);return;}
 try{
  const previousEditors=editorsByWriter.get(writer)||{};
  const editors=Object.fromEntries(editorKeys.map(key=>[key,Object.hasOwn(session.editors,key)?session.editors[key]:previousEditors[key]]));
  editorsByWriter.set(writer,editors);session={...session,editors};
  const savedProfiles={};
  if(profiles){
   let previous={};
   try{const value=JSON.parse(await readFile(path,'utf8'));if(record(value))previous=value;}catch{}
   if(record(previous.profiles))for(const [key,value]of Object.entries(previous.profiles)){
    const saved=decodeSession(value);if(/^[0-9a-f]{24}$/.test(key)&&saved)savedProfiles[key]=saved;
   }
   const old=decodeSession(previous);
   if(old&&typeof previous.identity==='string'&&/^[0-9a-f]{24}$/.test(previous.identity))savedProfiles[previous.identity]=old;
   savedProfiles[identity]=session;
  }
  const raw=JSON.stringify({...session,identity,...(profiles?{profiles:savedProfiles}:{})});
  await mkdir(dirname(path),{recursive:true,mode:0o700});
  const pending=path+'.'+process.pid+'.next';
  await writeFile(pending,raw,{mode:0o600});await chmod(pending,0o600);await rename(pending,path);
  parentPort.postMessage({id});
 }catch(error){parentPort.postMessage({id,error:error instanceof Error?error.message:String(error)});}
});
`;
let worker:Worker|undefined,serial=0;
const pending=new Map<number,{resolve:()=>void;reject:(error:Error)=>void}>();
function getWorker(decode:(value:unknown)=>SavedSession|undefined){
 if(worker)return worker;
 const current=new Worker(`const decodeSession=${decode.toString()};\n${source}`,{eval:true});worker=current;
 const fail=(error:Error)=>{if(worker!==current)return;worker=undefined;for(const task of pending.values())task.reject(error);pending.clear();void current.terminate();};
 current.on('message',(message:{id:number;error?:string})=>{const task=pending.get(message.id);if(!task)return;pending.delete(message.id);if(message.error!==undefined)task.reject(new Error(message.error));else task.resolve();if(!pending.size)current.unref();});
 current.on('error',fail);current.on('exit',code=>{if(worker===current)fail(new Error(`Recovery worker exited (${code})`));});
 current.unref();return current;
}
let writerSerial=0;
/** Callers serialize transactions by path, including stores for other hosts. */
export class RecoveryWriter {
 private readonly id=++writerSerial;
 private previousEditors:Record<string,unknown>={};
 private previousWorker?:Worker;
 async write(request:{path:string;identity:string;profiles:boolean;session:SavedSession},decode:(value:unknown)=>SavedSession|undefined,reuseEditors=false):Promise<void>{
  const current=getWorker(decode),editors=request.session.editors;
  // WorkspaceModel's editor snapshots are immutable and retained between edits.
  // Forward only changed snapshots; mutable external callers opt out by default.
  const changed=reuseEditors&&this.previousWorker===current?Object.fromEntries(Object.entries(editors).filter(([key,value])=>!Object.hasOwn(this.previousEditors,key)||this.previousEditors[key]!==value)):editors;
  await new Promise<void>((resolve,reject)=>{
   const id=++serial;pending.set(id,{resolve,reject});current.ref();const start=profileStart();
   try{current.postMessage({id,writer:this.id,...request,session:{...request.session,editors:changed},editorKeys:Object.keys(editors)});}
   catch(error){pending.delete(id);if(!pending.size)current.unref();reject(error);}
   finally{profileEnd('recovery.transfer',start);}
  });
  this.previousEditors={...editors};this.previousWorker=current;
 }
 dispose(){if(worker&&this.previousWorker===worker)worker.postMessage({release:this.id});this.previousEditors={};this.previousWorker=undefined;}
}
