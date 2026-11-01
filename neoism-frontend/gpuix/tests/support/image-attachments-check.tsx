import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const png=Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGMwSjnxH4QZYAwAT5AJde03lN8AAAAASUVORK5CYII=","base64");
let clipboard:Uint8Array|undefined=png;
mock.module("../../src/clipboard",()=>({readClipboardImage:async()=>clipboard,readClipboard:async()=>"text",writeClipboard:async()=>{}}));
const {App}=await import("../../src/ui/App");
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/image-attachments"],{}),endpoint:daemon.endpoint,reducedMotion:true,cursorBlink:false,agentStars:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer();
const uploaded:Uint8Array[]=[];host.renderer.setImage=(_id,bytes)=>{uploaded.push(bytes);};
const root=createRoot(host.renderer);let stage="setup",replayed=0;
(host.renderer as typeof host.renderer & {simulateKeystrokes:(keys:string)=>void}).simulateKeystrokes=(keys)=>{
 assert.equal(keys,process.platform==="darwin"?"cmd-v":"ctrl-v");replayed++;
 // Native Paste consumes the canonical chord. Only an image clipboard emits
 // the image-only fallback; text is inserted by the native input itself.
 if(clipboard)root.dispatch({elementId:host.find("composer")!.id,eventType:"keyDown",key:"paste"});
};
const until=async(check:()=>boolean)=>{const end=Date.now()+5000;while(!check()){await host.pump(root);if(Date.now()>end)throw Error(`Attachment test stalled: ${stage}`);await Bun.sleep(5);}await host.pump(root);};
const paste=async(cmd=false)=>{root.dispatch({elementId:host.find("composer")!.id,eventType:"keyDown",key:"v",modifiers:{ctrl:!cmd,cmd,alt:false,shift:false}});await host.pump(root);};
const preview=()=>[...host.nodes.values()].find(n=>String(n.props.testId).startsWith("attachment-preview:"))!;
try{
 client.start();await until(()=>client.state.phase==="connected");
 const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};model.update({activeWorkspace:workspace,workspaces:[workspace],loading:false});model.newAgentHome();
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await until(()=>!!host.find("composer"));
 host.renderer.focusElement?.(host.find("composer")!.id);await host.pump(root);
 stage="paste screenshot";await paste();await until(()=>!!preview());
 assert.equal(replayed,1,"A forwarded image shortcut invokes native Paste once");assert.ok(uploaded.some(bytes=>Buffer.from(bytes).equals(png)),"Thumbnail uploads the actual screenshot");
 root.dispatch({elementId:preview().id,eventType:"click"});stage="open preview";await until(()=>!!host.find("image-preview-modal")&&host.focused===host.find("image-preview-modal")?.id);
 assert.ok(host.find("image-preview-full"));root.dispatch({elementId:host.find("image-preview-close")!.id,eventType:"click"});await until(()=>!host.find("image-preview-modal"));
 assert.equal(host.focused,host.find("composer")!.id,"Closing preview restores typing focus");
 root.dispatch({elementId:preview().id,eventType:"click"});await host.pump(root);root.dispatch({elementId:host.find("image-preview-modal")!.id,eventType:"keyDown",key:"Escape"});await until(()=>!host.find("image-preview-modal"));
 stage="mac shortcut";await paste(true);await until(()=>Object.values(model.state.composerDrafts).some(d=>d.attachments.length===2));
 const draft=Object.values(model.state.composerDrafts).find(d=>d.attachments.length===2)!;
 assert.notEqual(draft.attachments[0].token,draft.attachments[1].token);assert.deepEqual(draft.attachments[0].bytes,Array.from(png));
 stage="text fallback";clipboard=undefined;await paste();await host.pump(root);assert.equal(replayed,3,"Each forwarded shortcut invokes native Paste exactly once");assert.equal(draft.attachments.length,2);
 stage="remove";const remove=[...host.nodes.values()].find(n=>n.props["aria-label"]==="Remove clipboard-image-1.png")!;root.dispatch({elementId:remove.id,eventType:"click"});await host.pump(root);
 assert.ok(Object.values(model.state.composerDrafts).some(d=>d.attachments.length===1&&!d.text.includes(draft.attachments[0].token)));
 console.log("screenshot paste, thumbnail, modal close/Escape, mac shortcut, text fallback and removal passed");
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}
