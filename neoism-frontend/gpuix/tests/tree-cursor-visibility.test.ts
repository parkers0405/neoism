import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual focused App tree hides its keyboard cursor when selected row scrolls outside either edge and restores it on return",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tree-cursor-visibility-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,hiddenAbove:true,hiddenBelow:true,selectedRowPreserved:true,viewportPreserved:true,focusPreserved:true,restoredOnReturn:true});
},15000);
