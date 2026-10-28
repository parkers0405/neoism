import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual buffer tabs slide through intermediate offsets while preserving the live editor and dirty source without a window",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/buffer-tab-slide-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,canonicalBufferReorder:true,draggedTabFollowsPointer:true,neighborsPaintIntermediateSpringOffsets:true,offsetsSettle:true,tabAndPaneHostsRetained:true,actualEditorMountedOnce:true,dirtyBytesRetained:true});
});
