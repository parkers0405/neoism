import {expect,test} from "bun:test";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {decodeSession,type SavedSession} from "../src/session-store";
import {coreCall} from "../src/native-core";
import type {GitReviewFrame} from "../src/git/GitReviewController";
import {createAgentDaemon} from "./support/agent-daemon";
const repo="/project/a",path="src/main.ts",source="const value = 2;\nconsole.log(value);\n",patch="diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1,2 +1,2 @@\n-const value = 1;\n+const value = 2;\n console.log(value);\n";
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Git model fixture did not converge");await Bun.sleep(2);}};
async function fixture(recovery?:SavedSession,options:Parameters<typeof createAgentDaemon>[0]={}){
 const daemon=createAgentDaemon(options);daemon.threads.splice(0);daemon.files.set(path,source);
 const config={...loadConfig(["--config","/nonexistent/git-source-model"],{}),endpoint:daemon.endpoint,authToken:undefined,vim:true};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),wire:Array<{family:string;message:unknown;root?:string}>=[],errors:unknown[]=[];
 model.report=error=>{errors.push(error);};const send=client.send.bind(client);client.send=((family:any,message:any,root?:string)=>{wire.push({family,message,root});return send(family,message,root);}) as typeof client.send;
 if(recovery)model.restoreSession(recovery);model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
 return{daemon,model,wire,errors,close(){model.dispose();client.stop();daemon.stop();}};
}
test("working Git review is one tab per repository; reveals reuse its leaf and do not create source tabs",async()=>{
 const f=await fixture();try{
  const first=f.model.openGitReview({mode:"working",root:repo,path}),leaf=f.model.getLayout().frame.focusedLeaf;
  const second=f.model.openGitReview({mode:"working",root:repo,path:"other.ts"});
  expect(second.id).toBe(first.id);expect(second.gitReveal).toBe(first.gitReveal!+1);expect(second.git?.path).toBe("other.ts");expect(f.model.getLayout().frame.focusedLeaf).toBe(leaf);expect(f.model.state.surfaces.filter(surface=>surface.kind==="git")).toHaveLength(1);
  const third=f.model.openGitReview({mode:"working",root:"/other/repository",path});expect(third.id).not.toBe(first.id);expect(f.model.state.surfaces.filter(surface=>surface.kind==="git")).toHaveLength(2);expect(f.daemon.ptyRequests).toHaveLength(0);
 }finally{f.close();}
});
test("Git source loading reuses an existing dirty canonical file and keeps the review focused",async()=>{
 const f=await fixture();try{
  await f.model.openFile(path);const file=f.model.state.surfaces.find(surface=>surface.id===f.model.state.activeSurfaceId)!,fileLeaf=f.model.getLayout().frame.focusedLeaf!,editor=f.model.getEditor(file,fileLeaf),canonical=f.model.getEditor(file);editor.call("paste",{text:"dirty "});const text=editor.text(),reads=f.daemon.fileRequests.filter(request=>request.tag==="ReadFile").length;
  const review=f.model.openGitReview({mode:"working",root:repo,path}),leaf=f.model.getLayout().frame.focusedLeaf;
  expect(await f.model.loadGitEditor(repo,path)).toBe(canonical);expect(canonical).not.toBe(editor);expect(canonical.text()).toBe(text);expect(editor.text()).toBe(text);expect(f.model.state.activeSurfaceId).toBe(review.id);expect(f.model.getLayout().frame.focusedLeaf).toBe(leaf);expect(f.model.state.surfaces.filter(surface=>surface.kind==="file")).toHaveLength(1);
  expect(f.daemon.fileRequests.filter(request=>request.tag==="ReadFile")).toHaveLength(reads);expect(f.model.isSurfaceModified(review.id)).toBe(true);
  expect(f.model.closeCurrentTab(false)).toBe(false);expect(f.model.state.activeSurfaceId).toBe(review.id);
  expect(f.model.closeCurrentTab(true)).toBe(true);expect(f.model.state.surfaces).toContainEqual(file);expect(editor.text()).toBe(text);expect(editor.frame.dirty).toBe(true);
 }finally{f.close();}
});
test("hidden source ownership never adds a file tab, and explicit file open promotes that same editor",async()=>{
 const f=await fixture();try{
  const review=f.model.openGitReview({mode:"working",root:repo,path}),editor=await f.model.loadGitEditor(repo,path),owner=f.model.state.surfaces.find(surface=>surface.kind==="file")!;
  editor.call("paste",{text:"inline "});const text=editor.text();expect(owner.hidden).toBe(true);expect(f.model.getLayout().frame.leaves.some(leaf=>leaf.surfaceId===owner.id)).toBe(false);expect(f.model.state.activeSurfaceId).toBe(review.id);
  await f.model.openFile(path);expect(f.model.state.activeSurfaceId).toBe(owner.id);expect(f.model.state.surfaces.find(surface=>surface.id===owner.id)?.hidden).toBe(false);expect(f.model.state.surfaces.filter(surface=>surface.kind==="file")).toHaveLength(1);expect(f.model.getEditor(owner)).toBe(editor);expect(editor.text()).toBe(text);
  f.model.selectSurface(review.id);expect(f.model.closeCurrentTab(true)).toBe(true);expect(f.model.getEditor(owner)).toBe(editor);expect(editor.frame.dirty).toBe(true);
 }finally{f.close();}
});
test("actual native inline edit uses the model's canonical source and survives recovery/save to its scoped host root",async()=>{
 const f=await fixture();let id:number|undefined;try{
  const review=f.model.openGitReview({mode:"working",root:repo,path}),editor=await f.model.loadGitEditor(repo,path),owner=f.model.state.surfaces.find(surface=>surface.kind==="file")!;
  id=coreCall<{id:number}>("gitreview.new",{mode:"working",root:repo,path,fontFamily:f.model.config.fontFamily,fontWeight:400,fontSize:14,theme:f.model.config.themeName}).id;
  const call=(op:string,args:Record<string,unknown>={})=>coreCall<GitReviewFrame>(`gitreview.${op}`,{id,...args});
  call("reply",{message:{ChangedFiles:{files:[{path,status:"Modified",additions:1,deletions:1,staged:false}],branch:"fixture",upstream:null,error:null}}});
  let frame=call("reply",{message:{FileDiffs:{diffs:[{path,patch}]}}});const hit=frame.editHits[0]!;frame=call("pointer",{kind:"down",x:hit.rect[0]+hit.rect[2]/2,y:hit.rect[1]+hit.rect[3]/2});
  expect(frame.edit).toMatchObject({root:repo,path});call("attach",{editorID:editor.id,line:0,column:0});call("key",{key:"i",char:"i"});call("key",{key:"/",char:"/"});call("key",{key:"escape"});editor.call("frame");
  const bytes=editor.text();expect(bytes).toStartWith("/const");expect(f.model.isSurfaceModified(review.id)).toBe(true);expect(f.model.closeCurrentTab(false)).toBe(false);
  const saved=decodeSession(JSON.parse(JSON.stringify(f.model.captureSession())))!;expect(saved.surfaces.find(surface=>surface.id===owner.id)?.hidden).toBe(true);expect(saved.surfaces.find(surface=>surface.id===review.id)?.gitEditorIds).toContain(owner.id);
  const restored=await fixture(saved);try{
   restored.model.selectSurface(review.id);const before=restored.daemon.fileRequests.filter(request=>request.tag==="ReadFile").length,again=await restored.model.loadGitEditor(repo,path);
   expect(again.text()).toBe(bytes);expect(again.frame.dirty).toBe(true);expect(restored.model.isSurfaceModified(review.id)).toBe(true);expect(restored.model.getLayout().frame.leaves.some(leaf=>leaf.surfaceId===owner.id)).toBe(false);expect(restored.daemon.fileRequests.filter(request=>request.tag==="ReadFile")).toHaveLength(before);
   await restored.model.saveGitEditor(again);expect(again.frame.dirty).toBe(false);expect(restored.model.isSurfaceModified(review.id)).toBe(false);expect(restored.daemon.files.get(path)).toBe(bytes);
   const write=restored.wire.find(entry=>entry.family==="Files"&&typeof entry.message==="object"&&entry.message!==null&&"WriteFile"in entry.message)!;expect(write.root).toBe(repo);expect((write.message as any).WriteFile.path).toBe(path);expect(restored.daemon.ptyRequests).toHaveLength(0);
  }finally{restored.close();}
 }finally{if(id!==undefined)coreCall("gitreview.dispose",{id});f.close();}
});
test("closing one split review or another owner never discards a shared dirty source",async()=>{
 const f=await fixture();try{
  const review=f.model.openGitReview({mode:"working",root:repo,path}),editor=await f.model.loadGitEditor(repo,path),owner=f.model.state.surfaces.find(surface=>surface.kind==="file")!;editor.call("paste",{text:"shared "});const bytes=editor.text();
  const layout=f.model.getLayout(),first=layout.frame.focusedLeaf!;layout.split("horizontal",review);expect(layout.frame.leaves.filter(leaf=>leaf.surfaceId===review.id)).toHaveLength(2);
  expect(f.model.closePane(review.id,first,true)).toBe(true);expect(f.model.state.surfaces.some(surface=>surface.id===owner.id)).toBe(true);expect(editor.text()).toBe(bytes);expect(f.model.isSurfaceModified(review.id)).toBe(true);
  const other=f.model.openGitReview({mode:"commit",root:repo,revision:"abc123"});expect(await f.model.loadGitEditor(repo,path)).toBe(editor);
  f.model.selectSurface(review.id);expect(f.model.closeCurrentTab(true)).toBe(true);expect(f.model.state.surfaces.some(surface=>surface.id===owner.id)).toBe(true);expect(f.model.state.surfaces.some(surface=>surface.id===other.id)).toBe(true);expect(editor.text()).toBe(bytes);
  f.model.selectSurface(other.id);expect(f.model.closeCurrentTab(true)).toBe(true);expect(f.model.state.surfaces.some(surface=>surface.id===owner.id)).toBe(false);
 }finally{f.close();}
});
test("closing a final clean review cannot select an invisible orphan source instead of Welcome",async()=>{
 const f=await fixture();try{
  const review=f.model.openGitReview({mode:"working",root:repo,path});await f.model.loadGitEditor(repo,path);
  f.model.closeSurface("welcome:a");f.model.selectSurface(review.id);expect(f.model.closeCurrentTab(false)).toBe(true);
  expect(f.model.getLayout().frame.leaves.length).toBeGreaterThan(0);expect(f.model.state.surfaces.find(surface=>surface.id===f.model.state.activeSurfaceId)?.hidden).not.toBe(true);expect(f.model.state.surfaces.find(surface=>surface.id===f.model.state.activeSurfaceId)?.kind).toBe("welcome");
 }finally{f.close();}
});
test("late source reads retain their captured review/workspace without stealing a newly selected workspace",async()=>{
 const f=await fixture(undefined,{fileReadDelayMs:30});try{
  const review=f.model.openGitReview({mode:"working",root:repo,path}),pending=f.model.loadGitEditor(repo,path);
  await until(()=>f.daemon.fileRequests.some(request=>request.tag==="ReadFile"));await f.model.activateWorkspace(f.model.state.workspaces.find(workspace=>workspace.id==="b")!);
  const selected=f.model.state.activeSurfaceId,editor=await pending,owner=f.model.state.surfaces.find(surface=>surface.kind==="file"&&surface.workspaceId==="a")!;
  expect(f.model.state.activeWorkspace?.id).toBe("b");expect(f.model.state.activeSurfaceId).toBe(selected);expect(f.model.state.surfaces.find(surface=>surface.id===review.id)?.gitEditorIds).toContain(owner.id);expect(editor.text()).toBe(source);expect(owner.hidden).toBe(true);
  expect(f.model.getLayout("b").frame.leaves.some(leaf=>leaf.surfaceId===owner.id)).toBe(false);expect(f.daemon.ptyRequests).toHaveLength(0);
 }finally{f.close();}
});

test("Git refresh of a hidden recovered source cannot revive its old snapshot on first open", async () => {
 const root = "/project/a", first = await fixture();
 let saved: SavedSession, id: string;
 try {
  const editor = await first.model.loadGitEditor(root, path);
  id = first.model.state.surfaces.find(surface => surface.kind === "file")!.id;
  expect(editor.frame.dirty).toBe(false);
  saved = first.model.captureSession();
 } finally { first.close(); }
 const restored = await fixture(saved!);
 try {
  expect(restored.model.editors.has(id!)).toBe(false);
  const changed = "const restored = true;\n";
  restored.daemon.files.set(path, changed);
  await restored.model.refreshGitSource(root, path);
  expect(restored.model.editors.has(id!)).toBe(false);
  const surface = restored.model.state.surfaces.find(surface => surface.id === id!)!;
  expect(restored.model.getEditor(surface).text()).toBe(changed);
  expect(restored.model.isSurfaceModified(id!)).toBe(false);
 } finally { restored.close(); }
});
