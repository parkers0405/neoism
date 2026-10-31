import assert from "node:assert/strict";
import * as Effect from "effect/Effect";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {dimensions} from "../../src/design/generated";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/context-host"],{}),reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"},requests:any[]=[];
const entries=[{name:"folder",is_dir:true},{name:"file.ts",is_dir:false}];
client.request=((family:string,message:any,workspaceRoot:string)=>{requests.push({family,message,root:workspaceRoot});return Effect.succeed(message.ReadFile?{FileContent:{path:message.ReadFile.path,bytes:[]}}:message.ListDir?{DirListing:{path:message.ListDir.path,entries:message.ListDir.path?[]:entries}}:message.CreateFile?{FileCreated:{path:`${message.CreateFile.dir}/${message.CreateFile.name}`,is_dir:false}}:{Error:{message:"Unused fixture action"}});})as typeof client.request;
model.update({activeWorkspace:workspace,workspaces:[workspace],directories:{"":{path:"",entries}},expanded:new Set([""]),loading:false,ui:{sidebar:"files",openWorkspaceIds:["a"]}});
const pump=()=>host.pump(root);
const key=async(key:string)=>{const menu=host.find("sidebar-context");assert.ok(menu);root.dispatch({elementId:menu.id,eventType:"keyDown",key,keyChar:key.length===1?key:undefined,modifiers:{ctrl:false,alt:false,cmd:false,shift:false}});await pump();};
const right=async(index:number)=>{const tree=host.find("file-tree")!;root.dispatch({elementId:tree.id,eventType:"mouseDown",button:2,x:50,y:90+(index+.5)*dimensions.treeRowHeight*config.scale});await pump();assert.ok(host.find("sidebar-context"));};
try{
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await pump();
 await right(1);assert.equal(host.find("sidebar-context")?.props["aria-label"],"file.ts");assert.equal(model.state.surfaces.filter(s=>s.kind==="file").length,0,"Right click must not open the file");
 await key("d");assert.equal(host.find("sidebar-context")?.props.role,"dialog");assert.equal(requests.filter(r=>r.message.Delete).length,0);await key("escape");assert.ok(!host.find("sidebar-context"));
 await right(0);await key("n");await key("x");await key("enter");await pump();assert.deepEqual(requests.find(r=>r.message.CreateFile)?.message,{CreateFile:{dir:"folder",name:"x"}});
 requests.length=0;await right(8);assert.equal(host.find("sidebar-context")?.props["aria-label"],"/project/a");await key("n");await key("y");await key("enter");await pump();assert.deepEqual(requests.find(r=>r.message.CreateFile)?.message,{CreateFile:{dir:"",name:"y"}});
 await right(1);await key("h");assert.ok(!host.find("sidebar-context"));
 console.log("Sidebar context host passed: right-click does not open; confirmation cancels; folder and empty-root targets are exact.");
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();}
