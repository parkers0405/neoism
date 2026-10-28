import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("original hover paint, pointer entry, captured Unicode selection and copy run through actual Solid and FFI without windows",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/lsp-hover-host-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({windowsCreated:0,actualSolidAndFfi:true,originalPaintGlyphUpload:true,bodyScopedPaintAndInput:true,nativeMarkdownWidgetAbsent:true,pointerEntryRetainsHover:true,selectionCaptureOutsideCard:true,unicodeCanonicalCopy:true,documentUnchanged:true,escapeRestoresEditorFocus:true});
});
