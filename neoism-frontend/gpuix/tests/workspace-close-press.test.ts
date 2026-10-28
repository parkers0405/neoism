import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("workspace close press is immediate, deduplicated and cannot stick after a dirty refusal",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-close-press-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualSolidPressClosesImmediately:true,clickDeduplicated:true,inactiveDirtyCloseNeverActivates:true,outsideReleaseCannotStickLatch:true,freshDragStillWorks:true,sourceTrailingInsetAtZoom:true,onLayoutBoundsFallback:true,dirtyBytesRetained:true});
});
