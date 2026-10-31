import assert from "node:assert/strict";
import {mock} from "bun:test";
import * as native from "../../src/native-core";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {createLspDaemon} from "./lsp-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
// Count the real native calls; forward every call to the production bridge.
const originalCoreCall=native.coreCall,calls=new Map<string,number>();
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args:Record<string,unknown>={})=>{calls.set(op,(calls.get(op)||0)+1);return originalCoreCall(op,args);}}));
const daemon=createLspDaemon(),config={...loadConfig(["--config","/nonexistent/lsp-status-app"],{}),endpoint:daemon.endpoint,authToken:undefined,reducedMotion:true,cursorBlink:false,statusFps:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await host.pump(root);if(Date.now()>end)throw new Error("LSP status App did not settle");await Bun.sleep(5);}await host.pump(root);};
try{
 model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);await model.openFile("main.ts");root.render(()=><App model={model} config={config}/>);
 await until(()=>!!host.find("code-editor")&&daemon.requests.some(request=>request.tag==="OpenBuffer"));
 const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!,editor=model.getEditor(surface,model.getLayout().frame.focusedLeaf||undefined);
 const surface_id=daemon.requests.find(request=>request.tag==="OpenBuffer")!.body.surface_id;
 const snapshot=(state:string,path=daemon.path)=>daemon.push({LspSnapshot:{surface_id,file_path:path,filetype:"typescript",servers:[{name:"typescript",binary:"typescript-language-server",filetype:"typescript",state,source:"managed",message:state==="error"?"Failed to initialize":undefined}]}});
 const position={line:editor.frame.cursor.line,col:editor.frame.cursor.col};
 snapshot("connected");await until(()=>host.find("status-lsp")?.props["aria-label"]==="LSP: Active");
 assert.deepEqual({line:editor.frame.cursor.line,col:editor.frame.cursor.col},position,"Status must update while the caret is idle");
 const statusId=host.find("original-statusline")!.id;
 const click=async()=>{const pill=host.find("status-lsp")!;root.dispatch({elementId:statusId,eventType:"mouseDown",button:0,x:300+Number(pill.style.left)+Number(pill.style.width)/2,y:90+Number(pill.style.top)+Number(pill.style.height)/2});await host.pump(root);};
 await click();await until(()=>!!host.find("lsp-status-popup"));assert.equal(host.find("lsp-server:typescript")!.props["aria-label"],"typescript: Attached");
 await Bun.sleep(350);await host.pump(root);
 const before={popup:calls.get("lsppopup.frame")||0,status:calls.get("chrome.frame")||0,requests:daemon.requests.filter(request=>["OpenBuffer","LspQueryAt"].includes(request.tag)).length};
 for(let i=0;i<240;i++){editor.call("frame");await host.pump(root);}
 assert.equal(calls.get("lsppopup.frame")||0,before.popup,"Cursor/paint frames must not resample LSP details");
 assert.equal(calls.get("chrome.frame")||0,before.status,"Unchanged LSP state must not repaint the status bar");
 assert.equal(daemon.requests.filter(request=>["OpenBuffer","LspQueryAt"].includes(request.tag)).length,before.requests,"Status rendering must not issue daemon requests");
 assert.ok([...host.nodes.values()].filter(node=>node.type==="anchored").every(node=>node.style.backgroundColor==="transparent"),"Status popup must not tint the whole window");
 snapshot("error");await until(()=>host.find("status-lsp")?.props["aria-label"]==="LSP: Missing");
 assert.equal(host.find("lsp-server:typescript")!.props["aria-label"],"typescript: Errored");
 snapshot("connected","/project/lsp/other.ts");await Bun.sleep(20);await host.pump(root);assert.equal(host.find("status-lsp")!.props["aria-label"],"LSP: Missing");
 await click();await until(()=>!host.find("lsp-status-popup"));
 root.dispatch({elementId:host.find("status-lsp")!.id,eventType:"keyDown",key:"enter",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await until(()=>!!host.find("lsp-status-popup"));
 root.dispatch({elementId:host.find("lsp-status-popup")!.id,eventType:"keyDown",key:"escape",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await until(()=>!host.find("lsp-status-popup"));
 await click();model.newAgentHome();await until(()=>!host.find("status-lsp")&&!host.find("lsp-status-popup"));
 snapshot("connected");await Bun.sleep(20);await host.pump(root);assert.equal(host.find("status-lsp"),undefined);
 console.log(JSON.stringify({idleStatusUpdates:true,originalPopup:true,scopedToActiveFile:true,unchangedFrames:240,extraStatusPaints:0,extraPopupSamples:0,extraLspRequests:0}));
}finally{root.unmount();model.dispose();client.stop();daemon.stop();}
