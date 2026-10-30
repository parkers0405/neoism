import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {createAgentDaemon} from "./support/agent-daemon";
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Agent state did not converge");await Bun.sleep(5);}};
async function fixture(options:Parameters<typeof createAgentDaemon>[0]={}){const daemon=createAgentDaemon(options);daemon.threads.splice(0);const config={...loadConfig(["--config","/nonexistent/agent-home-tabs"],{}),endpoint:daemon.endpoint,authToken:undefined};const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);return{daemon,model,close:()=>{model.dispose();client.stop();daemon.stop();}};}
test("each Alt+A home is a distinct local tab with independent persisted drafts and no server creation",async()=>{
 const {daemon,model,close}=await fixture();try{
  const a=model.newAgentHome()!,b=model.newAgentHome()!;expect(a.id).not.toBe(b.id);expect(model.getLayout().frame.leaves.some(leaf=>leaf.surfaceId===a.id)).toBe(true);expect(model.state.activeSurfaceId).toBe(b.id);
  const aKey=model.composerDraftKey(undefined,a.id),bKey=model.composerDraftKey(undefined,b.id);expect(aKey).not.toBe(bKey);
  model.setComposerDraft(aKey,{text:"first unsent",attachments:[]});model.setComposerDraft(bKey,{text:"second unsent",attachments:[]});
  expect(model.captureSession().composerDrafts[aKey]?.text).toBe("first unsent");expect(model.captureSession().composerDrafts[bKey]?.text).toBe("second unsent");
  model.selectSurface(a.id);expect(model.state.composerDrafts[model.composerDraftKey(undefined,a.id)]?.text).toBe("first unsent");expect(model.composerDraftKey("conversation",a.id)).toBe("conversation");
  expect(daemon.requests.some(request=>request.tag==="CreateThread")).toBe(false);expect(daemon.ptyRequests).toEqual([]);
 }finally{close();}
});
test("fresh catalogs are reused for picker opens and concurrent refreshes share requests",async()=>{
 const {daemon,model,close}=await fixture();try{
  await model.refreshCatalogs();const before=daemon.httpRequests.length;
  await Promise.all([model.refreshCatalogs({maxAgeMs:30000}),model.refreshCatalogs({maxAgeMs:30000})]);
  expect(daemon.httpRequests.length).toBe(before);
  const first=model.refreshCatalogs(),second=model.refreshCatalogs();expect(second).toBe(first);await first;
  expect(daemon.httpRequests.length).toBeGreaterThan(before);
 }finally{close();}
});
test("canonical history order reaches both the model and the native timeline subscribers",async()=>{
 const {daemon,model,close}=await fixture();try{
  const message=(id:string)=>({id,role:"Assistant",kind:id==="thinking"?"Reasoning":"Assistant",text:id});
  daemon.push({HistoryChunk:{session_id:"one",messages:[message("answer"),message("thinking")],next_cursor:null}});
  await until(()=>model.state.history.one?.length===2);
  let delivered:string[]=[];const off=model.subscribeAgentEvents((event:any)=>{if(event.HistoryChunk?.session_id==="one")delivered=event.HistoryChunk.messages.map((row:any)=>row.id);});
  daemon.push({HistoryChunk:{session_id:"one",messages:[message("thinking"),message("answer")],next_cursor:null}});
  await until(()=>delivered.length===2);off();
  expect(model.state.history.one.map(row=>row.id)).toEqual(["thinking","answer"]);expect(delivered).toEqual(["thinking","answer"]);
 }finally{close();}
});
test("a deferred submission adopts its originating home without stealing a newer home or its draft",async()=>{
 const {daemon,model,close}=await fixture();try{
  const source=model.newAgentHome()!,submission=model.submitNewPrompt("source prompt");await until(()=>daemon.creations.length===1);
  const later=model.newAgentHome()!,key=model.composerDraftKey(undefined,later.id);model.setComposerDraft(key,{text:"keep this newer draft",attachments:[]});daemon.creations.shift()!();expect(await submission).toBe(true);
  expect(model.state.activeSurfaceId).toBe(later.id);expect(model.state.surfaces.find(surface=>surface.id===source.id)?.sessionId).toBe("created-a");expect(model.state.surfaces.find(surface=>surface.id===later.id)?.sessionId).toBeUndefined();
  expect(model.state.composerDrafts[key]?.text).toBe("keep this newer draft");expect(daemon.requests.find(request=>request.tag==="SubmitPrompt")?.body).toMatchObject({session_id:"created-a",text:"source prompt"});
 }finally{close();}
});
test("closing a home during submission never adopts another tab when the response arrives",async()=>{
 const {daemon,model,close}=await fixture();try{
  const source=model.newAgentHome()!,submission=model.submitNewPrompt("detached source");await until(()=>daemon.creations.length===1);expect(model.closeCurrentTab()).toBe(true);const later=model.newAgentHome()!;daemon.creations.shift()!();expect(await submission).toBe(true);
  expect(model.state.surfaces.some(surface=>surface.id===source.id)).toBe(false);expect(model.state.activeSurfaceId).toBe(later.id);expect(model.state.surfaces.find(surface=>surface.id===later.id)?.sessionId).toBeUndefined();
 }finally{close();}
});
test("a failed first prompt retains the originating home draft atomically under its new session key",async()=>{
 const {daemon,model,close}=await fixture({promptError:"Prompt rejected"});try{
  const home=model.newAgentHome()!,homeKey=model.composerDraftKey(undefined,home.id);
  model.setComposerDraft(homeKey,{text:"submitted",attachments:[]});const pending=model.submitNewPrompt("submitted");await until(()=>daemon.creations.length===1);
  const later=model.newAgentHome()!,laterKey=model.composerDraftKey(undefined,later.id);model.setComposerDraft(laterKey,{text:"other tab draft",attachments:[]});
  const latest={text:"submitted, retained while waiting",attachments:[{kind:"image/png",path:"capture.png",bytes:[1,2,3],token:"capture",name:"capture.png"}]};model.setComposerDraft(homeKey,latest);
  let firstAdoptedDraft:unknown;const unsubscribe=model.subscribe(()=>{if(firstAdoptedDraft===undefined&&model.state.surfaces.find(surface=>surface.id===home.id)?.sessionId)firstAdoptedDraft=model.state.composerDrafts["created-a"];});
  daemon.creations.shift()!();expect(await pending).toBe(false);unsubscribe();
  expect(firstAdoptedDraft).toEqual(latest);expect(model.state.composerDrafts["created-a"]).toEqual(latest);expect(model.state.composerDrafts[homeKey]).toEqual(latest);
  expect(model.state.composerDrafts[laterKey]?.text).toBe("other tab draft");expect(model.state.activeSurfaceId).toBe(later.id);expect(model.state.error).toContain("Prompt rejected");
 }finally{close();}
});

