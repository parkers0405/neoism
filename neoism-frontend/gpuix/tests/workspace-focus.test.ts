import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("workspace switches restore the destination input or sidebar without another click",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-focus-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("focus restores independently");
},10000);
