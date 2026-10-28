import {expect,test} from "bun:test";
import {GitController} from "../src/git/GitController";
import {WorkspaceModel,type Surface} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {createGitDaemon} from "./support/git-daemon";
const until=async(predicate:()=>boolean)=>{const limit=Date.now()+6000;while(!predicate()){if(Date.now()>limit)throw new Error("Git fixture did not converge");await Bun.sleep(5);}};
async function fixture(){const daemon=createGitDaemon(),config={...loadConfig(["--config","/nonexistent/git-controller"],{}),endpoint:daemon.endpoint,vim:false},client=new DaemonConnection(config),model=new WorkspaceModel(client,config),errors:unknown[]=[];model.report=error=>{errors.push(error);};
 model.update({activeWorkspace:{id:"a",host_id:"host",title:"Fixture",root_dir:"/fixture"},workspaces:[{id:"a",host_id:"host",title:"Fixture",root_dir:"/fixture"}]});
 client.start();await until(()=>client.state.phase==="connected");const controller=new GitController({model,config});controller.open("/fixture");await until(()=>controller.frame.files.length===2&&!controller.frame.loading);return{daemon,client,model,controller,errors,close(){controller.dispose();model.dispose();client.stop();daemon.close();}};}
const messages=(context:Awaited<ReturnType<typeof fixture>>,kind:string)=>context.daemon.requests.filter(request=>request.family==="Git"&&(typeof request.message==="string"?request.message===kind:Object.hasOwn(request.message as object,kind)));
test("original native Git panel reads real daemon status, raw patches, latest commit and source hit metadata",async()=>{
 const context=await fixture();try{const{controller}=context;controller.layout(1200,700);await until(()=>messages(context,"DiffFiles").length>0&&controller.frame.commands.some(command=>command.kind==="text"&&command.text.includes("main.ts")));
  expect(controller.frame.branch).toBe("feature/fixture");expect(controller.frame.repoRoot).toBe("/fixture");expect(controller.frame.files[0]).toMatchObject({path:"src/main.ts",staged:false,additions:1,deletions:1});
  expect(controller.frame.hits.some(hit=>hit.id==="resize")).toBe(true);expect(controller.frame.hits.some(hit=>hit.id==="commit-input")).toBe(true);expect(messages(context,"Log").length).toBeGreaterThan(0);expect(context.errors).toEqual([]);
 }finally{context.close();}
});
test("staging saves the actual canonical dirty document before the root-scoped daemon mutation",async()=>{
 const context=await fixture();try{const surface:Surface={id:"file:git-save",workspaceId:"a",kind:"file",title:"main.ts",path:"src/main.ts",rootDir:"/fixture"};context.model.update({surfaces:[surface],activeSurfaceId:surface.id,files:{[surface.id]:"const value = 2;\nconsole.log(value);\n"}});const editor=context.model.getEditor(surface);editor.call("paste",{text:"// preserve unsaved source\n"});expect(editor.frame.dirty).toBe(true);
  await context.controller.execute({kind:"stage",root:"/fixture",path:"src/main.ts"});expect(editor.frame.dirty).toBe(false);expect(context.controller.frame.files[0].staged).toBe(true);
  const write=context.daemon.requests.findIndex(request=>request.family==="Files"&&Object.hasOwn(request.message as object,"WriteFile")),stage=context.daemon.requests.findIndex(request=>request.family==="Git"&&Object.hasOwn(request.message as object,"Stage"));expect(write).toBeGreaterThanOrEqual(0);expect(stage).toBeGreaterThan(write);expect(context.daemon.requests[stage].workspaceRoot).toBe("/fixture");
  await context.controller.execute({kind:"stage",root:"/fixture",path:"src/main.ts",unstage:true});expect(context.controller.frame.files[0].staged).toBe(false);expect(context.errors).toEqual([]);
 }finally{context.close();}
});
test("native commit keeps failed drafts and clears only the message explicitly confirmed by the daemon",async()=>{
 const context=await fixture();try{const{controller}=context;await controller.execute({kind:"stage",root:"/fixture",path:"src/main.ts"});controller.call("focus-commit");controller.call("commit-text",{text:"fixture rejected commit"});context.daemon.setCommitError("Fixture rejected commit");controller.call("key",{key:"enter"});await until(()=>controller.frame.error==="Fixture rejected commit");expect(controller.frame.commitText).toBe("fixture rejected commit");
  context.daemon.setCommitError(undefined);controller.call("commit-text",{text:"fixture accepted commit"});controller.call("key",{key:"enter"});await until(()=>controller.frame.commitText==="");expect(messages(context,"Commit")).toHaveLength(2);expect(controller.frame.error).toBeNull();
 }finally{context.close();}
});
test("review mutations retain explicit workspace scope and cancelled restore never sends a write",async()=>{
 const context=await fixture();try{const scopes:string[]=[];context.controller.subscribeReplies((_reply,root)=>{if(root)scopes.push(root);});await context.controller.execute({kind:"stage",root:"/other/repo",path:"src/main.ts"},"/other/workspace");expect(messages(context,"Stage").at(-1)?.workspaceRoot).toBe("/other/workspace");expect(scopes).toContain("/other/workspace");expect(context.controller.frame.repoRoot).toBe("/fixture");
  context.controller.setHooks({confirmRestore:async()=>false});expect(await context.controller.execute({kind:"restore",root:"/fixture",path:"src/main.ts"})).toBe(false);expect(messages(context,"Restore")).toHaveLength(0);
  let answer!:(value:boolean)=>void;context.controller.setHooks({confirmRestore:()=>new Promise(resolve=>{answer=resolve;})});const pending=context.controller.execute({kind:"restore",root:"/fixture",path:"src/main.ts"});context.controller.open("/second");answer(true);expect(await pending).toBe(false);expect(messages(context,"Restore")).toHaveLength(0);
 }finally{context.close();}
});
test("branch lookup, publish and original action menu perform real isolated daemon RPCs",async()=>{
 const context=await fixture();try{const{controller}=context;controller.layout(1200,700);const click=(id:string)=>{const hit=controller.frame.hits.find(hit=>hit.id===id)!;if(!hit)throw new Error(`Missing original hit ${id}`);controller.call("pointer",{kind:"down",x:hit.rect[0]+hit.rect[2]/2,y:hit.rect[1]+hit.rect[3]/2});};
  click("branch");await until(()=>controller.frame.hits.some(hit=>hit.id.startsWith("branch:")));controller.call("key",{key:"o",char:"o"});controller.call("key",{key:"enter"});await until(()=>controller.frame.branch==="other");expect(messages(context,"Checkout")).toHaveLength(1);
  click("push");await until(()=>controller.frame.upstream==="origin/other");expect(messages(context,"Publish")).toHaveLength(1);
  for(const label of["Fetch","Pull","Push"]){controller.call("key",{key:"f4"});const hit=controller.frame.hits.find(hit=>hit.id.startsWith("action:")&&controller.frame.commands.some(command=>command.kind==="text"&&command.text.startsWith(label)&&command.x>=hit.rect[0]&&command.x<=hit.rect[0]+hit.rect[2]&&command.y>=hit.rect[1]&&command.y<=hit.rect[1]+hit.rect[3]));expect(hit).toBeDefined();controller.call("pointer",{kind:"down",x:hit!.rect[0]+10,y:hit!.rect[1]+hit!.rect[3]/2});await until(()=>messages(context,label).length>0&&!controller.frame.loading);}
 }finally{context.close();}
});

