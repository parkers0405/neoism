import {expect,test} from "bun:test";
import {GitReviewController} from "../src/git/GitReviewController";
import {GitController} from "../src/git/GitController";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {createGitDaemon} from "./support/git-daemon";
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){if(Date.now()>end)throw Error("Review refresh did not converge");await Bun.sleep(5);}};
test("status bursts bound diff work and unchanged patches retain the painted review",async()=>{
 const daemon=createGitDaemon(),config={...loadConfig(["--config","/nonexistent/review-refresh"],{}),endpoint:daemon.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 client.start();await until(()=>client.state.phase==="connected");
 const git=new GitController({model,config}),review=new GitReviewController({model,config,git,descriptor:{mode:"working",root:"/fixture"}});
 const diffCount=()=>daemon.requests.filter(request=>typeof request.message==="object"&&request.message!==null&&"DiffFiles"in request.message).length;
 let held:ReturnType<typeof daemon.holdNextDiff>|undefined;
 try{
  await until(()=>review.frame.files.length===2&&review.frame.files.every(file=>file.loaded));await Bun.sleep(40);
  let applied=0;const call=review.call.bind(review);review.call=(op,args={})=>{if(op==="reply"&&(args.message as any)?.FileDiffs)applied++;return call(op,args);};
  held=daemon.holdNextDiff();daemon.push();await held.received;const first=diffCount();
  for(let i=0;i<30;i++)daemon.push();await Bun.sleep(40);
  expect(diffCount()).toBe(first);expect(review.frame.files.every(file=>file.loaded)).toBe(true);
  held.release();held=undefined;await until(()=>diffCount()===first+1);await Bun.sleep(40);
  expect(applied).toBe(0);
  // Same status metadata must still fetch changed bytes, then apply them once.
  daemon.setPatch("diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1 +1 @@\n-old\n+updated content\n");
  daemon.push();await until(()=>applied===1);
  expect(review.frame.commands.some(command=>command.kind==="text"&&command.text.includes("updated"))).toBe(true);
  const refreshed=diffCount();daemon.push();await until(()=>diffCount()>refreshed);await Bun.sleep(30);expect(applied).toBe(1);
 }finally{held?.release();review.dispose();git.dispose();model.dispose();client.stop();daemon.close();}
});
