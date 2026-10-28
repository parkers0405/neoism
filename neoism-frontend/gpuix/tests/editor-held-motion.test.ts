import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";
import {usePresentationScheduler} from "../src/animation/frame-scheduler";

function fixture(vim=true){
 const callbacks=new Map<number,()=>void>();let next=0;
 const reset=usePresentationScheduler(callback=>{const id=++next;callbacks.set(id,callback);return()=>{callbacks.delete(id);};});
 const editor=new NativeEditor("held.txt",Array.from({length:500},(_,i)=>`line ${i}`).join("\n"),vim);
 editor.call("layout",{width:600,height:210,cellWidth:10,rowHeight:21});
 let published=0;editor.subscribe(()=>published++);
 return {editor,callbacks,get published(){return published;},flush(){const batch=[...callbacks.values()];callbacks.clear();for(const callback of batch)callback();},dispose(){editor.dispose();reset();}};
}
test("held arrows apply immediately and coalesce paint without cancelling the display frame",()=>{
 const f=fixture();try{
  const before=f.editor.frame;
  for(let i=0;i<32;i++)f.editor.queueKey({key:"down"});
  expect(f.editor.frame).not.toBe(before);expect(f.editor.frame.cursor.line).toBe(32);
  expect(f.published).toBe(0);expect(f.callbacks.size).toBe(1);
  const restored=new NativeEditor("restored.txt","",true);
  try{restored.restore(f.editor.snapshot());expect(restored.frame.cursor.line).toBe(32);}finally{restored.dispose();}
  f.flush();expect(f.published).toBe(1);expect(f.editor.frame.cursor.line).toBe(32);expect(f.editor.frame.animating).toBe(true);
 }finally{f.dispose();}
});
test("deferred shift motions retain logical selection and interleaved insert uses the actual cursor",()=>{
 const f=fixture(false);try{
  f.editor.queueKey({key:"down",shift:true});expect(f.editor.frame.selection).not.toBeNull();expect(f.published).toBe(0);
  f.flush();expect(f.editor.frame.selection?.end[0]).toBe(1);
  f.editor.call("key",{key:"right"});f.editor.queueKey({key:"down"});const line=f.editor.frame.cursor.line;
  f.editor.call("key",{key:"x",char:"x"});expect(f.editor.frame.cursor.line).toBe(line);expect(f.editor.text().split("\n")[line]).toContain("x");
  expect(f.callbacks.size).toBe(0);f.editor.call("key",{key:"z",ctrl:true});expect(f.editor.text()).not.toContain("x");
 }finally{f.dispose();}
});
test("operator arrow edits and clipboard yanks publish immediately instead of being deferred",()=>{
 const f=fixture();try{
  f.editor.call("key",{key:"d",char:"d"});const before=f.published;
  f.editor.queueKey({key:"down"});expect(f.published).toBe(before+1);expect(f.editor.frame.dirty).toBe(true);expect(f.editor.text().split("\n")[0]).toBe("line 2");
  f.editor.call("key",{key:"u",char:"u"});expect(f.editor.text().split("\n")[0]).toBe("line 0");
  f.editor.call("key",{key:"y",char:"y"});const yanks=f.published;
  f.editor.queueKey({key:"down"});expect(f.published).toBe(yanks+1);expect(f.editor.frame.clipboard).toBe("line 0\nline 1\n");
 }finally{f.dispose();}
});