test("hosted PR read/write operations preserve complete typed payloads, scoped replies and stale-head errors",async()=>{
 const daemon=createGitDaemon({pullRequests:true}),config={...loadConfig(["--config","/nonexistent/git-pr"],{}),endpoint:daemon.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);client.start();await until(()=>client.state.phase==="connected");const controller=new GitController({model,config});const replies:unknown[]=[];controller.subscribeReplies(reply=>replies.push(reply));
 try{
  const root="/fixture",head="a".repeat(40),read=async(request:unknown)=>controller.read({PullRequest:{request}} as never,root);
  expect(await read({Discover:{branch:null}})).toMatchObject({PullRequest:{reply:{Discovered:{head,default_branch:"main"}}}});
  expect(await read({List:{query:"label:bug",sort:"Updated",filter:"Open",cursor:null,generation:42}})).toMatchObject({PullRequest:{reply:{Listed:{page:{generation:42,total_count:1}}}}});
  expect(await read({Read:{number:7}})).toMatchObject({PullRequest:{reply:{Detail:{detail:{summary:{number:7},body:"Original PR body"}}}}});
  expect(await read({ReadInbox:{number:7}})).toMatchObject({PullRequest:{reply:{InboxDetail:{item:{repository:"fixture/repo"}}}}});
  for(const request of[
   {Create:{title:"Exact fixture title",body:"Complete body\nsecond line",base:"main",draft:true,expected_branch:"feature/fixture",expected_head:head}},
   {SubmitReview:{number:8,expected_head:head,body:"Review\nbody",event:"RequestChanges",comments:[{path:"src/main.ts",side:"Right",line:1,start_line:null,body:"Keep exact comment"}]}},
   {ReplyThread:{number:8,thread_id:"thread-one",body:"Exact reply"}},{ResolveThread:{number:8,thread_id:"thread-one",resolved:true}},
   {Checkout:{number:8,expected_head:head,new_worktree:true}},{Merge:{number:8,expected_head:head,method:"Squash"}},
  ]){expect(await controller.execute({kind:"pull-request",root,request},root)).toBe(true);expect(daemon.requests.at(-1)?.message).toEqual({PullRequest:{request}});}
  expect(replies).toHaveLength(6);
  await expect(controller.execute({kind:"pull-request",root,request:{Merge:{number:8,expected_head:"stale",method:"Squash"}}},root)).rejects.toThrow("Fixture PR head changed");
  expect(await read({Read:{number:8}})).toMatchObject({PullRequest:{reply:{Detail:{detail:{summary:{state:"MERGED"},body:"Complete body\nsecond line"}}}}});
 }finally{controller.dispose();model.dispose();client.stop();daemon.close();}
});

