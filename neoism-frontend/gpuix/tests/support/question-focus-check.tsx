import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/question-focus"],{}),endpoint:daemon.endpoint,reducedMotion:true,cursorBlink:false,agentStars:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
let stage="setup";
const until=async(check:()=>boolean)=>{const end=Date.now()+5000;while(!check()){await host.pump(root);if(Date.now()>end)throw Error(`Question stalled: ${stage}`);await Bun.sleep(5);}await host.pump(root);};
const input=()=>host.find("inline-picker-input")!;
const key=async(key:string)=>{assert.equal(host.focused,input().id,`Answer input must own focus: ${stage}`);root.dispatch({elementId:host.focused!,eventType:"keyDown",key,modifiers:{ctrl:false,alt:false,shift:false,cmd:false}});await host.pump(root);};
const row=(label:string)=>[...host.nodes.values()].find(n=>n.props.role==="option"&&n.props["aria-label"]===label)!;
try {
 client.start();await until(()=>client.state.phase==="connected");
 const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};
 model.update({activeWorkspace:workspace,workspaces:[workspace],loading:false,threads:daemon.threads});model.openThread(daemon.threads[0]);
 model.setComposerDraft("one",{text:"Keep my draft",attachments:[]});
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await until(()=>!!host.find("composer"));
 model.update({questions:[{sessionId:"one",requestId:"focus-question",responding:false,request:{id:"focus-question",sessionID:"one",questions:[
  {question:"Color?",options:[{label:"Red"},{label:"Blue"}]},
  {question:"Size?",options:[{label:"Small"},{label:"Large"}]},
  {question:"When?",options:[{label:"Today"}]}
 ]}}]});
 stage="keyboard suggestion";await until(()=>!!input()&&host.focused===input().id);
 await key("ArrowDown");assert.equal(row("Blue").props["aria-selected"],true);
 await key("ArrowUp");assert.equal(row("Red").props["aria-selected"],true);
 await key("ArrowDown");await key("Enter");await until(()=>!!row("Large"));
 stage="mouse suggestion";assert.equal(row("Large").props["data-preserve-keyboard-focus"],true);
 root.dispatch({elementId:row("Large").id,eventType:"mouseDown",button:0});await host.pump(root);
 await until(()=>!!row("Today"));assert.equal(host.focused,input().id);
 stage="custom answer";root.dispatch({elementId:host.focused!,eventType:"change",value:"Tomorrow afternoon"});await host.pump(root);
 assert.ok(row("Answer: Tomorrow afternoon"));await key("Enter");
 stage="retry";await until(()=>!!model.state.questions[0]?.error);await key("Enter");
 stage="complete";await until(()=>!model.state.questions.length&&host.focused===host.find("composer")?.id);
 const replies=daemon.requests.filter(r=>r.tag==="AnswerQuestion");assert.equal(replies.length,2);
 for(const reply of replies)assert.deepEqual(reply.body.answers,[["Blue"],["Large"],["Tomorrow afternoon"]]);
 assert.equal(model.state.composerDrafts.one.text,"Keep my draft");
 console.log("question focus, suggestions, custom answer, retry and composer restoration passed");
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}
