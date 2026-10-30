import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";

test("cursor-only frames retain rows without replaying clipboard or LSP effects",()=>{
 const editor=new NativeEditor("/project/cursor.rs","one two three\nsecond line",true);
 let clipboard=0,requests=0;
 editor.subscribe(frame=>{if(frame.clipboard)clipboard++;requests+=frame.lsp?.requests.length||0;});
 try{
  editor.call("layout",{width:800,height:600,cellWidth:9,rowHeight:21});
  editor.call("lsp.enable",{surface:"cursor-effects",openPaths:["/project/cursor.rs"]});
  expect(requests).toBeGreaterThan(0);
  editor.call("key",{key:"y",char:"y"});editor.call("key",{key:"y",char:"y"});
  expect(clipboard).toBe(1);const rows=editor.frame.lines,requestsBefore=requests;
  for(let i=0;i<20;i++)editor.call("cursor-frame");
  expect(editor.frame.lines).toBe(rows);expect(clipboard).toBe(1);expect(requests).toBe(requestsBefore);
  expect(editor.frame.clipboard).toBeNull();expect(editor.frame.lsp?.requests).toEqual([]);
 }finally{editor.dispose();}
});
