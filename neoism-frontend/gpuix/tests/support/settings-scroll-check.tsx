import assert from "node:assert/strict";
import {Effect} from "effect";
import {createRoot} from "@gpuix/solid";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {ChromeActionsController} from "../../src/menu-actions/ChromeActionsController";
import {ChromeActionsView} from "../../src/menu-actions/ChromeActionsView";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const config=loadConfig(["--config","/nonexistent/settings-scroll"],{}),client=new DaemonConnection(config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const descriptors=Array.from({length:60},(_,index)=>({path:`test.option${index}`,label:`Scroll fixture option ${index}`,description:`Original setting ${index}`,value_kind:"boolean",default:false,static_suggestions:[],runtime_suggestions:[],options:[],constraints:{nullable:false},accepted_kinds:["boolean"],extensible:false,category:"general",control:{kind:"toggle"}}));
client.request=((_family:string,message:unknown)=>Effect.succeed(message==="GetConfig"?{Config:{value:{}}}:{ConfigSchema:{descriptors}}))as typeof client.request;
const model=new WorkspaceModel(client,config),controller=new ChromeActionsController({model,config,reload:()=>{}});
const position=()=>Number((controller.frame?.snapshot as {scroll:number}).scroll);
const labelY=()=>controller.frame!.commands.filter(op=>op.kind==="text"&&op.text.startsWith("Scroll fixture option")).map(op=>op.kind==="text"?op.y:0);
const scroll=(deltaY:number,precise:boolean)=>{root.dispatch({elementId:host.find("chrome-action-overlay")!.id,eventType:"scroll",deltaY,precise,x:400,y:400});root.flush();};
try{
 await controller.open("OpenSettings");root.render(()=><ChromeActionsView controller={controller} config={config}/>);await host.pump(root);
 assert.equal(position(),0);const before=labelY();assert.ok(before.length>2);
 scroll(-120,true);assert.equal(position(),120,"Negative compositor finger delta must move down the Settings content like Code");
 const after=labelY();assert.ok(after.length>0);assert.notDeepEqual(after,before,"Actual original painted rows must move, not just an adapter counter");
 scroll(30,true);assert.equal(position(),90);scroll(-1,false);assert.equal(position(),138,"Original settings line-wheel step is48px");
 scroll(10000,true);assert.equal(position(),0);scroll(100,true);assert.equal(position(),0);
 console.log(JSON.stringify({noWindow:true,actualSettingsView:true,preciseDirection:true,wheel48Pixels:true,visibleRowsMove:true,topClamped:true}));
}finally{root.unmount();controller.dispose();model.dispose();}
