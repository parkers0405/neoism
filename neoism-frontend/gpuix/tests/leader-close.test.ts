import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";
import {NativeMarkdown} from "../src/markdown/NativeMarkdown";
import {loadConfig} from "../src/config";
const config={...loadConfig(["--config","/nonexistent/leader-close"],{}),vim:true};
const key=(editor:NativeEditor,key:string,char=key)=>editor.call("key",{key,char});

test("Code Space+x emits a one-shot host close without editing, including shifted logical X",async()=>{
 const editor=new NativeEditor("leader.rs","alpha\nbeta",true);
 try{
  const revision=editor.frame.revision;
  expect(key(editor,"space"," ").pendingEmpty).toBe(false);
  // Code's original leader is a bool, unlike Markdown's 800ms timer.
  await Bun.sleep(825);
  editor.queueKey({key:"down"});expect(editor.frame.pendingEmpty).toBe(false);
  const close=editor.call("key",{key:"x",char:"X",shift:true});
  expect(close.hostAction).toBe("close");expect(close.cursor.line).toBe(1);
  expect(close.revision).toBe(revision);expect(editor.text()).toBe("alpha\nbeta");
  expect(editor.call("frame").hostAction).toBeNull();
 }finally{editor.dispose();}
});

test("Code leader cancellation, unknown chords, and Insert spaces keep canonical Vim behavior",()=>{
 const editor=new NativeEditor("leader.rs","alpha",true);
 try{
  key(editor,"space"," ");key(editor,"escape","");
  expect(key(editor,"x").hostAction).toBeNull();expect(editor.text()).toBe("lpha");
  key(editor,"u");key(editor,"space"," ");key(editor,"i");
  expect(editor.frame.mode).toBe("Normal");expect(editor.text()).toBe("alpha");
  key(editor,"i");key(editor,"space"," ");key(editor,"x");
  expect(editor.frame.mode).toBe("Insert");expect(editor.text()).toBe(" xalpha");
  expect(editor.frame.hostAction).toBeNull();
 }finally{editor.dispose();}
});

test("rich Markdown Space+x closes without source mutation and consumes its effect once",()=>{
 const editor=new NativeEditor("leader.md","alpha\n\nbeta",true),markdown=new NativeMarkdown(editor,config,"/project/leader.md");
 try{
  const before=markdown.text();markdown.call("key",{key:"space",char:" "});
  expect(markdown.call("key",{key:"x",char:"x"}).hostAction).toBe("close");
  expect(markdown.text()).toBe(before);expect(editor.frame.dirty).toBe(false);
  expect(markdown.call("frame").hostAction).toBeNull();
 }finally{markdown.dispose();editor.dispose();}
});

test("rich Markdown leader expires after 800ms, Escape cancels it, and Insert types Space+x",async()=>{
 const editor=new NativeEditor("leader.md","alpha\n\nbeta",true),markdown=new NativeMarkdown(editor,config,"/project/leader.md");
 try{
  markdown.call("key",{key:"space",char:" "});await Bun.sleep(825);
  expect(markdown.call("key",{key:"x",char:"x"}).hostAction).toBeNull();expect(markdown.text()).toStartWith("lpha");
  markdown.call("key",{key:"u",char:"u"});markdown.call("key",{key:"space",char:" "});markdown.call("key",{key:"escape"});
  expect(markdown.call("key",{key:"x",char:"x"}).hostAction).toBeNull();expect(markdown.text()).toStartWith("lpha");
  markdown.call("key",{key:"i",char:"i"});markdown.call("key",{key:"space",char:" "});
  expect(markdown.call("key",{key:"x",char:"x"}).hostAction).toBeNull();expect(markdown.text()).toStartWith(" xlpha");
 }finally{markdown.dispose();editor.dispose();}
});
