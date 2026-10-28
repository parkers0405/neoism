import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("inactive close hover survives parent/child event ordering in both actual tab strips",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/inactive-close-hover-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualSolidBufferAndWorkspaceTabs:true,parentLeaveCannotClearChildHover:true,bothEventOrdersRemainRed:true,childLeaveRestoresMutedAndHidden:true,childExitOntoTabRemainsVisible:true});
});
