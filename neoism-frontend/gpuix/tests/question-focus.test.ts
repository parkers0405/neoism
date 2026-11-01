import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("agent questions keep keyboard focus across suggested/custom answers and retry",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/question-focus-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("composer restoration passed");
},15000);
