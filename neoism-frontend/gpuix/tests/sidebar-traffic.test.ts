import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("mounted and cached sidebars skip transcript traffic and defer hover painting",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/sidebar-traffic-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("sidebar traffic and frame coalescing passed");
},15000);