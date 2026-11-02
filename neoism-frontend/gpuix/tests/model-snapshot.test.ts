import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("streamed history only invalidates readers of changed model fields",async()=>{
 // Bun's test runner resolves Solid's server export before preload plugins.
 // Use the same reactive client runtime as the GUI, not inert SSR signals.
 const child=Bun.spawn([process.execPath,"--conditions=browser","run","tests/support/model-snapshot-check.ts"],{
  cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe",
 });
 const [status,stderr]=await Promise.all([child.exited,new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});
});
