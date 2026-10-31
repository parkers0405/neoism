import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
test("chat menu mutations use the clicked identity and scope, publish acknowledgments and close deleted chat tabs",async()=>{
 const config=loadConfig(["--config","/nonexistent/agent-context"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const events:unknown[]=[],requests:unknown[]=[];
 model.agentSdk.forSession=((workspaceId:string,sessionId:string)=>({sessions:{
  pin:async(id:string,pinned:boolean)=>{requests.push({workspaceId,sessionId,id,pinned});},
  update:async(id:string,body:unknown)=>{requests.push({workspaceId,sessionId,id,body});},
  delete:async(id:string)=>{requests.push({workspaceId,sessionId,id,deleted:true});}
 }}))as unknown as typeof model.agentSdk.forSession;
 const workspace={id:"a",host_id:"host",title:"A",root_dir:"/a"};model.update({activeWorkspace:workspace,workspaces:[workspace],loading:false,threads:[{session_id:"one",title:"One",directory:"/a"},{session_id:"two",title:"Two",directory:"/a"}]});
 const stop=model.subscribeAgentEvents(event=>events.push(event));
 try{
  const tab=model.openThread(model.state.threads[0]);
  await model.mutateSidebarSession("a","one",{kind:"pin",pinned:true});
  model.update({activeWorkspace:{...workspace,id:"b"}});
  await model.mutateSidebarSession("a","one",{kind:"rename",title:"Renamed"});
  expect(model.state.surfaces.find(surface=>surface.id===tab)?.title).toBe("Renamed");expect(model.state.threads.find(thread=>thread.session_id==="two")?.title).toBe("Two");
  await model.mutateSidebarSession("a","one",{kind:"delete"});expect(model.state.surfaces.some(surface=>surface.id===tab)).toBe(false);expect(model.state.threads.map(thread=>thread.session_id)).toEqual(["two"]);
  expect(requests).toEqual([{workspaceId:"a",sessionId:"one",id:"one",pinned:true},{workspaceId:"a",sessionId:"one",id:"one",body:{title:"Renamed"}},{workspaceId:"a",sessionId:"one",id:"one",deleted:true}]);
  expect(events).toContainEqual({ThreadUpdated:{session_id:"one",pinned:true}});expect(events).toContainEqual({ThreadDeleted:{session_id:"one"}});
 }finally{stop();model.dispose();client.stop();}
});
