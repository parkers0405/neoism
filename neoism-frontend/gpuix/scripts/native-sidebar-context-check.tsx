// Isolated real window: no production daemon, files, chats or recovery touched.
import assert from "node:assert/strict";
import * as Effect from "effect/Effect";
import {createRenderer,render} from "@gpuix/solid";
import {startFrameLoop} from "@gpuix/native/runtime";
import {App as Automation,InProcessBackend,liveRendererAsTest} from "@gpuix/native/automation";
import {App} from "../src/ui/App";
import {AppVisualState} from "../src/ui/AppVisualState";
import {WorkspaceModel} from "../src/model";
import {DaemonConnection} from "../src/daemon/client";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {installDisplayClock} from "../src/animation/display-clock";
const config={...loadConfig(["--config","/nonexistent/context-native"],{}),reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),requests:any[]=[],listeners=new Set<(message:any)=>void>();
const subscribe=model.subscribeAgentEvents;model.subscribeAgentEvents=listener=>{listeners.add(listener);const stop=subscribe(listener);return()=>{listeners.delete(listener);stop();};};model.sendAgent=()=>{};
model.agentSdk.forSession=((workspaceId:string,sessionId:string)=>({sessions:{pin:async(id:string,pinned:boolean)=>requests.push({pin:{workspaceId,sessionId,id,pinned}})}}))as unknown as typeof model.agentSdk.forSession;
const entries=[{name:"folder",is_dir:true},{name:"file.ts",is_dir:false}];
client.request=((family:string,message:any,workspaceRoot:string)=>{requests.push({family,message,root:workspaceRoot});return Effect.succeed(message.ReadFile?{FileContent:{path:message.ReadFile.path,bytes:[]}}:message.ListDir?{DirListing:{path:message.ListDir.path,entries:message.ListDir.path?[]:entries}}:message.CreateFile?{FileCreated:{path:message.CreateFile.name,is_dir:false}}:{Error:{message:"Fixture action unavailable"}});})as typeof client.request;
const workspace={id:"fixture",host_id:"fixture",title:"Fixture",root_dir:"/fixture"};
model.update({activeWorkspace:workspace,workspaces:[workspace],directories:{"":{path:"",entries}},expanded:new Set([""]),loading:false,ui:{sidebar:"files",openWorkspaceIds:[workspace.id]}});
const renderer=createRenderer();prepareBundledFonts(renderer);renderer.init({title:"Neoism sidebar context check",appId:"ai.neoism.gpuix.sidebar-context-check",width:1100,height:800});
const clock=installDisplayClock(renderer,console.error),loop=startFrameLoop(renderer),root=render(()=><App model={model} config={config} visuals={visuals}/>,{renderer});
const app=new Automation(new InProcessBackend(liveRendererAsTest(renderer))),menu=()=>app.getByTestId("sidebar-context");
const until=async(check:()=>boolean|Promise<boolean>)=>{const end=Date.now()+5000;while(!await check()){if(Date.now()>end)throw Error("Native context check timed out");await Bun.sleep(10);}};
const right=async(point:{x:number;y:number})=>{await app.mouse.down(point,{button:2});await app.mouse.up(point,{button:2});await menu().waitFor();};
let failed=false,stage="file row";
try{
 await app.getByTestId("file:file.ts").waitFor();stage="file menu";await right(await app.getByTestId("file:file.ts").center());assert.equal(model.state.surfaces.filter(surface=>surface.kind==="file").length,0);
 stage="delete confirmation";let previousMenu=(await menu().element()).id;await menu().press("d");await until(async()=>(await menu().element()).id!==previousMenu);await menu().press("escape");await until(async()=>await menu().count()===0);assert.equal(requests.filter(r=>r.message?.Delete).length,0);
 stage="folder create";await right(await app.getByTestId("file:folder").center());previousMenu=(await menu().element()).id;await menu().press("n");await until(async()=>(await menu().element()).id!==previousMenu);await menu().press("z");await menu().press("enter");await until(()=>requests.some(r=>r.message?.CreateFile));assert.deepEqual(requests.find(r=>r.message?.CreateFile).message,{CreateFile:{dir:"folder",name:"z"}});
 stage="root menu";const tree=await app.getByTestId("file-tree").bounds();await right({x:tree.x+30,y:tree.y+tree.height-30});await menu().press("escape");
 stage="open conversations";await until(async()=>await menu().count()===0);await Bun.sleep(100);const toggle=await app.getByTestId("toggle-conversations").center();await app.mouse.down(toggle);await app.mouse.up(toggle);await app.getByTestId("conversation-list").waitFor();
 for(const listener of listeners)listener({ThreadList:{threads:[{session_id:"one",title:"Menu fixture chat",directory:"/fixture",updated_at:1}],requested_cursor:null,next_cursor:null}});
 await Bun.sleep(150);const panel=await app.getByTestId("conversation-list").bounds();
 // Rust exposes its selected row through the shared sidebar cursor. Move down
 // from New Chat past Search/Projects, then use the painted row coordinates.
 const sidebar=app.getByTestId("conversation-list");await sidebar.click();for(let i=0;i<3;i++)await sidebar.press("down");
 await Bun.sleep(150);
 // Locate the row by probing right-clicks without invoking any left action.
 stage="chat context";for(let y=panel.y+45;y<panel.y+Math.min(panel.height,350)&&await menu().count()===0;y+=15){await app.mouse.down({x:panel.x+80,y},{button:2});await app.mouse.up({x:panel.x+80,y},{button:2});}
 await menu().waitFor();await menu().press("p");await until(()=>requests.some(r=>r.pin));assert.deepEqual(requests.find(r=>r.pin).pin,{workspaceId:"fixture",sessionId:"one",id:"one",pinned:true});
 console.log("Native sidebar context passed: real right-click targeting, confirmation cancellation, folder creation, empty-root menu, and chat pin.");
}catch(error){failed=true;console.error(stage,error);await app.screenshot({path:"/tmp/neoism-context-check-failure.png"}).catch(()=>{});}
finally{root.unmount();visuals.dispose();model.dispose();client.stop();loop.stop();clock.dispose();}
process.exit(failed?1:0);
