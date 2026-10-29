import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("actual Code canvas paints native LSP errors and warnings as straight diagnostic bands",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/diagnostic-underlines-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toEqual({noWindow:true,actualNativeDiagnostics:true,straightErrorAndWarning:true,exactRunGeometry:true,wrappedRows:true,glyphImagesRetained:true,clearedDiagnosticsRemoveBands:true});
},10000);
