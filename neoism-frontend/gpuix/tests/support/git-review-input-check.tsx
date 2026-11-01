import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {GitReviewController} from "../../src/git/GitReviewController";
import {GitReviewView} from "../../src/git/GitReviewView";
import {GitController} from "../../src/git/GitController";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createGitDaemon} from "./git-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const daemon=createGitDaemon(),config={...loadConfig(["--config","/nonexistent/review-input"],{}),endpoint:daemon.endpoint,cursorBlink:false},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
const frames=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{frames.add(callback);return()=>frames.delete(callback);});
const host=createRecordingAppRenderer(),root=createRoot(host.renderer);let review:GitReviewController|undefined,git:GitController|undefined;
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){if(Date.now()>end)throw Error("Review fixture stalled");await Bun.sleep(5);}};
try{
 client.start();await until(()=>client.state.phase==="connected");git=new GitController({model,config});review=new GitReviewController({model,config,git,descriptor:{mode:"working",root:"/fixture"}});
 await until(()=>review!.frame.files.length===2&&review!.frame.files.every(file=>file.loaded));
 root.render(()=><GitReviewView controller={review!} config={config}/>);await host.pump(root);
 const moves:Record<string,unknown>[]=[],scrolls:Record<string,unknown>[]=[],call=review.call.bind(review);
 review.call=(op,args={})=>{if(op==="pointer"&&args.kind==="move")moves.push(args);if(op==="scroll")scrolls.push(args);return call(op,args);};
 const view=host.find("git-review")!;
 for(let i=0;i<100;i++){
  root.dispatch({elementId:view.id,eventType:"mouseMove",x:310+i,y:95});
  root.dispatch({elementId:view.id,eventType:"scroll",deltaY:-.25,deltaX:0,precise:true,x:320,y:120});
 }
 assert.equal(moves.length,0);assert.equal(scrolls.length,0);
 for(const frame of [...frames]){frames.delete(frame);frame();}root.flush();
 assert.equal(moves.length,1);assert.equal(moves[0].x,109);assert.equal(scrolls.length,1);assert.equal(scrolls[0].deltaY,50,"All fractional trackpad movement survives coalescing");
 root.dispatch({elementId:view.id,eventType:"mouseMove",x:315,y:95});
 root.dispatch({elementId:view.id,eventType:"mouseUp",button:0,x:315,y:95});
 assert.equal(moves.length,2,"Release flushes the last selection movement immediately");
 root.dispatch({elementId:view.id,eventType:"mouseMove",x:316,y:95});root.unmount();
 for(const frame of [...frames]){frames.delete(frame);frame();}
 assert.equal(moves.length,2,"Unmount cancels pending input");
 console.log("100 hover and scroll events produce one update each, retaining all movement");
}finally{root.unmount();review?.dispose();git?.dispose();restore();model.dispose();client.stop();daemon.close();}
