import { afterEach, expect, test } from "bun:test";
import { createAgentDaemon } from "./support/agent-daemon";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";
import {coreCall} from "../src/native-core";
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).reverse().forEach((close) => close()));
const until = async (predicate: () => boolean) => {
  const limit = Date.now() + 3000;
  while (!predicate()) { if (Date.now() > limit) throw new Error("Agent fixture did not converge"); await Bun.sleep(5); }
};
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={}) {
  const server = createAgentDaemon(options);
  const { requests, push, workspaces, threads, creations } = server;
  cleanups.push(server.stop);
  const config = { ...loadConfig(["--config", "/nonexistent/test-config"], {}), endpoint: server.endpoint, authToken: undefined };
  const client = new DaemonConnection(config), model = new WorkspaceModel(client, config);
  cleanups.push(() => { model.dispose(); client.stop(); });
  model.start(); client.start();
  await until(() => !model.state.loading && !model.state.catalogsLoading && model.state.providers.length > 0 && model.state.threads.some(thread=>thread.session_id==="one"));
  // Startup now selects the independent Welcome tab. These tests exercise
  // a viewed conversation, so open that thread explicitly before its state.
  model.openThread(threads[0]);
  await until(() => model.state.activeSurfaceId === "agent:one" && model.selection("one").thinking === "medium");
  return { model, requests, push, workspaces, threads, creations, prompts:server.prompts,statuses:server.statuses,statusRequests:server.statusRequests,httpRequests:server.httpRequests };
}
const finishedRuntimeSnapshot=(sessionId:string)=>({RuntimeSnapshot:{session_id:sessionId,snapshot:{rootSessionId:sessionId,familyRevision:1,branchesAuthoritative:true,execution:{executionId:"previous-run",rootSessionId:sessionId,rootMessageId:"previous-message",completedMs:1000,activeSegments:{},sessionActivities:{},revision:1,finished:true},branches:[]}}});
// Wait for frontend consumption, not merely the fixture returning HTTP bytes.
const statusCheckFinished=(model:WorkspaceModel)=>(model as unknown as {promptStatusChecks:Set<string>}).promptStatusChecks.size===0;

