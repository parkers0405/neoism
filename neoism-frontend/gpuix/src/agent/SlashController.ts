import {goals,mcp,subagents,type NeoismClient,type Session,type PermissionRule,type CapabilityInfo,type ProviderAuthMethod,type ProviderAuthPrompt} from "@neoism/sdk";
import type {AppConfig} from "../config";
import type {WorkspaceModel} from "../model";
import type {ProviderState} from "../daemon/protocol";
import {sdkModel} from "../daemon/sdk";
import {coreCall} from "../native-core";
import {thinkingOptions} from "../design/generated";

export interface SlashOption{title:string;description:string;footer:string;value:string;mcp?:unknown}
export type SlashEffect="piss"|"cuss"|"glitch"|"disco"|"gangfight"|"praise";
export interface SlashPickerRequest{token?:string;loading?:boolean;error?:string;usage?:{accounts:readonly {connection_id:string;label:string}[]};onRefresh?:()=>void|Promise<void>;onCancel?:()=>void;kind:string;title:string;options:SlashOption[];selected?:string;onSelect?:(value:string)=>void|Promise<void>;
 /** Original-card stage status (connect / OAuth / MCP authorization). */
 activity?:{body:string;status:string;busy?:boolean;url?:string};mcpServer?:string;mcpEntry?:unknown}
export interface SlashInputRequest{title:string;description?:string;secret?:boolean;initialValue?:string}
export interface SlashHooks{
 input?:(request:SlashInputRequest)=>Promise<string|undefined>;
 picker(request:SlashPickerRequest):void|Promise<void>;
 insert(text:string):void;
 toggleHints():void|Promise<void>;
 toggleSidebar():void|Promise<void>;
 closeTab():void|Promise<void>;
 notice(title:string,value:unknown,sessionId?:string):void;
 effect?:(kind:SlashEffect)=>void|Promise<void>;
 supported?:(actionKind:string)=>boolean;
 openExternal?:(url:string)=>void|Promise<void>;
 /** The original desktop directory browser; resolves the chosen folder. */
 chooseDirectory?:(start:string)=>Promise<string|undefined>;
}
export interface SlashControllerOptions{model:WorkspaceModel;config:AppConfig;sessionId:()=>string|undefined;attachments?:()=>readonly unknown[];hooks:SlashHooks}
interface Plan{kind:string;value?:unknown}
interface Scope{pickerId?:string;pickerCancelled?:boolean;pickerShell?:SlashPickerRequest;workspaceId:string;directory:string;sessionId?:string;surfaceId?:string;selection:ProviderState;sdk:NeoismClient}
const providerModelId=(providerId:string,modelId:string)=>modelId.startsWith(`${providerId}/`)?modelId:`${providerId}/${modelId}`;
const effects:Record<string,SlashEffect>={PissOnScreen:"piss",CussOnScreen:"cuss",GlitchOnScreen:"glitch",DiscoOnScreen:"disco",GangFightOnScreen:"gangfight",PraiseOnScreen:"praise"};
const option=(title:string,description:string,value:string,footer="server"):SlashOption=>({title,description,value,footer});

/** The original Rust parser/catalog owns command spelling and planning. This
 * controller executes actual SDK operations and explicit host UI actions. */
