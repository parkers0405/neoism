import {afterEach,expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {SlashController,type SlashPickerRequest,type SlashEffect,type SlashInputRequest} from "../src/agent/SlashController";
import {createSlashDaemon} from "./support/slash-daemon";
const cleanups:Array<()=>void>=[];afterEach(()=>cleanups.splice(0).reverse().forEach(close=>close()));
const until=async(test:()=>boolean)=>{const limit=Date.now()+2000;while(!test()){if(Date.now()>limit)throw new Error("Slash SDK fixture did not converge");await Bun.sleep(5);}};
async function fixture(){
 const server=createSlashDaemon(),config={...loadConfig(["--config","/nonexistent/slash-fixture"],{}),endpoint:server.endpoint,authToken:"fixture-token"},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};
 model.update({activeWorkspace:workspace,workspaces:[workspace],threads:[{session_id:"one",title:"One",directory:"/project/a"},{session_id:"two",title:"Two",directory:"/project/a"}],surfaces:[{id:"agent:one",kind:"agent",sessionId:"one",title:"One",workspaceId:"a"},{id:"agent:two",kind:"agent",sessionId:"two",title:"Two",workspaceId:"a"}],activeSurfaceId:"agent:one",providers:[{id:"openai",name:"OpenAI",models:[{id:"base",name:"Base"},{id:"advanced",name:"Advanced"}]}],agents:[{name:"build",description:"Build"},{name:"plan",description:"Plan"}],providerStates:{one:{agent:"build",model:"openai/base",thinking:"high",connection_id:"account-a"},two:{agent:"plan",model:"other/second",thinking:"low",connection_id:"account-b"}}});
 client.start();await until(()=>client.state.phase==="connected");
 const notices:Array<{title:string;value:unknown;sessionId?:string}>=[],pickers:SlashPickerRequest[]=[],inserts:string[]=[],effects:SlashEffect[]=[];const inputs:Array<SlashInputRequest>=[],answers:Array<string|undefined>=[],urls:string[]=[];let hints=0,sidebar=0,closed=0,attachments:unknown[]=[],deferInput=false,inputReply:((value:string|undefined)=>void)|undefined;
 const controller=new SlashController({model,config,sessionId:()=>model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.sessionId,attachments:()=>attachments,hooks:{input:async request=>{inputs.push(request);if(deferInput){deferInput=false;return new Promise<string|undefined>(resolve=>{inputReply=resolve;});}return answers.shift();},openExternal:url=>{urls.push(url);},picker:request=>{pickers.push(request);},notice:(title,value,sessionId)=>notices.push({title,value,sessionId}),insert:text=>inserts.push(text),effect:kind=>{effects.push(kind);},toggleHints:()=>{hints++;},toggleSidebar:()=>{sidebar++;},closeTab:()=>{closed++;}}});
 cleanups.push(()=>{inputReply?.(undefined);controller.dispose();model.dispose();client.stop();server.stop();});
 return {server,model,controller,notices,pickers,inserts,effects,inputs,answers,urls,deferNextInput:()=>{deferInput=true;},replyInput:(value:string|undefined)=>{const reply=inputReply;inputReply=undefined;reply?.(value);},get toggles(){return{hints,sidebar,closed};},setAttachments:(next:unknown[])=>{attachments=next;}};
}
test("slash catalog merges actual SDK commands without replacing native builtins or aliases",async()=>{
 const f=await fixture();await f.controller.refresh();const rows=f.controller.state.options;
 expect(rows.filter(row=>row.value==="/compact")).toHaveLength(1);expect(rows.some(row=>row.value==="/models")).toBe(false);expect(rows.find(row=>row.value==="/review")?.description).toBe("Review this repository");
 const request=f.server.requests.find(r=>r.path==="/commands")!;expect(request.directory).toBe("/project/a");expect(request.authorization).toBe("Bearer fixture-token");
 f.server.capabilities.delete("neoism.goals");await f.controller.refresh();expect(f.controller.state.options.some(row=>row.value==="/goal")).toBe(false);
});
test("server commands use the dedicated SDK endpoint and captured per-thread model, account and reasoning",async()=>{
 const f=await fixture(),other={...f.model.selection("two")};
 const first=f.controller.execute("/review src/main.ts --strict"),duplicate=f.controller.execute("/review src/main.ts --strict");expect(duplicate).toBe(first);await first;
 const requests=f.server.requests.filter(r=>r.path==="/sessions/one/commands");expect(requests).toHaveLength(1);expect(requests[0].body).toEqual({command:"review",arguments:"src/main.ts --strict",agent:"build",model:{providerId:"openai",modelId:"base",variant:"high",connectionId:"account-a"}});
 expect(f.server.requests.some(r=>r.path.endsWith("/prompt"))).toBe(false);expect(f.model.selection("two")).toEqual(other);
 f.server.setFailCommand();f.model.setComposerDraft("one",{text:"/review retry",attachments:[]});await expect(f.controller.execute("/review retry")).rejects.toThrow();expect(f.model.state.composerDrafts.one.text).toBe("/review retry");
});
test("compact, undo, redo, queues, goals and directory changes perform their actual SDK mutations",async()=>{
 const f=await fixture();for(const command of ["/compact","/undo","/redo","/queue clear","/queue pop","/abort"])await f.controller.execute(command);
 for(const suffix of ["summarize","undo","redo","queue","queue/pop","abort"])expect(f.server.requests.some(r=>r.path===`/sessions/one/${suffix}`)).toBe(true);
 expect(f.server.requests.some(r=>r.path.endsWith("/prompt"))).toBe(false);
 await f.controller.execute("/goal finish the implementation");await f.controller.execute("/goal pause");await f.controller.execute("/goal resume");await f.controller.execute("/goal clear");
 expect(f.server.requests.find(r=>r.path==="/plugins/dev.neoism.goals/one"&&r.body?.text)?.body).toEqual({text:"finish the implementation"});
 const prompts=f.server.requests.filter(r=>r.path==="/sessions/one/prompt");expect(prompts).toHaveLength(1);expect(prompts[0].body.parts[0].text).toBe("finish the implementation");expect(prompts[0].body.model).toEqual({providerId:"openai",modelId:"base",variant:"high",connectionId:"account-a"});
 await f.controller.execute("/cd /project/a/sub");expect(f.server.sessions.get("one")?.directory).toBe("/project/a/sub");expect(f.model.state.threads.find(t=>t.session_id==="one")?.directory).toBe("/project/a/sub");
});
test("permissions/questions remain session scoped and skip-permission toggle restores prior rules",async()=>{
 const f=await fixture();f.model.update({permissions:[{sessionId:"one",requestId:"p-one",tool:"read",title:"One"},{sessionId:"two",requestId:"p-two",tool:"read",title:"Two"}],questions:[{sessionId:"one",requestId:"q-one",request:{},responding:false},{sessionId:"two",requestId:"q-two",request:{},responding:false}]});
 await f.controller.execute("/permit always p-one");expect(f.server.requests.find(r=>r.path==="/interactions/permissions/p-one/reply")?.body).toEqual({reply:"always"});expect(f.model.state.permissions.map(p=>p.sessionId)).toEqual(["two"]);
 await f.controller.execute("/answer first; second");expect(f.server.requests.find(r=>r.path==="/interactions/questions/q-one/reply")?.body).toEqual({answers:[["first"],["second"]]});expect(f.model.state.questions.map(q=>q.sessionId)).toEqual(["two"]);
 const previous=f.server.sessions.get("one")!.permission;await f.controller.execute("/yolo");expect(f.server.sessions.get("one")?.permission?.at(-1)).toEqual({permission:"*",pattern:"*",action:"allow"});await f.controller.execute("/yolo");expect(f.server.sessions.get("one")?.permission).toEqual(previous);
});
test("native picker/UI commands retain real hooks, skill insertion and inherited new-session selection",async()=>{
 const f=await fixture();await f.controller.execute("/think max");expect(f.model.selection("one").thinking).toBe("xhigh");await f.controller.execute("/skill review");expect(f.inserts).toEqual(["$review "]);
 for(const command of ["/hints","/sidebar","/exit","/disco"])await f.controller.execute(command);expect(f.toggles).toEqual({hints:1,sidebar:1,closed:1});expect(f.effects).toEqual(["disco"]);
 await f.controller.execute("/model");expect(f.pickers.at(-1)?.kind).toBe("model");await f.pickers.at(-1)?.onSelect?.("openai/advanced");expect(f.model.selection("one").model).toBe("openai/advanced");
 await f.controller.execute("/new");const created=f.server.requests.find(r=>r.path==="/sessions"&&r.method==="POST")!;expect(created.body).toEqual({agent:"build",model:{providerId:"openai",id:"advanced",variant:"xhigh",connectionId:"account-a"}});expect(f.model.selection("created-1")).toMatchObject({model:"openai/advanced",thinking:"xhigh",connection_id:"account-a"});expect(f.model.selection("two").model).toBe("other/second");
});
test("MCP and subagent pickers execute plugin operations rather than send substitute prompts",async()=>{
 const f=await fixture();await f.controller.execute("/mcp");expect(f.pickers.at(-1)?.kind).toBe("mcp");await f.pickers.at(-1)?.onSelect?.("local");expect(f.pickers.at(-1)?.kind).toBe("mcp-action");await f.pickers.at(-1)?.onSelect?.("connect");
 expect(f.server.requests.some(r=>r.method==="POST"&&r.path==="/plugins/dev.neoism.mcp/local/connect")).toBe(true);
 await f.controller.execute("/sub-agent");expect(f.pickers.at(-1)?.kind).toBe("subagents");expect(f.pickers.at(-1)?.options[0].value).toBe("two");await f.pickers.at(-1)?.onSelect?.("two");expect(f.model.state.activeSurfaceId).toBe("agent:one");expect(f.model.state.surfaces.find(surface=>surface.id==="agent:one")).toMatchObject({sessionId:"two",agentRootSessionId:"one"});expect(f.model.state.ui.agentParentSessions).toEqual({two:"one"});expect(f.model.selection("two").connection_id).toBe("account-b");
 expect(f.server.requests.some(r=>r.path.endsWith("/prompt"))).toBe(false);
});
test("attached drafts and a workspace switch during session creation cannot become commands in the wrong conversation",async()=>{
 const f=await fixture();f.setAttachments([{}]);await expect(f.controller.execute("/compact")).rejects.toThrow("Attachments");expect(f.server.requests).toHaveLength(0);f.setAttachments([]);
 f.model.update({activeSurfaceId:undefined,surfaces:[],drafts:{a:{agent:"build",model:"openai/base",thinking:"high"}}});f.model.setComposerDraft("home",{text:"/review pending",attachments:[]});f.server.deferCreation();const pending=f.controller.execute("/review pending");await until(()=>!!f.server.creations.length);
 f.model.update({activeWorkspace:{id:"b",host_id:"host",title:"B",root_dir:"/project/b"}});f.server.creations.shift()!();await expect(pending).rejects.toThrow("conversation changed");expect(f.server.requests.some(r=>r.path.endsWith("/commands")&&r.path!=="/commands")).toBe(false);expect(f.model.state.composerDrafts.home.text).toBe("/review pending");
});

