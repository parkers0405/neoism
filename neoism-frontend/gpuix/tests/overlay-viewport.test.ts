import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual Solid menus reflow below all headers with original global pointer hit tests",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/overlay-viewport-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({dialogs:7,boundsChanges:2,checks:14,nativePickerSelections:2,hamburgerActions:1,nativeWindowsCreated:0});
},15_000);

import {dialogOverlayOffset} from "../src/ui/overlay-viewport";
test("dialog translation centers horizontally while retaining the upper anchor and clamping short windows",()=>{
 expect(dialogOverlayOffset({width:1440,height:800},[400,80,620,300])).toEqual({x:10,y:0});
 expect(dialogOverlayOffset({width:1440,height:450},[400,80,620,300])).toEqual({x:10,y:0});
 expect(dialogOverlayOffset({width:400,height:200},[20,80,360,300])).toEqual({x:0,y:-80});
});