export class SlashController{
 state:{options:SlashOption[];loading:boolean;error?:string}={options:[],loading:false};
 private builtins:SlashOption[];
 private listeners=new Set<()=>void>();
 private generation=0;
 private disposed=false;
 private abort=new AbortController();
 private flights=new Map<string,Promise<boolean>>();
 private capabilities?:CapabilityInfo[];
 private catalogScope="";
 constructor(private options:SlashControllerOptions){
  this.builtins=coreCall<SlashOption[]>("agentcomposer.slash-options");
  this.state.options=this.builtins.filter(row=>this.supported(this.plan(row.value).kind));
 }
 subscribe(listener:()=>void){this.listeners.add(listener);return()=>this.listeners.delete(listener);}
 dispose(){this.abort.abort();this.disposed=true;this.generation++;this.listeners.clear();}
 private update(patch:Partial<SlashController["state"]>){if(this.disposed)return;this.state={...this.state,...patch};for(const listener of this.listeners)listener();}
 private plan(text:string){return coreCall<Plan>("agentcomposer.plan",{text});}
 private supported(kind:string){
  const capability=kind==="ShowMcp"?"neoism.mcp":kind==="OpenSubagentPicker"?"neoism.subagents":["ShowGoal","SetGoal","ClearGoal","PauseGoal","ResumeGoal"].includes(kind)?"neoism.goals":undefined;
  return (!capability||!this.capabilities||this.capabilities.some(c=>c.id===capability&&c.enabled))&&(!effects[kind]||!!this.options.hooks.effect)&&this.options.hooks.supported?.(kind)!==false;
 }
 private scope():Scope{
  const model=this.options.model,workspace=model.state.activeWorkspace;if(!workspace)throw new Error("Select a workspace first");
  const sessionId=this.options.sessionId(),thread=model.state.threads.find(thread=>thread.session_id===sessionId);
  return {workspaceId:workspace.id,directory:thread?.directory||workspace.root_dir||"",sessionId,surfaceId:model.state.activeSurfaceId,
   selection:{...model.selection(sessionId)},sdk:model.agentSdk.forWorkspace(workspace.id)};
 }
 private current(scope:Scope){
  const state=this.options.model.state;if(state.activeWorkspace?.id!==scope.workspaceId)return false;
  const session=state.surfaces.find(surface=>surface.id===state.activeSurfaceId)?.sessionId;
  return scope.sessionId?session===scope.sessionId:state.activeSurfaceId===scope.surfaceId;
 }
 private assertCurrent(scope:Scope){if(!this.current(scope))throw new Error("The conversation changed. Run the command again in the current conversation.");}
 private notice(scope:Scope,title:string,value:unknown){if(this.current(scope))this.options.hooks.notice(title,value,scope.sessionId);}
 async refresh(){
  const scope=this.scope(),generation=++this.generation,key=`${scope.workspaceId}\0${scope.directory}`;
  if(key!==this.catalogScope){this.capabilities=undefined;this.catalogScope=key;}
  this.update({loading:true,error:undefined});
  try{
   const [commands,capabilities]=await Promise.allSettled([scope.sdk.catalog.commands.list(scope.directory),scope.sdk.capabilities.list(scope.directory)]);
   if(this.disposed||generation!==this.generation||this.options.model.state.activeWorkspace?.id!==scope.workspaceId)return;
   if(capabilities.status==="fulfilled")this.capabilities=capabilities.value;
   const rows=this.builtins.filter(row=>this.supported(this.plan(row.value).kind)),seen=new Set(rows.map(row=>row.value));
   for(const command of commands.status==="fulfilled"?commands.value:[]){
    const name=command.name.replace(/^\/+/,"");if(!name||/\s/.test(name))continue;
    const value=`/${name}`;
    if(seen.has(value)||this.plan(value).kind!=="RunServerCommand")continue;
    seen.add(value);rows.push(option(value,command.description||"Server command",value,"server"));
   }
   this.update({options:rows,loading:false,error:commands.status==="rejected"?(commands.reason instanceof Error?commands.reason.message:"Server commands unavailable"):undefined});
  }catch(error){if(generation===this.generation)this.update({loading:false,error:error instanceof Error?error.message:"Command catalog unavailable"});}
 }
 execute(text:string):Promise<boolean>{
  if(!text.trim().startsWith("/"))return Promise.resolve(false);
  if(this.disposed)return Promise.reject(new Error("The command composer has closed"));
  if(this.options.attachments?.().length)return Promise.reject(new Error("Attachments belong to a chat message, not a slash command. Remove them before running this command."));
  const scope=this.scope(),key=`${scope.workspaceId}\0${scope.sessionId||""}\0${text}`;
  const existing=this.flights.get(key);if(existing)return existing;
  const task=this.perform(scope,this.plan(text)).catch(async error=>{if(scope.pickerCancelled||this.disposed)return true;await this.pickerFailure(scope,error);throw error;}).finally(()=>{if(this.flights.get(key)===task)this.flights.delete(key);});
  this.flights.set(key,task);return task;
 }
 private async ensureSession(scope:Scope){
  if(scope.sessionId)return scope.sessionId;
  const session=await scope.sdk.sessions.create({directory:scope.directory||undefined,agent:scope.selection.agent||undefined,model:sdkModel(scope.selection)});
  this.assertCurrent(scope);this.openSession(scope,session,true);return session.id;
 }
 private requireSession(scope:Scope){if(!scope.sessionId)throw new Error("Open a conversation first");return scope.sessionId;}
 private openSession(scope:Scope,session:Session,inheritSelection=false){
  this.assertCurrent(scope);const model=this.options.model;
  const previous=model.selection(session.id),selection=inheritSelection?scope.selection:{agent:session.agent||previous.agent,model:session.model?`${session.model.providerId}/${session.model.id}`:previous.model,thinking:session.model?.variant??previous.thinking,connection_id:session.model?.connectionId??previous.connection_id};
  const thread={session_id:session.id,title:session.title||"New conversation",directory:session.directory||scope.directory,agent:selection.agent,model:selection.model};
  model.update({threads:[thread,...model.state.threads.filter(thread=>thread.session_id!==session.id)],providerStates:{...model.state.providerStates,
   [session.id]:model.state.providerStates[session.id]||{...selection}}});
  const parents={...(model.state.ui.agentParentSessions||{}) as Record<string,string>};
  if(session.parentId)parents[session.id]=session.parentId;else delete parents[session.id];
  model.setUI({agentParentSessions:parents});
  const surfaceId=model.openThread(thread);scope.sessionId=session.id;scope.surfaceId=surfaceId||`agent:${session.id}`;scope.directory=thread.directory||scope.directory;
 }
 private pickerRequest(scope:Scope,request:SlashPickerRequest):SlashPickerRequest{
  return{...request,token:`${scope.pickerId??=crypto.randomUUID()}:${request.kind}`,onCancel:()=>{scope.pickerCancelled=true;}};
 }
 private async loadingPicker(scope:Scope,kind:string,title:string){
  if(scope.pickerCancelled||this.disposed||!this.current(scope))return;
  const request=this.pickerRequest(scope,{kind,title,options:[],loading:true});scope.pickerShell=request;await this.options.hooks.picker(request);
 }
 private async pickerFailure(scope:Scope,error:unknown){
  const shell=scope.pickerShell;if(!shell||scope.pickerCancelled||this.disposed||!this.current(scope))return;
  const message=error instanceof Error?error.message:"Could not load this menu";
  await this.options.hooks.picker({...shell,loading:false,error:message,activity:shell.kind==="usage"?undefined:{body:message,status:"Could not load",busy:false}});
 }
 private async picker(scope:Scope,kind:string,title:string,rows:SlashOption[],select?:(value:string)=>Promise<void>|void,selected?:string,extra:Pick<SlashPickerRequest,"activity"|"mcpServer"|"mcpEntry"|"usage"|"onRefresh">={}){
  this.assertCurrent(scope);if(scope.pickerCancelled||this.disposed)return;scope.pickerShell=undefined;
  await this.options.hooks.picker(this.pickerRequest(scope,{kind,title,options:rows,selected,loading:false,...extra,onSelect:select?async value=>{this.assertCurrent(scope);await select(value);}:undefined}));
 }
 private async awaitCatalog(scope:Scope){
  if(!this.options.model.state.catalogsLoading)return;
  await new Promise<void>(resolve=>{let stop=()=>{};const done=()=>{stop();this.abort.signal.removeEventListener("abort",done);resolve();};stop=this.options.model.subscribe(()=>{if(!this.options.model.state.catalogsLoading||!this.current(scope)||this.disposed)done();});this.abort.signal.addEventListener("abort",done,{once:true});});
  this.assertCurrent(scope);
 }
 private async changeDirectory(scope:Scope,directory:string){
  const id=await this.ensureSession(scope);this.assertCurrent(scope);
  const session=await scope.sdk.sessions.update(id,{directory});this.assertCurrent(scope);scope.directory=session.directory||directory;
  const model=this.options.model;model.update({threads:model.state.threads.map(thread=>thread.session_id===id?{...thread,directory:scope.directory}:thread)});
  this.notice(scope,"Directory",`Switched location to ${scope.directory}`);
 }
 private async directoryPicker(scope:Scope,path=scope.directory){
  const result=await scope.sdk.operations.request("v2.directories.list",{query:{path}});this.assertCurrent(scope);
  const rows=[option("Use this directory",result.path,`use:${result.path}`,"directory"),...(result.parent?[option("..",result.parent,`browse:${result.parent}`,"directory")]:[]),...result.entries.map(entry=>option(entry.name,entry.path,`browse:${entry.path}`,"directory"))];
  await this.picker(scope,"directory","Change directory",rows,async value=>{if(value.startsWith("use:"))await this.changeDirectory(scope,value.slice(4));else await this.directoryPicker(scope,value.slice(7));});
 }
 private async permission(scope:Scope,args:string[]){
  const id=this.requireSession(scope),aliases:Record<string,"once"|"always"|"reject">={once:"once",allow:"once",yes:"once",always:"always",all:"always",reject:"reject",deny:"reject",no:"reject"};
  const first=args[0]||"once",reply=aliases[first]||"once",requested=args[1]||(!aliases[first]?first:undefined);
  const pending=await scope.sdk.interactions.permissions.list(id);this.assertCurrent(scope);
  const permission=requested?pending.find(p=>p.id===requested):pending[0];if(!permission)throw new Error("No matching pending permission");
  if(!await scope.sdk.interactions.permissions.reply(permission.id,reply))throw new Error("The permission reply was not accepted");
  const model=this.options.model;model.update({permissions:model.state.permissions.filter(p=>p.sessionId!==id||p.requestId!==permission.id)});this.notice(scope,"Permission",`${permission.id}: ${reply}`);
 }
 private async reject(scope:Scope,requested?:string){
  const id=this.requireSession(scope),questions=await scope.sdk.interactions.questions.list(id);this.assertCurrent(scope);
  const question=requested?questions.find(q=>q.id===requested):questions[0];
  if(question){if(!await scope.sdk.interactions.questions.reject(question.id))throw new Error("The question rejection was not accepted");const model=this.options.model;model.update({questions:model.state.questions.filter(q=>q.sessionId!==id||q.requestId!==question.id)});this.notice(scope,"Question","Rejected");return;}
  const permissions=await scope.sdk.interactions.permissions.list(id);this.assertCurrent(scope);const permission=requested?permissions.find(p=>p.id===requested):permissions[0];if(!permission)throw new Error("No matching pending interaction");
  await this.permission(scope,["reject",permission.id]);
 }
 private async skipPermissions(scope:Scope){
  const id=this.requireSession(scope),model=this.options.model,key=`${scope.workspaceId}:${id}`;
  const saved=(model.state.ui.slashSkipPermissions||{}) as Record<string,{previous:PermissionRule[]}>;
  if(saved[key]){await scope.sdk.sessions.update(id,{permission:saved[key].previous});const next={...(model.state.ui.slashSkipPermissions||{}) as typeof saved};delete next[key];model.setUI({slashSkipPermissions:next});this.notice(scope,"Permissions","Permission prompts restored");return;}
  const session=await scope.sdk.sessions.get(id);this.assertCurrent(scope);const previous=session.permission||[];
  await scope.sdk.sessions.update(id,{permission:[...previous,{permission:"*",pattern:"*",action:"allow"}]});
  model.setUI({slashSkipPermissions:{...(model.state.ui.slashSkipPermissions||{}) as typeof saved,[key]:{previous}}});
  const pending=await scope.sdk.interactions.permissions.list(id);
  for(const permission of pending)await scope.sdk.interactions.permissions.reply(permission.id,"once");
  model.update({permissions:model.state.permissions.filter(permission=>permission.sessionId!==id)});this.notice(scope,"Permissions","Skipping permission prompts for this conversation");
 }
 private async mcpPicker(scope:Scope){
  const client=await scope.sdk.plugins.use(mcp,{directory:scope.directory}),catalog=await client.catalog(scope.directory);this.assertCurrent(scope);
  // Footers use the original status vocabulary so the card paints its status dots.
  const status=(entry:{enabled:boolean;status:{status:string}})=>entry.enabled?String(entry.status.status).replace(/_/g," ").toLowerCase():"disabled";
  await this.picker(scope,"mcp","MCP servers",Object.entries(catalog).map(([name,entry])=>({...option(name,entry.status.status,name,status(entry)),mcp:{name,...entry}})),async name=>{
   const entry=catalog[name];if(!entry)return;
   const actions=[option(entry.runtimeConnected?"Disconnect":"Connect",name,entry.runtimeConnected?"disconnect":"connect"),option(entry.enabled?"Disable":"Enable",name,entry.enabled?"disable":"enable"),option("Tools",name,"tools"),option("Resources",name,"resources"),option("Prompts",name,"prompts")];
   if(entry.oauthCapable)actions.push(option("Authorize",name,"authorize"));
   const showActions=(activity?:SlashPickerRequest["activity"])=>this.picker(scope,"mcp-action",name,actions,async action=>{
    const directory=scope.directory;
    if(action==="tools")this.notice(scope,`${name} tools`,await client.tools(name,directory));
    else if(action==="resources")this.notice(scope,`${name} resources`,await client.resources(name,directory));
    else if(action==="prompts")this.notice(scope,`${name} prompts`,await client.prompts(name,directory));
    else if(action==="authorize"){const auth=await client.startAuth(name,directory);this.assertCurrent(scope);if(this.options.hooks.openExternal)await this.options.hooks.openExternal(auth.authorizationUrl);
     await showActions({body:`Authorize ${name} in your browser, then return here.`,status:"Waiting for authorization…",busy:true,url:auth.authorizationUrl});}
    else{if(action==="connect")await client.connect(name,directory);else if(action==="disconnect")await client.disconnect(name,directory);else await client.configure(name,{enabled:action==="enable"},directory);await this.mcpPicker(scope);}
   },undefined,{activity,mcpServer:name,mcpEntry:{name,...entry}});
   await showActions();
  });
 }
 private async input(scope:Scope,request:SlashInputRequest){
  this.assertCurrent(scope);if(!this.options.hooks.input)throw new Error("This host does not provide account input");
  const value=await this.options.hooks.input(request);this.assertCurrent(scope);return value;
 }
 private async usagePicker(scope:Scope){
  await this.loadingPicker(scope,"usage","Codex");
  const usage=await scope.sdk.operations.request("v2.providers.openai.usage",{query:{workspaceId:scope.workspaceId}});this.assertCurrent(scope);
  await this.picker(scope,"usage","Codex",usage.accounts.map(account=>option(account.label,account.error||account.windows.map(window=>`${window.label}: ${window.used_percent}% used`).join(" · ")||"No usage windows reported",account.connection_id,account.plan_type||account.auth_type)),id=>{
   const account=usage.accounts.find(account=>account.connection_id===id);if(account)this.notice(scope,account.label,account.error||account.windows.map(window=>`${window.label}: ${window.used_percent}% used${window.reset_at?` · resets ${new Date(window.reset_at*1000).toLocaleString()}`:""}`).join("\n")||"No usage windows reported");
  },scope.selection.connection_id||undefined,{usage,onRefresh:async()=>{try{await this.usagePicker(scope);}catch(error){await this.pickerFailure(scope,error);}}});
 }
 private async connectPicker(scope:Scope){
  const [catalog,methods]=await Promise.all([scope.sdk.catalog.providers.list(scope.directory),scope.sdk.catalog.providers.authMethods(scope.directory)]);this.assertCurrent(scope);
  await this.picker(scope,"connect","Connect a provider",catalog.all.map(provider=>option(provider.name,provider.id,provider.id,catalog.connected.includes(provider.id)?"connected":"provider")),async providerId=>{
   const choices:ProviderAuthMethod[]=[...(methods[providerId]||[])];if(!choices.some(method=>method.type==="api"))choices.push({type:"api",label:"API key"});
   await this.picker(scope,"connect-method",`Connect ${catalog.all.find(provider=>provider.id===providerId)?.name||providerId}`,choices.map((method,index)=>option(method.label,method.type,String(index),"authentication")),async index=>{
    const methodIndex=Number(index),method=choices[methodIndex];if(!method)throw new Error("Unknown authentication method");
    const label=await this.input(scope,{title:"Account label",description:"Name this account without changing other conversations",initialValue:catalog.all.find(provider=>provider.id===providerId)?.name||providerId});if(label===undefined)return;if(!label.trim())throw new Error("Account label is required");
    if(method.type==="api"){
     const key=await this.input(scope,{title:"API key",description:providerId,secret:true});if(key===undefined)return;if(!key.trim())throw new Error("API key is required");
     try{await scope.sdk.catalog.providers.createConnection(providerId,{label:label.trim(),credential:{type:"api",key:key.trim()},setDefault:false},scope.workspaceId);}catch{throw new Error("The account could not be connected. Check the key and server authorization.");}
     if(this.current(scope))void this.options.model.refreshCatalogs();
     this.notice(scope,"Connected",`${label.trim()} is available for ${providerId}. Existing conversation selections are unchanged.`);return;
    }
    await this.authInputs(scope,method.prompts||[],0,{},async inputs=>{
     let authorization;try{authorization=await scope.sdk.catalog.providers.oauthAuthorize(providerId,{method:methodIndex,label:label.trim(),inputs},scope.workspaceId);}catch{throw new Error("Provider authorization could not start");}
     this.assertCurrent(scope);if(!authorization)throw new Error("Provider authorization is unavailable");
     if(this.options.hooks.openExternal)await this.options.hooks.openExternal(authorization.url);
     this.notice(scope,"Provider authorization",`${authorization.instructions}\n${authorization.url}`);
     const finish=async(code?:string)=>{
      this.assertCurrent(scope);if(authorization.expiresAt&&authorization.expiresAt<=Date.now())throw new Error("Authorization expired. Run /connect again.");
      let accepted=false;try{accepted=await scope.sdk.catalog.providers.oauthCallback(providerId,{method:methodIndex,attemptId:authorization.attemptId,...(code===undefined?{}:{code})},scope.workspaceId);}catch{throw new Error("Provider authorization failed. Run /connect to retry.");}
      if(!accepted)throw new Error("Authorization is incomplete. Complete the browser flow and retry.");if(this.current(scope))void this.options.model.refreshCatalogs();this.notice(scope,"Connected",`${label.trim()} is connected. Existing conversation selections are unchanged.`);
     };
     if(authorization.method==="code"){const code=await this.input(scope,{title:"Authorization code",description:authorization.instructions,secret:true});if(code===undefined)return;if(!code.trim())throw new Error("Authorization code is required");await finish(code.trim());}
     else await this.picker(scope,"connect-authorize","Complete provider authorization",[option("Finish connecting",authorization.instructions,"finish","browser")],()=>finish(),undefined,
       {activity:{body:authorization.instructions,status:"Waiting for browser authorization…",busy:true,url:authorization.url}});
    });
   });
  });
 }
 private async authInputs(scope:Scope,prompts:ProviderAuthPrompt[],index:number,inputs:Record<string,string>,done:(inputs:Record<string,string>)=>Promise<void>):Promise<void>{
  if(index>=prompts.length){await done(inputs);return;}
  const prompt=prompts[index],condition=prompt.condition as {key?:string;op?:string;value?:string}|undefined;
  if(condition?.key&&(condition.op==="neq"?inputs[condition.key]===condition.value:inputs[condition.key]!==condition.value)){await this.authInputs(scope,prompts,index+1,inputs,done);return;}
  const next=(value:string)=>this.authInputs(scope,prompts,index+1,{...inputs,[prompt.key]:value},done);
  if(prompt.type==="select"){
   const rows=(Array.isArray(prompt.options)?prompt.options:[]).flatMap(value=>{if(!value||typeof value!=="object")return[];const row=value as Record<string,unknown>;return typeof row.value==="string"&&typeof row.label==="string"?[option(row.label,"",row.value,"authentication")]:[];});
   if(!rows.length)throw new Error("The provider returned no authentication choices");await this.picker(scope,"connect-input",prompt.message,rows,next);
  }else{const value=await this.input(scope,{title:prompt.message,secret:true});if(value!==undefined)await next(value);}
 }
 private async perform(scope:Scope,action:Plan):Promise<boolean>{
  const model=this.options.model,hooks=this.options.hooks,value=action.value;
  if(!this.supported(action.kind))throw new Error("This command is unavailable in this frontend");
  this.assertCurrent(scope);
  if(effects[action.kind]){await hooks.effect!(effects[action.kind]);return true;}
  const pending:Record<string,[string,string]>={OpenConnectPicker:["connect","Connect a provider"],OpenModelPicker:["model","Model"],OpenAgentPicker:["agent","Agent"],OpenSessionsPicker:["sessions","Sessions"],OpenSkillPicker:["skills","Skills"],OpenSubagentPicker:["subagents","Subagent sessions"],ShowMcp:["mcp","MCP servers"]};
  if(pending[action.kind])await this.loadingPicker(scope,...pending[action.kind]);
  if(action.kind==="OpenModelPicker"||action.kind==="OpenAgentPicker")await this.awaitCatalog(scope);
  switch(action.kind){
   case "Noop":return false;
   case "ShowHelp":this.notice(scope,"Commands",this.state.options.map(row=>`${row.value} — ${row.description}`).join("\n"));break;
   case "OpenConnectPicker":await this.connectPicker(scope);break;
   case "OpenUsagePicker":await this.usagePicker(scope);break;
   case "ApplyModel":{
    const requested=String(value),available=model.state.providers.flatMap(p=>(p.models||[]).map(m=>providerModelId(p.id,m.id)));
    const found=available.find(id=>id===requested)||available.find(id=>id.endsWith(`/${requested}`));if(!found)throw new Error("Model not found. Use /model to choose a server model");model.setSelection("model",found,scope.sessionId);break;
   }
   case "OpenModelPicker":await this.picker(scope,"model","Model",model.state.providers.flatMap(p=>(p.models||[]).map(m=>option(m.name||m.id,p.name||p.id,providerModelId(p.id,m.id),p.id))),value=>model.setSelection("model",value,scope.sessionId),scope.selection.model||undefined);break;
   case "ApplyAgent":if(!model.state.agents.some(a=>a.name===String(value)))throw new Error("Unknown agent. Use /agent to choose");model.setSelection("agent",String(value),scope.sessionId);break;
   case "OpenAgentPicker":await this.picker(scope,"agent","Agent",model.state.agents.map(a=>option(a.name,a.description||"",a.name)),value=>model.setSelection("agent",value,scope.sessionId),scope.selection.agent||undefined);break;
   case "ApplyThinking":model.setSelection("thinking",coreCall<string>("agentcomposer.normalize-thinking",{value:String(value)}),scope.sessionId);break;
   case "OpenThinkingPicker":await this.picker(scope,"thinking","Reasoning effort",thinkingOptions.filter(row=>!("modelIncludes"in row)||scope.selection.model?.toLowerCase().includes(row.modelIncludes)).map(row=>({...row})),value=>model.setSelection("thinking",value,scope.sessionId),scope.selection.thinking||undefined);break;
   case "SwitchSession":this.openSession(scope,await scope.sdk.sessions.get(String(value)));break;
   case "OpenSessionsPicker":{
    const sessions=await scope.sdk.sessions.list({directory:scope.directory});await this.picker(scope,"sessions","Sessions",sessions.items.map(s=>option(s.title,s.directory,s.id)),async id=>this.openSession(scope,await scope.sdk.sessions.get(id)),scope.sessionId);break;
   }
   case "ChangeDirectory":await this.changeDirectory(scope,String(value));break;
   case "OpenDirectoryPicker":{if(hooks.chooseDirectory){const path=await hooks.chooseDirectory(scope.directory);this.assertCurrent(scope);if(path)await this.changeDirectory(scope,path);}else await this.directoryPicker(scope);break;}
   case "OpenSubagentPicker":{const client=await scope.sdk.plugins.use(subagents,{directory:scope.directory}),tasks=await client.list(this.requireSession(scope));await this.picker(scope,"subagents","Subagent sessions",tasks.map(task=>option(task.description||task.agent,task.status,task.childSessionId,task.agent)),async id=>this.openSession(scope,await scope.sdk.sessions.get(id)));break;}
   case "ShowSkills":this.notice(scope,"Skills",await scope.sdk.catalog.skills.list(scope.directory));break;
   case "ShowSkill":{const skills=await scope.sdk.catalog.skills.list(scope.directory),skill=skills.find(s=>s.name===String(value));if(!skill)throw new Error("Usage: /skill info <available skill name>");this.notice(scope,"Skill",skill);break;}
   case "ShowSkillUsage":this.notice(scope,"Skill","Usage: /skill info <name>");break;
   case "InsertSkillMentionByName":{const name=String(value),skills=await scope.sdk.catalog.skills.list(scope.directory);this.assertCurrent(scope);if(!skills.some(s=>s.name===name))throw new Error("Skill not found. Use /skills to choose");hooks.insert(`$${name} `);break;}
   case "OpenSkillPicker":{const skills=await scope.sdk.catalog.skills.list(scope.directory);await this.picker(scope,"skills","Skills",skills.map(s=>option(s.name,s.description||"",s.name,"skill")),name=>hooks.insert(`$${name} `));break;}
   case "HandleQueue":{const id=this.requireSession(scope),command=value==null?"":String(value);const result=command==="clear"?await scope.sdk.sessions.clearQueue(id):command==="pop"?await scope.sdk.sessions.popQueue(id):await scope.sdk.sessions.queue(id);this.notice(scope,"Queue",result);break;}
   case "ShowMcp":await this.mcpPicker(scope);break;
   case "ShowPermissions":this.notice(scope,"Permissions",await scope.sdk.interactions.permissions.list(this.requireSession(scope)));break;
   case "ShowQuestions":this.notice(scope,"Questions",await scope.sdk.interactions.questions.list(this.requireSession(scope)));break;
   case "HandlePermit":await this.permission(scope,Array.isArray(value)?value.map(String):[]);break;
   case "HandleReject":await this.reject(scope,value==null?undefined:String(value));break;
   case "HandleAnswer":{
    const id=this.requireSession(scope),text=String(value||"").trim();if(!text)throw new Error("Usage: /answer <text>");const questions=await scope.sdk.interactions.questions.list(id);this.assertCurrent(scope);const question=questions[0];if(!question)throw new Error("No pending questions");const answers=question.questions.length<=1?[[text]]:text.split(";").map(v=>[v.trim()]);if(answers.length!==question.questions.length)throw new Error(`Provide ${question.questions.length} answers separated by semicolons`);if(!await scope.sdk.interactions.questions.reply(question.id,answers))throw new Error("The answer was not accepted");model.update({questions:model.state.questions.filter(q=>q.sessionId!==id||q.requestId!==question.id)});this.notice(scope,"Question","Answer submitted");break;
   }
   case "CompactSession":this.notice(scope,"Compaction",await scope.sdk.sessions.summarize(this.requireSession(scope)));break;
   case "UndoSession":this.notice(scope,"Undo",await scope.sdk.sessions.undo(this.requireSession(scope)));break;
   case "RedoSession":this.notice(scope,"Redo",await scope.sdk.sessions.redo(this.requireSession(scope)));break;
   case "ShowGoal":case "SetGoal":case "ClearGoal":case "PauseGoal":case "ResumeGoal":{
    const id=action.kind==="SetGoal"?await this.ensureSession(scope):this.requireSession(scope),client=await scope.sdk.plugins.use(goals,{directory:scope.directory});this.assertCurrent(scope);
    const result=action.kind==="ShowGoal"?await client.get(id):action.kind==="ClearGoal"?await client.clear(id):await client.set(id,action.kind==="SetGoal"?{text:String(value)}:{paused:action.kind==="PauseGoal"});
    this.notice(scope,"Goal",result);
    // Original /goal persists the goal, then starts its actual agent run.
    if(action.kind==="SetGoal"){this.assertCurrent(scope);if(!await model.submitPrompt(id,String(value),{selection:scope.selection}))throw new Error("Goal was saved, but its agent run could not start");}break;
   }
   case "ToggleInputHelp":await hooks.toggleHints();break;
   case "ToggleSidebar":await hooks.toggleSidebar();break;
   case "ToggleSkipPermissions":await this.skipPermissions(scope);break;
   case "AbortSession":this.notice(scope,"Abort",await scope.sdk.sessions.abort(this.requireSession(scope)));break;
   case "CreateNewSession":{const session=await scope.sdk.sessions.create({directory:scope.directory,agent:scope.selection.agent||undefined,model:sdkModel(scope.selection)});this.openSession(scope,session,true);break;}
   case "RequestCloseTab":await hooks.closeTab();break;
   case "RunServerCommand":{
    const {command,args}=value as {command:string;args:string},id=await this.ensureSession(scope);this.assertCurrent(scope);const selection=sdkModel(scope.selection);
    const result=await scope.sdk.sessions.command(id,command,{arguments:args,agent:scope.selection.agent||undefined,...(selection?{model:{providerId:selection.providerId,modelId:selection.id,variant:selection.variant,connectionId:selection.connectionId}}:{})});this.notice(scope,"Command result",result);break;
   }
   default:throw new Error(`Unsupported original slash action: ${action.kind}`);
  }
  return true;
 }
}
