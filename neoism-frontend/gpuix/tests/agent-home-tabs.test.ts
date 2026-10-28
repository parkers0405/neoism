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
