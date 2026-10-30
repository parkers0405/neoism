import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("picker query and placement changes do not resend catalogs",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/picker-work-budget-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
},15000);
