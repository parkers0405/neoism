import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("every original Welcome row keeps its rounded hover background across parent leave events",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/splash-hover-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualSolidWelcome:true,allEightOriginalMenuRows:true,originalRoundedHoverPaint:true,parentLeavePreservesChildHover:true,childLeaveClearsHover:true,allActionsRemainLive:true});
});