test("connect stores a scoped named account without replacing thread selections and usage shows actual windows",async()=>{
 const f=await fixture(),before=structuredClone(f.model.state.providerStates);f.answers.push("Work account","api-secret");
 await f.controller.execute("/connect");expect(f.pickers.at(-1)?.options.some(row=>row.title==="✓ OpenAI"&&row.value==="openai")).toBe(true);await f.pickers.at(-1)?.onSelect?.("openai");await f.pickers.at(-1)?.onSelect?.("__add_account__");await f.pickers.at(-1)?.onSelect?.("0");
 const stored=f.server.requests.find(request=>request.path==="/providers/openai/connections"&&request.method==="POST")!;expect(stored.body).toEqual({label:"Work account",credential:{type:"api",key:"api-secret"},setDefault:false});expect(f.inputs.at(-1)?.secret).toBe(true);expect(f.model.state.providerStates).toEqual(before);expect(JSON.stringify(f.notices)).not.toContain("api-secret");
 await f.controller.execute("/usage");expect(f.pickers.at(-1)?.options[0]).toMatchObject({title:"Personal",description:"5 hours: 24% used",value:"account-a"});await f.pickers.at(-1)?.onSelect?.("account-a");expect(f.notices.at(-1)?.value).toContain("24% used");expect(f.server.requests.some(request=>request.path.endsWith("/prompt"))).toBe(false);
});
test("provider OAuth honors conditional prompts, attempt identity and explicit code; cancelled credentials make no mutation",async()=>{
 const f=await fixture();f.answers.push("Browser account","my-tenant","secret-code");await f.controller.execute("/connect");await f.pickers.at(-1)?.onSelect?.("openai");await f.pickers.at(-1)?.onSelect?.("__add_account__");await f.pickers.at(-1)?.onSelect?.("1");expect(f.pickers.at(-1)?.title).toBe("Region");await f.pickers.at(-1)?.onSelect?.("us");
 expect(f.server.requests.find(request=>request.path==="/providers/openai/oauth/authorize")?.body).toEqual({method:1,label:"Browser account",inputs:{region:"us",tenant:"my-tenant"}});expect(f.server.requests.find(request=>request.path==="/providers/openai/oauth/callback")?.body).toEqual({method:1,attemptId:"attempt-one",code:"secret-code"});expect(f.urls).toEqual(["https://auth.example/sign-in"]);expect(JSON.stringify(f.notices)).not.toContain("secret-code");
 const count=f.server.requests.filter(request=>request.method==="POST").length;f.answers.push(undefined);await f.controller.execute("/connect");await f.pickers.at(-1)?.onSelect?.("openai");await f.pickers.at(-1)?.onSelect?.("__add_account__");expect(f.server.requests.filter(request=>request.method==="POST")).toHaveLength(count);
});

