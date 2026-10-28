import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual status directory click opens the original folder modal and commits only its captured local workspace",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/status-directory-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualStatusLineNativeHit:true,originalFolderModal:true,selectedDirectoryCommitsAndRefreshes:true,noImplicitPty:true,cancelHasNoWrites:true,workspaceSwitchDropsStaleSelection:true,remoteNeverUsesClientFilesystem:true,persistentRoot:true});
},10000);
