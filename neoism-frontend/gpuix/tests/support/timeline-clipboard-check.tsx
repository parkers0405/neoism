import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import * as native from "../../src/native-core";
const realCoreCall=native.coreCall;let timelineId=0;const scrollInputs:Record<string,unknown>[]=[];
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args:Record<string,unknown>={})=>{
 if(op==="agenttimeline.scroll")scrollInputs.push(args);
 const result=realCoreCall<any>(op,op==="agenttimeline.new"?{...args,compactPaint:false,canvasPaint:false}:args);
 if(op==="agenttimeline.new")timelineId=result.id;return result;
}}));
const copies:string[]=[];
mock.module("../../src/clipboard",()=>({writeClipboard:async(text:string)=>{copies.push(text);},readClipboard:async()=>copies.at(-1)||"",readClipboardImage:async()=>undefined}));
const {App}=await import("../../src/ui/App");
const {WorkspaceModel}=await import("../../src/model");
const {DaemonConnection}=await import("../../src/daemon/client");
const {loadConfig}=await import("../../src/config");
const {createAgentDaemon}=await import("./agent-daemon");
const {createRecordingAppRenderer}=await import("./full-app-recording-renderer");
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/timeline-clipboard"],{}),endpoint:daemon.endpoint,reducedMotion:true,cursorBlink:false,agentStars:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const until=async(check:()=>boolean)=>{const end=Date.now()+5000;while(!check()){await host.pump(root);if(Date.now()>end)throw Error("Timeline clipboard test stalled");await Bun.sleep(5);}await host.pump(root);};
try{
 model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);model.openThread(daemon.threads[0]);
 await until(()=>model.state.history.one!==undefined);
 model.update({history:{...model.state.history,one:[{id:"copy-answer",role:"Assistant",kind:"Assistant",text:"Clipboard selection works across platforms."}]}});
 root.render(()=><App model={model} config={config}/>);await until(()=>!!host.find("conversation")&&!!host.find("composer"));
 const composer=host.find("composer")!;host.renderer.focusElement?.(composer.id);await host.pump(root);
 root.dispatch({elementId:composer.id,eventType:"change",value:"Keep this draft"});await host.pump(root);
 let line:any;await until(()=>{const frame=realCoreCall<any>("agenttimeline.frame",{id:timelineId});line=frame.commands.find((c:any)=>c.kind==="text"&&c.text==="Clipboard");return !!line;});
 const conversation=host.find("conversation")!,box=host.renderer.getElementBounds!(conversation.id)!;
 const x=box.x+line.x+1,y=box.y+line.y+(line.size||14)/2;
 root.dispatch({elementId:conversation.id,eventType:"mouseDown",button:0,x,y});await host.pump(root);
 const capture=host.find("timeline-selection-capture")!;assert.ok(capture,"Selection captures release even outside the transcript");
 const captureLayer=[...host.nodes.values()].find(node=>node.children.includes(capture.id));
 assert.equal(captureLayer?.style.backgroundColor,"transparent","Window-wide selection capture must not paint the default opaque anchored background");
 root.dispatch({elementId:capture.id,eventType:"mouseMove",pressedButton:0,x:x+240,y});
 root.dispatch({elementId:capture.id,eventType:"mouseUp",button:0,x:x+240,y});await until(()=>copies.length===1);
 assert.match(copies[0],/Clipboard selection/);assert.equal(host.find("timeline-selection-capture"),undefined);assert.equal(host.focused,composer.id);
 for(const key of ["copy","c"]){const index=copies.length;root.dispatch({elementId:composer.id,eventType:"keyDown",key,modifiers:{ctrl:false,cmd:index===2,alt:false,shift:false}});await until(()=>copies.length===index+1);assert.equal(copies.at(-1),copies[0]);}
 assert.equal(model.state.composerDrafts.one.text,"Keep this draft");
 // Forwarded Super+V may arrive with both Control and Super held. Native
 // selection replacement must handle that raw chord, not image-only paste.
 const replayed:string[]=[];
 (host.renderer as any).simulateKeystrokes=(key:string)=>replayed.push(key);
 for(const modifiers of [{ctrl:true,cmd:true,shift:false,alt:false},{ctrl:true,shift:true,cmd:false,alt:false}]){
  root.dispatch({elementId:composer.id,eventType:"keyDown",key:"v",modifiers});await host.pump(root);
 }
 root.dispatch({elementId:composer.id,eventType:"keyDown",key:"insert",modifiers:{shift:true,ctrl:false,cmd:false,alt:false}});await host.pump(root);
 assert.deepEqual(replayed,[process.platform==="darwin"?"cmd-v":"ctrl-v",process.platform==="darwin"?"cmd-v":"ctrl-v",process.platform==="darwin"?"cmd-v":"ctrl-v"]);
 root.dispatch({elementId:composer.id,eventType:"keyDown",key:"paste"});await host.pump(root);
 assert.equal(replayed.length,3,"Image-only fallback must not replay and loop");

 // Missing mouse-up is recovered on the next button-free movement.
 root.dispatch({elementId:conversation.id,eventType:"mouseDown",button:0,x,y});await host.pump(root);
 const second=host.find("timeline-selection-capture")!;root.dispatch({elementId:second.id,eventType:"mouseMove",pressedButton:0,x:x+240,y});root.dispatch({elementId:second.id,eventType:"mouseMove",x:x+240,y});await until(()=>copies.length===4);
 assert.equal(host.find("timeline-selection-capture"),undefined);
 model.rememberComposerPrompt({text:"First submitted prompt",attachments:[]});model.rememberComposerPrompt({text:"Second submitted prompt",attachments:[]});
 const historyKey=async(key:string)=>{root.dispatch({elementId:composer.id,eventType:"keyDown",key});await host.pump(root);};
 await historyKey("up");assert.equal(model.state.composerDrafts.one.text,"Second submitted prompt");
 await historyKey("up");assert.equal(model.state.composerDrafts.one.text,"First submitted prompt");
 await historyKey("down");assert.equal(model.state.composerDrafts.one.text,"Second submitted prompt");
 await historyKey("down");assert.equal(model.state.composerDrafts.one.text,"Keep this draft");
 for(const precise of [true,false]) {
  root.dispatch({elementId:conversation.id,eventType:"scroll",deltaX:0,deltaY:-7.75,precise});
  const delivered=scrollInputs.at(-1)!;
  assert.equal(delivered.deltaY,precise?-15.5:-7.75);assert.equal(delivered.precise,precise);assert.equal(delivered.deferFrame,true,"Scroll keeps the display-frame scheduler");
 }
 console.log("timeline selection copy, release capture, native Copy, Cmd+C and focus passed");
}finally{root.unmount();model.dispose();client.stop();daemon.stop();}
