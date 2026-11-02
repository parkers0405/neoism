import assert from "node:assert/strict";
import {mock} from "bun:test";
let clipboard="",reads=0,writes=0;
mock.module("../../src/clipboard",()=>({readClipboard:async()=>{reads++;return clipboard;},writeClipboard:async(text:string)=>{writes++;clipboard=text;}}));
const {createRoot}=await import("@gpuix/solid");
const {TerminalView}=await import("../../src/terminal/TerminalView");
const {terminalSession,disposeTerminalSessions}=await import("../../src/terminal/sessions");
const {loadConfig}=await import("../../src/config");
const {KeyboardBindings}=await import("../../src/commands/KeyboardBindings");
const {createRecordingAppRenderer}=await import("./full-app-recording-renderer");
const config={...loadConfig(["--config","/nonexistent/terminal-shortcuts"],{}),cursorBlink:false};
const sent:number[][]=[];let global=0,sequence=0;
let bindings=new KeyboardBindings();
const client={send:(_family:string,message:any)=>{if(message.PtyInput)sent.push(message.PtyInput.bytes);return 1;},subscribe:()=>()=>{},subscribeStatus:()=>()=>{}}as any;
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),report=(error:unknown)=>{throw error;};
const buffer=terminalSession(client,"keys",config.theme,report).buffer;
const press=async(key:string,modifiers:Record<string,boolean>,keyChar=key)=>{
 const id=host.find("terminal")!.id;
 root.dispatch({elementId:id,eventType:"keyDown",key,keyChar,modifiers:{ctrl:false,cmd:false,alt:false,shift:false,...modifiers},keystrokeId:++sequence});
 await host.pump(root);
 // Modifiers can be released before the letter; copied/pasted key-up stays consumed.
 root.dispatch({elementId:id,eventType:"keyUp",key,modifiers:{ctrl:false,cmd:false,alt:false,shift:false},keystrokeId:++sequence});await host.pump(root);
};
try{
  root.render(()=><div><div testId="other-pane" tabIndex={0}/><TerminalView config={config} sessionId="keys" client={client} active onSearch={()=>{}} onCustomKey={event=>{
   const matches=bindings.match(event,buffer.bindingState);if(!matches.length)return false;
   global++;
   for(const binding of matches){
    if(binding.kind==="action"&&binding.action==="Copy"){writes++;clipboard=buffer.selectedText;}
    if(binding.kind==="action"&&binding.action==="Paste"){reads++;sent.push([...buffer.paste(clipboard)]);}
   }
   return true;
  }} report={report}/></div>);await host.pump(root);
 host.renderer.focusElement!(host.find("other-pane")!.id);await host.pump(root);
 root.dispatch({elementId:host.find("terminal")!.id,eventType:"mouseDown",button:0,x:301,y:98});await host.pump(root);
 assert.equal(host.focused,host.find("terminal")!.id,"Starting a drag focuses the terminal without waiting for a click");
 await buffer.write(new TextEncoder().encode("selected output"));
 buffer.select(0,0,true,0,10);buffer.select(0,8,false,80,10);assert.ok(buffer.selectedText);
 await press("c",{ctrl:true},"\x03");assert.deepEqual(sent,[[3]]);assert.equal(writes,0);assert.equal(global,0);
 sent.length=0;
 await press("c",{cmd:true});assert.equal(clipboard,buffer.selectedText);assert.equal(writes,1);assert.deepEqual(sent,[]);assert.equal(global,0);
 clipboard="echo pasted\nsecond line";await buffer.write(new TextEncoder().encode("\x1b[?2004h"));
 await press("v",{cmd:true});assert.equal(reads,1);assert.equal(new TextDecoder().decode(Uint8Array.from(sent.flat())),"\x1b[200~echo pasted\nsecond line\x1b[201~");assert.equal(global,0);
 sent.length=0;await press("c",{ctrl:true,shift:true});assert.equal(writes,2);assert.deepEqual(sent,[]);
 await press("v",{ctrl:true,shift:true});assert.equal(reads,2);assert.equal(sent.length,1);
 buffer.key({key:"space",char:" ",alt:true,shift:true});assert.equal(buffer.viMode,true);sent.length=0;
  await press("c",{ctrl:true});assert.equal(buffer.viMode,false);assert.deepEqual(sent,[[3]]);
  sent.length=0;
  bindings=new KeyboardBindings([{key:"c",with:"Control",action:"Copy"},{key:"v",with:"Control",action:"Paste"},{key:"v",with:"Command",action:"None"}]);
  await press("c",{ctrl:true},"\x03");assert.equal(writes,3);assert.equal(clipboard,buffer.selectedText);assert.deepEqual(sent,[]);
  clipboard="configured paste\nsecond line";
  await press("v",{ctrl:true},"\x16");assert.equal(reads,3);assert.equal(new TextDecoder().decode(Uint8Array.from(sent.flat())),"\x1b[200~configured paste\nsecond line\x1b[201~");
  sent.length=0;await press("v",{cmd:true});assert.equal(reads,3);assert.deepEqual(sent,[]);assert.equal(global,3);
  console.log(JSON.stringify({interruptWithSelection:true,interruptFromVi:true,commandClipboard:true,bracketedPaste:true,globalBindingsRespected:true}));
}finally{root.unmount();disposeTerminalSessions(client);}
