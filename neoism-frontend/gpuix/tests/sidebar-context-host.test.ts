import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("full App routes file, folder and background menus without opening or deleting on right-click",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/sidebar-context-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("Sidebar context host passed");
},15000);
