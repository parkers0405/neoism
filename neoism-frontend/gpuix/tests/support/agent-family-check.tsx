import assert from "node:assert/strict";
import{createRoot}from"@gpuix/solid";
import{App}from"../../src/ui/App";
import{AppVisualState}from"../../src/ui/AppVisualState";
import{loadConfig}from"../../src/config";
import{DaemonConnection}from"../../src/daemon/client";
import{WorkspaceModel}from"../../src/model";
import{createAgentDaemon}from"./agent-daemon";
import{createRecordingAppRenderer}from"./full-app-recording-renderer";
import{usePresentationScheduler}from"../../src/animation/frame-scheduler";
import{createWindowKeyRouter}from"../../src/commands/window-key-router";
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/gpuix-family-host-test"],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:true},client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const host=createRecordingAppRenderer(),keys=createWindowKeyRouter(),root=createRoot(host.renderer,{onKeyDown:keys.dispatch,keyboardFocusDim:false});const callbacks=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
let stage="startup";const pulse=async()=>{const list=[...callbacks];callbacks.clear();for(const frame of list)frame();await host.pump(root);await Bun.sleep(5);};const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+4000;while(!predicate()){await pulse();if(Date.now()>deadline)throw Error("Family host stage failed: "+stage);}};
const clickHeader=async(direction:string)=>{const marker=host.find("agent-family-navigation:"+direction),conversation=host.find("conversation");assert.ok(marker);assert.ok(conversation);root.dispatch({elementId:conversation.id,eventType:"mouseDown",button:0,x:300+Number(marker.style.left)+Number(marker.style.width)/2,y:90+Number(marker.style.top)+Number(marker.style.height)/2});await host.pump(root);};
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);const parent=daemon.threads[0],id=model.openThread(parent)!;
 model.setComposerDraft(parent.session_id,{text:"parent draft stays here",attachments:[]});
 daemon.push({CatalogFamily:{session_id:parent.session_id,snapshot:{directory:"/project/a",executionId:"exec",familyRevision:1,branches:[{sessionId:"child",parentSessionId:parent.session_id,title:"Child",status:"running",depth:1},{sessionId:"nested",parentSessionId:"child",title:"Nested",status:"running",depth:2},{sessionId:"last",parentSessionId:parent.session_id,title:"Last",status:"running",depth:1}]}}});await until(()=>model.agentFamily("nested").root===parent.session_id);
 root.render(()=><App model={model} config={config} visuals={visuals} registerWindowKeys={keys.register}/>);await host.pump(root);
 stage="viewed child has original measured Back/Next links and no composer";model.openThread({session_id:"child",title:"Child",directory:"/project/a"},false,parent.session_id);await until(()=>!!host.find("agent-family-navigation:previous")&&!!host.find("agent-family-navigation:next"));assert.equal(host.find("composer"),undefined);assert.equal(model.state.surfaces.filter(s=>s.kind==="agent").length,1);const first=host.find("agent-family-navigation:previous")!,next=host.find("agent-family-navigation:next")!;assert.ok(Number(first.style.width)>0);assert.ok(Number(next.style.left)>Number(first.style.left)+Number(first.style.width));
 stage="Next advances to sibling without creating tab; Prev returns; Back restores actual parent composer/draft";await clickHeader("next");await until(()=>model.state.surfaces.find(s=>s.id===id)?.sessionId==="nested");assert.equal(model.state.activeSurfaceId,id);await clickHeader("previous");await until(()=>model.state.surfaces.find(s=>s.id===id)?.sessionId==="child");await clickHeader("previous");await until(()=>model.state.surfaces.find(s=>s.id===id)?.sessionId===parent.session_id&&!!host.find("composer"));assert.equal(host.find("agent-family-navigation:previous"),undefined);assert.equal(model.state.composerDrafts[parent.session_id].text,"parent draft stays here");assert.equal(host.find("composer")!.props.value,"parent draft stays here");assert.equal(model.state.surfaces.filter(s=>s.kind==="agent").length,1);
 assert.equal(daemon.requests.some(r=>["CreateThread","SubmitPrompt"].includes(r.tag)),false);assert.equal(daemon.ptyRequests.length,0);
 console.log(JSON.stringify({noWindow:true,actualApp:true,originalMeasuredHeader:true,nestedPrevNextBack:true,singleFamilyTab:true,parentDraftRestored:true,noBackendCreation:true}));
}catch(error){console.error(JSON.stringify({stage,surfaces:model.state.surfaces,error:model.state.error}));throw error;}
finally{root.unmount();restore();visuals.dispose();model.dispose();client.stop();daemon.stop();}
