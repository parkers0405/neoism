import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("chat links open after a pane moves without resizing",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-link-click-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
},15000);
