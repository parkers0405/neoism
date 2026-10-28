import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {GitController} from "../../src/git/GitController";
import {GitDiffPanelView} from "../../src/git/GitDiffPanelView";
import type {ChromeCursorTarget} from "../../src/ui/ChromeFocusCursor";
import {createGitDaemon} from "./git-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const daemon=createGitDaemon(),config={...loadConfig(["--config","/nonexistent/git-view"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer),viewport={x:300,y:128,width:960,height:640};
let controller:GitController|undefined,cursor:ChromeCursorTarget|undefined,closed=0;const reviews:unknown[]=[];
const [panelActive,setPanelActive]=createSignal(true);
let focusRequests=0;const focusElement=host.renderer.focusElement!.bind(host.renderer);host.renderer.focusElement=id=>{focusRequests++;focusElement(id);};
const until=async(predicate:()=>boolean)=>{const end=Date.now()+4000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw new Error("Original Git panel view did not settle");await Bun.sleep(4);}await host.pump(root);};
const click=async(id:string)=>{
 const hit=controller!.frame.hits.find(hit=>hit.id===id);assert.ok(hit,`Original hit ${id} must exist`);
 const x=viewport.x+hit.rect[0]+hit.rect[2]/2,y=viewport.y+hit.rect[1]+hit.rect[3]/2;
 root.dispatch({elementId:host.find("git-diff-panel")!.id,eventType:"mouseDown",button:0,x,y});root.flush();
 const drag=host.find("git-panel-drag-layer");if(drag)root.dispatch({elementId:drag.id,eventType:"mouseUp",button:0,x,y});await host.pump(root);
};
const key=async(key:string,char=key.length===1?key:"",shift=false)=>{root.dispatch({elementId:host.find("git-diff-panel")!.id,eventType:"keyDown",key,keyChar:char,modifiers:{shift,ctrl:false,alt:false,cmd:false}});await host.pump(root);};
try{
 client.start();await until(()=>client.state.phase==="connected");
 controller=new GitController({model,config,hooks:{openReview:target=>{reviews.push(target);}}});controller.open("/fixture");
 root.render(()=><><div testId="other-focus-owner" tabIndex={0}/><GitDiffPanelView controller={controller!} config={config} viewport={viewport} active={panelActive()} onCursorTarget={target=>{cursor=target;}} onClose={()=>closed++}/></>);
 await until(()=>controller!.frame.files.length===2&&!controller!.frame.animating&&!!host.find("git-panel-hit:stage:0"));
 const labels=controller.frame.commands.flatMap(command=>command.kind==="text"?[command.text]:[]);assert.ok(labels.some(text=>text.includes("main.ts")));assert.ok(controller.frame.commands.some(command=>command.kind==="text"&&!!command.raster));
 const panel=host.find("git-diff-panel")!,paint=host.find("original-paint-layer")!.id;assert.equal(panel.style.width,controller.frame.width);assert.equal(host.find("git-panel-native-paint")!.style.left,controller.frame.width-viewport.width);
 const beforeFocus=focusRequests,beforeFrame=controller.frame;daemon.push();await until(()=>controller!.frame!==beforeFrame);assert.equal(focusRequests,beforeFocus,"Background repo status paint must not refocus the panel or steal another surface's focus");
 setPanelActive(false);host.renderer.focusElement!(host.find("other-focus-owner")!.id);await host.pump(root);const inactiveFocusRequests=focusRequests;await click("commit-input");assert.ok(focusRequests>inactiveFocusRequests,"An explicit pointer press must focus the panel even after opening a review made active false");assert.equal(host.focused,panel.id);setPanelActive(true);controller.call("key",{key:"escape"});await host.pump(root);
 await click("file:0");await until(()=>reviews.length===1);assert.deepEqual(reviews[0],{root:"/fixture",path:"/fixture/src/main.ts"});
 await click("stage:0");await until(()=>controller!.frame.files[0]!.staged);assert.ok(daemon.requests.some(request=>typeof request.message==="object"&&request.message!==null&&"Stage"in request.message));
 await click("commit-input");for(const char of "First")await key(char);await key("enter","",true);for(const char of "second")await key(char);assert.equal(controller.frame.commitText,"First\nsecond");
 daemon.setCommitError("fixture commit refused");await click("commit");await until(()=>!!controller!.frame.error);assert.equal(controller.frame.commitText,"First\nsecond","Failed real Git reply must not clear the original commit input");
 daemon.setCommitError(undefined);await click("commit");await until(()=>controller!.frame.commitText==="");
 await click("branch");await until(()=>controller!.frame.hits.some(hit=>hit.branch==="main"));const branch=controller.frame.hits.find(hit=>hit.branch==="main")!;await click(branch.id);await until(()=>controller!.frame.branch==="main");
 const width=controller.frame.width,edge=controller.frame.hits.find(hit=>hit.id==="resize")!;const x=viewport.x+edge.rect[0]+edge.rect[2]/2,y=viewport.y+100;
 root.dispatch({elementId:panel.id,eventType:"mouseDown",button:0,x,y});root.flush();const drag=host.find("git-panel-drag-layer")!;
 root.dispatch({elementId:drag.id,eventType:"mouseMove",pressedButton:0,x:x-35,y});root.dispatch({elementId:drag.id,eventType:"mouseUp",button:0,x:x-35,y});await host.pump(root);assert.equal(controller.frame.width,width+35);
 await until(()=>!!cursor?.bounds);const rect=controller.frame.cursorRect!;assert.deepEqual(cursor!.bounds,{x:viewport.x+rect[0],y:viewport.y+rect[1],width:rect[2],height:rect[3]});
 assert.equal(host.find("original-paint-layer")!.id,paint);assert.equal(host.find("git-diff-panel")!.id,panel.id);assert.ok(daemon.requests.filter(request=>request.family==="Git").every(request=>request.workspaceRoot==="/fixture"));
 await click("close");assert.equal(closed,1);assert.equal(host.find("git-diff-panel"),undefined);
 console.log(JSON.stringify({noWindow:true,originalRustGitPaint:true,sourceCoordinateTranslation:true,actualScopedFixtureRequests:true,nativeFileReviewAndStage:true,commitMessagePreservedOnFailure:true,branchCheckoutAndResize:true,exactGlobalCursorTarget:true,stablePaintAndFocusHosts:true,backgroundRefreshDoesNotRefocus:true,closeWorks:true}));
}catch(error){console.error(JSON.stringify({frame:controller&&{visible:controller.frame.visible,error:controller.frame.error,commit:controller.frame.commitText,files:controller.frame.files,branch:controller.frame.branch,cursor:controller.frame.cursorRect,hits:controller.frame.hits},requests:daemon.requests,closed}));throw error;}
finally{root.unmount();controller?.dispose();model.dispose();client.stop();daemon.close();}
