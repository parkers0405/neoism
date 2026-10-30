import{expect,test}from"bun:test";
import{fileURLToPath}from"node:url";
test("actual App file tree receives shared host Git snapshots and paints badges and parent rollups without resurrecting deleted files",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tree-git-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,existingEntryMarkers:true,nativeColoredPixels:true,presenceBeforeGitMarker:true,currentThemeColors:true,parentStatusRollup:true,hostRepoSubdirectoryScope:true,deletedPathsHidden:true,liveDeletionRemoved:true,physicalDeletedDirectoryLists:true,oneSharedWatch:true,liveUpdatesPreserveScroll:true,transientErrorsRetained:true,cachedWorkspaceAndZoomPaint:true,successfulClear:true,noGitMutations:true});
},20000);
