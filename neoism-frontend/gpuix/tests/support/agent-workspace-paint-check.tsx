import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {coreCall} from "../../src/native-core";
import {tabChrome} from "../../src/layout/tab-chrome";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer,type RecordedAppNode} from "./full-app-recording-renderer";
const daemon=createAgentDaemon({deferCreations:false}),config={...loadConfig(["--config","/nonexistent/agent-workspace-paint"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer((node,box)=>{
 // Agent content lives below the buffer strip. Give this fixture the same
 // geometry as the native pane instead of sizing both parent and child alike.
 return ["agent-pane","conversation"].includes(String(node.props.testId))?{...box,y:box.y+tabChrome(config.scale).buffer.height,height:box.height-tabChrome(config.scale).buffer.height}:box;
}),root=createRoot(host.renderer);
model.setUI({workspaceSidebars:{a:"conversations",b:"conversations"},openWorkspaceIds:["a","b"]});
const uploads:number[]=[],drops:number[]=[];
const imageRenderer=host.renderer as unknown as {putPaintImage:(id:number)=>void;dropPaintImage:(id:number)=>void};
imageRenderer.putPaintImage=id=>{uploads.push(id);};imageRenderer.dropPaintImage=id=>{drops.push(id);};
const below=(node:RecordedAppNode):RecordedAppNode[]=>[node,...node.children.flatMap(id=>host.nodes.has(id)?below(host.nodes.get(id)!):[])];
const sidebarPaint=()=>{const sidebar=host.find("conversation-list");return sidebar&&below(sidebar).find(node=>node.type==="canvas");};
const imageKeys=()=>new Set<number>(JSON.parse(String(sidebarPaint()?.props.commands||"[]")).filter((op:any)=>op.k==="i").map((op:any)=>op.id));
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+4000;while(!predicate()){await host.pump(root);if(Date.now()>deadline)throw new Error(`Agent workspace paint did not settle: ${JSON.stringify({active:model.state.activeSurfaceId,history:Object.keys(model.state.history),conversation:!!host.find("conversation"),home:host.find("agent-pane")?.props["data-home"]})}`);await Bun.sleep(4);}await host.pump(root);};
const input=(text:string)=>{root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:text});root.flush();};
let watching=false;const firstFrames:Array<{input:boolean;sidebarImages:number;wordmarkWidth:number}>=[],apply=host.renderer.applyBatch.bind(host.renderer);
host.renderer.applyBatch=(json:string)=>{const result=apply(json);if(watching&&model.state.activeWorkspace?.id==="a"&&host.find("agent-pane"))firstFrames.push({input:!!host.find("composer"),sidebarImages:imageKeys().size,wordmarkWidth:Number(host.find("agent-wordmark")?.style.width)||0});return result;};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);const agentA=model.newAgentHome()!;
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await until(()=>!!host.find("composer")&&imageKeys().size>3);
 input("A draft survives the workspace roundtrip");await host.pump(root);
 const composer=host.find("composer")!.id,sidebar=host.find("conversation-list")!.id,card={...host.find("agent-composer-shell")!.style};
 const cached=visuals.agentComposer(agentA.id,config),native=cached.native!.id;assert.ok(cached.viewport.width>0&&cached.frame);
 // A refreshed catalog record with the same workspace identity must not
 // remount either input or sidebar while its data hydrates.
 model.update({activeWorkspace:{...model.state.activeWorkspace!}});await host.pump(root);assert.equal(host.find("composer")!.id,composer);assert.equal(host.find("conversation-list")!.id,sidebar);
 const retained=imageKeys(),dropsBefore=drops.length;await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="b")!);model.newAgentHome();await until(()=>!!host.find("composer")&&imageKeys().size>3);input("B separate draft");await host.pump(root);
 assert.ok([...retained].every(id=>!drops.slice(dropsBefore).includes(id)),"Unmounting cached A sidebar must not drop its glyph textures");
 const uploadsBefore=uploads.length;watching=true;const returning=model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="a")!);root.flush();
 assert.ok(host.find("composer"),"Returning Agent must create its input before any new layout event");assert.equal(host.find("composer")!.props.value,"A draft survives the workspace roundtrip");
 assert.ok(firstFrames.length>0);assert.ok(firstFrames.every(frame=>frame.input&&frame.sidebarImages>3&&frame.wordmarkWidth>0),"Every published return frame must contain the input, cached sidebar paint and sized wordmark");
 assert.equal(visuals.agentComposer(agentA.id,config).native!.id,native);assert.ok([...retained].every(id=>!uploads.slice(uploadsBefore).includes(id)),"Returning sidebar reuses pinned GPU images instead of uploading its glyphs again");
 assert.deepEqual(host.find("agent-composer-shell")!.style,card);watching=false;await returning;await host.pump(root);
 assert.equal(model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.id,agentA.id);assert.equal(daemon.requests.filter(request=>request.tag==="SubmitPrompt").length,0);assert.equal(daemon.ptyRequests.length,0);
 assert.equal(model.closeCurrentTab(false),true);await host.pump(root);assert.throws(()=>coreCall("agentcomposer.frame",{id:native,width:800,height:600}),"Closing the actual Agent tab must retire its cached native composer");
 // Populated transcripts restore at their measured size with resident ink.
 daemon.history.set("one",[{id:"stable-text",role:"Assistant",kind:"Assistant",text:"Conversation text stays at the same position when returning."}]);
 model.openThread(daemon.threads[0]);await until(()=>!!host.find("conversation"));
 await Bun.sleep(800);await host.pump(root);
 const transcript=()=>{const view=host.find("conversation");return view&&below(view).find(node=>node.type==="canvas");};
 const ink=()=>JSON.parse(String(transcript()?.props.commands||"[]")).filter((op:any)=>op.k==="i");
 await until(()=>ink().length>0);const originalInk=ink(),ids=new Set<number>(originalInk.map((op:any)=>op.id)),beforeDrops=drops.length;
 await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="b")!);await host.pump(root);
 assert.ok([...ids].every(id=>!drops.slice(beforeDrops).includes(id)),"Leaving a conversation retains its bounded GPU glyph cache");
 const beforeUploads=uploads.length,back=model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="a")!);root.flush();
 assert.ok(ink().length>0,"Restored transcript has paint before any new layout event");
 assert.deepEqual(ink(),originalInk,"The first return paint uses the same text positions and textures");
 assert.ok([...ids].every(id=>!uploads.slice(beforeUploads).includes(id)),"Returning does not upload cached transcript ink again");
 await back;await host.pump(root);
 console.log(JSON.stringify({noWindow:true,actualProductionApp:true,stableHostsForSameWorkspaceRecord:true,returnFirstFrameAlreadyHasInputAndSidebar:true,nativeComposerRetained:true,sidebarGlyphsPinnedAcrossSwitch:true,noSidebarGlyphReupload:true,independentDraftsPreserved:true,noPromptOrPtySideEffects:true,closedTabComposerRetired:true}));
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}
