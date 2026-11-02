import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("wordmark hover follows translated paint bounds and remounted homes retain current sidebar geometry",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/wordmark-position-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("wordmark hover follows painted sidebar offsets");
});
