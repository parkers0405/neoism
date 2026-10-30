import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("daemon replies publish atomically across UI subscribers without reordering",async()=>{
  const child=Bun.spawn([process.execPath,"run","tests/support/daemon-batch-check.ts"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
  const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr,output:status===0?undefined:stdout}).toEqual({status:0,stderr:"",output:undefined});
  expect(JSON.parse(stdout)).toEqual({updates:2,ordered:true,noWindow:true});
});