test("partial staging and confirmed restore preserve the exact native change block",async()=>{
 const context=await fixture();try{const block={old_start:0,new_start:0,old_lines:["const value = 1;"],new_lines:["const value = 2;"]};const path="src/main.ts",root="/fixture";
  expect(await context.controller.execute({kind:"block",root,path,block,restore:false})).toBe(true);expect(messages(context,"ApplyBlock").at(-1)?.message).toEqual({ApplyBlock:{path,block,restore:false}});
  context.controller.setHooks({confirmRestore:async target=>{expect(target).toEqual({root,path,block});return true;}});
  expect(await context.controller.execute({kind:"block",root,path,block,restore:true})).toBe(true);expect(messages(context,"ApplyBlock").at(-1)?.message).toEqual({ApplyBlock:{path,block,restore:true}});
 }finally{context.close();}
});


test("a status update during a pending patch discards the stale reply and fetches the replacement",async()=>{
 const context=await fixture();try{
  await until(()=>messages(context,"WatchStatus").length>0&&messages(context,"DiffFiles").length>0&&!context.controller.frame.loading);await Bun.sleep(20);
  const initial=messages(context,"DiffFiles").length,held=context.daemon.holdNextDiff();context.daemon.push();await held.received;
  context.daemon.push();await Bun.sleep(20);held.release();
  await until(()=>messages(context,"DiffFiles").length>=initial+2&&!context.controller.frame.loading);
  expect(context.controller.frame.selected).toBe(0);expect(context.controller.frame.error).toBeNull();expect(context.errors).toEqual([]);
 }finally{context.close();}
});


for(const blockRestore of[false,true])test(`${blockRestore?"block":"file"} restore reloads its clean canonical owner only after a successful confirmed write`,async()=>{
 const context=await fixture();try{
  const root="/fixture",path="src/main.ts",before="const value = 2;\nconsole.log(value);\n",surface:Surface={id:"file:git-refresh",workspaceId:"a",kind:"file",title:"main.ts",path,rootDir:root,hidden:true};
  context.model.update({surfaces:[surface],files:{[surface.id]:before}});const editor=context.model.getEditor(surface);
  const action=blockRestore?{kind:"block",root,path,restore:true,block:{old_start:0,new_start:0,old_lines:["const value = 1;"],new_lines:["const value = 2;"]}}:{kind:"restore",root,path};
  const reads=()=>context.daemon.requests.filter(request=>request.family==="Files"&&Object.hasOwn(request.message as object,"ReadFile"));
  context.controller.setHooks({confirmRestore:async()=>false});expect(await context.controller.execute(action)).toBe(false);expect(reads()).toHaveLength(0);expect(editor.text()).toBe(before);
  context.controller.setHooks({confirmRestore:async()=>true});context.daemon.setRestoreError("Fixture refused restore");await expect(context.controller.execute(action)).rejects.toThrow("Fixture refused restore");expect(reads()).toHaveLength(0);expect(editor.text()).toBe(before);
  context.daemon.setRestoreError(undefined);expect(await context.controller.execute(action)).toBe(true);expect(reads()).toHaveLength(1);expect(editor.text()).toBe("const value = 1;\nconsole.log(value);\n");expect(editor.frame.dirty).toBe(false);expect(context.model.state.files[surface.id]).toBe(editor.text());
  const mutation=context.daemon.requests.findLastIndex(request=>request.family==="Git"&&Object.hasOwn(request.message as object,blockRestore?"ApplyBlock":"Restore")),read=context.daemon.requests.findLastIndex(request=>request.family==="Files"&&Object.hasOwn(request.message as object,"ReadFile"));expect(read).toBeGreaterThan(mutation);
 }finally{context.close();}
});
