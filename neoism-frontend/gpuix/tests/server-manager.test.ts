import {expect,test} from "bun:test";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
test("original Server manager and Host/Join/System forms work end to end against isolated registry",async()=>{
 const directory=mkdtempSync(join(tmpdir(),"neoism-server-manager-"));
 try{
 const child=Bun.spawn([process.execPath,"run","tests/support/server-manager-check.ts"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NEOISM_CONFIG_HOME:directory},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({originalManagementAndForms:true,registryCrud:true,maskedCredentials:true,hostJoinSwitch:true,systemForm:true,noWindow:true});
 }finally{rmSync(directory,{recursive:true,force:true});}
},15000);
