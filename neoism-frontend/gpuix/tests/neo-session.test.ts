import {expect,test} from "bun:test";
import {createAgentDaemon} from "./support/agent-daemon";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";

const until=async(predicate:()=>boolean)=>{const end=Date.now()+2500;while(!predicate()){if(Date.now()>end)throw new Error("Neo session timeout");await Bun.sleep(5);}};
async function fixture(){
 const daemon=createAgentDaemon({deferCreations:false});
 const config={...loadConfig(["--config","/nonexistent/neo-session-test"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();
 await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
 return{daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}

test("Neo reopens one normal chat without a catalog row and retains its dedicated agent",async()=>{
 const {daemon,model,close}=await fixture();
 try{
  daemon.neo.defer=true;
  const first=model.openNeo(),second=model.openNeo();
  await until(()=>daemon.neo.pending.length===1);expect(first).toBe(second);
  daemon.neo.pending.shift()!();const surfaceId=await first;
  expect(model.state.surfaces.find(surface=>surface.id===surfaceId)).toMatchObject({kind:"agent",title:"Neo",sessionId:"neo-a"});
  expect(model.state.threads.some(thread=>thread.session_id==="neo-a")).toBe(false);
  await until(()=>model.selection("neo-a").agent==="neo");
  model.setSelection("agent","build","neo-a");expect(model.selection("neo-a").agent).toBe("neo");
  await model.submitPrompt("neo-a","Handle this project for me");
  expect(daemon.requests.find(request=>request.tag==="SubmitPrompt")?.body).toMatchObject({session_id:"neo-a",mode:"neo"});
  model.newAgentHome();daemon.neo.defer=false;
  expect(await model.openNeo()).toBe(surfaceId);
  expect(model.state.surfaces.filter(surface=>surface.sessionId==="neo-a")).toHaveLength(1);
  model.closeSurface(surfaceId!);await model.openNeo();
  expect(model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId).toBe("neo-a");
  expect(model.selection("neo-a").agent).toBe("neo");
  await model.newThread();
  expect(daemon.requests.find(request=>request.tag==="CreateThread")?.body.agent).toBe("build");
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