test("catalogs and live provider selections stay scoped to their conversations", async () => {
  const { model, requests, threads, push } = await fixture();
  expect(model.state.providers[0].models?.[0].id).toBe("base"); expect(model.state.agents).toHaveLength(2);
  model.setSelection("model", "openai/changed", "one");
  model.openThread(threads[1]); await until(() => model.selection("two").thinking === "medium");
  expect(model.selection("one").model).toBe("openai/changed"); expect(model.selection("two").model).toBe("other/second");
  model.setSelection("thinking", "high", "two");
  await until(() => requests.some((r) => r.tag === "UpdateSession" && r.body.session_id === "two"));
  expect(requests.find((r) => r.tag === "UpdateSession" && r.body.session_id === "two")?.body).toEqual({ session_id: "two", agent: "plan", model: { providerId: "other", id: "second", variant: "high" } });
  push({ ProviderState: { session_id: "one", agent: "build", model: "openai/canonical", thinking: "low" } });
  await until(() => model.selection("one").model === "openai/canonical"); expect(model.selection("two").thinking).toBe("high");
  push({ ContentDelta: { session_id: "one", message_id: "reason", text: "working", kind: "Reasoning" } });
  await until(() => !!model.state.history.one?.length); expect(model.state.history.one[0].kind).toBe("Reasoning");
  expect(requests.some((r) => r.tag === "ResumeStream" && r.body.session_id === "two")).toBe(true);
});
test("questions preserve session identities and failed answers remain retryable", async () => {
  const { model, requests, push } = await fixture();
  const request = { id: "question", sessionID: "spoofed", questions: [{ question: "Proceed?", options: ["Yes", "No"] }] };
  push({ QuestionAsked: { session_id: "one", request } }); push({ QuestionAsked: { session_id: "two", request } });
  await until(() => model.state.questions.length === 2);
  const question = model.state.questions.find((q) => q.sessionId === "one")!;
  expect(question.request.sessionID).toBe("one"); model.answerQuestion(question, [["Yes"]]);
  await until(() => requests.some((r) => r.tag === "AnswerQuestion"));
  expect(requests.find((r) => r.tag === "AnswerQuestion")?.body).toEqual({ request_id: "question", session_id: "one", answers: [["Yes"]] });
  expect(model.state.questions.find((q) => q.sessionId === "two")?.responding).toBe(false);
  await until(() => !!model.state.questions[0]?.error); expect(model.state.questions.find((q) => q.sessionId === "one")?.responding).toBe(false);
  expect(model.state.questions.find((q) => q.sessionId === "two")?.error).toBeUndefined();
  push({ QuestionRemoved: { session_id: "two", request_id: "question" } });
  await until(() => model.state.questions.length === 1); expect(model.state.questions[0].sessionId).toBe("one");
});
test("a delayed new conversation preserves the submitted workspace, options, and attachment bytes", async () => {
  const { model, requests, creations, workspaces } = await fixture();
  model.setSelection("agent", "plan"); model.setSelection("model", "openai/chosen"); model.setSelection("thinking", "high");
  const submission = model.submitNewPrompt("Look at this", { attachments: [{ kind: "image/png", path: "capture.png", bytes: [1, 2, 3] }], delivery: "queue" });
  await until(() => creations.length === 1);
  await model.activateWorkspace(workspaces[1]); creations.shift()!(); expect(await submission).toBe(true);
  await until(() => requests.some((r) => r.tag === "SubmitPrompt"));
  const prompt = requests.find((r) => r.tag === "SubmitPrompt")!.body;
  expect(model.state.activeWorkspace?.id).toBe("b"); expect(model.state.activeSurfaceId).not.toBe("agent:created-a");
  expect(prompt.session_id).toBe("created-a"); expect(prompt.mode).toBe("plan"); expect(prompt.model).toBe("openai/chosen"); expect(prompt.thinking).toBe("high");
  expect(prompt.attachments).toEqual([{ kind: "image/png", path: "capture.png", bytes: [1, 2, 3] }]); expect(prompt.delivery).toBe("queue");
});

test("queue admission removes only the unconfirmed local echo and canonical messages deduplicate",async()=>{
 const {model,requests,push}=await fixture();
 await until(()=>model.state.history.one!==undefined);
 const timelineEvents:unknown[]=[];const off=model.subscribeAgentEvents(event=>timelineEvents.push(event));
 try{
   expect(await model.submitPrompt("one","Wait for this",{delivery:"queue"})).toBe(true);
   expect(model.state.streaming.one||"Idle").toBe("Idle");
  // Steer can also queue on the server, even before this client learns it is busy.
  expect(await model.submitPrompt("one","Also wait",{delivery:"steer"})).toBe(true);
  const sends=requests.filter(r=>r.tag==="SubmitPrompt"),ids=sends.map(r=>String(r.body.message_id));
   expect(model.state.history.one.some(row=>row.id===ids[1])).toBe(true);
   push({SessionEvent:{session_id:"one",kind:"session.queue.updated",properties:{action:"enqueue",messageID:ids[1],queue:{count:2,preview:"Wait for this"}}}});
  await until(()=>model.state.queues.one?.count===2);
   expect(model.state.history.one.filter(row=>ids.includes(row.id))).toEqual([]);
   expect(model.state.streaming.one).toBe("Idle");
   expect(timelineEvents.some((event:any)=>event.MessageUpdated&&event.MessageUpdated.message.id===ids[0])).toBe(false);
  const message={id:ids[0],role:"User",kind:"User",text:"Wait for this"};
  push({QueueUpdate:{session_id:"one",count:1,preview:"Also wait"}});
  push({MessageUpdated:{session_id:"one",message}});
  await until(()=>model.state.history.one.some(row=>row.id===ids[0]));
  push({MessageUpdated:{session_id:"one",message}});
  model.clearQueue("one");await until(()=>model.state.queues.one?.count===0);
  expect(model.state.history.one.filter(row=>row.id===ids[0])).toHaveLength(1);
  expect(model.state.history.one.some(row=>row.id===ids[1])).toBe(false);
 }finally{off();}
});

