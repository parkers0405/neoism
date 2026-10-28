import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual Solid menus reflow below all headers with original global pointer hit tests",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/overlay-viewport-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({dialogs:7,boundsChanges:2,checks:14,nativePickerSelections:2,hamburgerActions:1,nativeWindowsCreated:0});
},15_000);
