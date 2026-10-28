import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual keyed workspace panes restore measured tabs without entrance replay",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-switch-tab-slide-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualSolidKeyedWorkspaceSwitch:true,initialAndRestoredMeasurementsSnap:true,noTabEntranceOffsets:true,selectionPreserved:true});
});