for(const state of ["Generating","Thinking"])for(const queued of [false,true])test(`busy ${state} send never flashes a local bubble before ${queued?"queue admission":"direct steering consumption"}`,async()=>{
 const {model,requests,push,prompts}=await fixture({deferPrompts:true});
 await until(()=>model.state.history.one!==undefined);
 push({StreamingState:{session_id:"one",state,label:null}});
 await until(()=>model.state.streaming.one===state);
 const events:any[]=[];const off=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const before=model.state.history.one;
  const submitted=model.submitPrompt("one","Follow up while talking");
  expect(model.state.history.one).toBe(before);
  expect(events.some(event=>event.MessageUpdated)).toBe(false);
  expect(model.state.streaming.one).toBe(state);
  await until(()=>prompts.length===1);
  const request=requests.find(request=>request.tag==="SubmitPrompt")!.body;
  const id=String(request.message_id);
  expect(request.delivery).toBe("steer");
  if(queued){
   push({SessionEvent:{session_id:"one",kind:"session.queue.updated",properties:{action:"enqueue",messageID:id,queue:{count:1,preview:"Follow up while talking",primaryMessageID:"active-turn"}}}});
   await until(()=>model.state.queues.one?.count===1);
   expect(model.state.history.one).toBe(before);
   expect(events.some(event=>event.MessageUpdated||event.PartRemoved)).toBe(false);
   expect(model.state.streaming.one).toBe(state);
  }
  prompts.shift()!();expect(await submitted).toBe(true);
  expect(model.state.history.one).toBe(before);
  // Only actual consumption adds a bubble. Also cover a turn finishing
  // between the local busy check and server admission.
  if(!queued&&state==="Generating"){
   push({SessionIdle:{session_id:"one"}});
   await until(()=>model.state.streaming.one==="Idle");
   expect(model.state.history.one).toBe(before);
  }
  if(queued)push({QueueUpdate:{session_id:"one",count:0}});
  const message={id,role:"User",kind:"User",text:"Follow up while talking"};
  push({MessageUpdated:{session_id:"one",message}});
  await until(()=>model.state.history.one.some(row=>row.id===id));
  push({MessageUpdated:{session_id:"one",message}});
  push({ProviderState:{session_id:"one",agent:"consumed-barrier"}});
  await until(()=>model.selection("one").agent==="consumed-barrier");
  expect(model.state.history.one.filter(row=>row.id===id)).toHaveLength(1);
  expect(events.some(event=>event.MessageUpdated?.local_echo||event.PartRemoved)).toBe(false);
 }finally{off();}
});

test("a rejected busy send reports its error without flashing a bubble or stopping the active response",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true,promptError:"Rejected busy send"});
 await until(()=>model.state.history.one!==undefined);
 push({StreamingState:{session_id:"one",state:"Thinking",label:null}});
 await until(()=>model.state.streaming.one==="Thinking");
 const before=model.state.history.one;
 const submitted=model.submitPrompt("one","Do not flash on rejection");
 expect(model.state.history.one).toBe(before);
 await until(()=>prompts.length===1);
  prompts.shift()!();expect(await submitted).toBe(false);
 expect(model.state.history.one.filter(row=>row.kind!=="System")).toEqual([...before]);
 expect(model.state.history.one.at(-1)?.text).toBe("Rejected busy send");
 expect(model.state.streaming.one).toBe("Thinking");
});