test("model commands retain already-qualified catalog IDs without duplicating providers",async()=>{
 const f=await fixture();f.model.update({providers:[{id:"openai",name:"OpenAI",models:[{id:"openai/qualified",name:"Qualified"}]}]});await f.controller.execute("/model");expect(f.pickers.at(-1)?.options[0].value).toBe("openai/qualified");await f.controller.execute("/model openai/qualified");expect(f.model.selection("one").model).toBe("openai/qualified");
});

test("slash dialogs publish original loading shells immediately, retain their token and suppress cancelled late reads",async()=>{
 const f=await fixture();f.server.defer("/providers");
 const running=f.controller.execute("/connect"),loading=f.pickers.at(-1)!;expect(loading).toMatchObject({kind:"connect",loading:true,options:[]});expect(loading.token).toBeTruthy();
 await until(()=>f.server.requests.some(request=>request.path==="/providers"));f.server.release("/providers");await running;
 const ready=f.pickers.at(-1)!;expect(ready.loading).toBe(false);expect(ready.token).toBe(loading.token);expect(ready.options.length).toBeGreaterThan(0);
 f.server.defer("/providers");const cancelled=f.controller.execute("/connect");const shell=f.pickers.at(-1)!;shell.onCancel?.();const before=f.pickers.length;f.server.release("/providers");await cancelled;expect(f.pickers).toHaveLength(before);
});
test("usage retains original structured windows and supports an explicit refresh without account mutations",async()=>{
 const f=await fixture();f.server.defer("/providers/openai/usage");
 const pending=f.controller.execute("/usage"),loading=f.pickers.at(-1)!;expect(loading).toMatchObject({kind:"usage",title:"Codex",loading:true});
 await until(()=>f.server.requests.some(request=>request.path==="/providers/openai/usage"));f.server.release("/providers/openai/usage");await pending;
 const usage=f.pickers.at(-1)!;expect(usage.token).toBe(loading.token);expect(usage.usage?.accounts[0]).toMatchObject({label:"Personal",plan_type:"pro",windows:[{used_percent:24,reset_at:1900000000}]});
 await usage.onRefresh?.();expect(f.server.requests.filter(request=>request.path==="/providers/openai/usage")).toHaveLength(2);expect(f.server.requests.some(request=>request.method!=="GET")).toBe(false);
});

