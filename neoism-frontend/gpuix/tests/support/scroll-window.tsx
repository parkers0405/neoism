import { createSignal } from "solid-js";
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { readFileSync } from "node:fs";
import { loadConfig } from "../../src/config";
import { NativeEditor } from "../../src/native-core";
import { EditorView } from "../../src/editor/EditorView";
import { TerminalView } from "../../src/terminal/TerminalView";
import { terminalSession, disposeTerminalSessions } from "../../src/terminal/sessions";
import type { DaemonConnection } from "../../src/daemon/client";

// Production views with in-memory document/PTY input. No daemon or user files.
const renderer=createRenderer();
renderer.init({title:"Neoism wheel verification",appId:"ai.neoism.gpuix.test",width:1200,height:800});
renderer.registerFont(readFileSync("../../sugarloaf/src/font/resources/CascadiaCode/CascadiaCodeNF-Regular.otf"));
const config={...loadConfig(["--config","/nonexistent/scroll-verification"],{}),vim:true,cursorBlink:false};
const editor=new NativeEditor("scroll.rs",Array.from({length:5000},(_,i)=>`pub fn value_${i}() -> usize { ${i} }`).join("\n"),true);
let layouts=0,frames=0,work=0,focuses=0,scrollEvents=0;
let lastFrame=0,intervals:number[]=[];
const call=editor.call.bind(editor);
editor.call=(operation,args={})=>{const start=performance.now();if(operation==="scroll"){scrollEvents++;intervals=[];lastFrame=start;}if(operation==="frame"&&lastFrame){intervals.push(start-lastFrame);lastFrame=start;}const result=call(operation,args);work+=performance.now()-start;if(operation==="layout")layouts++;if(operation==="frame")frames++;return result;};
const listeners=new Set<(event:any)=>void>();let request=0,attached=false;
const state={phase:"connected"};
const client={state,subscribe:(listener:any)=>{listeners.add(listener);return()=>listeners.delete(listener);},
  subscribeStatus:(listener:any)=>{listener(state);return()=>{};},
  send:(_family:string,message:any)=>{if(message.AttachPty&&!attached){attached=true;queueMicrotask(()=>{
    const bytes=[...new TextEncoder().encode(Array.from({length:1500},(_,i)=>`terminal row ${i}\r\n`).join(""))];
    for(const listener of listeners)listener({family:"Pty",requestId:0,message:{PtyOutput:{session_id:"scroll-verification",bytes,offset:0}}});
  });}return++request;}} as unknown as DaemonConnection;
const terminal=terminalSession(client,"scroll-verification",config.theme,console.error).buffer;
const loop=startFrameLoop(renderer);
let monitor:ReturnType<typeof setInterval>;
render(()=>{
  const [stats,setStats]=createSignal("");
  monitor=setInterval(()=>{const timing=[...intervals].sort((a,b)=>a-b);setStats(JSON.stringify({layouts,frames,work,focuses,scrollEvents,nativeFrames:renderer.getDebugFrameOverlayStats().frames,frameInterval:timing[Math.floor(timing.length/2)]||0,p95Interval:timing[Math.floor(timing.length*.95)]||0,visible:editor.frame.cursor.visible,scroll:editor.frame.scrollY,cursor:editor.frame.cursor.line,
    terminalTop:Number(terminal.frame().lines[0]?.map(run=>run.text).join("").match(/terminal row (\d+)/)?.[1]||0)}));},100);
  return <div style={{display:"flex",flexDirection:"column",width:"100%",height:"100%",backgroundColor:config.theme.bg}}>
    <div style={{display:"flex",flexGrow:1,minHeight:0}}>
      <div style={{width:"60%",display:"flex",minWidth:0}}><EditorView editor={editor} config={config} active={true} focusToken={0} onFocus={()=>focuses++} prompt={()=>{}} save={async()=>{}} report={console.error} onState={()=>{}}/></div>
      <div style={{width:"40%",display:"flex",minWidth:0}}><TerminalView sessionId="scroll-verification" client={client} config={config} active={false} focusToken={0} onSearch={()=>{}} report={console.error}/></div>
    </div>
    <text testId="scroll-stats" style={{height:25,color:config.theme.fg,fontSize:12}}>{stats()}</text>
  </div>;
},{renderer});
const shutdown=()=>{clearInterval(monitor);editor.dispose();disposeTerminalSessions(client);loop.stop();process.exit(0);};
process.on("SIGINT",shutdown);process.on("SIGTERM",shutdown);
