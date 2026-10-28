import {expect,test} from "bun:test";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
test("layout-first activation hydrates a restored chat without requiring a scroll",()=>{
 const config=loadConfig(["--config","/nonexistent/pagination-cache"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const sent:any[]=[];client.send=((_family,body)=>{sent.push(body);return sent.length;}) as typeof client.send;
 const workspace={id:"workspace",host_id:"host",title:"Workspace",root_dir:"/project"};
 try {
  model.update({workspaces:[workspace],activeWorkspace:workspace,surfaces:[{id:"home",kind:"welcome",title:"Home",workspaceId:workspace.id},{id:"chat-tab",kind:"agent",title:"Chat",sessionId:"chat",workspaceId:workspace.id}],activeSurfaceId:"home"});
  const layout=model.getLayout();layout.focusSurface("home");
  // PaneLayoutView focuses the layout before invoking its model onFocus.
  layout.focusSurface("chat-tab");expect(model.state.activeSurfaceId).toBe("chat-tab");
  model.selectSurface("chat-tab",true);
  expect(sent.filter(message=>message.GetHistory)).toEqual([{GetHistory:{session_id:"chat",cursor:null,limit:200}}]);
  model.selectSurface("chat-tab",true);
  expect(sent.filter(message=>message.GetHistory)).toHaveLength(1);
 }finally{model.dispose();client.stop();}
});
test("older pages retain order and cursor through head refresh",()=>{
 const config=loadConfig(["--config","/nonexistent/pagination-cache"],{});
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 let requestId=0;client.send=(()=>++requestId) as typeof client.send;
 const row=(id:string,text=id)=>({id,role:"user",kind:"User",text});
 const delivered:Array<{message:any;kind?:string}>=[];model.subscribeAgentEvents((message,kind)=>delivered.push({message,kind}));
 const page=(messages:ReturnType<typeof row>[],cursor:string|null)=>{(model as any).receive({family:"Agent",requestId,message:{HistoryChunk:{session_id:"chat",messages,next_cursor:cursor}}});};
 try {
  model.sendAgent({GetHistory:{session_id:"chat",cursor:null,limit:2}});page([row("c"),row("d")],"before-c");
  model.sendAgent({GetHistory:{session_id:"chat",cursor:"before-c",limit:2}});page([row("a"),row("b")],"before-a");
  model.sendAgent({GetHistory:{session_id:"chat",cursor:null,limit:2}});page([row("c"),row("d","updated")],"before-c");
  expect(model.state.history.chat.map(m=>m.id)).toEqual(["a","b","c","d"]);
  expect(model.state.history.chat.at(-1)?.text).toBe("updated");
  expect(model.historyCursors.get("chat")).toBe("before-a");
  expect(delivered.at(-1)?.kind).toBe("head");
  expect(delivered.at(-1)?.message.HistoryChunk.messages.map((m:any)=>m.id)).toEqual(["a","b","c","d"]);
  expect(delivered.at(-1)?.message.HistoryChunk.next_cursor).toBe("before-a");
  model.sendAgent({GetHistory:{session_id:"chat",cursor:"before-a",limit:2}});page([row("zero"),row("a")],null);
  expect(model.state.history.chat.map(m=>m.id)).toEqual(["zero","a","b","c","d"]);
  expect(model.historyCursors.get("chat")).toBeNull();
  expect(delivered.at(-1)?.kind).toBe("older");
  expect(delivered.at(-1)?.message.HistoryChunk.messages.map((m:any)=>m.id)).toEqual(["zero","a"]);
 }finally{model.dispose();client.stop();}
});

test("history requests serialize per session despite unrelated envelope ids",async()=>{
 const config=loadConfig(["--config","/nonexistent/pagination-cache"],{});
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const sent:unknown[]=[];client.send=((_family,body)=>{sent.push(body);return sent.length;}) as typeof client.send;
 const kinds:Array<string|undefined>=[];model.subscribeAgentEvents((_message,kind)=>kinds.push(kind));
 const request=(cursor:string|null)=>model.sendAgent({GetHistory:{session_id:"chat",cursor,limit:2}});
 const page=(id:string,next_cursor:string|null)=>(model as any).receive({family:"Agent",requestId:999,message:{HistoryChunk:{session_id:"chat",messages:[{id,role:"User",kind:"User",text:id}],next_cursor}}});
 try {
  request(null);request("older");expect(sent).toHaveLength(1);
  page("new","older");await Promise.resolve();expect(sent).toHaveLength(2);
  page("old",null);expect(model.state.history.chat.map(m=>m.id)).toEqual(["old","new"]);
  expect(kinds).toEqual(["head","older"]);
  request("even-older");request(null);expect(sent).toHaveLength(3);
  page("oldest",null);await Promise.resolve();expect(sent).toHaveLength(4);
  page("new", "older");expect(kinds).toEqual(["head","older","older","head"]);
  expect(model.state.history.chat.map(m=>m.id)).toEqual(["oldest","old","new"]);
  expect(model.historyCursors.get("chat")).toBeNull();
 }finally{model.dispose();client.stop();}
});