async function connectAccounts(f:Awaited<ReturnType<typeof fixture>>){await f.controller.execute("/connect");await f.pickers.at(-1)!.onSelect!("openai");expect(f.pickers.at(-1)?.kind).toBe("connect-account");return f.pickers.at(-1)!;}
async function accountActions(f:Awaited<ReturnType<typeof fixture>>,id:string){await connectAccounts(f);await f.pickers.at(-1)!.onSelect!(id);expect(f.pickers.at(-1)?.kind).toBe("connect-account-actions");return f.pickers.at(-1)!;}
test("connect lists secret-free existing accounts with original account actions and explicitly selects only the current conversation",async()=>{
 const f=await fixture(),other={...f.model.selection("two")},before={...f.model.selection("one")},accounts=await connectAccounts(f);
 expect(accounts.title).toBe("OpenAI");expect(accounts.options.find(row=>row.value==="__add_account__")?.title).toBe("Add account");expect(accounts.options.some(row=>row.header&&row.title==="Connected accounts")).toBe(true);
 expect(accounts.options.find(row=>row.value==="account-a")?.title).toBe("Personal");expect(accounts.options.find(row=>row.value==="account-extra")?.title).toBe("Work");expect(JSON.stringify(accounts.options).toLowerCase()).toContain("oauth");expect(JSON.stringify(accounts.options).toLowerCase()).toContain("default");expect(JSON.stringify(accounts.options)).not.toContain("foreign");
 expect(f.model.selection("one")).toEqual(before);await accounts.onSelect!("account-extra");expect(f.pickers.at(-1)?.options.map(row=>row.value)).toEqual(["select","rename","default","disconnect"]);
 await f.pickers.at(-1)!.onSelect!("select");expect(f.model.selection("one")).toMatchObject({...before,connection_id:"account-extra"});expect(f.model.selection("two")).toEqual(other);expect(f.server.requests.some(request=>request.path.includes("/connections")&&request.method!=="GET")).toBe(false);
});
test("connect rename and make-default persist through SDK CRUD without replacing conversation selections or another workspace accounts",async()=>{
 const f=await fixture(),before=structuredClone(f.model.state.providerStates),foreign=structuredClone([...f.server.connections.get("b")!.values()]);
 const current=await accountActions(f,"account-a");expect(current.options.some(row=>row.value==="default")).toBe(false);f.answers.push("Personal renamed");await current.onSelect!("rename");
 expect(f.server.connections.get("a")!.get("account-a")?.label).toBe("Personal renamed");expect(f.server.requests.find(request=>request.method==="PATCH"&&request.path==="/providers/openai/connections/account-a")?.body).toEqual({label:"Personal renamed"});
 await accountActions(f,"account-extra");await f.pickers.at(-1)!.onSelect!("default");expect(f.server.connections.get("a")!.get("account-extra")?.isDefault).toBe(true);expect(f.server.connections.get("a")!.get("account-a")?.isDefault).toBe(false);
 expect(f.server.requests.filter(request=>request.path==="/providers/openai/connections/account-extra/default")).toHaveLength(1);expect(f.model.state.providerStates).toEqual(before);expect([...f.server.connections.get("b")!.values()]).toEqual(foreign);
 const updated=await accountActions(f,"account-extra");expect(updated.options.some(row=>row.value==="default")).toBe(false);
});
test("disconnect defaults to Cancel, only explicit confirmation deletes an account, and never switches another conversation",async()=>{
 const f=await fixture(),other={...f.model.selection("two")};await accountActions(f,"account-extra");await f.pickers.at(-1)!.onSelect!("disconnect");
 const confirm=f.pickers.at(-1)!;expect(confirm.kind).toBe("connect-confirm");expect(confirm.selected).toBe("cancel");expect(confirm.options.map(row=>row.value)).toEqual(["confirm_disconnect","cancel"]);
 await confirm.onSelect!("cancel");expect(f.server.connections.get("a")!.has("account-extra")).toBe(true);expect(f.server.requests.some(request=>request.method==="DELETE")).toBe(false);
 await accountActions(f,"account-extra");await f.pickers.at(-1)!.onSelect!("disconnect");await f.pickers.at(-1)!.onSelect!("confirm_disconnect");
 expect(f.server.connections.get("a")!.has("account-extra")).toBe(false);expect(f.server.connections.get("a")!.has("account-a")).toBe(true);expect(f.server.requests.filter(request=>request.method==="DELETE"&&request.path==="/providers/openai/connections/account-extra")).toHaveLength(1);expect(f.model.selection("two")).toEqual(other);expect(f.server.connections.get("b")!.has("foreign")).toBe(true);
});
test("cancelled account label, API key and rename inputs issue no connection mutation",async()=>{
 const f=await fixture(),before=structuredClone([...f.server.connections.get("a")!.values()]);
 await connectAccounts(f);f.answers.push(undefined);await f.pickers.at(-1)!.onSelect!("__add_account__");expect(f.inputs.at(-1)?.title).toBe("Label new account");expect(f.pickers.at(-1)?.kind).toBe("connect-account");
 await connectAccounts(f);f.answers.push("Never stored",undefined);await f.pickers.at(-1)!.onSelect!("__add_account__");expect(f.pickers.at(-1)?.title).toBe("OpenAI — select auth method");expect(f.pickers.at(-1)?.options.some(row=>row.title==="API key")).toBe(true);await f.pickers.at(-1)!.onSelect!("0");
 await accountActions(f,"account-a");f.answers.push(undefined);await f.pickers.at(-1)!.onSelect!("rename");
 expect([...f.server.connections.get("a")!.values()]).toEqual(before);expect(f.server.requests.filter(request=>request.path.includes("/connections")&&request.method!=="GET")).toHaveLength(0);
});
test("account menus reject stale workspace actions and delayed account lists cannot reopen in another workspace",async()=>{
 const f=await fixture(),actions=await accountActions(f,"account-extra"),before=structuredClone([...f.server.connections.get("a")!.values()]);
 f.model.update({activeSurfaceId:"agent:two"});await expect(Promise.resolve().then(()=>actions.onSelect!("default"))).rejects.toThrow("conversation changed");expect([...f.server.connections.get("a")!.values()]).toEqual(before);
 f.model.update({activeSurfaceId:"agent:one"});await f.controller.execute("/connect");f.server.defer("/providers/openai/connections");const pending=f.pickers.at(-1)!.onSelect!("openai");await until(()=>f.server.requests.filter(request=>request.path==="/providers/openai/connections").length>=2);
 const count=f.pickers.length;f.model.update({activeWorkspace:{id:"b",host_id:"host",title:"B",root_dir:"/project/b"}});f.server.release("/providers/openai/connections");await expect(pending).rejects.toThrow("conversation changed");expect(f.pickers).toHaveLength(count);expect(f.server.requests.some(request=>request.method!=="GET"&&request.path.includes("/connections"))).toBe(false);
});
test("a rejected account rename leaves stored credentials, conversation selection and other workspace unchanged",async()=>{
 const f=await fixture(),before=structuredClone([...f.server.connections.get("a")!.values()]),selection=structuredClone(f.model.state.providerStates);await accountActions(f,"account-a");f.answers.push("Rejected name");f.server.failConnection("PATCH");
 await expect(f.pickers.at(-1)!.onSelect!("rename")).rejects.toThrow();expect([...f.server.connections.get("a")!.values()]).toEqual(before);expect(f.model.state.providerStates).toEqual(selection);expect(f.server.connections.get("b")!.get("foreign")?.label).toBe("Other workspace");
});

