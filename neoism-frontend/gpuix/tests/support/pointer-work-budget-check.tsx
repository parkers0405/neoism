import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {NativeEditor} from "../../src/native-core";
import {NativeMarkdown} from "../../src/markdown/NativeMarkdown";
import {MarkdownView} from "../../src/markdown/MarkdownView";
import {EditorView} from "../../src/editor/EditorView";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/pointer-budget"],{}),cursorBlink:false,reducedMotion:true};
const pending=new Set<()=>void>();
const restore=usePresentationScheduler(callback=>{pending.add(callback);return()=>{pending.delete(callback);};});
const tick=()=>{for(const callback of [...pending]){pending.delete(callback);callback();}};
const report=(error:unknown)=>{throw error;};
try{for(const rich of [true,false]){
 const editor=new NativeEditor(rich?"test.md":"test.ts","# Heading\n\nText to select across the document.\n".repeat(10),false);
 const markdown=rich?new NativeMarkdown(editor,config,"/test.md"):undefined;
 const calls:Array<{op:string;args:Record<string,unknown>}>=[];
 const engine=markdown||editor,original=engine.call.bind(engine);
 engine.call=((op:string,args:Record<string,unknown>={},sync?:boolean)=>{calls.push({op,args});return original(op,args,sync);}) as typeof engine.call;
 const host=createRecordingAppRenderer(),root=createRoot(host.renderer);
 try{
  const common={config,active:true,focusToken:0,save:async()=>{},report,onState:()=>{},prompt:()=>{}};
  root.render(()=>rich?<MarkdownView {...common} markdown={markdown!} openLocation={()=>{}}/>:<EditorView {...common} editor={editor}/>);
  await host.pump(root);tick();root.flush();calls.length=0;
  const id=host.find(rich?"rich-markdown-editor":"code-editor")!.id;
  if(rich){
   // A pane moving without resizing must not re-layout the Markdown document.
   root.dispatch({elementId:id,eventType:"layout",x:320,y:100,width:960,height:680});root.flush();
   assert.equal(calls.filter(c=>c.op==="layout").length,0);
   for(let i=0;i<428;i++)root.dispatch({elementId:id,eventType:"mouseMove",x:400+i,y:180,pressedButton:-1});
   assert.equal(calls.filter(c=>c.op==="hover").length,0);tick();root.flush();
   assert.equal(calls.filter(c=>c.op==="hover").length,1);
   assert.equal(calls.find(c=>c.op==="hover")!.args.x,507);
  }
  root.dispatch({elementId:id,eventType:"mouseDown",button:0,x:430,y:170});calls.length=0;
  for(let i=0;i<428;i++)root.dispatch({elementId:id,eventType:"mouseMove",pressedButton:0,x:430+i,y:190});
  assert.equal(calls.filter(c=>c.op==="pointer").length,0);
  root.dispatch({elementId:id,eventType:"mouseUp",button:0,x:857,y:190});
  const ordered=calls.filter(c=>c.op==="pointer"||c.op==="pointer-up");
  assert.equal(ordered.length,2);assert.equal(ordered[0].args.x,rich?537:557);
  assert.ok(rich?ordered[1].args.kind==="up":ordered[1].op==="pointer-up");
  tick();assert.equal(calls.filter(c=>c.op==="pointer"||c.op==="pointer-up").length,2);
 }finally{root.unmount();markdown?.dispose();editor.dispose();pending.clear();}
}
console.log(JSON.stringify({noWindow:true,eventsPerBurst:428,markdownHoverCalls:1,dragCalls:1,orderedMouseUp:true}));
}finally{restore();}
