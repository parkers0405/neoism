import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("Home tabs migrate legacy titles and use the home glyph in the real strip and drag preview",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/home-tab-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,newHomeTitles:true,legacyTitlesMigratedWithoutNewIds:true,homeGlyphOnTabAndDrag:true,agentLogoUnchanged:true,newHomeLabel:true});
},10000);
