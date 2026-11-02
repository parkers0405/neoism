import {expect,test} from "bun:test";
import {shellCommand,exitShellMode} from "../src/agent/shell-mode";
import {AgentSdk} from "../src/daemon/sdk";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";

test("only a leading bang selects shell mode; empty mode cannot be a command",()=>{
  expect(shellCommand("! ls -la")).toBe("ls -la");
  expect(shellCommand("!printf '%s' '! prose'\npwd")).toBe("printf '%s' '! prose'\npwd");
  expect(shellCommand("!")).toBe("");
  for(const value of ["", "hello!", "please ! ls", " ! ls"] ) expect(shellCommand(value)).toBeUndefined();
  expect(exitShellMode("! ls","escape")).toBe("");
  expect(exitShellMode("! ls","composer-escape")).toBe("");
  expect(exitShellMode("!","backspace")).toBe("");
  expect(exitShellMode("!  ","backspace")).toBe("");
  expect(exitShellMode("! ls","backspace")).toBeUndefined();
  expect(exitShellMode("ordinary!","escape")).toBeUndefined();
  expect(exitShellMode("ordinary!","composer-escape")).toBeUndefined();
});

test("shell SDK executes only on explicit invocation using scoped authenticated remote routing",async()=>{
  const requests:Array<{url:string;auth:string|null;body:any}>=[];
  const sdk=new AgentSdk({endpoint:"wss://remote.example/prefix/session",pairingToken:"paired"},(async(input,init)=>{
    requests.push({url:String(input),auth:new Headers(init?.headers).get("authorization"),body:init?.body?JSON.parse(String(init.body)):undefined});
    return Response.json(init?.method==="POST"?{}:{execution:"direct"});
  }) as typeof fetch);
  try {
    shellCommand("! printf test");expect(requests).toHaveLength(0);
    await sdk.forWorkspace("workspace/a").sessions.shell("session/b","printf test",{messageId:"one"});
    const url="https://remote.example/prefix/agent/workspaces/workspace%2Fa/v2/sessions/session%2Fb/shell";
    expect(requests).toEqual([{url,auth:"Bearer paired",body:undefined},{url,auth:"Bearer paired",body:{messageId:"one",command:"printf test"}}]);
  } finally {sdk.dispose();}
});

test("legacy shell endpoints are never called as a model prompt fallback",async()=>{
  for(const status of [200,403,404,405]){
    const methods:string[]=[];
    const sdk=new AgentSdk({endpoint:"ws://localhost/session"},(async(_input,init)=>{
      methods.push(init?.method||"GET");return Response.json({}, {status});
    }) as typeof fetch);
    try{
      await expect(sdk.forWorkspace("workspace").sessions.shell("session","echo never")).rejects.toThrow();
      expect(methods).toEqual(["GET"]);
    }finally{sdk.dispose();}
  }
});

test("model shell submission follows the session owner, not the active workspace or model prompt API",async()=>{
  const config=loadConfig([],{},{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
  const calls:unknown[]=[];
  model.state.activeWorkspace={id:"active",host_id:"host",title:"Active",root_dir:"/active"};
  model.state.surfaces=[{id:"chat",kind:"agent",sessionId:"owned-session",workspaceId:"owner",title:"Chat"}];
  model.agentSdk.forSession=((workspace:string,session:string)=>({sessions:{shell:async(id:string,command:string)=>{calls.push({workspace,session,id,command});},prompt:()=>{throw new Error("must not prompt a model");}}})) as unknown as typeof model.agentSdk.forSession;
  try {
    expect(await model.submitShell("owned-session"," ")).toBe(false);
    expect(calls).toHaveLength(0);
    expect(await model.submitShell("owned-session","pwd")).toBe(true);
    expect(calls).toEqual([{workspace:"owner",session:"owned-session",id:"owned-session",command:"pwd"}]);
  } finally {model.dispose();client.stop();}
});