for(const echoFirst of [false,true])for(const count of [0,1])test(`idle prompt admission keeps its bubble and activity ${echoFirst?"after":"before"} canonical echo with ${count} waiting`,async()=>{
 const {model,requests,push,prompts}=await fixture({deferPrompts:true});
 await until(()=>model.state.history.one!==undefined);
 const send=model.submitPrompt("one","Start now");
 await until(()=>prompts.length===1);
 const id=String(requests.find(request=>request.tag==="SubmitPrompt")!.body.message_id);
 const message=model.state.history.one.find(row=>row.id===id)!;
 if(echoFirst)push({MessageUpdated:{session_id:"one",message}});
 push({SessionEvent:{session_id:"one",kind:"session.prompt.admitted",properties:{request:{messageId:id}}}});
 push({SessionEvent:{session_id:"one",kind:"session.queue.updated",properties:{action:"enqueue",messageID:id,queue:{count,primaryMessageID:id,running:false,worker:true}}}});
 push({QueueUpdate:{session_id:"one",count}});
 await until(()=>model.state.queues.one?.count===count);
 expect(model.state.history.one.filter(row=>row.id===id)).toHaveLength(1);
 expect(model.state.streaming.one).toBe("Generating");
 if(!echoFirst)push({MessageUpdated:{session_id:"one",message}});
 push({StreamingState:{session_id:"one",state:"Generating",label:null}});
 push({SessionEvent:{session_id:"one",kind:"session.queue.updated",properties:{action:"dequeue",removed:1,queue:{count,primaryMessageID:id,running:true,worker:true}}}});
 prompts.shift()!();expect(await send).toBe(true);
 expect(model.state.history.one.filter(row=>row.id===id)).toHaveLength(1);
 expect(model.state.streaming.one).toBe("Generating");
 expect(model.state.queues.one?.count).toBe(count);
});

test("existing chat prompts paint synchronously before a delayed response and roll back on rejection",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true,promptError:"Rejected send"});
 await until(()=>model.state.history.one!==undefined);
 const events:any[]=[];const off=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const submitted=model.submitPrompt("one","Instant bubble");
  const echo=model.state.history.one.find(row=>row.text==="Instant bubble")!;
   expect(echo).toBeDefined();expect(events.some(event=>event.MessageUpdated?.message.id===echo.id)).toBe(true);
   expect(model.state.streaming.one).toBe("Generating");
   expect(events.findIndex(event=>event.StreamingState?.state==="Generating")).toBeLessThan(events.findIndex(event=>event.MessageUpdated?.message.id===echo.id));
   await until(()=>prompts.length===1);expect(model.state.history.one.some(row=>row.id===echo.id)).toBe(true);
   push({SessionIdle:{session_id:"one"}});
   push({ProviderState:{session_id:"one",agent:"rejection-barrier"}});
   await until(()=>model.selection("one").agent==="rejection-barrier");
   expect(model.state.streaming.one).toBe("Generating");
  prompts.shift()!();expect(await submitted).toBe(false);
  expect(model.state.history.one.some(row=>row.id===echo.id)).toBe(false);
   expect(events.some(event=>event.PartRemoved?.part_id===echo.id)).toBe(true);
   expect(model.state.streaming.one).toBe("Idle");
 }finally{off();}
});

test("delayed Crafting state takes over the local reservation without an Idle gap, including canonical echo first",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true});
 await until(()=>model.state.history.one!==undefined);
 const states:string[]=[];const off=model.subscribeAgentEvents((event:any)=>{if(event.StreamingState?.session_id==="one")states.push(event.StreamingState.state);});
 try{
  const submitted=model.submitPrompt("one","Reserve the first frame");
  const echo=model.state.history.one.find(row=>row.text==="Reserve the first frame")!;
  expect(model.state.streaming.one).toBe("Generating");
  await until(()=>prompts.length===1);prompts.shift()!();expect(await submitted).toBe(true);
  expect(model.state.streaming.one).toBe("Generating");
  push({MessageUpdated:{session_id:"one",message:echo}});
  await Bun.sleep(25);
  push({StreamingState:{session_id:"one",state:"Generating",label:null}});
  await until(()=>states.length===2);
  expect(states).toEqual(["Generating","Generating"]);
  push({SessionIdle:{session_id:"one"}});
  await until(()=>model.state.streaming.one==="Idle");
 }finally{off();}
});

test("rejected steering cannot clear authoritative activity already received",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true,promptError:"Rejected send"});
 const submitted=model.submitPrompt("one","Reject only this send");
 await until(()=>prompts.length===1);
 push({StreamingState:{session_id:"one",state:"Thinking",label:null}});
 await until(()=>model.state.streaming.one==="Thinking");
 prompts.shift()!();expect(await submitted).toBe(false);
 expect(model.state.streaming.one).toBe("Thinking");
});

