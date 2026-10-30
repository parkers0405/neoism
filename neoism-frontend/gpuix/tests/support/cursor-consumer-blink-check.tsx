import assert from "node:assert/strict";
import {createRoot,type HostElement} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {loadConfig} from "../../src/config";
import {ChromeFocusCursor,type ChromeCursorTarget} from "../../src/ui/ChromeFocusCursor";
import {FinderController} from "../../src/finder/FinderController";
import {FinderView} from "../../src/finder/FinderView";
import {TerminalView} from "../../src/terminal/TerminalView";
import {disposeTerminalSessions,terminalSession} from "../../src/terminal/sessions";
import {invalidateNativeFocus} from "../../src/native-focus";
import type {DaemonConnection} from "../../src/daemon/client";
import {createRecordingAppRenderer,type RecordedAppNode} from "./full-app-recording-renderer";

const config={...loadConfig(["--config","/nonexistent/cursor-blink"],{}),cursorBlink:true,cursorBlinkInterval:350,cursorStyle:"solid",cursorColor:"#78b4e2"};
async function until(root:ReturnType<typeof createRoot>,condition:()=>boolean,message:string){
 const deadline=performance.now()+1800;
 while(!condition()){root.flush();if(performance.now()>deadline)throw new Error(message);await Bun.sleep(8);}root.flush();
}
const ops=(node:RecordedAppNode|undefined):Array<{k:string;c?:number;p?:number[][]}>=>node?JSON.parse(String(node.props.commands||"[]")):[];
function opaque(op:{c?:number}|undefined){assert.ok(op,"Actual cursor polygon must be painted");assert.equal(op.c,0x78b4e2ff,"Every local consumer keeps the same configured source color and full alpha");}
async function chrome(){
 const host=createRecordingAppRenderer(),root=createRoot(host.renderer),[target,setTarget]=createSignal<ChromeCursorTarget>();let element:HostElement|undefined;
 try{
  root.render(()=><><div ref={element} testId="chrome-owner" tabIndex={0}/><ChromeFocusCursor config={config} target={target()}/></>);await host.pump(root);
  host.renderer.focusElement!(element!.id);await host.pump(root);invalidateNativeFocus(host.renderer);
  setTarget({host:element!,kind:"buffer",bounds:{x:300,y:90,width:140,height:28}});
  await until(root,()=>!!host.find("chrome-focus-cursor:buffers"),"Chrome cursor must appear when focused");opaque(ops(host.find("chrome-focus-paint"))[0]);
  await until(root,()=>!host.find("chrome-focus-cursor:buffers"),"Chrome cached hold must expire while idle");
  await until(root,()=>!!host.find("chrome-focus-cursor:buffers"),"Chrome must continue blinking without another native frame");
  setTarget(undefined);root.flush();assert.equal(host.find("chrome-focus-cursor:buffers"),undefined);
 }finally{root.unmount();}
}
async function terminal(){
 const host=createRecordingAppRenderer(),root=createRoot(host.renderer);
 const client={send:()=>1,subscribe:()=>()=>{},subscribeStatus:()=>()=>{}} as unknown as DaemonConnection;
 const session=terminalSession(client,"blink",config.theme,error=>{throw error;});
 try{
  root.render(()=><TerminalView config={config} sessionId="blink" client={client} active onSearch={()=>{}} report={error=>{throw error;}}/>);await host.pump(root);
  await until(root,()=>!!host.find("terminal-caret"),"Terminal cursor must appear");
  const canvas=host.find("terminal-cursor-layer");opaque(ops(canvas)[0]);
  // Native frame's movement hold is allowed to stay cached true. It cannot
  // prevent the independent UI deadline from hiding and showing this caret.
  await until(root,()=>!host.find("terminal-caret"),"Terminal cached hold must expire while idle");
  await until(root,()=>!!host.find("terminal-caret"),"Terminal must blink without shell output");
  root.dispatch({elementId:host.find("terminal")!.id,eventType:"keyDown",key:"x",keyChar:"x",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});root.flush();assert.ok(host.find("terminal-caret"),"Typing restores the cursor immediately");
  assert.equal(session.buffer.frame().cursor.x,0,"Blink/input proof does not fake shell echo or move its actual VT cursor");
 }finally{root.unmount();disposeTerminalSessions(client);}
}
async function finder(){
 const host=createRecordingAppRenderer(),root=createRoot(host.renderer),finder=new FinderController({cwd:"/project",search:async()=>({SearchFilesResult:{req_id:0,hits:[{path:"main.rs",score:100}]}})});
 const caret=()=>ops(host.find("original-paint-overlay")).findLast(op=>op.k==="p"&&JSON.stringify(op.p?.[0])===JSON.stringify(finder.frame.cursorQuad.flat()));
 try{
  root.render(()=><FinderView controller={finder} config={config} onAccept={()=>{}} onClose={()=>{}}/>);await host.pump(root);
  await until(root,()=>finder.frame.rows.length===1&&!finder.frame.animating&&!!caret(),"Finder original row cursor must settle visible");opaque(caret());
  const id=host.find("original-paint-layer")!.id;
  await until(root,()=>!caret(),"Finder selected-row cursor must blink independently of input repaint cadence");
  await until(root,()=>!!caret(),"Finder selected-row cursor must resume after idle deadline");assert.equal(host.find("original-paint-layer")!.id,id);
 }finally{root.unmount();finder.dispose();}
}
await chrome();await terminal();await finder();
console.log(JSON.stringify({noWindow:true,actualChromeFinderTerminalConsumers:true,opaqueLocalCursorColors:true,idleHoldExpires:true,idleBlinkKeepsRunning:true,terminalInputRestoresVisibility:true,finderPaintHostRetained:true}));
