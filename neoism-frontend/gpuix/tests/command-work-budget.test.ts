import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("palette motion retains catalog/text and shared frame work publishes atomically",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/command-work-budget-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,output:status===0?undefined:stdout}).toEqual({status:0,stderr:"",output:undefined});
 expect(JSON.parse(stdout)).toMatchObject({noWindow:true,textWrites:0,sharedComputations:1,queryPreserved:true});
},10000);
