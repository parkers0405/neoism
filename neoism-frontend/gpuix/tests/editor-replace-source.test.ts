import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";
const documents=["", "α", "α\n", "α\n\n", "α\nβ", "α\nβ\n", "α\r\nβ\r\n", "α\r\nβ"];
test("whole disk replacement preserves exact EOF/EOL bytes, one revision and undo/redo across recovery",()=>{
 for(const before of documents)for(const after of documents){
  const editor=new NativeEditor("replacement.txt",before,true),recovered=new NativeEditor("recovered.txt","",true);
  try{
   const revision=editor.frame.revision;editor.call("replace-source",{text:after});expect(editor.text()).toBe(after);expect(editor.frame.revision).toBe(revision+Number(before!==after));expect(editor.frame.dirty).toBe(before!==after);
   const replacedRevision=editor.frame.revision;editor.call("replace-source",{text:after});expect(editor.frame.revision).toBe(replacedRevision);
   editor.call("saved",{revision:replacedRevision});expect(editor.frame.dirty).toBe(false);
   recovered.restore(editor.snapshot());expect(recovered.text()).toBe(after);expect(recovered.frame.dirty).toBe(false);
   if(before===after)continue;
   recovered.call("key",{key:"u",char:"u"});expect(recovered.text()).toBe(before);expect(recovered.frame.dirty).toBe(true);
   recovered.call("key",{key:"r",ctrl:true});expect(recovered.text()).toBe(after);expect(recovered.frame.dirty).toBe(false);
   const peer=recovered.cloneView();try{expect(peer.text()).toBe(after);peer.call("key",{key:"u",char:"u"});recovered.syncFrom(peer);expect(recovered.text()).toBe(before);expect(recovered.frame.dirty).toBe(true);}finally{peer.dispose();}
  }finally{editor.dispose();recovered.dispose();}
 }
});
test("legacy recovery without EOF/EOL history metadata remains readable and undoable",()=>{
 const editor=new NativeEditor("old.txt","before\r\n",true),restored=new NativeEditor("restore.txt","",true);
 try{
  editor.call("replace-source",{text:"after\r\n"});editor.call("saved");const snapshot=JSON.parse(JSON.stringify(editor.snapshot()));
  const strip=(value:unknown)=>{if(!value||typeof value!=="object")return;for(const key of Object.keys(value)){if(["saved_line_ending","saved_trailing_newline"].includes(key)||["line_ending","trailing_newline"].includes(key)&&value!==(snapshot.buffer as any).buffer)delete (value as any)[key];else strip((value as any)[key]);}};
  strip(snapshot.buffer);restored.restore(snapshot);expect(restored.text()).toBe("after\r\n");expect(restored.frame.dirty).toBe(false);restored.call("replace-source",{text:"after"});expect(restored.frame.dirty).toBe(true);restored.call("key",{key:"u",char:"u"});expect(restored.text()).toBe("after\r\n");expect(restored.frame.dirty).toBe(false);restored.call("key",{key:"u",char:"u"});expect(restored.text()).toBe("before\r\n");
 }finally{editor.dispose();restored.dispose();}
});
