import { expect,test } from "bun:test";
import { navigateChrome,workspaceShortcut } from "../src/commands/chrome-keyboard";
import { CommandController } from "../src/commands/CommandController";
import { NativeEditor,coreCall } from "../src/native-core";
import { NativeMarkdown } from "../src/markdown/NativeMarkdown";
import { loadConfig } from "../src/config";
const alt=(key:string)=>({key,modifiers:{alt:true}}),plain=(key:string)=>({key});
const context={bufferCount:3,activeBuffer:1,workspaceCount:3,activeWorkspace:0,sidebar:true};

test("Alt digits use the original workspace slots and exact modifier policy",()=>{
 for(let i=1;i<=9;i++)expect(workspaceShortcut(alt(String(i)))).toBe(i-1);
 expect(workspaceShortcut(alt("0"))).toBe(9);
 expect(workspaceShortcut({...alt("1"),keyChar:"¡"})).toBe(0);
 for(const modifier of ["ctrl","shift","cmd"]){expect(workspaceShortcut({key:"1",modifiers:{alt:true,[modifier]:true}})).toBeUndefined();}
 expect(workspaceShortcut(plain("1"))).toBeUndefined();
});

test("original chrome ladder moves the focus cursor before Enter commits a buffer or workspace",()=>{
 let result=navigateChrome({zone:"content"},alt("up"),context)!;expect(result.focus).toEqual({zone:"buffers",index:1});
 result=navigateChrome(result.focus,alt("right"),context)!;expect(result.focus).toEqual({zone:"buffers",index:2});expect(result.activate).toBeUndefined();
 result=navigateChrome(result.focus,plain("enter"),context)!;expect(result.activate).toBe("buffer");
 result=navigateChrome(result.focus,alt("right"),context)!;expect(result.focus).toEqual({zone:"buffers",index:3});
 result=navigateChrome(result.focus,alt("right"),context)!;expect(result.focus).toEqual({zone:"buffers",index:0});
 expect(navigateChrome(result.focus,alt("left"),context)?.focus).toEqual({zone:"sidebar"});
 result=navigateChrome(result.focus,alt("up"),context)!;expect(result.focus).toEqual({zone:"workspaces",index:0});
 result=navigateChrome(result.focus,plain("right"),context)!;expect(result.focus).toEqual({zone:"workspaces",index:1});expect(result.activate).toBeUndefined();
 expect(navigateChrome(result.focus,plain("enter"),context)?.activate).toBe("workspace");
 result=navigateChrome(result.focus,alt("down"),context)!;expect(result.focus).toEqual({zone:"buffers",index:1});
 expect(navigateChrome(result.focus,plain("escape"),context)?.focus).toEqual({zone:"content"});
 expect(navigateChrome({zone:"sidebar"},alt("right"),context)?.focus).toEqual({zone:"content"});
 expect(navigateChrome({zone:"content"},{key:"up",modifiers:{alt:true,shift:true}},context)).toBeUndefined();
});

test("optional workspace + participates in arrows, Enter and wrap without changing legacy workspace policy",()=>{
 const withPlus={...context,workspacePlus:true};
 let result=navigateChrome({zone:"workspaces",index:2},plain("right"),withPlus)!;
 expect(result.focus).toEqual({zone:"workspaces",index:3});expect(result.activate).toBeUndefined();
 expect(navigateChrome(result.focus,plain("enter"),withPlus)?.activate).toBe("workspace");
 expect(navigateChrome(result.focus,plain("right"),withPlus)?.focus).toEqual({zone:"workspaces",index:0});
 expect(navigateChrome({zone:"workspaces",index:0},plain("left"),withPlus)?.focus).toEqual({zone:"workspaces",index:3});
 expect(navigateChrome({zone:"workspaces",index:2},plain("right"),context)?.focus).toEqual({zone:"workspaces",index:0});
 expect(navigateChrome({zone:"workspaces",index:3},alt("down"),withPlus)?.focus).toEqual({zone:"buffers",index:1});
});

test("strip focus cursor uses shared original geometry and the original animated quad",()=>{
 const id=coreCall<{id:number}>("keyboard.cursor.new").id;
 try{
  const frame=coreCall<{rect:number[];quad:number[][]}>("keyboard.cursor.frame",{id,kind:"buffer",x:10,y:20,width:150,height:28,padding:12,scale:1});
  expect(frame.rect).toEqual([19,24,3,20]);expect(frame.quad).toHaveLength(4);
  const plus=coreCall<{rect:number[]}>("keyboard.cursor.frame",{id,kind:"buffer",x:10,y:20,width:30,height:28,padding:12,scale:1,plus:true});expect(plus.rect).toEqual([10,24,3,20]);
  const workspace=coreCall<{rect:number[]}>("keyboard.cursor.frame",{id,kind:"workspace",x:10,y:20,width:150,height:28,scale:1});expect(workspace.rect).toEqual([31,24,3,20]);
 }finally{coreCall("keyboard.cursor.dispose",{id});}
});

test("plain 1 and $ command-palette queries execute original source-line jumps in code and rich Markdown",()=>{
 const config={...loadConfig(["--config","/nonexistent/keyboard-test"],{}),vim:true};
 for(const rich of [false,true]){
  const editor=new NativeEditor(rich?"README.md":"lines.txt","first sentence. Second sentence.\nsecond\nlast",true),markdown=rich?new NativeMarkdown(editor,config,"/project/README.md"):undefined;
  const palette=new CommandController({surface:rich?"markdown":"editor",supportedActions:["GoToLine"]});
  try{
   const command=(value:string)=>{palette.query(value);const target=palette.target();expect(target).toEqual({action:"ExCommand",argument:value.replace(/^:/,"")});return markdown?markdown.call("command",{command:target!.argument}):editor.call("command",{command:target!.argument});};
   expect(command("$").cursor.line).toBe(2);expect(command("1").cursor.line).toBe(0);expect(command(":2").cursor.line).toBe(1);
   if(markdown){markdown.call("goto",{line:0,col:0});const result=markdown.call("key",{key:"$",char:"$"});expect(result.cursor.line).toBe(0);expect(result.cursor.col).toBeGreaterThan("first sentence.".length);}
   else{editor.call("goto",{line:0,col:0});const result=editor.call("key",{key:"$",char:"$"});expect(result.cursor.line).toBe(0);expect(result.cursor.col).toBeGreaterThan("first sentence.".length);}
  }finally{palette.dispose();markdown?.dispose();editor.dispose();}
 }
});
