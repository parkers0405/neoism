import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import * as native from "../../src/native-core";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const original=native.coreCall;let engineId=0,failBrowser=false;
const opened:string[]=[],errors:unknown[]=[];
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args?:Record<string,unknown>)=>{const value=original<any>(op,args);if(op==="agenttimeline.new")engineId=value.id;return value;}}));
mock.module("../../src/menu-actions/web-frontend",()=>({openExternalUrl:async(url:string)=>{if(failBrowser)throw Error("Browser failed");opened.push(url);}}));
const {AgentTimelineView}=await import("../../src/agent/AgentTimelineView");
const config=loadConfig(["--config","/nonexistent/link-click"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
model.update({threads:[{session_id:"links",title:"Links"}],history:{links:[{id:"answer",role:"Assistant",kind:"Assistant",text:"Source: [GitHub: anomalyco/opentui](https://github.com/anomalyco/opentui)."}]}});
try{
 root.render(()=><AgentTimelineView sessionId="links" model={model} config={config} report={error=>errors.push(error)} openFile={()=>{throw Error("External link was routed to a file");}}/>);await host.pump(root);
 const conversation=host.find("conversation")!;
 const frame=original<{commands:Array<{kind:string;text?:string;x?:number;y?:number;size?:number}>}>("agenttimeline.frame",{id:engineId,includeCommands:true,compactPaint:false});
 const link=frame.commands.find(command=>command.kind==="text"&&command.text?.includes("anomalyco/opentui"));assert.ok(link);
 const shifted={x:610,y:270,width:960,height:680},bounds=host.renderer.getElementBounds!.bind(host.renderer);
 host.renderer.getElementBounds=id=>id===conversation.id?shifted:bounds(id);
 const click=(button:number)=>{const event={elementId:conversation.id,button,x:shifted.x+link.x!+3,y:shifted.y+link.y!+(link.size||14)/2};root.dispatch({...event,eventType:"mouseDown"});root.dispatch({...event,eventType:"mouseUp"});};
 click(2);assert.equal(opened.length,0);
 click(0);await host.pump(root);assert.deepEqual(opened,["https://github.com/anomalyco/opentui"]);
 assert.deepEqual(errors,[]);
 failBrowser=true;click(0);await host.pump(root);assert.match(String(errors[0]),/Browser failed/);
 console.log(JSON.stringify({noWindow:true,translatedLinkOpened:true,rightClickIgnored:true,browserErrorsReported:true}));
}finally{root.unmount();model.dispose();client.stop();}
