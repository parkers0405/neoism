import assert from"node:assert/strict";
import{createRoot}from"@gpuix/solid";
import{App}from"../../src/ui/App";
import{loadConfig}from"../../src/config";
import{DaemonConnection}from"../../src/daemon/client";
import{WorkspaceModel}from"../../src/model";
import{coreCall}from"../../src/native-core";
import{statusInfo}from"../../src/chrome/status-info";
import{createAgentDaemon}from"./agent-daemon";
import{createRecordingAppRenderer}from"./full-app-recording-renderer";
const daemon=createAgentDaemon({scopedFiles:{"/project/a":{"main.ts":Array.from({length:20},(_,i)=>`line ${i}`).join("\n")}}});
const config={...loadConfig(["--config","/nonexistent/diagnostic-popup-app"],{}),endpoint:daemon.endpoint,reducedMotion:true,cursorBlink:false,statusFps:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const{id:probe}=coreCall<{id:number}>("chrome.new",{kind:"status",family:config.fontFamily,weight:config.fontWeight,fonts:[config.iconFontFamily]});
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await host.pump(root);if(Date.now()>end)throw new Error("Status diagnostic App did not settle");await Bun.sleep(5);}await host.pump(root);};
try{
 model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);await model.openFile("main.ts");root.render(()=><App model={model} config={config}/>);await until(()=>!!host.find("code-editor"));
 const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!,editor=model.getEditor(surface,model.getLayout().frame.focusedLeaf||undefined);
 editor.call("lsp.reply",{message:{Diagnostics:{file_path:"/project/a/main.ts",items:[{line:7,col:2,end_line:7,end_col:3,lnum:8,severity:"Error",message:"one error"},{line:3,col:1,end_line:3,end_col:2,lnum:4,severity:"Warn",message:"one warning"}]}}});await host.pump(root);
 const fileRequests=daemon.fileRequests.length;
 const click=async(pill:"error"|"warn")=>{
  const info=statusInfo(surface,"/project/a",{mode:"Normal",line:editor.frame.cursor.line,col:editor.frame.cursor.col,dirty:false},editor.frame);
  const measured=coreCall<{anchors:Array<{kind:string;x:number;y:number}>}>("chrome.frame",{id:probe,width:1440,scale:config.scale,theme:config.themeName,info});
  const anchor=measured.anchors.find(anchor=>anchor.kind===pill)!;assert.ok(anchor);
  root.dispatch({elementId:host.find("original-statusline")!.id,eventType:"mouseDown",button:0,x:300+anchor.x,y:95});await until(()=>!!host.find("diagnostics-popup"));await Bun.sleep(190);await host.pump(root);
 };
 await click("error");assert.equal(host.find("diagnostics-popup")!.props["aria-label"],"Errors");assert.equal(host.find("diagnostic-row:0")!.props["aria-label"],"Line 8: one error");
 root.dispatch({elementId:host.find("diagnostics-popup")!.id,eventType:"keyDown",key:"enter",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await until(()=>editor.frame.cursor.line===7&&!host.find("diagnostics-popup"));
 assert.equal(host.focused,host.find("code-editor")!.id);
 await click("warn");assert.equal(host.find("diagnostics-popup")!.props["aria-label"],"Warnings");assert.equal(host.find("diagnostic-row:0")!.props["aria-label"],"Line 4: one warning");
 // Native popup x centers on the pill; its first row sits 40px below its top.
 const popup=host.find("diagnostics-popup")!,row=host.find("diagnostic-row:0")!,container=[...host.nodes.values()].find(node=>node.children.includes(popup.id))!;
 root.dispatch({elementId:row.id,eventType:"mouseDown",button:0,x:Number(container.style.left)+Number(popup.style.left)+Number(row.style.left)+10,y:Number(container.style.top)+Number(popup.style.top)+Number(row.style.top)+5});await until(()=>editor.frame.cursor.line===3&&!host.find("diagnostics-popup"));
 assert.equal(host.focused,host.find("code-editor")!.id);assert.equal(model.state.activeSurfaceId,surface.id);assert.equal(daemon.fileRequests.length,fileRequests);assert.equal(daemon.ptyRequests.length,0);
 console.log(JSON.stringify({actualApp:true,noWindow:true,errorAndWarningBadgeClicks:true,originalRowsAndCounts:true,clickAndKeyboardJump:true,editorFocusRestored:true,noUnexpectedFileOrPty:true}));
}finally{root.unmount();model.dispose();client.stop();daemon.stop();coreCall("chrome.dispose",{id:probe});}