test("account labels returned after switching conversations cannot advance authentication or create credentials",async()=>{
 const f=await fixture();await connectAccounts(f);f.deferNextInput();const pending=f.pickers.at(-1)!.onSelect!("__add_account__");await until(()=>f.inputs.at(-1)?.title==="Label new account");const count=f.pickers.length;
 f.model.update({activeSurfaceId:"agent:two"});f.replyInput("Wrong conversation account");await expect(pending).rejects.toThrow("conversation changed");expect(f.pickers).toHaveLength(count);expect(f.server.requests.some(request=>request.method!=="GET"&&request.path.includes("/providers/"))).toBe(false);expect(f.server.connections.get("a")!.size).toBe(2);
});
test("connect nested Back actions return to provider/account stages without storing labels or credentials",async()=>{
 const f=await fixture(),before=structuredClone(f.model.state.providerStates);await connectAccounts(f);await f.pickers.at(-1)!.onBack!();expect(f.pickers.at(-1)?.kind).toBe("connect");
 await f.pickers.at(-1)!.onSelect!("openai");f.answers.push("Draft label");await f.pickers.at(-1)!.onSelect!("__add_account__");expect(f.pickers.at(-1)?.kind).toBe("connect-method");await f.pickers.at(-1)!.onBack!();expect(f.pickers.at(-1)?.kind).toBe("connect-account");
 await f.pickers.at(-1)!.onSelect!("account-extra");await f.pickers.at(-1)!.onBack!();expect(f.pickers.at(-1)?.kind).toBe("connect-account");
 await f.pickers.at(-1)!.onSelect!("account-extra");await f.pickers.at(-1)!.onSelect!("disconnect");await f.pickers.at(-1)!.onBack!();expect(f.pickers.at(-1)?.kind).toBe("connect-account");
 expect(f.server.requests.some(request=>request.path.includes("/connections")&&request.method!=="GET")).toBe(false);expect(f.server.connections.get("a")!.size).toBe(2);expect(f.model.state.providerStates).toEqual(before);
});
