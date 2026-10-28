import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {EditorView} from "../../src/editor/EditorView";
import {MarkdownView} from "../../src/markdown/MarkdownView";
import {NativeMarkdown} from "../../src/markdown/NativeMarkdown";
import {NativeEditor} from "../../src/native-core";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const config={...loadConfig(["--config","/nonexistent/editor-blink"],{}),vim:true,cursorBlink:true,cursorBlinkInterval:350,cursorStyle:"solid",cursorColor:"#78b4e2"};
async function check(kind:"code"|"markdown"){
 const host=createRecordingAppRenderer(),root=createRoot(host.renderer),editor=new NativeEditor(kind==="code"?"blink.rs":"blink.md","abcdefghij\n\nSecond paragraph.",true);
 const markdown=kind==="markdown"?new NativeMarkdown(editor,config,"/project/blink.md"):undefined;
 const marker=kind==="code"?"code-editor-caret":"rich-markdown-caret";
 const frame=()=>markdown?.frame||editor.frame;
 const move=(key:string)=>markdown?markdown.call("key",{key,char:key}):editor.call("key",{key,char:key});
 async function until(condition:()=>boolean,message:string){const deadline=performance.now()+2000;while(!condition()){root.flush();if(performance.now()>deadline)throw new Error(`${kind}: ${message}`);await Bun.sleep(8);}root.flush();}
 try{
  const common={config,active:true,focusToken:0,prompt:()=>{},save:async()=>{},report:(error:unknown)=>{throw error;},onState:()=>{}};
  root.render(()=>markdown?<MarkdownView {...common} markdown={markdown} openLocation={()=>{}}/>:<EditorView {...common} editor={editor}/>);
  await host.pump(root);await until(()=>!frame().animating&&!!host.find(marker),"initial native cursor must settle visibly");
  move("l");await until(()=>!frame().animating&&!!host.find(marker),"new cursor target must settle visibly");
  assert.equal(frame().cursorHold,true,"The regression must leave the native hold cached true after movement");
  const ink=[...host.nodes.values()].filter(node=>node.type==="canvas").flatMap(node=>JSON.parse(String(node.props.commands||"[]"))).find(op=>op.k==="p"&&op.c===0x78b4e2ff);
  assert.ok(ink,"The actual painter must keep the configured cursor color at original full opacity");
  const frozen=frame();
  await until(()=>!host.find(marker),"idle hold must expire and allow the cursor to blink off");
  await until(()=>!!host.find(marker),"the cursor must blink on again without document input or repaint frames");
  assert.equal(frame(),frozen,"Idle blink must not resample the native document frame to expire its cached hold");
  await until(()=>!host.find(marker),"wait for hidden phase to verify movement wakes the cursor");
  move("h");root.flush();assert.ok(host.find(marker),"Moving the actual document cursor must restore visibility immediately");
 }finally{root.unmount();markdown?.dispose();editor.dispose();}
}
await check("code");await check("markdown");
console.log(JSON.stringify({noWindow:true,actualCodeAndMarkdown:true,opaqueCursorInk:true,cachedHoldExpires:true,idleBlinkWithoutNewDocumentFrame:true,motionRestoresVisibility:true}));
