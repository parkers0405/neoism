import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("real App tree opens with complete original glyph paint immediately and a coherent short slide",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tree-first-paint-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({firstPaintRows:21,glyphsBeforeLayout:42,openingMs:120,zeroOpacityRows:0,zoomReplay:false,closeReopenSlide:true,reducedMotion:true,nativeWindowsCreated:0});
},15_000);