for(const first of [false,true])for(const afterResponse of [false,true])test(`${first?"first":"existing"} send owns activity across delayed attach Idle and runtime hydration ${afterResponse?"after HTTP success":"before admission"}`,async()=>{
 const {model,creations,prompts,push,requests,statuses}=await fixture({deferPrompts:true});
 await until(()=>model.state.history.one!==undefined);
 if(first)model.newAgentHome();
 const events:any[]=[];const off=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const submitted=first?model.submitNewPrompt("Hydration race"):model.submitPrompt("one","Hydration race");
  if(first){await until(()=>creations.length===1);creations.shift()!();}
  await until(()=>prompts.length===1);
  const sessionId=first?"created-a":"one";
  if(afterResponse){
   statuses.set(sessionId,{type:"busy"});prompts.shift()!();expect(await submitted).toBe(true);
   const echo=model.state.history[sessionId].find(row=>row.text==="Hydration race")!;
   push({MessageUpdated:{session_id:sessionId,message:echo}});
  }
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId,rootSessionId:sessionId,theme:"pastel_dark",scale:1,reducedMotion:true});
  const nativeEvent=(message:unknown)=>coreCall("agenttimeline.event",{id,message});
  nativeEvent({StreamingState:{session_id:sessionId,state:"Generating",label:null}});
  const nativeOff=model.subscribeAgentEvents(nativeEvent);
  cleanups.push(()=>{nativeOff();coreCall("agenttimeline.dispose",{id});});
  const nativeText=()=>coreCall<{commands:Array<{kind:string;text?:string}>}>("agenttimeline.frame",{id}).commands.filter(c=>c.kind==="text").map(c=>c.text||"").join("");
  // The verb itself is scrambled during its arrival animation; the status
  // tail remains literal and is absent when native activity is cleared.
  expect(nativeText()).toContain("model");
  if(first)expect(requests.filter(r=>r.body.session_id===sessionId).map(r=>r.tag)).toEqual(expect.arrayContaining(["SwitchThread","GetHistory","ResumeStream","SubmitPrompt"]));
  const mark=events.length;
  push({SessionIdle:{session_id:sessionId}});
  push({StreamingState:{session_id:sessionId,state:"Idle",label:null}});
  const idleRuntime=finishedRuntimeSnapshot(sessionId);
  push(idleRuntime);
  push({CatalogFamily:{session_id:sessionId,snapshot:{directory:"/project/a",familyRevision:1,executionId:null,branches:[]}}});
  // Ordered wire barrier: no timing assumption about websocket delivery.
  push({ProviderState:{session_id:sessionId,agent:"hydrated"}});
  await until(()=>model.selection(sessionId).agent==="hydrated");
  expect(model.state.streaming[sessionId]).toBe("Generating");
  expect(events.slice(mark).some(e=>e.SessionIdle||e.StreamingState?.state==="Idle"||e.RuntimeSnapshot)).toBe(false);
  // CatalogFamily is metadata, not a streaming lifecycle edge. Keep routing it.
  expect(events.slice(mark).some(e=>e.CatalogFamily)).toBe(true);
  expect(nativeText()).toContain("model");
  // Control: forwarding that same finished previous execution straight to
  // native really does clear the newly reserved activity (independent of Idle).
  nativeEvent(idleRuntime);
  expect(nativeText()).not.toContain("model");
  push({StreamingState:{session_id:sessionId,state:"Thinking",label:null}});
  await until(()=>model.state.streaming[sessionId]==="Thinking");
  // Completion is real even if HTTP is still outstanding.
  push({SessionIdle:{session_id:sessionId}});
  await until(()=>model.state.streaming[sessionId]==="Idle");
  if(!afterResponse){prompts.shift()!();expect(await submitted).toBe(true);}
  expect(model.state.streaming[sessionId]).toBe("Idle");
 }finally{off();}
});

test("matching admission permits no-busy completion while HTTP is still pending",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true});
 const submitted=model.submitPrompt("one","No reply required");
 await until(()=>prompts.length===1);
 const messageId=model.state.history.one.find(row=>row.text==="No reply required")!.id;
 push({SessionIdle:{session_id:"one"}});
 push({SessionEvent:{session_id:"one",kind:"session.prompt.admitted",properties:{request:{messageId},delivery:"steer"}}});
 push({SessionIdle:{session_id:"one"}});
 await until(()=>model.state.streaming.one==="Idle");
 prompts.shift()!();expect(await submitted).toBe(true);
 expect(model.state.streaming.one).toBe("Idle");
});

