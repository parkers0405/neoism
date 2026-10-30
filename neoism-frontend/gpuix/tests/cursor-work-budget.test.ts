import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("cursor animation retains document paint while scrolling and edits still render",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/cursor-work-budget-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({noWindow:true,documentUpdatesDuringCursor:0,glyphUploadsDuringCursor:0,scrollAndEditsPreserved:true});
},15000);
