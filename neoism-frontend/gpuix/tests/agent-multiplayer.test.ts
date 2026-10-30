import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {coreCall} from "../src/native-core";
import {createAgentDaemon} from "./support/agent-daemon";
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){if(Date.now()>end)throw new Error("Shared Agent clients did not converge");await Bun.sleep(5);}};
test("two independent Agent clients share one live chat; closing one preserves the other's subscription and authorship",async()=>{
 const daemon=createAgentDaemon({deferCreations:false});
 const config={...loadConfig(["--config","/nonexistent/agent-multiplayer"],{}),endpoint:daemon.endpoint};
 const ca=new DaemonConnection(config),cb=new DaemonConnection(config);
 const a=new WorkspaceModel(ca,{...config,presenceDisplayName:"Alice"}),b=new WorkspaceModel(cb,{...config,presenceDisplayName:"Bob"});
 const ea=coreCall<{id:number}>("agenttimeline.new",{sessionId:"one",localName:"Alice",scale:1}).id,eb=coreCall<{id:number}>("agenttimeline.new",{sessionId:"one",localName:"Bob",scale:1}).id;
 const offa=a.subscribeAgentEvents((message,historyKind)=>coreCall("agenttimeline.event",{id:ea,message,historyKind})),offb=b.subscribeAgentEvents((message,historyKind)=>coreCall("agenttimeline.event",{id:eb,message,historyKind}));
 try{
  a.start();b.start();ca.start();cb.start();await until(()=>!!a.state.activeWorkspace&&!a.state.loading&&!!b.state.activeWorkspace&&!b.state.loading);
  a.openThread(daemon.threads[0]);b.openThread(daemon.threads[0]);await until(()=>daemon.streamSubscribers("one")===2);
  expect(await a.submitPrompt("one","Alice question")).toBe(true);expect(await b.submitPrompt("one","Bob question")).toBe(true);
  const prompts=daemon.requests.filter(request=>request.tag==="SubmitPrompt");expect(prompts).toHaveLength(2);expect(prompts.map(request=>request.body.author)).toEqual(["Alice","Bob"]);
  // The server echoes the submitted message ID; optimistic prompts reconcile by ID.
  const [aliceId,bobId]=prompts.map(request=>String(request.body.message_id));
  daemon.pushSubscribed({MessageUpdated:{session_id:"one",parent_message_id:aliceId,message:{id:aliceId,role:"User",kind:"User",text:"Alice question",author:"Alice"}}});
  daemon.pushSubscribed({MessageUpdated:{session_id:"one",parent_message_id:bobId,message:{id:bobId,role:"User",kind:"User",text:"Bob question",author:"Bob"}}});
  for(const piece of ["Shared ","live ","answer"]){daemon.pushSubscribed({ContentDelta:{session_id:"one",message_id:"answer",parent_message_id:"reply",kind:"Text",text:piece}});}
  await until(()=>a.state.history.one?.find(row=>row.id==="answer")?.text==="Shared live answer"&&b.state.history.one?.find(row=>row.id==="answer")?.text==="Shared live answer");
  expect(a.state.history.one).toEqual(b.state.history.one);
  await Bun.sleep(350);
  for(const id of [ea,eb]){const frame=coreCall<{commands:Array<{kind:string;text?:string}>}>("agenttimeline.layout",{id,width:1000,height:1400});const painted=frame.commands.filter(command=>command.kind==="text").map(command=>command.text).join(" ").replace(/\s+/g," ");expect(painted).toContain("Shared live answer");expect(painted).toContain("Alice question");expect(painted).toContain("Bob question");}
  expect(a.closeCurrentTab()).toBe(true);await until(()=>daemon.streamSubscribers("one")===1);
  daemon.pushSubscribed({ContentDelta:{session_id:"one",message_id:"answer",parent_message_id:"reply",kind:"Text",text:" still live"}});
  await until(()=>b.state.history.one?.find(row=>row.id==="answer")?.text==="Shared live answer still live");
  expect(a.state.history.one?.find(row=>row.id==="answer")?.text).toBe("Shared live answer");
  cb.stop();await until(()=>daemon.streamSubscribers("one")===0);cb.start();
  await until(()=>cb.state.phase==="connected"&&!b.state.loading&&daemon.streamSubscribers("one")===1);
  daemon.pushSubscribed({ContentDelta:{session_id:"one",message_id:"answer",parent_message_id:"reply",kind:"Text",text:" after reconnect"}});
  await until(()=>b.state.history.one?.find(row=>row.id==="answer")?.text==="Shared live answer still live after reconnect");

  expect(daemon.requests.filter(request=>request.tag==="SubmitPrompt")).toHaveLength(2);expect(daemon.ptyRequests).toHaveLength(0);
 }finally{offa();offb();a.dispose();b.dispose();ca.stop();cb.stop();daemon.stop();coreCall("agenttimeline.dispose",{id:ea});coreCall("agenttimeline.dispose",{id:eb});}
},10000);
