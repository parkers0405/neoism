import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {coreCall} from "../../src/native-core";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const options={deferCreations:false,rejectSessionUpdates:true,deferWorkspaceSubscriptions:false};
const daemon=createAgentDaemon(options);
daemon.history.set("one",[{id:"msg_recoveredOne",role:"Assistant",kind:"Assistant",text:"# Recovered first conversation\n\nExisting transcript, without a scroll event."}]);
daemon.history.set("two",[{id:"msg_recoveredTwo",role:"Assistant",kind:"Assistant",text:"Second existing conversation"}]);
const config={...loadConfig(["--config","/nonexistent/agent-recovery-mode"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
let client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
const visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const until=async(predicate:()=>boolean,message:string)=>{const end=Date.now()+5000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw new Error(message+"; model error="+model.state.error);await Bun.sleep(3);}await host.pump(root);};
const click=async(id:string)=>{const leaf=model.getLayout().frame.leaves.find(leaf=>leaf.surfaceId===id)!;assert.ok(leaf);root.dispatch({elementId:host.find(`pane-tab:${leaf.leaf}`)!.id,eventType:"mouseDown",button:0,x:700,y:100});await host.pump(root);};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading,"initial workspace");
 await model.openFile("README.md");const file=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;model.getEditor(file).call("paste",{text:"unsaved before reopening "});
 const first=model.openThread(daemon.threads[0])!;await until(()=>!!model.state.history.one?.length,"first initial transcript");const second=model.openThread(daemon.threads[1])!;await until(()=>!!model.state.history.two?.length,"second initial transcript");
 model.setComposerDraft("one",{text:"retained conversation draft",attachments:[]});const empty=model.newAgentHome()!;const saved=model.captureSession();model.dispose();client.stop();
 options.deferWorkspaceSubscriptions=true;
 model=new WorkspaceModel(client=new DaemonConnection(config),config);model.restoreSession(saved);model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&daemon.workspaceSubscriptions.length>0,"recovered workspace subscription pending");
 assert.equal(model.state.history.one,undefined,"Recovery must hydrate from server, rather than a fabricated cached transcript");
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await host.pump(root);
 for(const[id,sessionId]of [[first,"one"],[second,"two"]]){
  const before=daemon.requests.filter(request=>request.tag==="GetHistory"&&request.body.session_id===sessionId).length;
  await click(id);await until(()=>model.state.activeSurfaceId===id&&!!model.state.history[sessionId]?.length&&!!host.find("conversation"),`recovered ${sessionId} transcript must load on tab activation`);
  assert.ok(daemon.requests.filter(request=>request.tag==="GetHistory"&&request.body.session_id===sessionId).length>before,"Tab activation issues history without wheel input");
  await until(()=>coreCall<Array<{sessionId:string;commandsByKind:{text:number}}>>("agenttimeline.diagnostics").some(frame=>frame.sessionId===sessionId&&frame.commandsByKind.text>0),"original timeline must paint restored content");
  while(daemon.workspaceSubscriptions.length)daemon.workspaceSubscriptions.shift()!.release();await until(()=>!model.state.loading,"workspace subscription settled");
  const expected=model.selection(sessionId).agent==="plan"?"build":"plan";
  const composer=host.find("composer")!;root.dispatch({elementId:composer.id,eventType:"keyDown",key:"tab",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await host.pump(root);
  await until(()=>daemon.requests.some(request=>request.tag==="SetAgent"&&request.body.session_id===sessionId&&request.body.agent===expected)&&model.selection(sessionId).agent===expected,"Build/Plan must use the original session-scoped Agent command");
  assert.equal(model.state.error,undefined);assert.ok(!daemon.httpRequests.some(request=>request.path.endsWith(`/sessions/${sessionId}`)),"Changing Agent mode must not PATCH an existing chat through the wrong UI-workspace tenant");
 }
 await click(empty.id);await until(()=>!!host.find("composer"),"empty Agent input");const before=daemon.requests.filter(request=>request.tag==="SetAgent").length;
 root.dispatch({elementId:host.find("composer")!.id,eventType:"keyDown",key:"tab",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await host.pump(root);assert.equal(daemon.requests.filter(request=>request.tag==="SetAgent").length,before,"A local draft has no server session to mutate");
 assert.ok(model.getEditor(file).text().startsWith("unsaved before reopening "));assert.ok(model.getEditor(file).frame.dirty);assert.equal(model.state.composerDrafts.one.text,"retained conversation draft");assert.equal(daemon.ptyRequests.length,0);assert.equal(daemon.requests.filter(request=>request.tag==="CreateThread"||request.tag==="SubmitPrompt").length,0);
 console.log(JSON.stringify({noWindow:true,actualApp:true,coldAgentTabsHydrateWithoutScrolling:true,pendingWorkspaceAckDoesNotBlockTranscript:true,originalTimelinePaints:true,buildPlanUsesCanonicalAgentCommand:true,emptyHomeRemainsLocal:true,dirtyFileAndDraftRetained:true,noPtyOrSessionCreation:true}));
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}