test("generated titles update canonical tab routes without replacing leaves or stealing focus, including raw session events",async()=>{
 const {daemon,model,close}=await fixture();try{
  const source=model.newAgentHome()!,submission=model.submitNewPrompt("title this conversation");await until(()=>daemon.creations.length===1);daemon.creations.shift()!();expect(await submission).toBe(true);
  const leaf=model.getLayout().frame.leaves.find(leaf=>leaf.surfaceId===source.id)!.leaf;
  const other=model.newAgentHome()!,draftKey=model.composerDraftKey(undefined,other.id);model.setComposerDraft(draftKey,{text:"keep draft",attachments:[]});
  daemon.push({ThreadUpdated:{session_id:"created-a",title:"Generated conversation title",pinned:null}});
  await until(()=>model.getLayout().frame.leaves.find(tab=>tab.leaf===leaf)?.title==="Generated conversation title");
  expect(model.state.surfaces.find(surface=>surface.id===source.id)?.title).toBe("Generated conversation title");expect(model.state.activeSurfaceId).toBe(other.id);
  daemon.push({SessionEvent:{session_id:"created-a",kind:"session.updated",properties:{info:{id:"created-a",title:"Refined title"}}}});
  await until(()=>model.getLayout().frame.leaves.find(tab=>tab.leaf===leaf)?.title==="Refined title");
  expect(model.captureSession().surfaces.find(surface=>surface.id===source.id)?.title).toBe("Refined title");expect(model.state.composerDrafts[draftKey]?.text).toBe("keep draft");
 }finally{close();}
});

test("a generated title arriving before the create response survives home adoption",async()=>{
 const {daemon,model,close}=await fixture();try{
  const home=model.newAgentHome()!,pending=model.submitNewPrompt("first prompt");await until(()=>daemon.creations.length===1);
  daemon.push({ThreadUpdated:{session_id:"created-a",title:"Generated before adoption",pinned:null}});
  // The raw Agent FIFO processes the event before the deferred HTTP response.
  let observed=false;const off=model.subscribeAgentEvents(message=>{if(typeof message==="object"&&message!==null&&"ThreadUpdated"in message)observed=true;});
  await until(()=>observed);off();daemon.creations.shift()!();expect(await pending).toBe(true);
  expect(model.state.surfaces.find(surface=>surface.id===home.id)?.title).toBe("Generated before adoption");
  expect(model.getLayout().frame.leaves.find(tab=>tab.surfaceId===home.id)?.title).toBe("Generated before adoption");
 }finally{close();}
});

test("prompts appear before ACK, reconcile by id, and roll back only unconfirmed sends",async()=>{
 const {daemon,model,close}=await fixture({deferPrompts:true,promptError:"Delayed rejection"});try{
  model.openThread({session_id:"one",title:"One",directory:"/project/a"});await until(()=>model.state.history.one!==undefined);
  let removals=0;const off=model.subscribeAgentEvents((event:any)=>{if(event.PartRemoved)removals++;});
  const first=model.submitPrompt("one","same text"),second=model.submitPrompt("one","same text");
  const local=model.state.history.one.filter(row=>row.text==="same text");expect(local).toHaveLength(2);expect(local[0].id).not.toBe(local[1].id);
  await until(()=>daemon.prompts.length===2);
  daemon.push({MessageUpdated:{session_id:"one",message:{...local[0],author:"canonical"}}});await until(()=>model.state.history.one.some(row=>row.author==="canonical"));
  daemon.prompts.splice(0).forEach(resolve=>resolve());expect(await first).toBe(false);expect(await second).toBe(false);
  expect(model.state.history.one.filter(row=>row.text==="same text")).toEqual([{...local[0],author:"canonical"}]);expect(removals).toBe(1);off();
 }finally{close();}
});
