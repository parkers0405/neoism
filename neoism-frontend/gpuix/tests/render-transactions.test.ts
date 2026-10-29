import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("real App theme/zoom/directory updates retain every committed canvas texture until replacement",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/render-transactions-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({noWindow:true,actualApp:true,themeSwaps:4,fontSwaps:4,cwdDirectoryReplacement:true,committedImagesNeverDropped:true,allImagesReleased:true});
},20000);
