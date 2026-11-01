import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {clipboardCommands} from "../src/clipboard";
test("platform text clipboard commands preserve UTF-8 and exact text",()=>{
 expect(clipboardCommands(false,"darwin")).toEqual([["pbcopy"]]);
 expect(clipboardCommands(false,"linux",true)[0]).toContain("text/plain;charset=utf-8");
 expect(clipboardCommands(true,"win32")[0].at(-1)).toContain("[Console]::Write");
 expect(clipboardCommands(false,"win32")[0].at(-1)).toContain("UTF8Encoding");
});
test("actual App copies timeline selections while retaining composer focus",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/timeline-clipboard-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("focus passed");
},15000);
