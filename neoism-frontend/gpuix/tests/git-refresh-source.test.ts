import {expect,test} from "bun:test";
import {Effect} from "effect";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {createAgentDaemon} from "./support/agent-daemon";
const root="/project/a",path="src/source.ts",initial="const first = 1;\nconst second = 2;\n";
const until=async(predicate:()=>boolean)=>{const end=Date.now()+2000;while(!predicate()){if(Date.now()>end)throw new Error("Refresh fixture did not settle");await Bun.sleep(2);}};
async function fixture(){
 const daemon=createAgentDaemon();daemon.threads.splice(0);daemon.files.set(path,initial);const config={...loadConfig(["--config","/nonexistent/git-refresh"],{}),endpoint:daemon.endpoint,authToken:undefined,vim:true},client=new DaemonConnection(config),model=new WorkspaceModel(client,config),wire:Array<{family:string;root?:string;message:unknown}>=[];
 const send=client.send.bind(client);client.send=((family:any,message:any,root?:string)=>{wire.push({family,message,root});return send(family,message,root);}) as typeof client.send;model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 return{daemon,client,model,wire,close(){model.dispose();client.stop();daemon.stop();}};
}
function holdRead(client:DaemonConnection){
 const request=client.request.bind(client);let release!:()=>void,captured!:()=>void;const gate=new Promise<void>(resolve=>release=resolve),ready=new Promise<void>(resolve=>captured=resolve);
 client.request=((family:any,message:any,root?:string)=>{const effect=request(family,message,root);return family==="Files"&&message.ReadFile?Effect.flatMap(effect,reply=>Effect.promise(async()=>{captured();await gate;return reply;})):effect;}) as typeof client.request;
 return{ready,release,restore(){client.request=request;release();}};
}
test("confirmed restore refreshes existing clean owners and leaf views without stealing focus or replacing their identities",async()=>{
 const f=await fixture();try{
  await f.model.openFile(path);const owner=f.model.state.surfaces.find(surface=>surface.id===f.model.state.activeSurfaceId)!,leaf=f.model.getLayout().frame.focusedLeaf!,editor=f.model.getEditor(owner),view=f.model.getEditor(owner,leaf);
  view.call("paste",{text:"saved history\n"});await f.model.saveGitEditor(editor);const before=editor.text();view.call("goto",{line:1,col:3});const position={line:view.frame.cursor.line,col:view.frame.cursor.col};
  const review=f.model.openGitReview({mode:"working",root,path}),reviewLeaf=f.model.getLayout().frame.focusedLeaf,ids=[editor.id,view.id];f.daemon.files.set(path,"restored first\nrestored second\n");
  await f.model.refreshGitSource(root,path);expect(editor.text()).toBe("restored first\nrestored second\n");expect(view.text()).toBe(editor.text());expect(editor.frame.dirty).toBe(false);expect(view.frame.dirty).toBe(false);expect([editor.id,view.id]).toEqual(ids);expect({line:view.frame.cursor.line,col:view.frame.cursor.col}).toEqual(position);
  expect(f.model.state.files[owner.id]).toBe(editor.text());expect(f.model.state.activeSurfaceId).toBe(review.id);expect(f.model.getLayout().frame.focusedLeaf).toBe(reviewLeaf);expect(f.wire.filter(entry=>entry.family==="Files"&&typeof entry.message==="object"&&entry.message!==null&&"ReadFile"in entry.message).at(-1)?.root).toBe(root);
  editor.call("key",{key:"u",char:"u"});expect(editor.text()).toBe(before);expect(editor.frame.dirty).toBe(true);expect(view.text()).toBe(before);expect(f.daemon.ptyRequests).toHaveLength(0);
 }finally{f.close();}
});
test("clean source refresh preserves exact disk newline transitions",async()=>{
 const f=await fixture();try{
  f.model.openGitReview({mode:"working",root,path});const editor=await f.model.loadGitEditor(root,path);
  for(const text of["ends with newline\n","no final newline","CRLF first\r\nCRLF second\r\n","restored final newline\n",""]){f.daemon.files.set(path,text);await f.model.refreshGitSource(root,path);expect(editor.text()).toBe(text);expect(editor.frame.dirty).toBe(false);}
 }finally{f.close();}
});
test("dirty owners skip refresh IO entirely, and edits while a clean read is pending cannot be clobbered",async()=>{
 const f=await fixture();try{
  f.model.openGitReview({mode:"working",root,path});const editor=await f.model.loadGitEditor(root,path);editor.call("paste",{text:"dirty "});const bytes=editor.text(),reads=f.daemon.fileRequests.length;
  await f.model.refreshGitSource(root,path);expect(editor.text()).toBe(bytes);expect(f.daemon.fileRequests).toHaveLength(reads);
  await f.model.saveGitEditor(editor);f.daemon.files.set(path,"server restore\n");const held=holdRead(f.client);
  try{const refresh=f.model.refreshGitSource(root,path);await held.ready;editor.call("paste",{text:"late "});const local=editor.text();held.release();await refresh;expect(editor.text()).toBe(local);expect(editor.frame.dirty).toBe(true);}finally{held.restore();}
 }finally{f.close();}
});
test("a late restore read cannot overwrite a newer edit that was already saved again",async()=>{
 const f=await fixture();try{
  f.model.openGitReview({mode:"working",root,path});const editor=await f.model.loadGitEditor(root,path),owner=f.model.state.surfaces.find(surface=>surface.kind==="file")!;f.daemon.files.set(path,"old reply captured before editing\n");const held=holdRead(f.client);
  try{const refresh=f.model.refreshGitSource(root,path);await held.ready;editor.call("paste",{text:"new saved "});await f.model.saveGitEditor(editor);const local=editor.text(),revision=editor.frame.revision;held.release();await refresh;expect(editor.text()).toBe(local);expect(editor.frame.revision).toBe(revision);expect(editor.frame.dirty).toBe(false);expect(f.model.state.files[owner.id]).toBe(local);}finally{held.restore();}
 }finally{f.close();}
});
test("closing a hidden owner during refresh cannot resurrect a source or steal Welcome focus",async()=>{
 const f=await fixture();try{
  f.model.openGitReview({mode:"working",root,path});await f.model.loadGitEditor(root,path);const owner=f.model.state.surfaces.find(surface=>surface.kind==="file")!,held=holdRead(f.client);
  try{const refresh=f.model.refreshGitSource(root,path);await held.ready;expect(f.model.closeCurrentTab(true)).toBe(true);const selected=f.model.state.activeSurfaceId;held.release();await refresh;expect(f.model.state.surfaces.some(surface=>surface.id===owner.id)).toBe(false);expect(f.model.editors.has(owner.id)).toBe(false);expect(f.model.state.activeSurfaceId).toBe(selected);expect(f.model.state.surfaces.find(surface=>surface.id===selected)?.kind).toBe("welcome");}finally{held.restore();}
 }finally{f.close();}
});
