import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {sessionErrorMessage} from "../src/agent/session-error";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {createAgentDaemon} from "./support/agent-daemon";
import {NativeComposer} from "../src/agent/NativeComposer";

const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){if(Date.now()>end)throw Error("Session error fixture timed out");await Bun.sleep(5);}};
test("provider errors preserve plain, v2 and nested OAuth messages without surfacing interruptions",()=>{
  for(const error of ["OpenAI OAuth refresh failed",{message:"OpenAI OAuth refresh failed"},{name:"ProviderAuthError",data:{message:"OpenAI OAuth refresh failed"}},{message:" ",data:{message:"OpenAI OAuth refresh failed"}}])expect(sessionErrorMessage({error})).toBe("OpenAI OAuth refresh failed");
  for(const error of [{message:"Session aborted"},{message:"Stopped",interrupted:true},{name:"MessageAbortedError",data:{message:"Stopped"}},{data:{message:"Stopped",interrupted:true}}])expect(sessionErrorMessage({error})).toBeUndefined();
  expect(sessionErrorMessage({error:{}})).toBe("Agent session failed");
});
test("failure footer has reserved geometry even without hints or a checkout directory",()=>{
  const composer=new NativeComposer(loadConfig(["--config","/nonexistent/error-footer"],{}));
  try{
    for(const width of [240,310,800]){
      const frame=composer.frame({width,height:600,home:false,text:"",help:false,images:0,noProviders:false,busy:false,state:"Idle",system:"Agent error"});
      expect(frame.footerVisible).toBe(true);expect(frame.trayRect).toBeDefined();
      const [x,y,w,h]=frame.trayRect!;
      expect(x).toBeGreaterThanOrEqual(0);expect(x+w).toBeLessThanOrEqual(width);
      expect(frame.inputRect[1]+y+h).toBeLessThanOrEqual(600);
    }
  }finally{composer.dispose();}
});
test("session errors survive idle and toast expiry, stay scoped, reach live history and clear on a new run",async()=>{
  const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/session-errors"],{}),endpoint:daemon.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config),events:unknown[]=[];
  const off=model.subscribeAgentEvents(event=>events.push(event));
  try{
    model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
    daemon.push({StreamingState:{session_id:"one",state:"Working",label:null}});
    daemon.push({SessionEvent:{session_id:"one",kind:"session.error",properties:{error:{name:"ProviderAuthError",data:{message:"OpenAI OAuth token expired. Sign in again."}}}}});
    await until(()=>!!model.state.sessionErrors.one);
    expect(model.state.streaming.one).toBe("Idle");expect(model.state.sessionErrors.two).toBeUndefined();
    expect(events.some((event:any)=>event.MessageUpdated?.message.title==="Agent error"&&event.MessageUpdated.message.text===model.state.sessionErrors.one)).toBe(true);
    model.clearError();daemon.push({SessionIdle:{session_id:"one"}});await Bun.sleep(250);
    expect(model.state.error).toBeUndefined();expect(model.state.sessionErrors.one).toContain("OpenAI OAuth token expired");
    daemon.push({SessionEvent:{session_id:"two",kind:"session.error",properties:{error:"Other provider failed"}}});await until(()=>!!model.state.sessionErrors.two);
    model.clearSessionError("two");expect(model.state.sessionErrors.one).toBeDefined();
    daemon.push({StreamingState:{session_id:"one",state:"Generating",label:null}});await until(()=>!model.state.sessionErrors.one);
    daemon.push({SessionEvent:{session_id:"one",kind:"session.error",properties:{error:{name:"MessageAbortedError"}}}});await Bun.sleep(25);
    expect(model.state.sessionErrors.one).toBeUndefined();
  }finally{off();model.dispose();client.stop();daemon.stop();}
});
test("initial history recovers only the latest failed turn and head refresh does not undo dismissal",async()=>{
  const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/history-errors"],{}),endpoint:daemon.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
  const error={id:"failure",role:"System",kind:"System",title:"Agent error",text:"OpenAI authorization expired"};
  try{
    model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
    daemon.push({HistoryChunk:{session_id:"one",messages:[{id:"prompt",role:"User",kind:"User",text:"hello"},error],next_cursor:null}});
    await until(()=>!!model.state.sessionErrors.one);model.clearSessionError("one");
    daemon.push({HistoryChunk:{session_id:"one",messages:[error],next_cursor:null}});await Bun.sleep(30);expect(model.state.sessionErrors.one).toBeUndefined();
    daemon.push({HistoryChunk:{session_id:"two",messages:[error,{id:"new-turn",role:"User",kind:"User",text:"try again"}],next_cursor:null}});
    await until(()=>!!model.state.history.two);expect(model.state.sessionErrors.two).toBeUndefined();
  }finally{model.dispose();client.stop();daemon.stop();}
});
test("HTTP prompt rejection is persistent without stopping an existing run",async()=>{
  const daemon=createAgentDaemon({promptError:"OpenAI credentials rejected"}),config={...loadConfig(["--config","/nonexistent/prompt-errors"],{}),endpoint:daemon.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
  try{
    model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
    model.update({streaming:{one:"Working"}});
    expect(await model.submitPrompt("one","hello")).toBe(false);
    expect(model.state.sessionErrors.one).toContain("OpenAI credentials rejected");expect(model.state.streaming.one).toBe("Working");
    model.clearError();expect(model.state.sessionErrors.one).toBeDefined();
  }finally{model.dispose();client.stop();daemon.stop();}
});
test("actual App retains the error footer and exposes full details and recovery actions",async()=>{
  // This renderer records host mutations; no production display/window is used.
  const child=Bun.spawn([process.execPath,"run","--preload="+fileURLToPath(new URL("../node_modules/@gpuix/solid/dist/preload.js",import.meta.url)),fileURLToPath(new URL("support/agent-session-errors-check.tsx",import.meta.url))],{cwd:fileURLToPath(new URL("../../..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  if(status!==0)throw Error(stdout+stderr);expect(status).toBe(0);
},15000);