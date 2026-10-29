import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual WorkspaceTabs Solid host retains drag identity, isolates X bubbling, and restores durable order without a window",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/workspace-tabs-host-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});
 expect(JSON.parse(stdout)).toMatchObject({actualSolidHost:true,windowsCreated:0,metadataCloudIcons:true,cloudAndFolderDragPreviews:true,nonselectableTabChromeAndGhost:true,childParentClickMicrotaskGuard:true,dirtyInactiveCloseRejected:true,cleanInactiveCloseDoesNotActivate:true,retainedHostReorders:2,pointerFollowingSamples:2,intermediateSlideSamples:2,scrolledDropUsesCurrentGeometry:true,outsideDropCancels:true,backendCreates:1,oneBackendActionPerClick:true,persistedOrderRestored:true,ptyCreates:0});
});
