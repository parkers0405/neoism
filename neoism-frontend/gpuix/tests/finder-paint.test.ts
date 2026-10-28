import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual Finder view paints original Rust chrome inside the focused pane body without opening a window",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/finder-paint-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});
 expect(JSON.parse(stdout)).toEqual({noWindow:true,originalRustFinderPaint:true,nativeGlyphRasters:true,transparentNativeInput:true,focusedPaneBodyGeometry:true,viewportResizeRetainsController:true});
});
