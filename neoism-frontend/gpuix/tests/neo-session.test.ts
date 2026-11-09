import {expect,test} from "bun:test";
import {createAgentDaemon} from "./support/agent-daemon";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {decodeSession} from "../src/session-store";

const until=async(predicate:()=>boolean)=>{const end=Date.now()+2500;while(!predicate()){if(Date.now()>end)throw new Error("Neo session timeout");await Bun.sleep(5);}};
async function fixture(noWorkspaces=false){
 const daemon=createAgentDaemon({deferCreations:false,noWorkspaces});
 const config={...loadConfig(["--config","/nonexistent/neo-session-test"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 if(noWorkspaces)model.setUI({openWorkspaceIds:[]});
 model.start();client.start();
 await until(()=>model.state.connection.phase==="connected"&&!model.state.loading&&(noWorkspaces||model.state.activeWorkspace?.id==="a"));
 return{daemon,model,config,close:()=>{model.dispose();client.stop();daemon.stop();}};
}

test("Neo is one root chat across projects, outside their layouts and conversation lists",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  daemon.neo.defer=true;
  const first=model.openNeo(),second=model.openNeo();
  await until(()=>daemon.neo.pending.length===1);expect(first).toBe(second);
  daemon.neo.pending.shift()!();const surfaceId=await first;
  expect(model.state.surfaces.find(surface=>surface.id===surfaceId)).toMatchObject({kind:"agent",title:"Neo",sessionId:"neo-root",scope:"root",workspaceId:""});
  expect(model.getLayout().frame.focusedSurfaceId).toBe(surfaceId);
  expect(model.getLayout("a").frame.leaves.some(leaf=>leaf.surfaceId===surfaceId)).toBe(false);
  expect(model.state.threads.some(thread=>thread.session_id==="neo-root")).toBe(false);
  await until(()=>model.selection("neo-root").agent==="neo");
  model.setSelection("agent","build","neo-root");expect(model.selection("neo-root").agent).toBe("neo");
  await model.submitPrompt("neo-root","Handle this project for me");
  expect(daemon.requests.find(request=>request.tag==="SubmitPrompt")?.body).toMatchObject({session_id:"neo-root",mode:"neo"});
  expect(daemon.httpRequests.filter(request=>request.path.includes("neo/session"))).toEqual([expect.objectContaining({path:"/agent/system/v2/neo/session",directory:null})]);
  expect(daemon.httpRequests.some(request=>request.path==="/agent/system/v2/sessions/neo-root/prompt")).toBe(true);
  await model.activateWorkspace(daemon.workspaces[1]);daemon.neo.defer=false;
  expect(await model.openNeo()).toBe(surfaceId);
  expect(model.state.activeWorkspace?.id).toBe("b");
  expect(model.getLayout("b").frame.leaves.some(leaf=>leaf.surfaceId===surfaceId)).toBe(false);
  model.newAgentHome();daemon.neo.defer=false;
  expect(await model.openNeo()).toBe(surfaceId);
  expect(model.state.surfaces.filter(surface=>surface.sessionId==="neo-root")).toHaveLength(1);
  model.closeSurface(surfaceId!);await model.openNeo();
  expect(model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId).toBe("neo-root");
  expect(model.selection("neo-root").agent).toBe("neo");
  await model.newThread();
  expect(daemon.requests.find(request=>request.tag==="CreateThread")?.body.agent).toBe("build");
 }finally{close();}
});

test("root Neo opens, sends, and restores with no project selected or declared",async()=>{
 const {daemon,model,config,close}=await fixture(true);
 let restored:WorkspaceModel|undefined,restoredClient:DaemonConnection|undefined;
 try{
  const id=await model.openNeo();
  expect(model.state.activeWorkspace).toBeUndefined();expect(model.state.workspaces).toHaveLength(0);
  expect(model.getLayout().frame.focusedSurfaceId).toBe(id);
  expect(await model.submitPrompt("neo-root","Hello Neo")).toBe(true);
  await until(()=>model.state.history["neo-root"]!==undefined);
  expect(daemon.workspaceRequests.some(request=>request.tag==="CreateWorkspace")).toBe(false);
  const saved=decodeSession(JSON.parse(JSON.stringify(model.captureSession())))!;
  restoredClient=new DaemonConnection(config);restored=new WorkspaceModel(restoredClient,config);
  restored.restoreSession(saved);restored.start();restoredClient.start();
  await until(()=>restored!.state.connection.phase==="connected"&&restored!.state.activeSurfaceId===id&&restored!.state.history["neo-root"]!==undefined);
  expect(restored.state.activeWorkspace).toBeUndefined();
  expect(restored.state.surfaces.find(surface=>surface.id===id)).toMatchObject({scope:"root",workspaceId:""});
  expect(await restored.openNeo()).toBe(id);
 }finally{restored?.dispose();restoredClient?.stop();close();}
});

test("closing a project does not close or rehome the root assistant",async()=>{
 const {model,close}=await fixture();
 try{
  const id=await model.openNeo();
  expect(await model.closeWorkspace(false,"a")).toBe(true);
  expect(model.state.activeWorkspace).toBeUndefined();
  expect(model.state.activeSurfaceId).toBe(id);
  expect(model.getLayout().frame.focusedSurfaceId).toBe(id);
  expect(await model.submitPrompt("neo-root","Still here?")).toBe(true);
  model.closeSurface(id!);expect(model.getLayout("").frame.leaves).toHaveLength(0);
  expect(await model.openNeo()).toBe(id);
 }finally{close();}
});

test("a delayed Neo open never switches away from another workspace",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  daemon.neo.defer=true;const pending=model.openNeo();
  await until(()=>daemon.neo.pending.length===1);
  await model.activateWorkspace(daemon.workspaces[1]);const selected=model.state.activeSurfaceId;
  daemon.neo.pending.shift()!();expect(await pending).toBeUndefined();
  expect(model.state.activeWorkspace?.id).toBe("b");expect(model.state.activeSurfaceId).toBe(selected);
 }finally{close();}
});