for(const admitted of [false,true])test(`${admitted?"matching admission":"returned busy GET"} retains protection against old runtime before live busy`,async()=>{
 const {model,prompts,push,statuses,statusRequests}=await fixture({deferPrompts:true});
 const events:any[]=[];const off=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const submitted=model.submitPrompt("one","Keep ownership");
  await until(()=>prompts.length===1);
  const messageId=model.state.history.one.find(row=>row.text==="Keep ownership")!.id;
  statuses.set("one",{type:"busy"});
  if(admitted){
   push({SessionEvent:{session_id:"one",kind:"session.prompt.admitted",properties:{request:{messageId},delivery:"steer"}}});
  }else{
   push({SessionIdle:{session_id:"one"}});
   push({ProviderState:{session_id:"one",agent:"read-barrier"}});
   await until(()=>model.selection("one").agent==="read-barrier");
   prompts.shift()!();expect(await submitted).toBe(true);
   await until(()=>statusRequests.count===1&&statusCheckFinished(model));
  }
  const mark=events.length;
  push(finishedRuntimeSnapshot("one"));
  push({SessionIdle:{session_id:"one"}});
  push({ProviderState:{session_id:"one",agent:"snapshot-barrier"}});
  await until(()=>model.selection("one").agent==="snapshot-barrier");
  await until(()=>statusRequests.count>=(admitted?1:2)&&statusCheckFinished(model));
  expect(model.state.streaming.one).toBe("Generating");
  expect(events.slice(mark).some(e=>e.RuntimeSnapshot||e.SessionIdle||e.StreamingState?.state==="Idle")).toBe(false);
  push({StreamingState:{session_id:"one",state:"Thinking",label:null}});
  await until(()=>model.state.streaming.one==="Thinking");
  push({SessionIdle:{session_id:"one"}});
  await until(()=>model.state.streaming.one==="Idle");
  if(admitted){prompts.shift()!();expect(await submitted).toBe(true);}
 }finally{off();}
});

test("completion during an in-flight busy GET schedules one fresh idle recheck before HTTP prompt returns",async()=>{
 const {model,prompts,push,statuses,statusRequests}=await fixture({deferPrompts:true});
 statusRequests.defer=true;statuses.set("one",{type:"busy"});
 const submitted=model.submitPrompt("one","Finish without live busy");
 await until(()=>prompts.length===1);
 const messageId=model.state.history.one.find(row=>row.text==="Finish without live busy")!.id;
 push({SessionEvent:{session_id:"one",kind:"session.prompt.admitted",properties:{request:{messageId},delivery:"steer"}}});
 push({SessionIdle:{session_id:"one"}});
 await until(()=>statusRequests.pending.length===1);
 statuses.set("one",{type:"idle"});
 for(let i=0;i<3;i++)push({SessionIdle:{session_id:"one"}});
 push({ProviderState:{session_id:"one",agent:"completion-barrier"}});
 await until(()=>model.selection("one").agent==="completion-barrier");
 expect(statusRequests.count).toBe(1);
 expect(model.state.streaming.one).toBe("Generating");
 statusRequests.pending.shift()!(); // Captured busy before completion.
 await until(()=>statusRequests.count===2&&statusRequests.pending.length===1);
 expect(model.state.streaming.one).toBe("Generating");
 statusRequests.pending.shift()!(); // Fresh idle after completion.
 await until(()=>model.state.streaming.one==="Idle"&&statusCheckFinished(model));
 expect(statusRequests.count).toBe(2);
 prompts.shift()!();expect(await submitted).toBe(true);
 expect(model.state.streaming.one).toBe("Idle");
});

