import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("explicit close retires exact native agent caches without evicting hidden open workspaces",async()=>{
 const config=process.env.NEOISM_TEST_BUN_CONFIG;
 const child=Bun.spawn([process.execPath,...(config?[`--config=${config}`]:[]),"run","tests/support/agent-cache-lifecycle-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("agent cache lifecycle passed");
},20000);