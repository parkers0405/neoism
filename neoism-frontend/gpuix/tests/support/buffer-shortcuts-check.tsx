import assert from "node:assert/strict";
import {mkdtempSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {coreCall} from "../../src/native-core";
import {createWindowKeyRouter} from "../../src/commands/window-key-router";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const directory=mkdtempSync(join(tmpdir(),"neoism-buffer-shortcuts-")),path=join(directory,"config.json");writeFileSync(path,"{}");process.argv.push("--config",path);
const daemon=createAgentDaemon({deferCreations:false});daemon.history.set("one",[{id:"existing",role:"Assistant",kind:"Assistant",text:"Existing conversation history"}]);daemon.files.set("code.rs","fn retained() {}\nsecond\nthird\n");
const config={...loadConfig(["--config",path],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),keys=createWindowKeyRouter(),root=createRoot(host.renderer,{onKeyDown:keys.dispatch,keyboardFocusDim:false});
let sequence=0,stage="startup";
const until=async(predicate:()=>boolean)=>{const end=Date.now()+4000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw new Error("Buffer shortcut App did not settle");await Bun.sleep(3);}await host.pump(root);};
const press=async(key:string,modifiers={ctrl:true,shift:true,alt:false,cmd:false})=>{host.dispatchKey(root,host.focused,{key,keyChar:key.length===1?key:"",modifiers,keystrokeId:++sequence});await host.pump(root);};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);const home=model.state.activeSurfaceId!;await model.openFile("code.rs");const code=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const editor=model.getEditor(code);editor.call("paste",{text:"dirty "});const bytes=editor.text();await model.openFile("README.md");const markdown=model.state.activeSurfaceId!;const agent=model.newAgentHome()!;
 root.render(()=><App model={model} config={config} visuals={visuals} registerWindowKeys={keys.register}/>);await host.pump(root);model.selectSurface(home);await host.pump(root);
 const layout=model.getLayout(),order=layout.frame.panes[0].tabs.map(tab=>tab.surfaceId);
 for(const id of [code.id,markdown,agent.id,home]){stage=`cycling to ${id}`;await press("right");assert.equal(model.state.activeSurfaceId,id);}
 await press("ArrowLeft");assert.equal(model.state.activeSurfaceId,agent.id,"Left wraps to final buffer");
 stage="Agent picker must not trap the buffer navigation chord";
 root.dispatch({elementId:host.find("select-agent")!.id,eventType:"click",button:0});await until(()=>!!host.find("inline-picker-input"));
 await press("right");assert.equal(model.state.activeSurfaceId,home,"CtrlShiftRight must leave Agent even while its picker owns focus");
 await press("left");assert.equal(model.state.activeSurfaceId,agent.id);
 assert.deepEqual(layout.frame.panes[0].tabs.map(tab=>tab.surfaceId),order,"Selecting does not reorder tabs");assert.equal(model.state.activeWorkspace?.id,"a");
 await press("right");assert.equal(model.state.activeSurfaceId,home);await press("up",{ctrl:false,shift:false,alt:true,cmd:false});assert.equal(host.focused,host.find(`pane-tab:${layout.frame.focusedLeaf}`)!.id);const before=model.state.activeSurfaceId;
 await press("right",{ctrl:false,shift:false,alt:true,cmd:false});assert.equal(model.state.activeSurfaceId,before,"Alt arrow still moves focus cursor without selecting");
 stage="cycling while strip focus parked";await press("right");assert.equal(model.state.activeSurfaceId,code.id);assert.equal(host.focused,host.find(`pane-tab:${layout.frame.focusedLeaf}`)!.id,"Cycling while parked on buffer strip retains focus on newly selected tab");
 await press("down",{ctrl:false,shift:false,alt:true,cmd:false});await until(()=>host.focused===host.find("code-editor")?.id);
 const active=model.state.activeSurfaceId;await press("up");await press("down");assert.equal(model.state.activeSurfaceId,active,"CtrlShift vertical arrows never cycle tabs");assert.equal(editor.text(),bytes);assert.ok(editor.frame.dirty);
 stage="every actual tab host, terminal and Agent history included";
 model.openThread(daemon.threads[0]!);const conversation=model.state.activeSurfaceId!;
 await model.newTerminal();const terminal=model.state.activeSurfaceId!;
 model.openPage("extensions");const extensions=model.state.activeSurfaceId!;
 model.openGitReview({mode:"working",root:model.state.activeWorkspace!.root_dir!});const review=model.state.activeSurfaceId!;await host.pump(root);
 const kinds=new Map([[home,"welcome-page"],[code.id,"code-editor"],[markdown,"rich-markdown-editor"],[agent.id,"composer"],[conversation,"composer"],[terminal,"terminal"],[extensions,"extensions-page"],[review,"git-review"]]);
 const all=layout.frame.panes.find(pane=>pane.focused)!.tabs.map(tab=>tab.surfaceId!);
 for(const [id,testId] of kinds){
  model.selectSurface(id);await host.pump(root);const node=host.find(testId);assert.ok(node,`${testId} real host is mounted`);assert.equal(host.focused,node.id,`${testId} must own native focus`);
  const expected=all[(all.indexOf(id)+1)%all.length];await press("right");assert.equal(model.state.activeSurfaceId,expected,`CtrlShiftRight switches once from ${testId}`);
  await press("left");assert.equal(model.state.activeSurfaceId,id,`CtrlShiftLeft returns to ${testId}`);
 }
 stage="native root capture owns Agent navigation before textarea word-selection actions";
 for(const id of [agent.id,conversation]){
  model.selectSurface(id);model.setComposerDraft(model.composerDraftKey(model.state.surfaces.find(surface=>surface.id===id)!.sessionId,id),{text:"keep these draft words",attachments:[]});await host.pump(root);
  const draftKey=model.composerDraftKey(model.state.surfaces.find(surface=>surface.id===id)!.sessionId,id);
  const index=all.indexOf(id),expected=all[(index+1)%all.length];
  // The native capture path emits only windowKeyDown: no textarea keyDown
  // follows, because its SelectWord action is stopped before dispatch.
  root.dispatch({elementId:host.windowId,eventType:"windowKeyDown",key:"right",modifiers:{ctrl:true,shift:true,alt:false,cmd:false},keystrokeId:++sequence});await host.pump(root);
  assert.equal(model.state.activeSurfaceId,expected,"root capture leaves focused Agent textarea");
  root.dispatch({elementId:host.windowId,eventType:"windowKeyDown",key:"left",modifiers:{ctrl:true,shift:true,alt:false,cmd:false},keystrokeId:++sequence});await host.pump(root);
  assert.equal(model.state.activeSurfaceId,id);assert.equal(model.state.composerDrafts[draftKey]?.text,"keep these draft words");
 }
 model.selectSurface(conversation);await host.pump(root);const history=host.find("conversation")!;root.dispatch({elementId:history.id,eventType:"mouseDown",button:0,x:400,y:150});await host.pump(root);assert.equal(host.focused,history.id);await press("right");assert.equal(model.state.activeSurfaceId,terminal,"History focus routes the same chord without interpreting it as chat input");
 stage="CtrlU/D half-page navigation works while Agent composer owns focus";
 model.selectSurface(conversation);await host.pump(root);
 const sessionId=model.state.surfaces.find(surface=>surface.id===conversation)!.sessionId!;
 const tallHistory=Array.from({length:45},(_,i)=>({id:`half-page-${i}`,role:"User",kind:"User",text:`History paragraph ${i} with enough content to retain a real scrolling viewport.`}));
 daemon.history.set(sessionId,tallHistory);daemon.push({HistoryChunk:{session_id:sessionId,messages:tallHistory,next_cursor:null}});
 const timeline=()=>coreCall<Array<{sessionId:string;offset:number;scrollbar?:{content:number;viewport:number}}>>("agenttimeline.diagnostics",{}).find(engine=>engine.sessionId===sessionId);
 await until(()=>!!timeline()?.scrollbar&&timeline()!.scrollbar!.content>timeline()!.scrollbar!.viewport);
 const composer=host.find("composer")!;root.dispatch({elementId:composer.id,eventType:"focus"});
 const latest=timeline()!.offset;
 root.dispatch({elementId:composer.id,eventType:"keyDown",key:"u",keyChar:"u",modifiers:{ctrl:true,shift:false,alt:false,cmd:false},keystrokeId:++sequence});await host.pump(root);
 await until(()=>timeline()!.offset>latest+1);const older=timeline()!.offset;
 root.dispatch({elementId:composer.id,eventType:"keyDown",key:"d",keyChar:"d",modifiers:{ctrl:true,shift:false,alt:false,cmd:false},keystrokeId:++sequence});await host.pump(root);
 await until(()=>timeline()!.offset<older-1);
 stage="Agent sidebar owns its CtrlU/D navigation without scrolling the conversation";
 await press("c",{ctrl:false,shift:false,alt:true,cmd:false});await press("left",{ctrl:false,shift:false,alt:true,cmd:false});
 const conversations=host.find("conversation-list")!;assert.ok(conversations);await until(()=>host.focused===conversations.id);
 const beforeSidebar=timeline()!.offset;
 root.dispatch({elementId:conversations.id,eventType:"keyDown",key:"u",keyChar:"u",modifiers:{ctrl:true,shift:false,alt:false,cmd:false},keystrokeId:++sequence});
 assert.equal(timeline()!.offset,beforeSidebar,"Sidebar CtrlU does not invoke chat half-page command");
 await host.pump(root);await press("right",{ctrl:false,shift:false,alt:true,cmd:false});
 stage="pending Agent permission remains pending when navigating away";
 model.selectSurface(conversation);model.update({permissions:[{sessionId:model.state.surfaces.find(surface=>surface.id===conversation)!.sessionId!,requestId:"keep-pending",tool:"shell",title:"Allow shell?"}]});await until(()=>!!host.find("agent-picker:permission"));
 await press("right");assert.equal(model.state.activeSurfaceId,terminal);assert.equal(model.state.permissions[0]?.requestId,"keep-pending");assert.equal(daemon.httpRequests.some(request=>request.path.includes("interactions/permissions/keep-pending/")),false);model.update({permissions:[]});await host.pump(root);
 stage="Finder and command inputs keep the global tab chord";
 await press("p",{ctrl:false,shift:false,alt:true,cmd:false});await until(()=>!!host.find("command-dialog"));await press("right");assert.equal(model.state.activeSurfaceId,extensions);assert.equal(host.find("command-dialog"),undefined);
 await press("s",{ctrl:false,shift:false,alt:true,cmd:false});await until(()=>!!host.find("finder"));await press("left");assert.equal(model.state.activeSurfaceId,terminal);assert.equal(host.find("finder"),undefined);
 const split=model.newWelcome()!;layout.split("horizontal",split);const second=model.newWelcome()!;await host.pump(root);const other=layout.frame.panes.find(pane=>!pane.focused)!,otherLeaf=other.leaf,local=layout.frame.panes.find(pane=>pane.focused)!;assert.equal(local.tabs.length,2);
 await press("right");assert.equal(model.state.activeSurfaceId,split.id);assert.equal(layout.frame.panes.find(pane=>!pane.focused)!.leaf,otherLeaf);await press("right");assert.equal(model.state.activeSurfaceId,second.id);
 assert.equal(daemon.ptyRequests.filter(request=>request.tag==="CreatePty").length,1);assert.equal(daemon.ptyRequests.some(request=>request.tag==="Input"||request.tag==="ClosePty"),false);assert.equal(daemon.requests.some(request=>request.tag==="SubmitPrompt"),false);
 assert.equal(editor.text(),bytes);assert.ok(editor.frame.dirty);
 console.log(JSON.stringify({noWindow:true,actualApp:true,allBufferTypesCycle:true,wrapsBothDirections:true,selectsWithoutReordering:true,altFocusUnchanged:true,stripFocusRetained:true,verticalEditingNotIntercepted:true,splitScoped:true,dirtyBytesPreserved:true,actualHostTypes:kinds.size,agentHistoryAndPicker:true,commandAndFinderInputs:true,noShellInputOrPrompt:true}));
}catch(error){console.error(JSON.stringify({stage,focused:host.focused,focusHost:[...host.nodes.values()].find(node=>node.id===host.focused)?.props,surface:model.state.activeSurfaceId,error:model.state.error}));throw error;}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();rmSync(directory,{recursive:true,force:true});}
