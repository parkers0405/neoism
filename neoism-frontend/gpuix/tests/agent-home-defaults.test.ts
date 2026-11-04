import{expect,test}from"bun:test";
import{loadConfig}from"../src/config";
import{DaemonConnection}from"../src/daemon/client";
import{WorkspaceModel}from"../src/model";
import{createAgentDaemon}from"./support/agent-daemon";
const until=async(predicate:()=>boolean)=>{const end=Date.now()+3500;while(!predicate()){if(Date.now()>end)throw Error("Agent home defaults did not converge");await Bun.sleep(5);}};
test("a caller without desktop capability cannot silently load the old project's metadata for an outside cwd",async()=>{
 const f=fixture({agentConfigDefaults:{a:{defaultAgent:"build",model:"openai/advanced",variant:"high"}}});
 try{
  await until(()=>f.model.state.activeWorkspace?.id==="a"&&!f.model.state.agentDefaultsLoading);
  for(const cwd of ["/project/a/subdirectory","/home/fixture","/project/ab"]){
   f.model.setUI({terminalWorkspaceRoots:{a:{declaredRoot:"/project/a",root:cwd}}});
   const before=f.daemon.httpRequests.length;
   await f.model.activateWorkspace(f.daemon.workspaces[0]);
   await until(()=>!f.model.state.catalogsLoading);
   expect(f.model.state.activeWorkspace?.root_dir).toBe(cwd);
   const inside=cwd==="/project/a/subdirectory";
   if(inside){expect(f.model.selection()).toMatchObject({model:"openai/advanced",thinking:"high"});expect(f.model.state.catalogsError).toBeUndefined();}
   else expect(f.model.state.catalogsError).toContain("not authorized");
   const reads=f.daemon.httpRequests.slice(before).filter(r=>/\/(config\/defaults|providers\/configured|agents|skills)$/.test(r.path));
   expect(reads).toHaveLength(4);
   expect(reads.every(r=>r.directory===(r.path.endsWith("/providers/configured")&&!inside?null:cwd))).toBe(true);
  }
 }finally{f.close();}
});
test("desktop project metadata follows cwd while configured providers and paired clients retain workspace scope",async()=>{
 for(const mode of ["desktop","paired","denied"]){
  const requests:Array<{path:string;directory:string|null}>=[];
  const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request){
   const url=new URL(request.url),path=url.pathname,directory=url.searchParams.get("directory");requests.push({path,directory});
   if(path==="/agent/system/info")return mode==="desktop"?Response.json({systemId:"fixture",capabilityVersion:1}):new Response(null,{status:403});
   if(path.startsWith("/agent/workspaces/a/")&&directory&&directory!=="/project/a"&&!directory.startsWith("/project/a/"))return Response.json({code:"forbidden",message:"The caller is not authorized for this directory",retryable:false,details:{}},{status:403});
   if(path.endsWith("/providers/configured"))return Response.json({default:{},providers:[{id:"openai",name:"OpenAI",models:{base:{id:"base",name:"Base",limit:{context:128000}}}}]});
   if(path.endsWith("/config/defaults"))return Response.json({defaultAgent:"plan",model:directory==="/project/b"?"openai/new-project":"openai/old-project",variant:"high"});
   if(path.endsWith("/agents"))return Response.json([{name:"plan",description:directory}]);
   if(path.endsWith("/skills"))return Response.json([{name:"review",description:directory}]);
   return new Response(null,{status:404});
  }});
  const config={...loadConfig(["--config","/nonexistent/agent-directory-catalogs"],{}),endpoint:`ws://127.0.0.1:${server.port}/session`,authToken:"operator",pairingToken:mode==="paired"?"guest":undefined},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
  try{
   const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};model.update({workspaces:[workspace],activeWorkspace:{...workspace,root_dir:"/project/b"}});
   await model.refreshCatalogs();
   if(mode==="desktop"){
    expect(model.state.catalogsError).toBeUndefined();expect(model.selection()).toMatchObject({agent:"plan",model:"openai/new-project",thinking:"high"});
    expect(model.state.skills[0]?.description).toBe("/project/b");expect(model.state.agents[0]?.description).toBe("/project/b");
   }else expect(model.state.catalogsError).toContain("not authorized");
   const project=requests.filter(request=>/\/(config\/defaults|agents|skills)$/.test(request.path));
   expect(project).toHaveLength(3);expect(project.every(request=>request.directory==="/project/b"&&request.path.startsWith(mode==="desktop"?"/agent/system/":"/agent/workspaces/a/"))).toBe(true);
   expect(requests.filter(request=>request.path.endsWith("/providers/configured"))).toEqual([{path:"/agent/workspaces/a/v2/providers/configured",directory:null}]);
   expect(requests.filter(request=>request.path==="/agent/system/info")).toHaveLength(mode==="paired"?0:1);
   if(mode==="desktop"){
    model.update({threads:[{session_id:"moved",title:"Moved",directory:"/project/c"}],surfaces:[{id:"agent:moved",kind:"agent",sessionId:"moved",title:"Moved",workspaceId:"a"}],activeSurfaceId:"agent:moved"});
    await model.refreshCatalogs({maxAgeMs:30000});expect(model.state.agents[0]?.description).toBe("/project/c");expect(model.state.skills[0]?.description).toBe("/project/c");
    model.update({activeSurfaceId:undefined});await model.refreshCatalogs({maxAgeMs:30000});
    expect(model.state.agents[0]?.description).toBe("/project/b");expect(model.selection()).toMatchObject({model:"openai/new-project",thinking:"high"});
   }
  }finally{model.dispose();client.stop();server.stop(true);}
 }
});
function fixture(options:Parameters<typeof createAgentDaemon>[0]){const daemon=createAgentDaemon(options),config={...loadConfig(["--config","/nonexistent/agent-home-defaults"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();return{daemon,model,close(){model.dispose();client.stop();daemon.stop();}};}
test("empty Agent home resolves configured agent/model/thinking through workspace SDK and explicit choice overrides defaults without changing conversation state",async()=>{
 const f=fixture({deferCreations:false,agentConfigDefaults:{a:{defaultAgent:"plan",model:"openai/advanced",variant:"high"}}});try{await until(()=>f.model.state.activeWorkspace?.id==="a"&&!f.model.state.agentDefaultsLoading);f.model.newAgentHome();expect(f.model.selection()).toMatchObject({agent:"plan",model:"openai/advanced",thinking:"high"});expect(f.daemon.httpRequests.some(r=>r.path.endsWith("/v2/config/defaults")&&r.directory==="/project/a")).toBe(true);expect(f.daemon.requests.some(r=>r.tag==="CreateThread")).toBe(false);
  f.model.setSelection("model","other/chosen");f.model.setSelection("thinking","low");await f.model.refreshCatalogs();expect(f.model.selection()).toMatchObject({agent:"plan",model:"other/chosen",thinking:"low"});f.model.openThread(f.daemon.threads[0]);await until(()=>f.model.selection("one").thinking==="medium");expect(f.model.selection("one")).toMatchObject({agent:"build",model:"openai/base",thinking:"medium"});expect(f.model.selection()).toMatchObject({model:"other/chosen",thinking:"low"});
 }finally{f.close();}
});
test("configured model wins, otherwise original connected-provider fallback prefers non-OpenCode in catalog order",async()=>{
 const catalog={providers:[{id:"opencode",name:"OpenCode",models:{free:{id:"free",name:"Free"}}},{id:"openai",name:"OpenAI",models:{base:{id:"base",name:"Base"}}},{id:"xai",name:"xAI",models:{code:{id:"code",name:"Code"}}}],default:{opencode:"free",openai:"base",xai:"code"},connected:["xai","opencode"]};
 const f=fixture({agentProviderCatalog:catalog,agentConfigDefaults:{a:{defaultAgent:"build",model:null,variant:"medium"},b:{defaultAgent:"plan",model:"openai/explicit",variant:"high"}}});try{await until(()=>f.model.state.activeWorkspace?.id==="a"&&!f.model.state.agentDefaultsLoading);expect(f.model.selection()).toMatchObject({model:"xai/code",thinking:"medium"});await f.model.activateWorkspace(f.daemon.workspaces[1]);await until(()=>!f.model.state.agentDefaultsLoading);expect(f.model.selection()).toMatchObject({agent:"plan",model:"openai/explicit",thinking:"high"});}finally{f.close();}
});
test("late workspace defaults cannot overwrite current home and cached return never restarts loading",async()=>{
 const f=fixture({agentConfigDefaults:{a:{defaultAgent:"plan",model:"openai/a",variant:"high"},b:{defaultAgent:"build",model:"openai/b",variant:"low"}},agentDefaultsDelayMs:{a:100,b:5}});try{await until(()=>f.model.state.activeWorkspace?.id==="a"&&f.model.state.agentDefaultsLoading===true);await f.model.activateWorkspace(f.daemon.workspaces[1]);await until(()=>!f.model.state.agentDefaultsLoading);expect(f.model.selection().model).toBe("openai/b");await Bun.sleep(120);expect(f.model.selection().model).toBe("openai/b");await f.model.activateWorkspace(f.daemon.workspaces[0]);await until(()=>!f.model.state.agentDefaultsLoading);expect(f.model.selection().model).toBe("openai/a");await f.model.activateWorkspace(f.daemon.workspaces[1]);await until(()=>!f.model.state.agentDefaultsLoading);const publications:boolean[]=[];const off=f.model.subscribe(()=>{if(f.model.state.activeWorkspace?.id==="a")publications.push(!!f.model.state.agentDefaultsLoading);});await f.model.activateWorkspace(f.daemon.workspaces[0]);await Bun.sleep(130);off();expect(publications.length).toBeGreaterThan(0);expect(publications.every(loading=>!loading)).toBe(true);expect(f.model.selection().model).toBe("openai/a");}finally{f.close();}
});
test("first home submission sends hydrated configured selection to SDK creation and prompt",async()=>{const f=fixture({deferCreations:false,agentConfigDefaults:{a:{defaultAgent:"plan",model:"openai/advanced",variant:"high"}}});try{await until(()=>f.model.state.activeWorkspace?.id==="a"&&!f.model.state.agentDefaultsLoading);f.model.newAgentHome();expect(await f.model.submitNewPrompt("Use my configured defaults")).toBe(true);expect(f.daemon.requests.find(request=>request.tag==="CreateThread")?.body).toMatchObject({agent:"plan",model:{providerId:"openai",id:"advanced",variant:"high"}});expect(f.daemon.requests.find(request=>request.tag==="SubmitPrompt")?.body).toMatchObject({mode:"plan",model:"openai/advanced",thinking:"high"});}finally{f.close();}});
