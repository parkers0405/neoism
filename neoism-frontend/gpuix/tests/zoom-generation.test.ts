import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("real Solid zoom generations retain wallpaper, warmed original splash/tree paint, textures and animation state without windows",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/zoom-generation-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({swaps:8,stableRasterReuse:2,attachments:1,persistentWallpaper:true,emptyWarmTransactions:0,startupReplays:0,treeRevealReplays:0,prematureMenuDrops:0,nativeWindowsCreated:0});
},15_000);
