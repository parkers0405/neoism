import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("review hover and trackpad bursts paint once per display frame without losing movement",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/git-review-input-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("one update each");
});
