import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {localPresenceIdentity} from "../src/agent/local-presence";
import {coreCall} from "../src/native-core";
import type {PaintCommand} from "../src/ui/PaintLayer";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {createAgentDaemon} from "./support/agent-daemon";

test("profile uses the desktop's canonical precedence, device identity and Unicode name bound",()=>{
 const config={presenceDisplayName:"  Configured person  "},device="actual-device";
 const configured=localPresenceIdentity(config,{USER:"participant"},device);expect(configured.displayName).toBe("Configured person");expect(configured.peerId).toBe("participant@actual-device");
 const override=localPresenceIdentity(config,{USER:"participant",NEOISM_DISPLAY_NAME:"  Environment person  "},device);expect(override.displayName).toBe("Environment person");expect(override.color).toEqual(configured.color);
 expect(localPresenceIdentity(config,{USERNAME:"windows-user",NEOISM_DISPLAY_NAME:" "},device).displayName).toBe("Configured person");
 expect(localPresenceIdentity({}, {}, device).displayName).toBe(device);expect(localPresenceIdentity({}, {}, " ").displayName).toBe("neoism");
 expect([...localPresenceIdentity({presenceDisplayName:"😀".repeat(40)}, {}, device).displayName]).toHaveLength(32);
 expect(loadConfig([],{}, {presence:{"display-name":"From actual config"}}).presenceDisplayName).toBe("From actual config");
});

function hoverLabel(author:string|undefined,localName:string){
 const{id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"profile",localName,theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF"});
 try{
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"profile",messages:[{id:"user",role:"User",kind:"User",text:"Profile identity fixture",...(author?{author}:{})}],next_cursor:null}}});
  let frame=coreCall<{commands:PaintCommand[];animating:boolean}>("agenttimeline.layout",{id,width:900,height:600});
  const end=Date.now()+1500;while(frame.animating&&Date.now()<end){Bun.sleepSync(8);frame=coreCall("agenttimeline.frame",{id});}
  const cells=frame.commands.filter((command):command is Extract<PaintCommand,{kind:"rect"}>=>command.kind==="rect"&&command.rect[2]>0&&command.rect[2]<5&&command.rect[3]>0&&command.rect[3]<5);
  expect(cells.length).toBeGreaterThan(0);
  const cell=cells[Math.floor(cells.length/2)];
  const hover=coreCall<{commands:PaintCommand[]}>("agenttimeline.pointer",{id,kind:"move",x:cell.rect[0]+cell.rect[2]/2,y:cell.rect[1]+cell.rect[3]/2});
  return hover.commands.flatMap(command=>command.kind==="text"?[command.text]:[]);
 }finally{coreCall("agenttimeline.dispose",{id});}
}
test("actual original message avatar labels local sender You and preserves remote/unknown identities",()=>{
 expect(hoverLabel("Configured person","Configured person")).toContain("You");
 expect(hoverLabel("Remote participant","Configured person")).toContain("Remote participant");
 expect(hoverLabel(undefined,"Configured person")).toContain("Unknown user");
},10000);

test("actual model SDK prompt sends the same canonical local display name as the renderer",async()=>{
 const daemon=createAgentDaemon({deferCreations:false}),config={...loadConfig([],{}, {presence:{"display-name":"Configured person"}}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 try{
  model.start();client.start();const end=Date.now()+4000;while(!model.state.activeWorkspace||model.state.loading){if(Date.now()>end)throw Error("Identity prompt fixture startup timeout");await Bun.sleep(4);}
  expect(await model.submitPrompt("one","Author identity fixture")).toBe(true);
  expect(daemon.requests.find(request=>request.tag==="SubmitPrompt")?.body.author).toBe(localPresenceIdentity(config).displayName);
  expect(daemon.ptyRequests).toHaveLength(0);
 }finally{model.dispose();client.stop();daemon.stop();}
});
