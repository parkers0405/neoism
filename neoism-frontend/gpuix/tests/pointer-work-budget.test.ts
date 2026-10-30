import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("real Markdown and code views coalesce pointer bursts and preserve selection order",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/pointer-work-budget-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({noWindow:true,eventsPerBurst:428,markdownHoverCalls:1,dragCalls:1,orderedMouseUp:true});
},15000);
