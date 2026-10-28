import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {createWindowKeyRouter} from "../../src/commands/window-key-router";
import {tabChrome} from "../../src/layout/tab-chrome";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer,type RecordedAppNode} from "./full-app-recording-renderer";
const daemon=createAgentDaemon({deferCreations:false}),config={...loadConfig(["--config","/nonexistent/new-agent-firstpaint"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false,splashAutoAnimate:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),keys=createWindowKeyRouter(),root=createRoot(host.renderer,{onKeyDown:keys.dispatch,keyboardFocusDim:false});
const below=(node:RecordedAppNode):RecordedAppNode[]=>[node,...node.children.flatMap(id=>below(host.nodes.get(id)!))];
let keyId=0,watching=false;const batches:Array<{id:string;input:boolean;logo:Record<string,unknown>;ops:unknown[];viewport:{width:number;height:number}}>=[];
const apply=host.renderer.applyBatch.bind(host.renderer);host.renderer.applyBatch=json=>{const reply=apply(json);if(watching){const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId),logo=host.find("agent-wordmark");if(surface?.kind==="agent"&&host.find("agent-pane")){const canvas=logo&&below(logo).find(node=>node.type==="canvas");batches.push({id:surface.id,input:!!host.find("composer"),logo:{...logo?.style},ops:JSON.parse(String(canvas?.props.commands||"[]")),viewport:{...visuals.agentComposer(surface.id,config).viewport}});}}return reply;};
const until=async(predicate:()=>boolean)=>{const end=Date.now()+4000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw new Error("New Agent first paint did not settle");await Bun.sleep(3);}await host.pump(root);};
const fresh=()=>{
 const before=model.state.surfaces.length;batches.length=0;watching=true;
 // Only route actual key events + flush mutations. No layout events/pump until
 // after we inspect every published native transaction for the new tab.
 host.dispatchKey(root,host.focused,{key:"a",keyChar:"a",keystrokeId:++keyId,modifiers:{alt:true,ctrl:false,shift:false,cmd:false}});root.flush();watching=false;
 const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;assert.equal(surface.kind,"agent");assert.equal(model.state.surfaces.length,before+1);assert.ok(batches.length);
 const frame=visuals.agentComposer(surface.id,config).frame!;assert.ok(frame);const pane=model.getLayout().frame.panes.find(pane=>pane.focused)!,border=model.getLayout().frame.panes.length>1?config.scale:0,expected={width:pane.rect[2]-2*border,height:pane.rect[3]-tabChrome(config.scale).buffer.height-2*border};
 for(const batch of batches){assert.equal(batch.id,surface.id);assert.ok(batch.input,"Fresh Agent textarea must exist before any layout event");assert.deepEqual(batch.viewport,expected,"Prime from this actual pane, not a previous/global Agent viewport");assert.deepEqual([batch.logo.left,batch.logo.top,batch.logo.width,batch.logo.height],frame.logoRect);assert.ok(Number(batch.logo.width)>0&&Number(batch.logo.height)>0);assert.ok(batch.ops.length,"Fresh native wordmark paints in the same first transaction");}
 assert.equal(host.find("composer")!.props.value,"");return surface;
};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);root.render(()=><App model={model} config={config} visuals={visuals} registerWindowKeys={keys.register}/>);await host.pump(root);assert.ok(host.find("file-tree"));
 const first=fresh();await host.pump(root);root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:"first private draft"});await host.pump(root);
 const second=fresh();assert.notEqual(second.id,first.id);assert.equal(model.state.composerDrafts[model.composerDraftKey(undefined,first.id)]?.text,"first private draft");await host.pump(root);
 // A fresh Agent in a narrow split must derive the smaller current body,
 // rather than borrowing the first full-width Agent's cached dimensions.
 const home=model.newWelcome()!,layout=model.getLayout();layout.split("horizontal",home);await host.pump(root);const third=fresh();assert.ok(visuals.agentComposer(third.id,config).viewport.width<visuals.agentComposer(second.id,config).viewport.width);await host.pump(root);
 assert.equal(daemon.ptyRequests.length,0);assert.equal(daemon.requests.some(request=>request.tag==="SubmitPrompt"),false);
 console.log(JSON.stringify({noWindow:true,actualAltA:true,freshNativeComposerReadyInFirstBatch:true,wordmarkPaintAndGeometryReadyBeforeLayout:true,canonicalSplitAndSidebarViewport:true,independentFreshDrafts:true,noPtyOrPrompt:true}));
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}
