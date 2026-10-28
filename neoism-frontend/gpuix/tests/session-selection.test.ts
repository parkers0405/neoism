import {expect,test} from "bun:test";
import {createAgentDaemon} from "./support/agent-daemon";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
const until=async(predicate:()=>boolean)=>{const end=Date.now()+2500;while(!predicate()){if(Date.now()>end)throw new Error("Session selection timeout");await Bun.sleep(5);}};
test("new default conversation keeps the server's concrete selection for variant updates",async()=>{
 const daemon=createAgentDaemon();daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/session-selection-test"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();
 try{
  await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
  const pending=model.newThread();await until(()=>daemon.creations.length===1);daemon.creations.shift()!();await pending;
  expect(model.selection("created-a")).toMatchObject({agent:"build",model:"openai/base",provider_id:"openai",thinking:"medium"});
  model.setSelection("thinking","high","created-a");
  await until(()=>daemon.requests.some(request=>request.tag==="UpdateSession"&&request.body.session_id==="created-a"));
  expect(daemon.requests.find(request=>request.tag==="UpdateSession"&&request.body.session_id==="created-a")?.body).toMatchObject({model:{providerId:"openai",id:"base",variant:"high"}});
 }finally{model.dispose();client.stop();daemon.stop();}
});

async function selectionFixture(home=false){
 const daemon=createAgentDaemon({deferCreations:false});if(home)daemon.threads.splice(0);
 const config={...loadConfig(["--config","/nonexistent/session-selection-test"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();
 await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading&&(home||model.state.threads.some(thread=>thread.session_id==="one")));
 if(!home){model.openThread(daemon.threads[0]);await until(()=>model.selection("one").thinking==="medium");}
 return {daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};
}
test("thread model changes clear only accounts belonging to the previous provider",async()=>{
 const {daemon,model,close}=await selectionFixture();
 try{
  model.update({providerStates:{...model.state.providerStates,one:{model:"openai/base",provider_id:"openai",connection_id:"openai-account",thinking:"medium",agent:"build"},two:{model:"other/second",connection_id:"other-account"}}});
  model.setSelection("model","openai/advanced","one");model.setSelection("thinking","high","one");model.setSelection("agent","plan","one");
  const updates=()=>daemon.requests.filter(request=>request.tag==="UpdateSession"&&request.body.session_id==="one");
  await until(()=>updates().length===3);
  for(const update of updates())expect(update.body.model).toMatchObject({providerId:"openai",connectionId:"openai-account"});
  expect(updates()[2].body).toMatchObject({agent:"plan",model:{id:"advanced",variant:"high"}});
  model.setSelection("model","other/next","one");
  expect(model.selection("one").connection_id).toBeUndefined();expect(model.selection("one").provider_id).toBe("other");
  expect(model.selection("two").connection_id).toBe("other-account");
  await until(()=>updates().length===4);
  expect(updates()[3].body.model).toEqual({providerId:"other",id:"next",variant:"high"});
  model.setSelection("thinking","low","one");await until(()=>updates().length===5);
  expect(updates()[4].body.model).toEqual({providerId:"other",id:"next",variant:"low"});
 }finally{close();}
});
test("home drafts retain same-provider accounts and omit them atomically on cross-provider creation",async()=>{
 const {daemon,model,close}=await selectionFixture(true);
 try{
  model.update({drafts:{a:{model:"openai/base",provider_id:"openai",connection_id:"openai-account",thinking:"medium",agent:"build"},b:{model:"other/base",connection_id:"workspace-b-account"}}});
  model.setSelection("model","openai/advanced");model.setSelection("thinking","high");model.setSelection("agent","plan");
  expect(model.selection().connection_id).toBe("openai-account");await model.newThread();
  const creations=()=>daemon.requests.filter(request=>request.tag==="CreateThread");
  expect(creations()[0].body).toMatchObject({agent:"plan",model:{providerId:"openai",id:"advanced",variant:"high",connectionId:"openai-account"}});
  model.setSelection("model","other/next");expect(model.selection().connection_id).toBeUndefined();
  expect(model.state.drafts.b.connection_id).toBe("workspace-b-account");
  await model.newThread();expect(creations()[1].body.model).toEqual({providerId:"other",id:"next",variant:"high"});
 }finally{close();}
});
