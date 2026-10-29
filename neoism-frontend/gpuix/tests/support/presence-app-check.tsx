import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/presence-app"],{}),endpoint:daemon.endpoint,collaborative:true,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await host.pump(root);if(Date.now()>end)throw new Error("App presence did not converge");await Bun.sleep(5);}await host.pump(root);};
const peer={buffer_id:"file:///project/a/README.md",peer_id:"remote@windows",display_name:"Remote",client_os:"windows",color:{r:60,g:90,b:120},cursor:{line:1,column:2},selection:null,insert:false,rainbow:false,updated_at_ms:Date.now()};
try{
 model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 root.render(()=><App model={model} config={config}/>);await host.pump(root);
 daemon.pushCrdt({Presence:{update:{Upsert:peer}}});await until(()=>!!host.find("presence:remote@windows")&&!!host.find("tree-presence:README.md"));
 assert.equal(host.find("tree-presence:README.md")!.props["aria-label"],"Remote");assert.equal(host.find("presence:remote@windows")!.props["aria-label"],"Remote");
 await model.openFile("README.md");await until(()=>!!host.find("rich-markdown-editor"));
 const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;
 const markdown=model.getMarkdown(surface,model.getLayout().frame.focusedLeaf||undefined);markdown.call("key",{key:"j"});await host.pump(root);
 await until(()=>daemon.crdtRequests.some(request=>request.tag==="PublishPresence"&&(request.body.presence as any)?.buffer_id==="file:///project/a/README.md"&&(request.body.presence as any)?.cursor.line===markdown.getPresence()!.cursor.line));
 const published=daemon.crdtRequests.filter(request=>request.tag==="PublishPresence").map(request=>request.body.presence as any).at(-1);
 assert.equal(published.cursor.line,markdown.getPresence()!.cursor.line);
 const toggle=host.find("toggle-files")!;root.dispatch({elementId:toggle.id,eventType:"mouseDown",button:0});await host.pump(root);
 daemon.pushCrdt({Presence:{update:{Remove:{buffer_id:peer.buffer_id,peer_id:peer.peer_id}}}});await until(()=>!host.find("presence:remote@windows"));
 assert.equal(daemon.ptyRequests.length,0);
 console.log(JSON.stringify({actualApp:true,noWindow:true,topChromeOriginalPeer:true,fileTreeOriginalPeer:true,focusedMarkdownPublishesCursor:true,remoteDepartureClearsChrome:true,noPty:true}));
}finally{root.unmount();model.dispose();client.stop();daemon.stop();}