for(const liveBusy of [false,true])test(`matching runtime completion is accepted ${liveBusy?"after live busy with old finished runtime rejected":"without any live busy edge"}`,async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true});
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"one",rootSessionId:"one",theme:"pastel_dark",scale:1});
 const events:any[]=[];
 const off=model.subscribeAgentEvents(event=>{events.push(event);coreCall("agenttimeline.event",{id,message:event});});
 const nativeText=()=>coreCall<{commands:Array<{kind:string;text?:string}>}>("agenttimeline.frame",{id}).commands.filter(c=>c.kind==="text").map(c=>c.text||"").join("");
 try{
  const submitted=model.submitPrompt("one","Own this runtime");
  await until(()=>prompts.length===1);
  const messageId=model.state.history.one.find(row=>row.text==="Own this runtime")!.id;
  if(liveBusy){
   push({StreamingState:{session_id:"one",state:"Thinking",label:null}});
   await until(()=>model.state.streaming.one==="Thinking");
   const mark=events.length;
   push(finishedRuntimeSnapshot("one"));
   push({ProviderState:{session_id:"one",agent:"late-runtime-barrier"}});
   await until(()=>model.selection("one").agent==="late-runtime-barrier");
   expect(events.slice(mark).some(e=>e.RuntimeSnapshot)).toBe(false);
   expect(nativeText()).toContain("model");
   expect(model.state.streaming.one).toBe("Thinking");
  }
  const current=finishedRuntimeSnapshot("one");
  current.RuntimeSnapshot.snapshot.execution.rootMessageId=messageId;
  current.RuntimeSnapshot.snapshot.execution.executionId="current-run";
  push(current);
  await until(()=>model.state.streaming.one==="Idle");
  expect(events.some(e=>e.RuntimeSnapshot?.snapshot.execution?.rootMessageId===messageId)).toBe(true);
  expect(nativeText()).not.toContain("model");
  prompts.shift()!();expect(await submitted).toBe(true);
  expect(model.state.streaming.one).toBe("Idle");
 }finally{off();coreCall("agenttimeline.dispose",{id});}
});

test("steering an already-busy session does not fence its existing execution identity",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true});
 push({StreamingState:{session_id:"one",state:"Thinking",label:null}});
 await until(()=>model.state.streaming.one==="Thinking");
 const events:any[]=[];const off=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const submitted=model.submitPrompt("one","Steer the current run");
  await until(()=>prompts.length===1);
  push(finishedRuntimeSnapshot("one"));
  push({ProviderState:{session_id:"one",agent:"steer-barrier"}});
  await until(()=>model.selection("one").agent==="steer-barrier");
  expect(events.some(e=>e.RuntimeSnapshot)).toBe(true);
  push({SessionIdle:{session_id:"one"}});
  await until(()=>model.state.streaming.one==="Idle");
  prompts.shift()!();expect(await submitted).toBe(true);
 }finally{off();}
});

test("status-read failure is not completion, while an explicit session error still settles the guard",async()=>{
 const {model,prompts,push,statusRequests}=await fixture({deferPrompts:true});
 statusRequests.fail=true;
 const submitted=model.submitPrompt("one","Do not infer completion");
 await until(()=>prompts.length===1);
 prompts.shift()!();expect(await submitted).toBe(true);
 push({SessionIdle:{session_id:"one"}});
 await until(()=>statusRequests.count===1&&statusCheckFinished(model));
 expect(model.state.streaming.one).toBe("Generating");
 push({SessionEvent:{session_id:"one",kind:"session.error",properties:{error:"Real turn failure"}}});
 await until(()=>model.state.streaming.one==="Idle");
 expect(model.state.streaming.one).toBe("Idle");
});

test("queue admission releases a reservation despite delayed pre-admission Idle",async()=>{
 const {model,prompts,push}=await fixture({deferPrompts:true});
 const submitted=model.submitPrompt("one","Actually queued");
 await until(()=>prompts.length===1);
 const messageID=model.state.history.one.find(row=>row.text==="Actually queued")!.id;
 push({SessionIdle:{session_id:"one"}});
 push({SessionEvent:{session_id:"one",kind:"session.queue.updated",properties:{action:"enqueue",messageID,queue:{count:1,preview:"Actually queued"}}}});
 await until(()=>model.state.queues.one?.count===1);
 expect(model.state.streaming.one).toBe("Idle");
 expect(model.state.history.one.some(row=>row.id===messageID)).toBe(false);
 prompts.shift()!();expect(await submitted).toBe(true);
 expect(model.state.streaming.one).toBe("Idle");
});

