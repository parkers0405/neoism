import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {NativeEditor} from "../../src/native-core";
import {EditorView} from "../../src/editor/EditorView";
import {PaintLayer,type PaintCommand} from "../../src/ui/PaintLayer";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/cursor-budget"],{}),cursorBlink:false,cursorStyle:"solid",vim:true};
const pending=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{pending.add(callback);return()=>{pending.delete(callback);};});
const host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const writes:Array<{id:number;bytes:number}>=[];let uploads=0;
const apply=host.renderer.applyBatch.bind(host.renderer);
host.renderer.applyBatch=json=>{for(const [op,id,key,value]of JSON.parse(json))if(op==="setCustomProp"&&key==="commands")writes.push({id,bytes:String(value).length});return apply(json);};
(host.renderer as unknown as {putPaintImage:()=>void}).putPaintImage=()=>{uploads++;};
const editor=new NativeEditor("budget.rs",Array.from({length:200},(_,i)=>`let value_${i}: usize = ${i};`).join("\n"),true);
const operations:string[]=[],call=editor.call.bind(editor);
editor.call=(op,args)=>{operations.push(op);return call(op,args);};
const report=(error:unknown)=>{throw error;};
async function settle(){const deadline=performance.now()+3000;while(pending.size){await Bun.sleep(8);const batch=[...pending];pending.clear();for(const callback of batch)callback();root.flush();if(performance.now()>deadline)throw Error("Cursor did not settle");}}
try{
 root.render(()=><EditorView config={config} editor={editor} active focusToken={0} prompt={()=>{}} save={async()=>{}} onState={()=>{}} report={report}/>);
 await host.pump(root);await settle();
 const document=host.find("code-viewport")!.id,cursor=host.find("code-cursor-layer")!.id;
 editor.call("key",{key:"l",char:"l"});root.flush();writes.length=0;operations.length=0;uploads=0;
 const rows=editor.frame.lines,source=editor.text();await settle();
 assert.ok(operations.filter(op=>op==="cursor-frame").length>0,"Real cursor animation must advance");
 assert.equal(operations.filter(op=>op==="frame").length,0,"Settled viewport must not rebuild native document frames");
 assert.equal(editor.frame.lines,rows);assert.equal(editor.text(),source);
 assert.equal(writes.filter(w=>w.id===document).length,0,"Cursor motion must retain the document canvas");
 assert.ok(writes.filter(w=>w.id===cursor).length>0);assert.equal(uploads,0);
 const cursorFrames=operations.length,cursorBytes=writes.reduce((sum,w)=>sum+w.bytes,0);
 // A request from another consumer must override the cursor-only budget.
 operations.length=0;editor.requestFrame(true);editor.requestFrame();await settle();assert.ok(operations.includes("frame"));
 // The actual native viewport moves through fractional offsets without
 // repacking its glyph list; all rows still land at native paint coordinates.
 const snapshot=editor.snapshot(),rh=editor.frame.rowHeight;
 editor.restore({...snapshot,scrollY:rh*3});root.flush();writes.length=0;uploads=0;
 let maxPositionError=0;
 for(let i=1;i<=80;i++){
  const next=editor.restore({...snapshot,scrollY:rh*(3+i/100)});root.flush();
  const canvas=host.find("code-viewport")!,ops=JSON.parse(String(canvas.props.commands));
  const numbers=ops.filter((op:any)=>op.k==="t");
  for(let row=0;row<next.lines.length;row++)maxPositionError=Math.max(maxPositionError,Math.abs(numbers[row].y+Number(canvas.style.top)-next.lines[row].y));
 }
 assert.ok(maxPositionError<.001,`Retained row positions drifted by ${maxPositionError}`);
 const subrowWrites=writes.filter(w=>w.id===document).length;
 assert.equal(subrowWrites,0,"Fractional scrolling must retain document ink");assert.equal(uploads,0);
 editor.restore({...snapshot,scrollY:rh*4.1});root.flush();assert.ok(writes.some(w=>w.id===document),"Entering a new row must rebuild visible ink");
 // Real scrolling must still rebuild the viewport and advance visible rows.
 operations.length=0;writes.length=0;editor.queueScroll(-1000);
 const beforeHover=editor.frame;
 for(let i=0;i<100;i++)editor.call("lsp.pointer",{x:100+i,y:80});
 assert.equal(editor.frame.lines,beforeHover.lines,"LSP motion must retain document rows");
 assert.equal(editor.frame.scrollY,beforeHover.scrollY,"LSP motion must not tick the scroll spring");
 assert.equal(editor.frame.cursorQuad,beforeHover.cursorQuad,"LSP motion must not tick the cursor spring");
 root.flush();assert.equal(writes.filter(w=>w.id===document).length,0);
 await settle();
 assert.ok(operations.includes("scroll"));assert.ok(operations.includes("frame"));assert.ok(editor.frame.scrollY>0);assert.ok(writes.some(w=>w.id===document));
 editor.call("paste",{text:"changed"});root.flush();assert.ok(editor.text().includes("changed"));assert.ok(writes.some(w=>w.id===document));
 console.log(JSON.stringify({noWindow:true,cursorFrames,cursorBytes,documentUpdatesDuringCursor:0,glyphUploadsDuringCursor:0,subrowFrames:80,subrowWrites,maxPositionError,scrollAndEditsPreserved:true}));
}finally{root.unmount();editor.dispose();restore();}
// Exercise both the ordinary and pre-encoded shared painter paths. Overlay
// changes must not iterate/re-encode the document primitives.
for(const prepared of [false,true]){
 const host=createRecordingAppRenderer(),root=createRoot(host.renderer);let reads=0;
 const commands:PaintCommand[]=[{kind:"rect",rect:[0,0,100,100],color:[1,0,0,1],radii:[0,0,0,0],depth:0,order:0,layer:0}];
 const iterate=commands[Symbol.iterator].bind(commands);commands[Symbol.iterator]=()=>{reads++;return iterate();};
 const [point,setPoint]=createSignal(0);
 try{
  root.render(()=><PaintLayer commands={commands} prepared={prepared?{commands:'[{"k":"r","b":[0,0,100,100],"c":4278190335}]',images:[]}:undefined} config={config} extra={[{k:"p",c:0xffffffff,p:[[point(),0,10,0,10,10,0,10]]}]}/>);
  root.flush();const before=reads,base=host.find("original-paint-layer")!.props.commands;
  for(let i=1;i<=40;i++){setPoint(i);root.flush();}
  assert.equal(reads,before);assert.equal(host.find("original-paint-layer")!.props.commands,base);
  assert.ok(String(host.find("original-paint-overlay")!.props.commands).includes("40"));
 }finally{root.unmount();}
}
