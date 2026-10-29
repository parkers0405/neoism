import {expect,test} from "bun:test";
import {Effect} from "effect";
import {AppVisualState} from "../src/ui/AppVisualState";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {coreCall,NativeEditor} from "../src/native-core";

test("one Git surface keeps independent native split-leaf viewport, folds and lifetime across zoom",async()=>{
 const config=loadConfig(["--config","/nonexistent/git-leaf-views"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
 const workspace={id:"a",host_id:"host",title:"A",root_dir:"/fixture"};model.update({activeWorkspace:workspace,workspaces:[workspace]});
 const path="src/main.ts",surface=model.openGitReview({mode:"working",root:"/fixture",path}),layout=model.getLayout();layout.split("horizontal",surface);layout.layout(1200,600);
 const leaves=layout.frame.leaves.filter(leaf=>leaf.surfaceId===surface.id);expect(leaves).toHaveLength(2);
 const first=visuals.gitReview(surface,model,config,()=>{},leaves[0].leaf),second=visuals.gitReview(surface,model,config,()=>{},leaves[1].leaf);
 const text=Array.from({length:200},(_,i)=>`const value${i} = ${i};`).join("\n")+"\n",editor=new NativeEditor("/fixture/"+path,text,true);
 try{
  expect(first).not.toBe(second);expect(first.id).not.toBe(second.id);
  const patch=`diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1,200 +1,200 @@\n`+text.trimEnd().split("\n").map((line,i)=>i===0?`-${line}\n+const value0 = 42;`:` ${line}`).join("\n")+"\n";
  for(const review of[first,second]){review.call("reply",{message:{ChangedFiles:{files:[{path,status:"Modified",additions:1,deletions:1,staged:false}],branch:"fixture",upstream:null,error:null}}});review.call("reply",{message:{FileDiffs:{diffs:[{path,patch}]}}});}
  first.layout(350,220);second.layout(900,400);expect(first.frame.effectiveSplit).toBe(false);expect(second.frame.effectiveSplit).toBe(true);
  first.call("scroll",{deltaY:180,deltaX:0,x:100,y:150});const limit=performance.now()+2000;
  while(first.frame.scrollY<10&&performance.now()<limit){await Bun.sleep(8);first.call("frame");}
  expect(first.frame.scrollY).toBeGreaterThan(10);expect(second.frame.scrollY).toBe(0);
  const before=first.frame.scrollY;
  expect(visuals.gitReview(surface,model,{...config,fontSize:15,scale:15/14},()=>{},leaves[0].leaf)).toBe(first);
  expect(first.frame.scrollY).toBeGreaterThanOrEqual(before);expect(second.frame.scrollY).toBe(0);
  // Both review projections attach the same authoritative CodeBuffer, while
  // the review's viewport/folding policy belongs to its own native host.
  client.request=((_family:string,message:unknown)=>Effect.succeed({BaseFile:{path,text}}))as typeof client.request;
  for(const review of[first,second]){
   review.options.loadEditor=async()=>editor;review.call("reveal",{path});
   const hit=review.frame.editHits[0];expect(hit).toBeDefined();review.pointer("down",hit.rect[0]+2,hit.rect[1]+hit.rect[3]/2);
   for(let attempt=0;attempt<20&&!review.frame.cursorToken;attempt++)await Promise.resolve();
   expect(review.frame.editingPath).toBe("/fixture/"+path);
   expect(review.frame.cursorToken).toBeDefined();
  }
  const sourceBefore=editor.text();first.key({key:"i",char:"i"});first.key({key:"/",char:"/"});first.key({key:"escape"});expect(editor.text()).not.toBe(sourceBefore);expect(editor.text().length).toBe(sourceBefore.length+1);
  expect(second.call("frame").dirty).toBe(true);
  first.call("reveal",{path});const header=first.frame.headers[0];expect(header).toBeDefined();first.pointer("down",header.rect[0]+header.rect[2]/2,header.rect[1]+header.rect[3]/2);
  expect(first.frame.files[0].collapsed).toBe(true);expect(second.frame.files[0].collapsed).toBe(false);
  layout.close(leaves[0].leaf);visuals.pruneGitReviews(model.state.surfaces);
  expect(()=>coreCall("gitreview.frame",{id:first.id})).toThrow();expect(()=>second.call("frame")).not.toThrow();
  expect(visuals.gitReview(surface,model,config,()=>{},leaves[1].leaf)).toBe(second);
 }finally{visuals.dispose();editor.dispose();model.dispose();}
});