for(const busy of [false,true])test(`suppressed hydration reconciles fresh ${busy?"busy":"idle"} after HTTP acceptance without replaying stale Idle`,async()=>{
 const {model,prompts,push,statuses,httpRequests}=await fixture({deferPrompts:true});
 const submitted=model.submitPrompt("one","Status fallback");
 await until(()=>prompts.length===1);
 push({SessionIdle:{session_id:"one"}});
 push({ProviderState:{session_id:"one",agent:"barrier"}});
 await until(()=>model.selection("one").agent==="barrier");
 expect(model.state.streaming.one).toBe("Generating");
 statuses.set("one",{type:busy?"busy":"idle"});
 prompts.shift()!();expect(await submitted).toBe(true);
 await until(()=>httpRequests.some(r=>r.path.endsWith("/sessions/status")));
 if(!busy)await until(()=>model.state.streaming.one==="Idle");
 else{
  push({StreamingState:{session_id:"one",state:"Thinking",label:null}});
  await until(()=>model.state.streaming.one==="Thinking");
  push({SessionIdle:{session_id:"one"}});
  await until(()=>model.state.streaming.one==="Idle");
 }
 expect(model.state.streaming.one).toBe("Idle");
});

test("new-home activity transfers with its echo and rejected admission clears the adopted reservation",async()=>{
 const {model,creations,prompts}=await fixture({deferPrompts:true,promptError:"Rejected first send"});
 model.newAgentHome();const home=model.state.activeSurfaceId!;
 const submitted=model.submitNewPrompt("First frame");
 const pending=model.state.pendingAgentSessions[home];
 expect(pending).toBeDefined();expect(model.state.streaming[pending]).toBe("Generating");
 await until(()=>creations.length===1);creations.shift()!();
 await until(()=>prompts.length===1);
 expect(model.state.streaming["created-a"]).toBe("Generating");
 expect(model.state.streaming[pending]).toBeUndefined();
 prompts.shift()!();expect(await submitted).toBe(false);
 expect(model.state.streaming["created-a"]).toBe("Idle");
 expect(Object.values(model.state.history).flat().some(row=>row.text==="First frame")).toBe(false);
});

test("live optimistic arrival seeds once and adoption never emits the same local echo twice",async()=>{
 const {model,creations,prompts}=await fixture({deferPrompts:true});
 model.newAgentHome();const home=model.state.activeSurfaceId!;
 const events:any[]=[];const off=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const submitted=model.submitNewPrompt("One arrival");const pending=model.state.pendingAgentSessions[home];
  const arrivals=model.takePromptArrivals(pending);expect(arrivals.map(message=>message.text)).toEqual(["One arrival"]);
  expect(model.takePromptArrivals(pending)).toEqual([]);
  await until(()=>creations.length===1);creations.shift()!();await until(()=>prompts.length===1);
  expect(events.filter(event=>event.MessageUpdated?.local_echo&&event.MessageUpdated.message.id===arrivals[0].id)).toHaveLength(1);
  expect(events.some(event=>event.LocalSessionAdopted?.from===pending&&event.LocalSessionAdopted.to==="created-a")).toBe(true);
  expect(model.takePromptArrivals("created-a")).toEqual([]);
  prompts.shift()!();expect(await submitted).toBe(true);
 }finally{off();}
});

test("explicitly queued first prompts do not create provisional timeline messages",async()=>{
 const {model,requests,creations}=await fixture();
 model.newAgentHome();const submission=model.submitNewPrompt("Queue first",{delivery:"queue"});
 await until(()=>creations.length===1);
 expect(Object.values(model.state.history).flat().some(row=>row.text==="Queue first")).toBe(false);
 creations.shift()!();expect(await submission).toBe(true);
 const id=String(requests.find(r=>r.tag==="SubmitPrompt")!.body.message_id);
 expect(Object.values(model.state.history).flat().some(row=>row.id===id)).toBe(false);
});
