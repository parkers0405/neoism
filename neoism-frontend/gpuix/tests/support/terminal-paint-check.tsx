import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {loadConfig} from "../../src/config";
import {TerminalView} from "../../src/terminal/TerminalView";
import {terminalSession,disposeTerminalSessions} from "../../src/terminal/sessions";
import type {DaemonConnection} from "../../src/daemon/client";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
const config={...loadConfig(["--config","/nonexistent/terminal-paint"],{}),cursorBlink:false};
const client={send:()=>1,subscribe:()=>()=>{},subscribeStatus:()=>()=>{}} as unknown as DaemonConnection;
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),report=(error:unknown)=>{throw error;};
const session=terminalSession(client,"paint",config.theme,report),buffer=session.buffer;
const text=()=>JSON.parse(String(host.find("terminal-viewport")!.props.commands)).filter((op:any)=>op.k==="t").map((op:any)=>op.t).join("");
const pending=new Map<number,()=>void>();let next=0;
const releaseSchedule=usePresentationScheduler(callback=>{const id=++next;pending.set(id,callback);return()=>pending.delete(id);});
const present=()=>{const work=[...pending];for(const [id,callback]of work)if(pending.delete(id))callback();root.flush();};
try{
  root.render(()=><TerminalView config={config} sessionId="paint" client={client} active onSearch={()=>{}} report={report}/>);await host.pump(root);
  present();await host.pump(root);present();
 const output=Array.from({length:300},(_,row)=>`row_${row} `+Array.from({length:18},(_,i)=>`\x1b[${31+i%7}mcell_${i} `).join("")+"\x1b[0m\r\n").join("");
  await buffer.write(new TextEncoder().encode(output));present();await host.pump(root);
 const before=text(),nodes=host.nodes.size,runs=buffer.frame().lines.reduce((sum,row)=>sum+row.length,0);
 assert.ok(runs>200,`Expected a dense terminal, got ${runs} runs`);
 assert.ok(nodes<12,`Terminal still builds a per-run layout tree: ${nodes} nodes`);
  buffer.scrollPixels(240,config.fontSize*1.3);present();assert.notEqual(text(),before);
  buffer.select(0,0,true,0,config.fontSize*.6);buffer.select(0,5,false,config.fontSize*3.6,config.fontSize*.6);present();
 assert.ok(buffer.selectedText.length>0);assert.ok(host.nodes.size<12);
  assert.ok(JSON.parse(String(host.find("terminal-viewport")!.props.commands)).some((op:any)=>op.k==="r"),"Selection background must be painted");
  buffer.scrollToBottom();present();
  let frameReads=0;const originalFrame=buffer.frame.bind(buffer);
  buffer.frame=()=>{frameReads++;return originalFrame();};
  for(let n=0;n<20;n++){await buffer.write(new TextEncoder().encode(`burst_${n}\r\n`));root.flush();}
  assert.equal(frameReads,0,"PTY packets must not each rebuild the visible frame before presentation");
  present();assert.equal(frameReads,1,"A packet burst must publish the latest terminal state once per display frame");
  assert.ok(text().includes("burst_19"),"Coalescing must retain the latest output, not the first packet");
  await buffer.write(new TextEncoder().encode("pending-unmount\r\n"));
 // PTY exit retires the engine before the mounted view receives its cleanup.
  disposeTerminalSessions(client); root.unmount();
  assert.equal(pending.size,0,"Unmount must cancel pending terminal presentation work");
  console.log(JSON.stringify({noWindow:true,nodes,runs,selectionPreserved:true,scrollbackChanged:true,burstCoalesced:true}));
}finally{root.unmount();disposeTerminalSessions(client);releaseSchedule();}
